use crate::inventory::{InventoryItem, Snapshot};
use rusqlite::{
    params, params_from_iter, types::Value, Connection, OptionalExtension, Transaction,
    TransactionBehavior,
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Manager};

// Increment when schema or derived inventory rules change. Version 0 is the
// existing pre-versioned desktop index and is upgraded without discarding data.
const INVENTORY_SCHEMA_VERSION: i64 = 1;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportSummary {
    pub avatar_id: String,
    pub cache_version: u32,
    pub category_count: usize,
    pub item_count: usize,
    pub snapshot_at: String,
    pub recent_count: usize,
    pub empty_folder_count: usize,
    pub candidate_counts: CandidateCounts,
    pub inventory_cache_path: Option<String>,
    pub cache_modified_at_ms: Option<i64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AvatarSummary {
    pub avatar_id: String,
    pub cache_version: u32,
    pub category_count: usize,
    pub item_count: usize,
    pub imported_at: String,
    pub recent_count: usize,
    pub empty_folder_count: usize,
    pub candidate_counts: CandidateCounts,
    pub inventory_cache_path: Option<String>,
    pub cache_modified_at_ms: Option<i64>,
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CandidateCounts {
    pub all_clues: usize,
    pub demo_like_name: usize,
    pub possible_package: usize,
    pub body_fit_variant: usize,
    pub possible_duplicate: usize,
    pub notecard: usize,
    pub landmark: usize,
}

impl CandidateCounts {
    fn include(&mut self, label: Option<&str>, possible_duplicate: bool) {
        if label.is_some() || possible_duplicate {
            self.all_clues += 1;
        }
        if possible_duplicate {
            self.possible_duplicate += 1;
        }
        match label {
            Some("Demo-like name") => self.demo_like_name += 1,
            Some("Possible package") => self.possible_package += 1,
            Some("Possible body-fit variant") => self.body_fit_variant += 1,
            Some("Notecard") => self.notecard += 1,
            Some("Landmark") => self.landmark += 1,
            _ => {}
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DbItem {
    pub id: String,
    pub avatar_id: String,
    pub parent_folder_id: String,
    pub name: String,
    pub description: String,
    pub path: Vec<String>,
    pub kind: String,
    pub asset_type: String,
    pub inventory_type: String,
    pub creator_id: Option<String>,
    pub owner_mask: Option<u32>,
    pub next_owner_mask: Option<u32>,
    pub created_at: Option<String>,
    pub tags: Vec<String>,
    pub is_link: bool,
    pub linked_target_id: Option<String>,
    pub is_favorite: bool,
    pub possible_duplicate: bool,
    pub candidate_label: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InventoryPage {
    pub items: Vec<DbItem>,
    pub has_more: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DbFolder {
    pub id: String,
    pub parent_id: String,
    pub name: String,
    pub preferred_type: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkTarget {
    pub id: String,
    pub name: String,
    pub path: Vec<String>,
    pub parent_folder_id: String,
    pub is_folder: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderContents {
    pub folders: Vec<DbFolder>,
    pub items: Vec<DbItem>,
    pub has_more: bool,
    pub total_item_count: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EmptyFoldersPage {
    pub folders: Vec<DbFolder>,
    pub has_more: bool,
    pub total_count: usize,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AvatarTagProfile {
    pub avatar_id: String,
    pub item_tags: HashMap<String, Vec<String>>,
}

pub fn import_with_progress(
    app: &AppHandle,
    snapshot: Snapshot,
    cache_dir: &Path,
    cache_source_path: &Path,
    cache_modified_at_ms: i64,
    is_cancelled: &dyn Fn() -> bool,
    report: &mut dyn FnMut(&str, u64, Option<u64>),
) -> Result<ImportSummary, String> {
    let mut connection = open_connection(app)?;
    import_snapshot_with_cache_dir_progress(
        &mut connection,
        snapshot,
        Some(cache_dir),
        Some(cache_source_path),
        Some(cache_modified_at_ms),
        is_cancelled,
        report,
    )
}

#[cfg(test)]
fn import_snapshot(
    connection: &mut Connection,
    snapshot: Snapshot,
) -> Result<ImportSummary, String> {
    import_snapshot_with_cache_dir(connection, snapshot, None)
}

#[cfg(test)]
fn import_snapshot_with_cache_dir(
    connection: &mut Connection,
    snapshot: Snapshot,
    cache_dir: Option<&Path>,
) -> Result<ImportSummary, String> {
    import_snapshot_with_cache_dir_progress(
        connection,
        snapshot,
        cache_dir,
        None,
        None,
        &|| false,
        &mut |_, _, _| {},
    )
}

fn import_snapshot_with_cache_dir_progress(
    connection: &mut Connection,
    snapshot: Snapshot,
    cache_dir: Option<&Path>,
    cache_source_path: Option<&Path>,
    cache_modified_at_ms: Option<i64>,
    is_cancelled: &dyn Fn() -> bool,
    report: &mut dyn FnMut(&str, u64, Option<u64>),
) -> Result<ImportSummary, String> {
    check_import_cancelled(is_cancelled)?;
    let tx = connection.transaction().map_err(db_error)?;
    let now = now_timestamp();
    let recent_cutoff = now.saturating_sub(30 * 24 * 60 * 60);
    let mut candidate_counts = CandidateCounts::default();
    let duplicate_groups = duplicate_group_counts(&snapshot.items);
    let mut recent_count = 0;
    report("scoring_items", 0, Some(snapshot.items.len() as u64));
    for (index, item) in snapshot.items.iter().enumerate() {
        if index % 512 == 0 {
            check_import_cancelled(is_cancelled)?;
            report(
                "scoring_items",
                index as u64,
                Some(snapshot.items.len() as u64),
            );
        }
        candidate_counts.include(
            candidate_label(&item.name, &item.path, &item.kind),
            is_possible_duplicate(item, &duplicate_groups),
        );
        recent_count += usize::from(
            item.created_at
                .is_some_and(|created| created >= recent_cutoff && created <= now),
        );
    }
    tx.execute(
        "DELETE FROM inventory_fts WHERE avatar_id = ?1",
        [&snapshot.avatar_id],
    )
    .map_err(db_error)?;
    tx.execute(
        "DELETE FROM inventory_items WHERE avatar_id = ?1",
        [&snapshot.avatar_id],
    )
    .map_err(db_error)?;
    tx.execute(
        "DELETE FROM inventory_categories WHERE avatar_id = ?1",
        [&snapshot.avatar_id],
    )
    .map_err(db_error)?;
    tx.execute(
        "DELETE FROM inventory_thumbnails WHERE avatar_id = ?1",
        [&snapshot.avatar_id],
    )
    .map_err(db_error)?;
    let imported_at = now_timestamp();
    tx.execute(
        "INSERT INTO avatars (avatar_id, cache_version, category_count, item_count, imported_at, texture_cache_dir, inventory_cache_path, cache_modified_at_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(avatar_id) DO UPDATE SET cache_version=excluded.cache_version,
         category_count=excluded.category_count, item_count=excluded.item_count, imported_at=excluded.imported_at,
         texture_cache_dir=excluded.texture_cache_dir, inventory_cache_path=excluded.inventory_cache_path,
         cache_modified_at_ms=excluded.cache_modified_at_ms",
        params![snapshot.avatar_id, snapshot.format_version, snapshot.categories.len(), snapshot.items.len(), imported_at, cache_dir.map(|path| path.to_string_lossy().into_owned()), cache_source_path.map(|path| path.to_string_lossy().into_owned()), cache_modified_at_ms],
    ).map_err(db_error)?;

    {
        let mut insert = tx.prepare("INSERT INTO inventory_categories (avatar_id, folder_id, parent_id, name, preferred_type) VALUES (?1, ?2, ?3, ?4, ?5)").map_err(db_error)?;
        report(
            "indexing_folders",
            0,
            Some(snapshot.categories.len() as u64),
        );
        for (index, category) in snapshot.categories.iter().enumerate() {
            if index % 512 == 0 {
                check_import_cancelled(is_cancelled)?;
                report(
                    "indexing_folders",
                    index as u64,
                    Some(snapshot.categories.len() as u64),
                );
            }
            insert
                .execute(params![
                    snapshot.avatar_id,
                    category.id,
                    category.parent_id,
                    category.name,
                    category.preferred_type
                ])
                .map_err(db_error)?;
            if (index + 1) % 512 == 0 || index + 1 == snapshot.categories.len() {
                report(
                    "indexing_folders",
                    (index + 1) as u64,
                    Some(snapshot.categories.len() as u64),
                );
            }
        }
    }
    {
        let mut insert_item = tx.prepare(
            "INSERT INTO inventory_items (avatar_id, item_id, parent_id, name, description, path_json, path_text, kind, asset_type, inventory_type, creator_id, created_at, is_link, candidate_label, owner_mask, next_owner_mask, linked_target_id, is_favorite, possible_duplicate)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)"
        ).map_err(db_error)?;
        let mut insert_fts = tx.prepare(
            "INSERT INTO inventory_fts (avatar_id, item_id, name, path, description, tags)
             VALUES (?1, ?2, ?3, ?4, ?5,
               COALESCE((SELECT group_concat(tag, ' ') FROM inventory_tags WHERE avatar_id=?1 AND item_id=?2), ''))"
        ).map_err(db_error)?;
        let mut insert_thumbnail = tx.prepare(
            "INSERT INTO inventory_thumbnails (avatar_id, item_id, thumbnail_id) VALUES (?1, ?2, ?3)",
        ).map_err(db_error)?;
        report("indexing_items", 0, Some(snapshot.items.len() as u64));
        for (index, item) in snapshot.items.iter().enumerate() {
            if index % 512 == 0 {
                check_import_cancelled(is_cancelled)?;
                report(
                    "indexing_items",
                    index as u64,
                    Some(snapshot.items.len() as u64),
                );
            }
            let path_json = serde_json::to_string(&item.path).map_err(|e| e.to_string())?;
            let path_text = item.path.join(" / ");
            let candidate_label = candidate_label(&item.name, &item.path, &item.kind);
            insert_item
                .execute(params![
                    snapshot.avatar_id,
                    item.id,
                    item.parent_id,
                    item.name,
                    item.description,
                    path_json,
                    path_text,
                    item.kind,
                    item.asset_type,
                    item.inventory_type,
                    item.creator_id,
                    item.created_at,
                    item.is_link,
                    candidate_label.unwrap_or_default(),
                    item.owner_mask.map(i64::from),
                    item.next_owner_mask.map(i64::from),
                    item.linked_target_id,
                    item.is_favorite,
                    is_possible_duplicate(item, &duplicate_groups)
                ])
                .map_err(db_error)?;
            if let Some(thumbnail_id) = &item.thumbnail_id {
                insert_thumbnail
                    .execute(params![snapshot.avatar_id, item.id, thumbnail_id])
                    .map_err(db_error)?;
            }
            insert_fts
                .execute(params![
                    snapshot.avatar_id,
                    item.id,
                    item.name,
                    path_text,
                    item.description
                ])
                .map_err(db_error)?;
            if (index + 1) % 512 == 0 || index + 1 == snapshot.items.len() {
                report(
                    "indexing_items",
                    (index + 1) as u64,
                    Some(snapshot.items.len() as u64),
                );
            }
        }
    }
    check_import_cancelled(is_cancelled)?;
    report("finishing", 0, None);
    let empty_folder_count = empty_folder_count_in(&tx, &snapshot.avatar_id)?;
    check_import_cancelled(is_cancelled)?;
    tx.commit().map_err(db_error)?;

    Ok(ImportSummary {
        avatar_id: snapshot.avatar_id,
        cache_version: snapshot.format_version,
        category_count: snapshot.categories.len(),
        item_count: snapshot.items.len(),
        snapshot_at: timestamp_iso(imported_at),
        recent_count,
        empty_folder_count,
        candidate_counts,
        inventory_cache_path: cache_source_path.map(|path| path.to_string_lossy().into_owned()),
        cache_modified_at_ms,
    })
}

fn check_import_cancelled(is_cancelled: &dyn Fn() -> bool) -> Result<(), String> {
    if is_cancelled() {
        Err(crate::llsd::CANCELLED_MESSAGE.into())
    } else {
        Ok(())
    }
}

pub fn thumbnail_source(
    app: &AppHandle,
    avatar_id: &str,
    item_id: &str,
) -> Result<Option<(PathBuf, String)>, String> {
    let connection = open_connection(app)?;
    let source = connection
        .query_row(
            "SELECT avatars.texture_cache_dir, inventory_thumbnails.thumbnail_id
             FROM inventory_thumbnails
             JOIN avatars ON avatars.avatar_id = inventory_thumbnails.avatar_id
             WHERE inventory_thumbnails.avatar_id = ?1 AND inventory_thumbnails.item_id = ?2",
            params![avatar_id, item_id],
            |row| Ok((row.get::<_, Option<String>>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(db_error)?;
    Ok(source.and_then(|(directory, thumbnail_id)| {
        directory.map(|directory| (PathBuf::from(directory), thumbnail_id))
    }))
}

pub fn inventory_entry_exists(
    app: &AppHandle,
    avatar_id: &str,
    entry_id: &str,
) -> Result<bool, String> {
    let connection = open_connection(app)?;
    inventory_entry_exists_in(&connection, avatar_id, entry_id)
}

fn inventory_entry_exists_in(
    connection: &Connection,
    avatar_id: &str,
    entry_id: &str,
) -> Result<bool, String> {
    connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM inventory_items WHERE avatar_id=?1 AND lower(item_id)=?2)
                 OR EXISTS(SELECT 1 FROM inventory_categories WHERE avatar_id=?1 AND lower(folder_id)=?2)",
            params![avatar_id, entry_id.to_ascii_lowercase()],
            |row| row.get(0),
        )
        .map_err(db_error)
}

pub fn list_avatars(app: &AppHandle) -> Result<Vec<AvatarSummary>, String> {
    let connection = open_connection(app)?;
    let snapshot = connection.unchecked_transaction().map_err(db_error)?;
    let connection = &snapshot;
    let mut stmt = connection.prepare("SELECT avatar_id, cache_version, category_count, item_count, imported_at, inventory_cache_path, cache_modified_at_ms FROM avatars ORDER BY imported_at DESC").map_err(db_error)?;
    let avatars = stmt
        .query_map([], |row| {
            let imported_at: i64 = row.get(4)?;
            Ok(AvatarSummary {
                avatar_id: row.get(0)?,
                cache_version: row.get::<_, i64>(1)? as u32,
                category_count: row.get::<_, i64>(2)? as usize,
                item_count: row.get::<_, i64>(3)? as usize,
                imported_at: timestamp_iso(imported_at),
                recent_count: 0,
                empty_folder_count: 0,
                candidate_counts: CandidateCounts::default(),
                inventory_cache_path: row.get(5)?,
                cache_modified_at_ms: row.get(6)?,
            })
        })
        .map_err(db_error)?;
    let mut avatars = avatars.collect::<Result<Vec<_>, _>>().map_err(db_error)?;
    for avatar in &mut avatars {
        avatar.candidate_counts = candidate_counts(&connection, &avatar.avatar_id)?;
        avatar.recent_count = recent_item_count(&connection, &avatar.avatar_id)?;
        avatar.empty_folder_count = empty_folder_count_in(&connection, &avatar.avatar_id)?;
    }
    Ok(avatars)
}

fn candidate_counts(connection: &Connection, avatar_id: &str) -> Result<CandidateCounts, String> {
    let mut statement = connection
        .prepare(
            "SELECT candidate_label, COUNT(*) FROM inventory_items
             WHERE avatar_id=?1 AND candidate_label<>'' GROUP BY candidate_label",
        )
        .map_err(db_error)?;
    let rows = statement
        .query_map([avatar_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, usize>(1)?))
        })
        .map_err(db_error)?;
    let mut counts = CandidateCounts::default();
    for row in rows {
        let (label, count) = row.map_err(db_error)?;
        match label.as_str() {
            "Demo-like name" => counts.demo_like_name = count,
            "Possible package" => counts.possible_package = count,
            "Possible body-fit variant" => counts.body_fit_variant = count,
            "Notecard" => counts.notecard = count,
            "Landmark" => counts.landmark = count,
            _ => {}
        }
    }
    counts.possible_duplicate = connection
        .query_row(
            "SELECT COUNT(*) FROM inventory_items WHERE avatar_id=?1 AND possible_duplicate=1",
            [avatar_id],
            |row| row.get::<_, i64>(0),
        )
        .map_err(db_error)?
        .max(0) as usize;
    counts.all_clues = connection
        .query_row(
            "SELECT COUNT(*) FROM inventory_items WHERE avatar_id=?1 AND (candidate_label<>'' OR possible_duplicate=1)",
            [avatar_id],
            |row| row.get::<_, i64>(0),
        )
        .map_err(db_error)?
        .max(0) as usize;
    Ok(counts)
}

fn recent_item_count(connection: &Connection, avatar_id: &str) -> Result<usize, String> {
    let now = now_timestamp();
    let count = connection
        .query_row(
            "SELECT COUNT(*) FROM inventory_items
             WHERE avatar_id=?1 AND created_at>=?2 AND created_at<=?3",
            params![avatar_id, now.saturating_sub(30 * 24 * 60 * 60), now],
            |row| row.get::<_, i64>(0),
        )
        .map_err(db_error)?;
    Ok(count.max(0) as usize)
}

pub fn search(
    app: &AppHandle,
    avatar_id: String,
    query: String,
    kind: Option<String>,
    limit: Option<u32>,
) -> Result<Vec<DbItem>, String> {
    Ok(search_page(
        app,
        avatar_id,
        query,
        kind,
        None,
        false,
        None,
        0,
        limit.unwrap_or(150),
    )?
    .items)
}

#[allow(clippy::too_many_arguments)]
pub fn search_page(
    app: &AppHandle,
    avatar_id: String,
    query: String,
    kind: Option<String>,
    candidate_filter: Option<String>,
    recent_only: bool,
    sort: Option<&str>,
    offset: u32,
    limit: u32,
) -> Result<InventoryPage, String> {
    let connection = open_connection(app)?;
    let snapshot = connection.unchecked_transaction().map_err(db_error)?;
    search_page_in(
        &snapshot,
        avatar_id,
        query,
        kind,
        candidate_filter,
        recent_only,
        sort,
        offset,
        limit,
    )
}

pub fn items_by_ids(
    app: &AppHandle,
    avatar_id: String,
    item_ids: Vec<String>,
) -> Result<Vec<DbItem>, String> {
    let connection = open_connection(app)?;
    let snapshot = connection.unchecked_transaction().map_err(db_error)?;
    items_by_ids_in(&snapshot, &avatar_id, &item_ids)
}

pub fn list_folders(app: &AppHandle, avatar_id: String) -> Result<Vec<DbFolder>, String> {
    let connection = open_connection(app)?;
    list_folders_in(&connection, &avatar_id)
}

pub fn empty_folders_page(
    app: &AppHandle,
    avatar_id: String,
    offset: u32,
    limit: u32,
) -> Result<EmptyFoldersPage, String> {
    let connection = open_connection(app)?;
    let snapshot = connection.unchecked_transaction().map_err(db_error)?;
    empty_folders_page_in(&snapshot, &avatar_id, offset, limit)
}

fn empty_folders_page_in(
    connection: &Connection,
    avatar_id: &str,
    offset: u32,
    limit: u32,
) -> Result<EmptyFoldersPage, String> {
    let total_count = empty_folder_count_in(connection, avatar_id)?;
    let page_size = limit.clamp(1, 200) as i64;
    let mut statement = connection
        .prepare(
            "SELECT folder_id, parent_id, name, preferred_type
             FROM inventory_categories AS folders
             WHERE avatar_id=?1 AND parent_id<>'00000000-0000-0000-0000-000000000000'
               AND preferred_type<>'root'
               AND NOT EXISTS (
                   SELECT 1 FROM inventory_categories AS child
                   WHERE child.avatar_id=folders.avatar_id AND child.parent_id=folders.folder_id
               )
               AND NOT EXISTS (
                   SELECT 1 FROM inventory_items AS item
                   WHERE item.avatar_id=folders.avatar_id AND item.parent_id=folders.folder_id
               )
             ORDER BY name COLLATE NOCASE, folder_id LIMIT ?2 OFFSET ?3",
        )
        .map_err(db_error)?;
    let mut folders = statement
        .query_map(params![avatar_id, page_size + 1, offset as i64], |row| {
            Ok(DbFolder {
                id: row.get(0)?,
                parent_id: row.get(1)?,
                name: row.get(2)?,
                preferred_type: row.get(3)?,
            })
        })
        .map_err(db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_error)?;
    let has_more = folders.len() > page_size as usize;
    folders.truncate(page_size as usize);
    Ok(EmptyFoldersPage {
        folders,
        has_more,
        total_count,
    })
}

fn empty_folder_count_in(connection: &Connection, avatar_id: &str) -> Result<usize, String> {
    connection
        .query_row(
            "SELECT COUNT(*) FROM inventory_categories AS folders
             WHERE avatar_id=?1 AND parent_id<>'00000000-0000-0000-0000-000000000000'
               AND preferred_type<>'root'
               AND NOT EXISTS (
                   SELECT 1 FROM inventory_categories AS child
                   WHERE child.avatar_id=folders.avatar_id AND child.parent_id=folders.folder_id
               )
               AND NOT EXISTS (
                   SELECT 1 FROM inventory_items AS item
                   WHERE item.avatar_id=folders.avatar_id AND item.parent_id=folders.folder_id
               )",
            [avatar_id],
            |row| row.get::<_, i64>(0),
        )
        .map(|count| count.max(0) as usize)
        .map_err(db_error)
}

pub fn inventory_link_target(
    app: &AppHandle,
    avatar_id: String,
    target_id: String,
) -> Result<Option<LinkTarget>, String> {
    let connection = open_connection(app)?;
    let snapshot = connection.unchecked_transaction().map_err(db_error)?;
    inventory_link_target_in(&snapshot, &avatar_id, &target_id)
}

fn inventory_link_target_in(
    connection: &Connection,
    avatar_id: &str,
    target_id: &str,
) -> Result<Option<LinkTarget>, String> {
    let item = connection
        .query_row(
            "SELECT item_id, parent_id, name, path_json FROM inventory_items
             WHERE avatar_id=?1 AND item_id=?2",
            params![avatar_id, target_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .optional()
        .map_err(db_error)?;
    if let Some((id, parent_folder_id, name, path_json)) = item {
        let path = serde_json::from_str(&path_json).unwrap_or_default();
        return Ok(Some(LinkTarget {
            id,
            name,
            path,
            parent_folder_id,
            is_folder: false,
        }));
    }

    let mut current_id = target_id.to_owned();
    let mut parent_folder_id = String::new();
    let mut path = Vec::new();
    let mut seen = HashSet::new();
    for depth in 0..256 {
        if current_id == "00000000-0000-0000-0000-000000000000" || !seen.insert(current_id.clone())
        {
            break;
        }
        let folder = connection
            .query_row(
                "SELECT parent_id, name FROM inventory_categories
                 WHERE avatar_id=?1 AND folder_id=?2",
                params![avatar_id, current_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(db_error)?;
        let Some((parent_id, name)) = folder else {
            break;
        };
        if depth == 0 {
            parent_folder_id = parent_id.clone();
        }
        path.push(name);
        current_id = parent_id;
    }
    if path.is_empty() {
        return Ok(None);
    }
    path.reverse();
    let name = path.last().cloned().unwrap_or_default();
    Ok(Some(LinkTarget {
        id: target_id.to_owned(),
        name,
        path,
        parent_folder_id,
        is_folder: true,
    }))
}

fn list_folders_in(connection: &Connection, avatar_id: &str) -> Result<Vec<DbFolder>, String> {
    let mut statement = connection
        .prepare(
            "SELECT folder_id, parent_id, name, preferred_type
             FROM inventory_categories WHERE avatar_id=?1 ORDER BY name COLLATE NOCASE",
        )
        .map_err(db_error)?;
    let folders = statement
        .query_map([avatar_id], |row| {
            Ok(DbFolder {
                id: row.get(0)?,
                parent_id: row.get(1)?,
                name: row.get(2)?,
                preferred_type: row.get(3)?,
            })
        })
        .map_err(db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_error)?;
    Ok(folders)
}

pub fn folder_contents(
    app: &AppHandle,
    avatar_id: String,
    folder_id: Option<String>,
    offset: u32,
    limit: u32,
) -> Result<FolderContents, String> {
    let connection = open_connection(app)?;
    let snapshot = connection.unchecked_transaction().map_err(db_error)?;
    folder_contents_in(&snapshot, &avatar_id, folder_id.as_deref(), offset, limit)
}

fn folder_contents_in(
    connection: &Connection,
    avatar_id: &str,
    folder_id: Option<&str>,
    offset: u32,
    limit: u32,
) -> Result<FolderContents, String> {
    let parent_id = folder_id.unwrap_or("00000000-0000-0000-0000-000000000000");
    if let Some(folder_id) = folder_id {
        let exists: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM inventory_categories WHERE avatar_id=?1 AND folder_id=?2)",
                params![avatar_id, folder_id],
                |row| row.get(0),
            )
            .map_err(db_error)?;
        if !exists {
            return Err("That folder is not part of the selected avatar snapshot.".into());
        }
    }

    let folders = {
        let mut statement = connection
            .prepare(
                "SELECT folder_id, parent_id, name, preferred_type
                 FROM inventory_categories WHERE avatar_id=?1 AND parent_id=?2
                 ORDER BY name COLLATE NOCASE",
            )
            .map_err(db_error)?;
        let folders = statement
            .query_map(params![avatar_id, parent_id], |row| {
                Ok(DbFolder {
                    id: row.get(0)?,
                    parent_id: row.get(1)?,
                    name: row.get(2)?,
                    preferred_type: row.get(3)?,
                })
            })
            .map_err(db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_error)?;
        folders
    };
    let total_item_count = connection
        .query_row(
            "SELECT COUNT(*) FROM inventory_items WHERE avatar_id=?1 AND parent_id=?2",
            params![avatar_id, parent_id],
            |row| row.get::<_, i64>(0),
        )
        .map_err(db_error)?
        .max(0) as usize;
    let page_size = limit.clamp(1, 200) as i64;
    let mut statement = connection
        .prepare(
            "SELECT item_id, avatar_id, name, description, path_json, kind, asset_type,
                    inventory_type, creator_id, created_at, is_link, candidate_label, owner_mask, next_owner_mask, linked_target_id, is_favorite, parent_id, possible_duplicate
             FROM inventory_items WHERE avatar_id=?1 AND parent_id=?2
             ORDER BY name COLLATE NOCASE, item_id LIMIT ?3 OFFSET ?4",
        )
        .map_err(db_error)?;
    let mut items = statement
        .query_map(
            params![avatar_id, parent_id, page_size + 1, offset as i64],
            row_to_item,
        )
        .map_err(db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_error)?;
    let has_more = items.len() > page_size as usize;
    items.truncate(page_size as usize);
    attach_item_tags(connection, avatar_id, &mut items)?;

    Ok(FolderContents {
        folders,
        items,
        has_more,
        total_item_count,
    })
}

fn items_by_ids_in(
    connection: &Connection,
    avatar_id: &str,
    item_ids: &[String],
) -> Result<Vec<DbItem>, String> {
    let mut seen = HashSet::new();
    let unique_ids = item_ids
        .iter()
        .filter(|item_id| seen.insert(item_id.as_str()))
        .collect::<Vec<_>>();
    let mut found_by_id = HashMap::new();

    for chunk in unique_ids.chunks(400) {
        let placeholders = (2..=chunk.len() + 1)
            .map(|index| format!("?{index}"))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT item_id, avatar_id, name, description, path_json, kind, asset_type,
                    inventory_type, creator_id, created_at, is_link, candidate_label, owner_mask, next_owner_mask, linked_target_id, is_favorite, parent_id, possible_duplicate
             FROM inventory_items WHERE avatar_id=?1 AND item_id IN ({placeholders})"
        );
        let mut values = Vec::with_capacity(chunk.len() + 1);
        values.push(Value::Text(avatar_id.to_owned()));
        values.extend(chunk.iter().map(|item_id| Value::Text((*item_id).clone())));
        let mut statement = connection.prepare(&sql).map_err(db_error)?;
        let rows = statement
            .query_map(params_from_iter(values.iter()), row_to_item)
            .map_err(db_error)?;
        let mut chunk_items = rows
            .map(|row| row.map_err(db_error))
            .collect::<Result<Vec<_>, _>>()?;
        attach_item_tags(connection, avatar_id, &mut chunk_items)?;
        for item in chunk_items {
            found_by_id.insert(item.id.clone(), item);
        }
    }

    Ok(unique_ids
        .into_iter()
        .filter_map(|item_id| found_by_id.remove(item_id))
        .collect())
}

#[allow(clippy::too_many_arguments)]
fn search_page_in(
    connection: &Connection,
    avatar_id: String,
    query: String,
    kind: Option<String>,
    candidate_filter: Option<String>,
    recent_only: bool,
    sort: Option<&str>,
    offset: u32,
    limit: u32,
) -> Result<InventoryPage, String> {
    let page_size = limit.clamp(1, 200) as i64;
    let fetch_limit = page_size + 1;
    let offset = offset as i64;
    let raw_query = query;
    let query = fts_query(&raw_query);
    let kind = kind.filter(|kind| !kind.is_empty() && kind != "Any type");
    let candidate_filter = candidate_filter.filter(|label| !label.is_empty());
    if candidate_filter.as_deref().is_some_and(|label| {
        !matches!(
            label,
            "All clues"
                | "Demo-like name"
                | "Possible package"
                | "Possible body-fit variant"
                | "Possible duplicate"
                | "Notecard"
                | "Landmark"
        )
    }) {
        return Err("Unknown inventory clue filter.".into());
    }
    let sort = sort.unwrap_or("Relevance");
    let fts_order = match sort {
        "Name A–Z" => "i.name COLLATE NOCASE, i.item_id",
        "Newest first" => "i.created_at DESC, i.name COLLATE NOCASE, i.item_id",
        _ => "rank, i.item_id",
    };
    let browse_order = match sort {
        "Name A–Z" => "name COLLATE NOCASE, item_id",
        _ => "created_at DESC, name COLLATE NOCASE, item_id",
    };
    let now = now_timestamp();
    let recent_cutoff = now.saturating_sub(30 * 24 * 60 * 60);
    let mut rows = if let Some(fts_query) = query {
        let sql = format!(
             "SELECT i.item_id, i.avatar_id, i.name, i.description, i.path_json, i.kind, i.asset_type,
                    i.inventory_type, i.creator_id, i.created_at, i.is_link, i.candidate_label,
                    i.owner_mask, i.next_owner_mask, i.linked_target_id, i.is_favorite, i.parent_id, i.possible_duplicate
             FROM inventory_fts JOIN inventory_items i ON i.avatar_id=inventory_fts.avatar_id AND i.item_id=inventory_fts.item_id
             WHERE inventory_fts MATCH ?1 AND i.avatar_id=?2 AND (?3 IS NULL OR i.kind=?3)
               AND (?4 IS NULL OR (?4='All clues' AND (i.candidate_label<>'' OR i.possible_duplicate=1))
                    OR (?4='Possible duplicate' AND i.possible_duplicate=1) OR i.candidate_label=?4)
               AND (?5=0 OR (i.created_at>=?6 AND i.created_at<=?7))
             ORDER BY {fts_order} LIMIT ?8 OFFSET ?9"
        );
        let mut statement = connection.prepare(&sql).map_err(db_error)?;
        let found = statement
            .query_map(
                params![
                    fts_query,
                    avatar_id,
                    kind,
                    candidate_filter,
                    recent_only,
                    recent_cutoff,
                    now,
                    fetch_limit,
                    offset
                ],
                row_to_item,
            )
            .map_err(db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_error)?;
        if found.is_empty() {
            let exact_match_exists = if offset > 0 {
                connection
                    .query_row(
                        "SELECT EXISTS(
                           SELECT 1 FROM inventory_fts
                           JOIN inventory_items i ON i.avatar_id=inventory_fts.avatar_id AND i.item_id=inventory_fts.item_id
                           WHERE inventory_fts MATCH ?1 AND i.avatar_id=?2
                             AND (?3 IS NULL OR i.kind=?3)
                             AND (?4 IS NULL OR (?4='All clues' AND (i.candidate_label<>'' OR i.possible_duplicate=1))
                                  OR (?4='Possible duplicate' AND i.possible_duplicate=1) OR i.candidate_label=?4)
                             AND (?5=0 OR (i.created_at>=?6 AND i.created_at<=?7))
                         )",
                        params![
                            fts_query,
                            avatar_id,
                            kind,
                            candidate_filter,
                            recent_only,
                            recent_cutoff,
                            now
                        ],
                        |row| row.get::<_, bool>(0),
                    )
                    .map_err(db_error)?
            } else {
                false
            };
            if exact_match_exists {
                found
            } else {
                fuzzy_search_page(
                    connection,
                    &avatar_id,
                    &raw_query,
                    kind.as_deref(),
                    candidate_filter.as_deref(),
                    recent_only,
                    sort,
                    recent_cutoff,
                    now,
                    offset,
                    fetch_limit,
                )?
            }
        } else {
            found
        }
    } else {
        let sql = format!(
            "SELECT item_id, avatar_id, name, description, path_json, kind, asset_type,
                    inventory_type, creator_id, created_at, is_link, candidate_label, owner_mask, next_owner_mask, linked_target_id, is_favorite, parent_id, possible_duplicate
             FROM inventory_items WHERE avatar_id=?1 AND (?2 IS NULL OR kind=?2)
               AND (?3 IS NULL OR (?3='All clues' AND (candidate_label<>'' OR possible_duplicate=1))
                    OR (?3='Possible duplicate' AND possible_duplicate=1) OR candidate_label=?3)
               AND (?4=0 OR (created_at>=?5 AND created_at<=?6))
             ORDER BY {browse_order} LIMIT ?7 OFFSET ?8"
        );
        let mut statement = connection.prepare(&sql).map_err(db_error)?;
        let found = statement
            .query_map(
                params![
                    avatar_id,
                    kind,
                    candidate_filter,
                    recent_only,
                    recent_cutoff,
                    now,
                    fetch_limit,
                    offset
                ],
                row_to_item,
            )
            .map_err(db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_error)?;
        found
    };
    attach_item_tags(connection, &avatar_id, &mut rows)?;
    let has_more = rows.len() as i64 > page_size;
    rows.truncate(page_size as usize);
    Ok(InventoryPage {
        items: rows,
        has_more,
    })
}

#[allow(clippy::too_many_arguments)]
fn fuzzy_search_page(
    connection: &Connection,
    avatar_id: &str,
    query: &str,
    kind: Option<&str>,
    candidate_filter: Option<&str>,
    recent_only: bool,
    sort: &str,
    recent_cutoff: i64,
    now: i64,
    offset: i64,
    limit: i64,
) -> Result<Vec<DbItem>, String> {
    let terms = search_tokens(query);
    if terms.is_empty() {
        return Ok(Vec::new());
    }
    let mut groups = Vec::with_capacity(terms.len());
    for term in &terms {
        let candidates = indexed_spelling_candidates(connection, term)?;
        if candidates.is_empty() {
            return Ok(Vec::new());
        }
        groups.push(candidates);
    }

    let fts_query = groups
        .iter()
        .map(|group| {
            let alternatives = group
                .iter()
                .map(|term| format!("\"{}\"", term.replace('"', "\"\"")))
                .collect::<Vec<_>>()
                .join(" OR ");
            format!("({alternatives})")
        })
        .collect::<Vec<_>>()
        .join(" AND ");

    let fts_order = match sort {
        "Name A–Z" => "i.name COLLATE NOCASE, i.item_id",
        "Newest first" => "i.created_at DESC, i.name COLLATE NOCASE, i.item_id",
        _ => "rank, i.item_id",
    };
    let sql = format!(
        "SELECT i.item_id, i.avatar_id, i.name, i.description, i.path_json, i.kind, i.asset_type,
                i.inventory_type, i.creator_id, i.created_at, i.is_link, i.candidate_label,
                i.owner_mask, i.next_owner_mask, i.linked_target_id, i.is_favorite, i.parent_id, i.possible_duplicate
         FROM inventory_fts JOIN inventory_items i
           ON i.avatar_id=inventory_fts.avatar_id AND i.item_id=inventory_fts.item_id
         WHERE inventory_fts MATCH ?1 AND i.avatar_id=?2 AND (?3 IS NULL OR i.kind=?3)
           AND (?4 IS NULL OR (?4='All clues' AND (i.candidate_label<>'' OR i.possible_duplicate=1))
                OR (?4='Possible duplicate' AND i.possible_duplicate=1) OR i.candidate_label=?4)
           AND (?5=0 OR (i.created_at>=?6 AND i.created_at<=?7))
         ORDER BY {fts_order} LIMIT ?8 OFFSET ?9"
    );
    let mut statement = connection.prepare(&sql).map_err(db_error)?;
    let rows = statement
        .query_map(
            params![
                fts_query,
                avatar_id,
                kind,
                candidate_filter,
                recent_only,
                recent_cutoff,
                now,
                limit,
                offset.max(0)
            ],
            row_to_item,
        )
        .map_err(db_error)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(db_error)
}

fn indexed_spelling_candidates(connection: &Connection, term: &str) -> Result<Vec<String>, String> {
    let variants = spelling_variants(term);
    let mut found = HashSet::new();
    for chunk in variants.chunks(400) {
        let placeholders = vec!["?"; chunk.len()].join(",");
        let sql = format!("SELECT term FROM inventory_fts_vocab WHERE term IN ({placeholders})");
        let mut statement = connection.prepare(&sql).map_err(db_error)?;
        let rows = statement
            .query_map(params_from_iter(chunk.iter()), |row| {
                row.get::<_, String>(0)
            })
            .map_err(db_error)?;
        for row in rows {
            found.insert(row.map_err(db_error)?);
        }
    }

    let mut candidates = found.into_iter().collect::<Vec<_>>();
    candidates.sort();
    Ok(candidates)
}

fn spelling_variants(term: &str) -> Vec<String> {
    const ASCII_LETTERS: &[u8] = b"abcdefghijklmnopqrstuvwxyz";
    const ASCII_DIGITS: &[u8] = b"0123456789";
    const MAX_FUZZY_LENGTH: usize = 24;

    let chars = term.chars().collect::<Vec<_>>();
    let mut variants = HashSet::from([term.to_string()]);
    if chars.len() < 4 || chars.len() > MAX_FUZZY_LENGTH {
        return variants.into_iter().collect();
    }

    let alphabet = if chars.iter().any(|character| character.is_numeric()) {
        [ASCII_LETTERS, ASCII_DIGITS].concat()
    } else {
        ASCII_LETTERS.to_vec()
    };

    for index in 0..chars.len() {
        let mut deleted = chars.clone();
        deleted.remove(index);
        variants.insert(deleted.iter().collect());

        for replacement in &alphabet {
            let replacement = char::from(*replacement);
            if replacement == chars[index] {
                continue;
            }
            let mut substituted = chars.clone();
            substituted[index] = replacement;
            variants.insert(substituted.iter().collect());
        }

        if index + 1 < chars.len() && chars[index] != chars[index + 1] {
            let mut transposed = chars.clone();
            transposed.swap(index, index + 1);
            variants.insert(transposed.iter().collect());
        }
    }

    for index in 0..=chars.len() {
        for inserted in &alphabet {
            let mut expanded = chars.clone();
            expanded.insert(index, char::from(*inserted));
            variants.insert(expanded.iter().collect());
        }
    }

    variants.into_iter().collect()
}

fn search_tokens(value: &str) -> Vec<String> {
    let normalized = value
        .chars()
        .map(|c| {
            if c.is_alphanumeric() {
                c.to_lowercase().collect::<String>()
            } else {
                " ".into()
            }
        })
        .collect::<String>();
    normalized
        .split_whitespace()
        .take(8)
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
fn fuzzy_fields_score(
    query_terms: &[String],
    name: &str,
    path: &str,
    tags: &str,
    description: &str,
) -> Option<u8> {
    let fields = [(name, 3u8), (path, 2u8), (tags, 2u8), (description, 1u8)];
    let mut score = 0u8;
    for term in query_terms {
        let best = fields
            .iter()
            .filter_map(|(text, weight)| {
                text.split(|c: char| !c.is_alphanumeric())
                    .filter(|word| distance_within_one(word, term))
                    .map(|word| *weight + u8::from(word == term.as_str()))
                    .max()
            })
            .max()?;
        score = score.saturating_add(best);
    }
    Some(score)
}

#[cfg(test)]
fn distance_within_one(a: &str, b: &str) -> bool {
    let left = a.to_lowercase().chars().collect::<Vec<_>>();
    let right = b.to_lowercase().chars().collect::<Vec<_>>();
    if left == right {
        return true;
    }
    if left.len() < 4 || right.len() < 4 || left.len().abs_diff(right.len()) > 1 {
        return false;
    }
    if left.len() == right.len() {
        let mismatches = left
            .iter()
            .zip(&right)
            .enumerate()
            .filter_map(|(i, (l, r))| (l != r).then_some(i))
            .collect::<Vec<_>>();
        return mismatches.len() == 1
            || (mismatches.len() == 2
                && mismatches[1] == mismatches[0] + 1
                && left[mismatches[0]] == right[mismatches[1]]
                && left[mismatches[1]] == right[mismatches[0]]);
    }
    let (longer, shorter) = if left.len() > right.len() {
        (&left, &right)
    } else {
        (&right, &left)
    };
    let mut i = 0;
    let mut j = 0;
    let mut skipped = false;
    while i < longer.len() && j < shorter.len() {
        if longer[i] == shorter[j] {
            i += 1;
            j += 1;
        } else if skipped {
            return false;
        } else {
            skipped = true;
            i += 1;
        }
    }
    true
}

pub fn update_tags(
    app: &AppHandle,
    avatar_id: String,
    item_id: String,
    tags: Vec<String>,
) -> Result<(), String> {
    let tags = tags
        .into_iter()
        .map(|tag| tag.trim().to_string())
        .filter(|tag| !tag.is_empty())
        .map(|tag| tag.chars().take(40).collect::<String>())
        .take(16)
        .collect::<Vec<_>>();
    let mut connection = open_connection(app)?;
    let tx = connection.transaction().map_err(db_error)?;
    let exists = tx
        .query_row(
            "SELECT 1 FROM inventory_items WHERE avatar_id=?1 AND item_id=?2",
            params![avatar_id, item_id],
            |_| Ok(()),
        )
        .optional()
        .map_err(db_error)?
        .is_some();
    if !exists {
        return Err("That item is no longer in this avatar's imported snapshot.".into());
    }
    tx.execute(
        "DELETE FROM inventory_tags WHERE avatar_id=?1 AND item_id=?2",
        params![avatar_id, item_id],
    )
    .map_err(db_error)?;
    {
        let mut insert = tx.prepare("INSERT OR IGNORE INTO inventory_tags (avatar_id, item_id, tag) VALUES (?1, ?2, ?3)").map_err(db_error)?;
        for tag in &tags {
            insert
                .execute(params![avatar_id, item_id, tag])
                .map_err(db_error)?;
        }
    }
    let stored_tags = {
        let mut stmt = tx
            .prepare(
                "SELECT tag FROM inventory_tags WHERE avatar_id=?1 AND item_id=?2 ORDER BY tag",
            )
            .map_err(db_error)?;
        let tags = stmt
            .query_map(params![avatar_id, item_id], |row| row.get::<_, String>(0))
            .map_err(db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_error)?;
        tags.join(" ")
    };
    tx.execute(
        "UPDATE inventory_fts SET tags=?3 WHERE avatar_id=?1 AND item_id=?2",
        params![avatar_id, item_id, stored_tags],
    )
    .map_err(db_error)?;
    tx.commit().map_err(db_error)
}

pub fn replace_avatar_tags(
    app: &AppHandle,
    avatar_id: String,
    item_tags: HashMap<String, Vec<String>>,
) -> Result<bool, String> {
    validate_avatar_tags(&item_tags)?;
    let mut connection = open_connection(app)?;
    replace_avatar_tags_in(&mut connection, &avatar_id, &item_tags)
}

pub fn replace_profile_tags(
    app: &AppHandle,
    profiles: Vec<AvatarTagProfile>,
) -> Result<(), String> {
    if profiles.len() > 1_000 {
        return Err("The profile contains too many avatar tag sets.".into());
    }
    let mut seen = HashSet::new();
    for profile in &profiles {
        if profile.avatar_id.is_empty()
            || profile.avatar_id.len() > 256
            || !seen.insert(&profile.avatar_id)
        {
            return Err("The profile contains an invalid or duplicate avatar tag set.".into());
        }
        validate_avatar_tags(&profile.item_tags)?;
    }

    let mut connection = open_connection(app)?;
    let tx = connection.transaction().map_err(db_error)?;
    for profile in profiles {
        replace_avatar_tags_in_transaction(&tx, &profile.avatar_id, &profile.item_tags)?;
    }
    tx.commit().map_err(db_error)
}

fn validate_avatar_tags(item_tags: &HashMap<String, Vec<String>>) -> Result<(), String> {
    if item_tags.len() > 100_000 {
        return Err("The profile contains too many tagged items.".into());
    }
    for (item_id, tags) in item_tags {
        if item_id.is_empty() || item_id.len() > 256 || tags.len() > 16 {
            return Err("The profile contains an invalid tag entry.".into());
        }
        if tags.iter().any(|tag| tag.trim().chars().count() > 40) {
            return Err("A personal tag is longer than 40 characters.".into());
        }
    }
    Ok(())
}

fn replace_avatar_tags_in(
    connection: &mut Connection,
    avatar_id: &str,
    item_tags: &HashMap<String, Vec<String>>,
) -> Result<bool, String> {
    validate_avatar_tags(item_tags)?;
    let tx = connection.transaction().map_err(db_error)?;
    let indexed = replace_avatar_tags_in_transaction(&tx, avatar_id, item_tags)?;
    tx.commit().map_err(db_error)?;
    Ok(indexed)
}

fn replace_avatar_tags_in_transaction(
    tx: &Transaction<'_>,
    avatar_id: &str,
    item_tags: &HashMap<String, Vec<String>>,
) -> Result<bool, String> {
    let indexed: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM avatars WHERE avatar_id=?1)",
            [avatar_id],
            |row| row.get(0),
        )
        .map_err(db_error)?;
    if !indexed {
        return Ok(false);
    }
    tx.execute("DELETE FROM inventory_tags WHERE avatar_id=?1", [avatar_id])
        .map_err(db_error)?;
    {
        let mut insert = tx
            .prepare_cached(
                "INSERT OR IGNORE INTO inventory_tags (avatar_id, item_id, tag)
                 SELECT ?1, ?2, ?3 WHERE EXISTS(
                   SELECT 1 FROM inventory_items WHERE avatar_id=?1 AND item_id=?2
                 )",
            )
            .map_err(db_error)?;
        for (item_id, tags) in item_tags {
            for tag in tags
                .iter()
                .map(|tag| tag.trim())
                .filter(|tag| !tag.is_empty())
            {
                let tag = tag.chars().take(40).collect::<String>();
                insert
                    .execute(params![avatar_id, item_id, tag])
                    .map_err(db_error)?;
            }
        }
    }
    tx.execute(
        "UPDATE inventory_fts SET tags=COALESCE((
           SELECT group_concat(tag, ' ') FROM inventory_tags
           WHERE avatar_id=?1 AND item_id=inventory_fts.item_id
         ), '') WHERE avatar_id=?1 AND tags<>COALESCE((
           SELECT group_concat(tag, ' ') FROM inventory_tags
           WHERE avatar_id=?1 AND item_id=inventory_fts.item_id
         ), '')",
        [avatar_id],
    )
    .map_err(db_error)?;
    Ok(true)
}

fn open_connection(app: &AppHandle) -> Result<Connection, String> {
    let mut path: PathBuf = app.path().app_data_dir().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&path)
        .map_err(|e| format!("Could not create local Shelfmark data folder: {e}"))?;
    path.push("inventory.sqlite3");
    let connection = Connection::open(path).map_err(db_error)?;
    connection
        .busy_timeout(std::time::Duration::from_secs(5))
        .map_err(db_error)?;
    initialize_schema(&connection)?;
    Ok(connection)
}

fn initialize_schema(connection: &Connection) -> Result<(), String> {
    // synchronous is connection-local; journal mode and schema persist.
    connection
        .execute_batch("PRAGMA synchronous=NORMAL;")
        .map_err(db_error)?;
    let version = inventory_schema_version(connection)?;
    if version == INVENTORY_SCHEMA_VERSION {
        return Ok(());
    }
    if version > INVENTORY_SCHEMA_VERSION {
        return Err("This inventory index was created by a newer Shelfmark version. Update Shelfmark to open it.".into());
    }
    connection
        .execute_batch("PRAGMA journal_mode=WAL;")
        .map_err(db_error)?;
    // Serialize concurrent first opens, and roll back both schema and derived
    // data if any upgrade step fails. Recheck after obtaining the write lock.
    let migration =
        Transaction::new_unchecked(connection, TransactionBehavior::Immediate).map_err(db_error)?;
    let connection = &migration;
    let version = inventory_schema_version(connection)?;
    if version == INVENTORY_SCHEMA_VERSION {
        return migration.commit().map_err(db_error);
    }
    if version > INVENTORY_SCHEMA_VERSION {
        return Err("This inventory index was created by a newer Shelfmark version. Update Shelfmark to open it.".into());
    }
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS avatars (
           avatar_id TEXT PRIMARY KEY, cache_version INTEGER NOT NULL, category_count INTEGER NOT NULL,
           item_count INTEGER NOT NULL, imported_at INTEGER NOT NULL, texture_cache_dir TEXT,
           inventory_cache_path TEXT, cache_modified_at_ms INTEGER
         );
         CREATE TABLE IF NOT EXISTS inventory_categories (
           avatar_id TEXT NOT NULL, folder_id TEXT NOT NULL, parent_id TEXT NOT NULL, name TEXT NOT NULL,
           preferred_type TEXT NOT NULL, PRIMARY KEY(avatar_id, folder_id)
         );
         CREATE INDEX IF NOT EXISTS idx_inventory_categories_parent_name
           ON inventory_categories(avatar_id, parent_id, name COLLATE NOCASE);
         CREATE TABLE IF NOT EXISTS inventory_items (
           avatar_id TEXT NOT NULL, item_id TEXT NOT NULL, parent_id TEXT NOT NULL, name TEXT NOT NULL,
           description TEXT NOT NULL, path_json TEXT NOT NULL, path_text TEXT NOT NULL, kind TEXT NOT NULL,
           asset_type TEXT NOT NULL, inventory_type TEXT NOT NULL, creator_id TEXT, created_at INTEGER,
            is_link INTEGER NOT NULL, candidate_label TEXT NOT NULL DEFAULT '', owner_mask INTEGER,
            next_owner_mask INTEGER, linked_target_id TEXT, is_favorite INTEGER NOT NULL DEFAULT 0,
            possible_duplicate INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY(avatar_id, item_id)
         );
         CREATE INDEX IF NOT EXISTS idx_inventory_items_avatar_date ON inventory_items(avatar_id, created_at DESC);
         CREATE INDEX IF NOT EXISTS idx_inventory_items_parent_name
           ON inventory_items(avatar_id, parent_id, name COLLATE NOCASE, item_id);
         CREATE TABLE IF NOT EXISTS inventory_tags (
           avatar_id TEXT NOT NULL, item_id TEXT NOT NULL, tag TEXT NOT NULL,
           PRIMARY KEY(avatar_id, item_id, tag)
         );
         CREATE TABLE IF NOT EXISTS inventory_thumbnails (
           avatar_id TEXT NOT NULL, item_id TEXT NOT NULL, thumbnail_id TEXT NOT NULL,
           PRIMARY KEY(avatar_id, item_id)
         );
         CREATE VIRTUAL TABLE IF NOT EXISTS inventory_fts USING fts5(
           avatar_id UNINDEXED, item_id UNINDEXED, name, path, description, tags,
           tokenize='unicode61 remove_diacritics 2'
         );
         CREATE VIRTUAL TABLE IF NOT EXISTS inventory_fts_vocab USING fts5vocab(inventory_fts, 'row');"
    )
    .map_err(db_error)?;
    ensure_candidate_labels(connection)?;
    ensure_possible_duplicate_flags(connection)?;
    ensure_texture_cache_dir_column(connection)?;
    ensure_inventory_cache_source_columns(connection)?;
    ensure_permission_columns(connection)?;
    ensure_link_target_column(connection)?;
    ensure_favorite_column(connection)?;
    connection
        .execute_batch(
            "CREATE INDEX IF NOT EXISTS idx_inventory_items_avatar_candidate
             ON inventory_items(avatar_id, candidate_label);
             CREATE INDEX IF NOT EXISTS idx_inventory_items_avatar_duplicate
             ON inventory_items(avatar_id, possible_duplicate, kind);",
        )
        .map_err(db_error)?;
    connection
        .pragma_update(None, "user_version", INVENTORY_SCHEMA_VERSION)
        .map_err(db_error)?;
    migration.commit().map_err(db_error)
}

fn inventory_schema_version(connection: &Connection) -> Result<i64, String> {
    connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(db_error)
}

fn ensure_inventory_cache_source_columns(connection: &Connection) -> Result<(), String> {
    for (name, declaration) in [
        ("inventory_cache_path", "inventory_cache_path TEXT"),
        ("cache_modified_at_ms", "cache_modified_at_ms INTEGER"),
    ] {
        let exists: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('avatars') WHERE name=?1",
                [name],
                |row| row.get(0),
            )
            .map_err(db_error)?;
        if exists == 0 {
            connection
                .execute(&format!("ALTER TABLE avatars ADD COLUMN {declaration}"), [])
                .map_err(db_error)?;
        }
    }
    Ok(())
}

fn ensure_texture_cache_dir_column(connection: &Connection) -> Result<(), String> {
    let exists: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('avatars') WHERE name='texture_cache_dir'",
            [],
            |row| row.get(0),
        )
        .map_err(db_error)?;
    if exists == 0 {
        connection
            .execute("ALTER TABLE avatars ADD COLUMN texture_cache_dir TEXT", [])
            .map_err(db_error)?;
    }
    Ok(())
}

fn ensure_permission_columns(connection: &Connection) -> Result<(), String> {
    for (name, declaration) in [
        ("owner_mask", "owner_mask INTEGER"),
        ("next_owner_mask", "next_owner_mask INTEGER"),
    ] {
        let exists: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('inventory_items') WHERE name=?1",
                [name],
                |row| row.get(0),
            )
            .map_err(db_error)?;
        if exists == 0 {
            connection
                .execute(
                    &format!("ALTER TABLE inventory_items ADD COLUMN {declaration}"),
                    [],
                )
                .map_err(db_error)?;
        }
    }
    Ok(())
}

fn ensure_link_target_column(connection: &Connection) -> Result<(), String> {
    let exists: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('inventory_items') WHERE name='linked_target_id'",
            [],
            |row| row.get(0),
        )
        .map_err(db_error)?;
    if exists == 0 {
        connection
            .execute(
                "ALTER TABLE inventory_items ADD COLUMN linked_target_id TEXT",
                [],
            )
            .map_err(db_error)?;
    }
    Ok(())
}

fn ensure_favorite_column(connection: &Connection) -> Result<(), String> {
    let exists: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('inventory_items') WHERE name='is_favorite'",
            [],
            |row| row.get(0),
        )
        .map_err(db_error)?;
    if exists == 0 {
        connection
            .execute(
                "ALTER TABLE inventory_items ADD COLUMN is_favorite INTEGER NOT NULL DEFAULT 0",
                [],
            )
            .map_err(db_error)?;
    }
    Ok(())
}

fn ensure_candidate_labels(connection: &Connection) -> Result<(), String> {
    connection
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS shelfmark_meta (
               key TEXT PRIMARY KEY, value TEXT NOT NULL
             );",
        )
        .map_err(db_error)?;
    let has_label_column: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('inventory_items') WHERE name='candidate_label'",
            [],
            |row| row.get(0),
        )
        .map_err(db_error)?;
    if has_label_column == 0 {
        connection
            .execute(
                "ALTER TABLE inventory_items ADD COLUMN candidate_label TEXT NOT NULL DEFAULT ''",
                [],
            )
            .map_err(db_error)?;
    }
    let rules_version: Option<String> = connection
        .query_row(
            "SELECT value FROM shelfmark_meta WHERE key='candidate_rules_version'",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(db_error)?;
    if rules_version.as_deref() == Some("2") {
        return Ok(());
    }

    let rows = {
        let mut statement = connection
            .prepare("SELECT avatar_id, item_id, name, path_json, kind FROM inventory_items")
            .map_err(db_error)?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            })
            .map_err(db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_error)?;
        rows
    };
    let mut update = connection
        .prepare("UPDATE inventory_items SET candidate_label=?3 WHERE avatar_id=?1 AND item_id=?2")
        .map_err(db_error)?;
    for (avatar_id, item_id, name, path_json, kind) in rows {
        let path = serde_json::from_str::<Vec<String>>(&path_json).unwrap_or_default();
        let label = candidate_label(&name, &path, &kind).unwrap_or_default();
        update
            .execute(params![avatar_id, item_id, label])
            .map_err(db_error)?;
    }
    connection
        .execute(
            "INSERT INTO shelfmark_meta(key,value) VALUES('candidate_rules_version','2')
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            [],
        )
        .map_err(db_error)?;
    Ok(())
}

fn ensure_possible_duplicate_flags(connection: &Connection) -> Result<(), String> {
    let has_column: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('inventory_items') WHERE name='possible_duplicate'",
            [],
            |row| row.get(0),
        )
        .map_err(db_error)?;
    if has_column == 0 {
        connection
            .execute(
                "ALTER TABLE inventory_items ADD COLUMN possible_duplicate INTEGER NOT NULL DEFAULT 0",
                [],
            )
            .map_err(db_error)?;
    }
    let rules_version: Option<String> = connection
        .query_row(
            "SELECT value FROM shelfmark_meta WHERE key='possible_duplicate_rules_version'",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(db_error)?;
    if rules_version.as_deref() == Some("1") {
        return Ok(());
    }

    let rows = {
        let mut statement = connection
            .prepare("SELECT avatar_id, item_id, name, kind, is_link FROM inventory_items")
            .map_err(db_error)?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)? != 0,
                ))
            })
            .map_err(db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_error)?;
        rows
    };
    let mut group_counts: HashMap<(String, String, String), usize> = HashMap::new();
    for (avatar_id, _, name, kind, is_link) in &rows {
        if *is_link || kind == "Folder" {
            continue;
        }
        let key = normalized_duplicate_name(name);
        if !key.is_empty() {
            *group_counts
                .entry((avatar_id.clone(), kind.clone(), key))
                .or_insert(0) += 1;
        }
    }
    let mut update = connection
        .prepare(
            "UPDATE inventory_items SET possible_duplicate=?3 WHERE avatar_id=?1 AND item_id=?2",
        )
        .map_err(db_error)?;
    for (avatar_id, item_id, name, kind, is_link) in rows {
        let key = normalized_duplicate_name(&name);
        let is_duplicate = !is_link
            && kind != "Folder"
            && !key.is_empty()
            && group_counts
                .get(&(avatar_id.clone(), kind, key))
                .is_some_and(|count| *count > 1);
        update
            .execute(params![avatar_id, item_id, is_duplicate])
            .map_err(db_error)?;
    }
    connection
        .execute(
            "INSERT INTO shelfmark_meta(key,value) VALUES('possible_duplicate_rules_version','1')
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            [],
        )
        .map_err(db_error)?;
    Ok(())
}

fn row_to_item(row: &rusqlite::Row<'_>) -> rusqlite::Result<DbItem> {
    let path_json: String = row.get(4)?;
    let path =
        serde_json::from_str(&path_json).unwrap_or_else(|_| vec!["Folder path unavailable".into()]);
    let created_at: Option<i64> = row.get(9)?;
    let candidate_label: String = row.get(11)?;
    let owner_mask: Option<i64> = row.get(12)?;
    let next_owner_mask: Option<i64> = row.get(13)?;
    let linked_target_id: Option<String> = row.get(14)?;
    let is_favorite: i64 = row.get(15)?;
    let possible_duplicate: i64 = row.get(17)?;
    Ok(DbItem {
        id: row.get(0)?,
        avatar_id: row.get(1)?,
        parent_folder_id: row.get(16)?,
        name: row.get(2)?,
        description: row.get(3)?,
        path,
        kind: row.get(5)?,
        asset_type: row.get(6)?,
        inventory_type: row.get(7)?,
        creator_id: row.get(8)?,
        owner_mask: owner_mask.and_then(|mask| u32::try_from(mask).ok()),
        next_owner_mask: next_owner_mask.and_then(|mask| u32::try_from(mask).ok()),
        created_at: created_at.map(timestamp_iso),
        tags: Vec::new(),
        is_link: row.get::<_, i64>(10)? != 0,
        linked_target_id,
        is_favorite: is_favorite != 0,
        possible_duplicate: possible_duplicate != 0,
        candidate_label: (!candidate_label.is_empty()).then_some(candidate_label),
    })
}

fn attach_item_tags(
    connection: &Connection,
    avatar_id: &str,
    items: &mut [DbItem],
) -> Result<(), String> {
    if items.is_empty() {
        return Ok(());
    }
    let placeholders = (2..=items.len() + 1)
        .map(|index| format!("?{index}"))
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT item_id, tag FROM inventory_tags WHERE avatar_id=?1 AND item_id IN ({placeholders}) ORDER BY item_id, tag COLLATE NOCASE"
    );
    let mut values = Vec::with_capacity(items.len() + 1);
    values.push(Value::Text(avatar_id.to_owned()));
    values.extend(items.iter().map(|item| Value::Text(item.id.clone())));
    let mut statement = connection.prepare(&sql).map_err(db_error)?;
    let tags = statement
        .query_map(params_from_iter(values.iter()), |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(db_error)?;
    let mut tags_by_id: HashMap<String, Vec<String>> = HashMap::new();
    for row in tags {
        let (item_id, tag) = row.map_err(db_error)?;
        tags_by_id.entry(item_id).or_default().push(tag);
    }
    for item in items {
        item.tags = tags_by_id.remove(&item.id).unwrap_or_default();
    }
    Ok(())
}

fn candidate_label(name: &str, path: &[String], kind: &str) -> Option<&'static str> {
    let text = std::iter::once(name)
        .chain(path.iter().map(String::as_str))
        .flat_map(|part| part.split(|character: char| !character.is_alphanumeric()))
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
        .collect::<Vec<_>>();
    if text.iter().any(|word| word == "demo" || word == "tryon")
        || text
            .windows(2)
            .any(|words| words[0] == "try" && words[1] == "on")
    {
        Some("Demo-like name")
    } else if kind == "Object"
        && text
            .iter()
            .any(|word| matches!(word.as_str(), "unpack" | "box" | "package" | "delivery"))
    {
        Some("Possible package")
    } else if matches!(kind, "Clothing" | "Object") && has_body_fit_variant_clue(&text) {
        Some("Possible body-fit variant")
    } else if kind == "Notecard" {
        Some("Notecard")
    } else if kind == "Landmark" {
        Some("Landmark")
    } else {
        None
    }
}

fn has_body_fit_variant_clue(words: &[String]) -> bool {
    const BODY_FIT_TERMS: &[&str] = &[
        "belleza",
        "ebody",
        "freya",
        "genx",
        "gianni",
        "hourglass",
        "isis",
        "jake",
        "kario",
        "kupra",
        "lara",
        "legacy",
        "maitreya",
        "physique",
        "reborn",
        "signature",
        "slink",
        "tmp",
        "venus",
    ];
    const EXTENDED_SIZES: &[&str] = &["xxs", "xs", "xl", "xxl"];
    const ALL_CLOTHING_SIZES: &[&str] = &["xxs", "xs", "s", "m", "l", "xl", "xxl"];

    words
        .iter()
        .any(|word| BODY_FIT_TERMS.contains(&word.as_str()))
        || words
            .iter()
            .any(|word| matches!(word.as_str(), "size" | "sizes"))
        || words
            .iter()
            .any(|word| EXTENDED_SIZES.contains(&word.as_str()))
        || (words
            .iter()
            .any(|word| matches!(word.as_str(), "body" | "fit" | "fitted"))
            && words
                .iter()
                .any(|word| ALL_CLOTHING_SIZES.contains(&word.as_str())))
}

fn normalized_duplicate_name(name: &str) -> String {
    let mut normalized = String::new();
    let mut needs_space = false;
    for character in name.chars().flat_map(char::to_lowercase) {
        if character.is_alphanumeric() {
            if needs_space && !normalized.is_empty() {
                normalized.push(' ');
            }
            normalized.push(character);
            needs_space = false;
        } else if !normalized.is_empty() {
            needs_space = true;
        }
    }
    normalized
}

type DuplicateGroups = HashMap<(String, String), usize>;

fn duplicate_group_counts(items: &[InventoryItem]) -> DuplicateGroups {
    let mut counts = HashMap::new();
    for item in items {
        if item.is_link || item.kind == "Folder" {
            continue;
        }
        let name = normalized_duplicate_name(&item.name);
        if name.is_empty() {
            continue;
        }
        *counts.entry((item.kind.clone(), name)).or_insert(0) += 1;
    }
    counts
}

fn is_possible_duplicate(item: &InventoryItem, groups: &DuplicateGroups) -> bool {
    if item.is_link || item.kind == "Folder" {
        return false;
    }
    let name = normalized_duplicate_name(&item.name);
    !name.is_empty()
        && groups
            .get(&(item.kind.clone(), name))
            .is_some_and(|count| *count > 1)
}

fn fts_query(value: &str) -> Option<String> {
    let normalized = value
        .chars()
        .map(|c| {
            if c.is_alphanumeric() {
                c.to_lowercase().collect::<String>()
            } else {
                " ".into()
            }
        })
        .collect::<String>();
    let tokens = normalized
        .split_whitespace()
        .filter(|token| token.len() <= 64)
        .take(8)
        .collect::<Vec<_>>();
    if tokens.is_empty() {
        return None;
    }
    Some(
        tokens
            .iter()
            .map(|token| format!("\"{}\"*", token.replace('"', "\"\"")))
            .collect::<Vec<_>>()
            .join(" AND "),
    )
}

fn now_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn timestamp_iso(timestamp: i64) -> String {
    let days = timestamp.div_euclid(86_400);
    let secs = timestamp.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        secs / 3600,
        (secs % 3600) / 60,
        secs % 60
    )
}

fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    year += if month <= 2 { 1 } else { 0 };
    (year, month, day)
}

fn db_error(error: rusqlite::Error) -> String {
    format!("Local inventory index error: {error}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inventory::{Category, InventoryItem};

    fn snapshot(avatar_id: &str, item_id: &str, item_name: &str) -> Snapshot {
        let folder_id = format!("folder-{avatar_id}");
        Snapshot {
            format_version: 5,
            avatar_id: avatar_id.into(),
            categories: vec![Category {
                id: folder_id.clone(),
                parent_id: "00000000-0000-0000-0000-000000000000".into(),
                name: "Winter wardrobe".into(),
                preferred_type: "clothing".into(),
            }],
            items: vec![InventoryItem {
                id: item_id.into(),
                avatar_id: avatar_id.into(),
                parent_id: folder_id,
                name: item_name.into(),
                description: "A synthetic fixture item".into(),
                path: vec!["Clothing".into(), "Winter wardrobe".into()],
                asset_type: "clothing".into(),
                inventory_type: "wearable".into(),
                kind: "Clothing".into(),
                creator_id: None,
                owner_mask: Some(0x0008_ffff),
                next_owner_mask: Some(0x0008_a000),
                created_at: Some(1_700_000_000),
                is_link: false,
                linked_target_id: None,
                is_favorite: false,
                thumbnail_id: None,
            }],
        }
    }

    #[test]
    fn schema_upgrade_adds_the_texture_path_without_dropping_existing_avatars() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE avatars (
                   avatar_id TEXT PRIMARY KEY, cache_version INTEGER NOT NULL, category_count INTEGER NOT NULL,
                   item_count INTEGER NOT NULL, imported_at INTEGER NOT NULL
                 );
                 INSERT INTO avatars VALUES ('avatar-old@secondlife', 5, 2, 12, 1700000000);",
            )
            .unwrap();

        initialize_schema(&connection).unwrap();

        let (item_count, cache_dir): (i64, Option<String>) = connection
            .query_row(
                "SELECT item_count, texture_cache_dir FROM avatars WHERE avatar_id=?1",
                ["avatar-old@secondlife"],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(item_count, 12);
        assert_eq!(cache_dir, None);
    }

    #[test]
    fn link_targets_resolve_only_inside_the_selected_avatar_snapshot() {
        let mut connection = Connection::open_in_memory().unwrap();
        initialize_schema(&connection).unwrap();
        let avatar_id = "avatar-links@secondlife";
        let mut data = snapshot(avatar_id, "coat-target", "Winter coat");
        let folder_id = data.categories[0].id.clone();

        let mut item_link = data.items[0].clone();
        item_link.id = "coat-link".into();
        item_link.name = "Outfit coat link".into();
        item_link.is_link = true;
        item_link.linked_target_id = Some("coat-target".into());

        let mut folder_link = data.items[0].clone();
        folder_link.id = "folder-link".into();
        folder_link.name = "Wardrobe folder link".into();
        folder_link.is_link = true;
        folder_link.linked_target_id = Some(folder_id.clone());
        data.items.extend([item_link, folder_link]);
        import_snapshot(&mut connection, data).unwrap();
        assert!(inventory_entry_exists_in(&connection, avatar_id, "coat-target").unwrap());
        assert!(inventory_entry_exists_in(&connection, avatar_id, &folder_id).unwrap());
        assert!(!inventory_entry_exists_in(
            &connection,
            "another-avatar@secondlife",
            "coat-target"
        )
        .unwrap());

        let target = inventory_link_target_in(&connection, avatar_id, "coat-target")
            .unwrap()
            .unwrap();
        assert_eq!(target.name, "Winter coat");
        assert_eq!(target.path, vec!["Clothing", "Winter wardrobe"]);
        assert_eq!(target.parent_folder_id, folder_id);
        assert!(!target.is_folder);

        let target = inventory_link_target_in(&connection, avatar_id, &folder_id)
            .unwrap()
            .unwrap();
        assert_eq!(target.name, "Winter wardrobe");
        assert_eq!(target.path, vec!["Winter wardrobe"]);
        assert_eq!(
            target.parent_folder_id,
            "00000000-0000-0000-0000-000000000000"
        );
        assert!(target.is_folder);

        assert!(
            inventory_link_target_in(&connection, "another-avatar@secondlife", "coat-target")
                .unwrap()
                .is_none()
        );
        assert!(
            inventory_link_target_in(&connection, avatar_id, "missing-target")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn import_marks_same_name_same_type_items_as_review_clues_only() {
        let mut connection = Connection::open_in_memory().unwrap();
        initialize_schema(&connection).unwrap();
        let avatar_id = "avatar-duplicates@secondlife";
        let mut data = snapshot(avatar_id, "necklace-1", "Pearl clasp necklace");

        let mut punctuation_variant = data.items[0].clone();
        punctuation_variant.id = "necklace-2".into();
        punctuation_variant.name = "Pearl-clasp necklace".into();

        let mut different_type = data.items[0].clone();
        different_type.id = "necklace-object".into();
        different_type.name = "Pearl clasp necklace".into();
        different_type.kind = "Object".into();

        let mut link = data.items[0].clone();
        link.id = "necklace-link".into();
        link.name = "Pearl clasp necklace".into();
        link.is_link = true;
        data.items
            .extend([punctuation_variant, different_type, link]);

        let summary = import_snapshot(&mut connection, data).unwrap();
        assert_eq!(summary.candidate_counts.possible_duplicate, 2);
        assert_eq!(summary.candidate_counts.all_clues, 2);

        let duplicates = search_page_in(
            &connection,
            avatar_id.into(),
            "Pearl".into(),
            None,
            Some("Possible duplicate".into()),
            false,
            Some("Name A–Z"),
            0,
            10,
        )
        .unwrap();
        assert_eq!(duplicates.items.len(), 2);
        assert!(duplicates.items.iter().all(|item| item.possible_duplicate));
        assert_eq!(
            candidate_counts(&connection, avatar_id)
                .unwrap()
                .possible_duplicate,
            2
        );
    }

    fn indexed_ids(connection: &Connection, avatar_id: &str, query: &str) -> Vec<String> {
        let mut statement = connection
            .prepare(
                "SELECT item_id FROM inventory_fts
                 WHERE inventory_fts MATCH ?1 AND avatar_id = ?2 ORDER BY item_id",
            )
            .unwrap();
        statement
            .query_map(params![fts_query(query).unwrap(), avatar_id], |row| {
                row.get(0)
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    }

    #[test]
    fn cache_snapshot_import_is_searchable_avatar_scoped_and_transactional() {
        let mut connection = Connection::open_in_memory().unwrap();
        initialize_schema(&connection).unwrap();

        let mut parsed =
            crate::inventory::parse_cache(&crate::inventory::sample_cache_for_store_tests())
                .unwrap();
        crate::inventory::apply_cache_namespace(
            &mut parsed,
            std::path::Path::new("20000000-0000-0000-0000-000000000002.inv.llsd.gz"),
        )
        .unwrap();
        let parsed_avatar = parsed.avatar_id.clone();
        let first = import_snapshot(&mut connection, parsed).unwrap();
        assert_eq!(
            parsed_avatar,
            "20000000-0000-0000-0000-000000000002@secondlife"
        );
        assert_eq!(first.item_count, 2);
        assert_eq!(first.category_count, 2);
        let cached_thumbnail: String = connection
            .query_row(
                "SELECT thumbnail_id FROM inventory_thumbnails WHERE avatar_id=?1 AND item_id=?2",
                params![parsed_avatar, "30000000-0000-0000-0000-000000000003"],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(cached_thumbnail, "50000000-0000-0000-0000-000000000005");
        let parsed_items = items_by_ids_in(
            &connection,
            &parsed_avatar,
            &["30000000-0000-0000-0000-000000000003".into()],
        )
        .unwrap();
        assert!(parsed_items[0].is_favorite);
        assert_eq!(
            indexed_ids(&connection, &parsed_avatar, "rain"),
            vec![
                "30000000-0000-0000-0000-000000000003",
                "30000000-0000-0000-0000-000000000004"
            ]
        );
        import_snapshot(
            &mut connection,
            snapshot("avatar-b@secondlife", "coat-1", "Rain coat"),
        )
        .unwrap();
        assert_eq!(
            indexed_ids(&connection, "avatar-b@secondlife", "rain"),
            vec!["coat-1"]
        );
        assert_eq!(
            indexed_ids(&connection, &parsed_avatar, "rain"),
            vec![
                "30000000-0000-0000-0000-000000000003",
                "30000000-0000-0000-0000-000000000004"
            ]
        );

        import_snapshot(
            &mut connection,
            snapshot(&parsed_avatar, "coat-2", "Rainlight jacket"),
        )
        .unwrap();
        assert!(indexed_ids(&connection, &parsed_avatar, "closet").is_empty());
        assert_eq!(
            indexed_ids(&connection, &parsed_avatar, "rainlight"),
            vec!["coat-2"]
        );

        let mut invalid = snapshot(&parsed_avatar, "broken-1", "Broken replacement");
        invalid.items.push(invalid.items[0].clone());
        assert!(import_snapshot(&mut connection, invalid).is_err());
        assert_eq!(
            indexed_ids(&connection, &parsed_avatar, "rainlight"),
            vec!["coat-2"]
        );
        assert!(indexed_ids(&connection, &parsed_avatar, "broken").is_empty());
        assert_eq!(
            indexed_ids(&connection, "avatar-b@secondlife", "rain"),
            vec!["coat-1"]
        );
    }

    #[test]
    fn cancelling_a_replacement_rolls_back_to_the_previous_avatar_catalogue() {
        let mut connection = Connection::open_in_memory().unwrap();
        initialize_schema(&connection).unwrap();
        let avatar_id = "avatar-cancel@secondlife";
        import_snapshot(
            &mut connection,
            snapshot(avatar_id, "previous-item", "Previous wardrobe item"),
        )
        .unwrap();

        let mut replacement = snapshot(avatar_id, "replacement-0000", "Replacement item 0");
        let seed = replacement.items[0].clone();
        replacement.items.reserve(1024);
        for index in 1..=1024 {
            let mut item = seed.clone();
            item.id = format!("replacement-{index:04}");
            item.name = format!("Replacement item {index}");
            replacement.items.push(item);
        }

        let cancelled = std::sync::atomic::AtomicBool::new(false);
        let is_cancelled = || cancelled.load(std::sync::atomic::Ordering::Relaxed);
        let mut report = |stage: &str, completed: u64, _total: Option<u64>| {
            if stage == "indexing_items" && completed >= 512 {
                cancelled.store(true, std::sync::atomic::Ordering::Relaxed);
            }
        };
        let error = import_snapshot_with_cache_dir_progress(
            &mut connection,
            replacement,
            None,
            None,
            None,
            &is_cancelled,
            &mut report,
        )
        .unwrap_err();

        assert_eq!(error, crate::llsd::CANCELLED_MESSAGE);
        assert_eq!(
            indexed_ids(&connection, avatar_id, "previous"),
            vec!["previous-item"]
        );
        let item_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM inventory_items WHERE avatar_id=?1",
                [avatar_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(item_count, 1);
    }

    #[test]
    fn candidate_counts_and_result_pages_cover_the_complete_snapshot() {
        let mut connection = Connection::open_in_memory().unwrap();
        initialize_schema(&connection).unwrap();
        let avatar_id = "large-avatar@secondlife";
        let folder_id = "wardrobe-folder";
        let now = now_timestamp();
        let items = (0..205)
            .map(|index| {
                let name = if index % 10 == 0 {
                    format!("Demo outfit {index:03}")
                } else if index % 10 == 1 {
                    format!("Harbor coat — Legacy {index:03}")
                } else {
                    format!("Saved outfit {index:03}")
                };
                InventoryItem {
                    id: format!("item-{index:03}"),
                    avatar_id: avatar_id.into(),
                    parent_id: folder_id.into(),
                    name,
                    description: String::new(),
                    path: vec!["Clothing".into(), "Wardrobe".into()],
                    asset_type: "clothing".into(),
                    inventory_type: "wearable".into(),
                    kind: "Clothing".into(),
                    creator_id: None,
                    owner_mask: None,
                    next_owner_mask: None,
                    created_at: Some(if index % 5 == 0 {
                        now.saturating_sub(60)
                    } else {
                        1_700_000_000
                    }),
                    is_link: false,
                    linked_target_id: None,
                    is_favorite: false,
                    thumbnail_id: None,
                }
            })
            .collect();
        let imported = Snapshot {
            format_version: 5,
            avatar_id: avatar_id.into(),
            categories: vec![Category {
                id: folder_id.into(),
                parent_id: "00000000-0000-0000-0000-000000000000".into(),
                name: "Wardrobe".into(),
                preferred_type: "clothing".into(),
            }],
            items,
        };
        let summary = import_snapshot(&mut connection, imported).unwrap();
        assert_eq!(summary.item_count, 205);
        assert_eq!(summary.candidate_counts.demo_like_name, 21);
        assert_eq!(summary.candidate_counts.body_fit_variant, 21);
        assert_eq!(summary.recent_count, 41);

        let first_page = search_page_in(
            &connection,
            avatar_id.into(),
            String::new(),
            None,
            Some("All clues".into()),
            false,
            Some("Name A–Z"),
            0,
            10,
        )
        .unwrap();
        assert_eq!(first_page.items.len(), 10);
        assert!(first_page.has_more);
        assert_eq!(
            first_page.items[0].candidate_label.as_deref(),
            Some("Demo-like name")
        );

        let body_fit_page = search_page_in(
            &connection,
            avatar_id.into(),
            String::new(),
            None,
            Some("Possible body-fit variant".into()),
            false,
            Some("Name A–Z"),
            0,
            10,
        )
        .unwrap();
        assert_eq!(body_fit_page.items.len(), 10);
        assert!(body_fit_page.has_more);
        assert!(body_fit_page
            .items
            .iter()
            .all(|item| item.candidate_label.as_deref() == Some("Possible body-fit variant")));

        let second_page = search_page_in(
            &connection,
            avatar_id.into(),
            String::new(),
            None,
            Some("All clues".into()),
            false,
            Some("Name A–Z"),
            10,
            10,
        )
        .unwrap();
        assert_eq!(second_page.items.len(), 10);
        assert!(second_page.has_more);

        let final_page = search_page_in(
            &connection,
            avatar_id.into(),
            String::new(),
            None,
            Some("Demo-like name".into()),
            false,
            Some("Name A–Z"),
            20,
            10,
        )
        .unwrap();
        assert_eq!(final_page.items.len(), 1);
        assert!(!final_page.has_more);

        let fuzzy_first = search_page_in(
            &connection,
            avatar_id.into(),
            "demoo".into(),
            None,
            Some("Demo-like name".into()),
            false,
            Some("Name A–Z"),
            0,
            10,
        )
        .unwrap();
        assert_eq!(fuzzy_first.items.len(), 10);
        assert!(fuzzy_first.has_more);
        let fuzzy_last = search_page_in(
            &connection,
            avatar_id.into(),
            "demoo".into(),
            None,
            Some("Demo-like name".into()),
            false,
            Some("Name A–Z"),
            20,
            10,
        )
        .unwrap();
        assert_eq!(fuzzy_last.items.len(), 1);
        assert!(!fuzzy_last.has_more);

        let recent_page = search_page_in(
            &connection,
            avatar_id.into(),
            String::new(),
            None,
            None,
            true,
            Some("Newest first"),
            0,
            15,
        )
        .unwrap();
        assert_eq!(recent_page.items.len(), 15);
        assert!(recent_page.has_more);
        assert!(recent_page
            .items
            .iter()
            .all(|item| item.created_at.is_some()));
    }

    #[test]
    fn saved_collection_lookup_preserves_order_chunks_ids_and_isolates_avatar() {
        let mut connection = Connection::open_in_memory().unwrap();
        initialize_schema(&connection).unwrap();
        let avatar_id = "collection-avatar@secondlife";
        let mut base = snapshot(avatar_id, "item-000", "Saved outfit 000");
        let template = base.items[0].clone();
        base.items = (0..405)
            .map(|index| {
                let mut item = template.clone();
                item.id = format!("item-{index:03}");
                item.name = format!("Saved outfit {index:03}");
                item
            })
            .collect();
        import_snapshot(&mut connection, base).unwrap();
        import_snapshot(
            &mut connection,
            snapshot(
                "other-avatar@secondlife",
                "other-only",
                "Other resident item",
            ),
        )
        .unwrap();
        connection
            .execute(
                "INSERT INTO inventory_tags(avatar_id, item_id, tag) VALUES(?1, ?2, ?3)",
                params![avatar_id, "item-404", "keep"],
            )
            .unwrap();

        let mut requested_ids = (0..405)
            .rev()
            .map(|index| format!("item-{index:03}"))
            .collect::<Vec<_>>();
        requested_ids.extend([
            "other-only".into(),
            "missing-item".into(),
            "item-001".into(),
        ]);
        let items = items_by_ids_in(&connection, avatar_id, &requested_ids).unwrap();
        assert_eq!(items.len(), 405);
        assert_eq!(items.first().unwrap().id, "item-404");
        assert_eq!(items.last().unwrap().id, "item-000");
        assert!(items.iter().all(|item| item.avatar_id == avatar_id));
        assert_eq!(items[0].tags, vec!["keep"]);
    }

    #[test]
    fn folder_browser_is_hierarchical_paged_and_avatar_scoped() {
        let mut connection = Connection::open_in_memory().unwrap();
        initialize_schema(&connection).unwrap();
        let avatar_id = "browse-avatar@secondlife";
        let other_avatar = "other-browse-avatar@secondlife";
        let mut inventory = snapshot(avatar_id, "root-item", "Saved outfit");
        let root_id = inventory.categories[0].id.clone();
        inventory.categories.push(Category {
            id: "winter-folder".into(),
            parent_id: root_id.clone(),
            name: "Winter set".into(),
            preferred_type: "clothing".into(),
        });
        inventory.categories.extend([
            Category {
                id: "empty-tray".into(),
                parent_id: root_id.clone(),
                name: "Empty tray".into(),
                preferred_type: "".into(),
            },
            Category {
                id: "empty-parent".into(),
                parent_id: root_id.clone(),
                name: "Empty parent".into(),
                preferred_type: "".into(),
            },
            Category {
                id: "empty-child".into(),
                parent_id: "empty-parent".into(),
                name: "Empty child".into(),
                preferred_type: "".into(),
            },
        ]);
        let mut nested_item = inventory.items[0].clone();
        nested_item.id = "nested-item".into();
        nested_item.parent_id = "winter-folder".into();
        nested_item.name = "Another outfit".into();
        nested_item.path.push("Winter set".into());
        let mut second_nested_item = nested_item.clone();
        second_nested_item.id = "nested-item-2".into();
        second_nested_item.name = "Last winter outfit".into();
        inventory.items.extend([nested_item, second_nested_item]);
        let imported = import_snapshot(&mut connection, inventory).unwrap();
        assert_eq!(imported.empty_folder_count, 2);
        let mut other_inventory = snapshot(other_avatar, "other-root-item", "Other avatar outfit");
        let other_root_id = other_inventory.categories[0].id.clone();
        other_inventory.categories.push(Category {
            id: "other-empty-folder".into(),
            parent_id: other_root_id,
            name: "Private empty folder".into(),
            preferred_type: "".into(),
        });
        import_snapshot(&mut connection, other_inventory).unwrap();

        let root = folder_contents_in(&connection, avatar_id, None, 0, 100).unwrap();
        assert_eq!(root.folders.len(), 1);
        assert_eq!(root.folders[0].id, root_id);
        assert_eq!(root.total_item_count, 0);

        let top_level = folder_contents_in(&connection, avatar_id, Some(&root_id), 0, 100).unwrap();
        assert!(top_level
            .folders
            .iter()
            .any(|folder| folder.name == "Winter set"));
        assert_eq!(top_level.items.len(), 1);
        assert_eq!(top_level.items[0].id, "root-item");

        let first_page =
            folder_contents_in(&connection, avatar_id, Some("winter-folder"), 0, 1).unwrap();
        assert_eq!(first_page.total_item_count, 2);
        assert_eq!(first_page.items[0].id, "nested-item");
        assert!(first_page.has_more);
        let last_page =
            folder_contents_in(&connection, avatar_id, Some("winter-folder"), 1, 1).unwrap();
        assert_eq!(last_page.items[0].id, "nested-item-2");
        assert!(!last_page.has_more);

        assert!(folder_contents_in(&connection, other_avatar, Some(&root_id), 0, 100).is_err());
        let other_folders = list_folders_in(&connection, other_avatar).unwrap();
        assert_eq!(other_folders.len(), 2);
        assert!(other_folders.iter().all(|folder| folder.id != root_id));

        let first_empty_page = empty_folders_page_in(&connection, avatar_id, 0, 1).unwrap();
        assert_eq!(first_empty_page.total_count, 2);
        assert_eq!(first_empty_page.folders[0].id, "empty-child");
        assert!(first_empty_page.has_more);
        let last_empty_page = empty_folders_page_in(&connection, avatar_id, 1, 1).unwrap();
        assert_eq!(last_empty_page.folders[0].id, "empty-tray");
        assert!(!last_empty_page.has_more);
        let other_empty_page = empty_folders_page_in(&connection, other_avatar, 0, 20).unwrap();
        assert_eq!(other_empty_page.total_count, 1);
        assert_eq!(other_empty_page.folders[0].id, "other-empty-folder");
    }

    #[test]
    fn local_database_import_and_search_page_cover_200k_items() {
        let mut connection = Connection::open_in_memory().unwrap();
        initialize_schema(&connection).unwrap();
        let avatar_id = "stress-avatar@secondlife";
        let folder_id = "stress-wardrobe";
        let now = now_timestamp();
        let items = (0..200_000)
            .map(|index| InventoryItem {
                id: format!("stress-item-{index:06}"),
                avatar_id: avatar_id.into(),
                parent_id: folder_id.into(),
                name: if index % 1_000 == 0 {
                    format!("Demo collection outfit {index:06}")
                } else {
                    format!("Collection outfit {index:06}")
                },
                description: "Synthetic search scale fixture".into(),
                path: vec!["Clothing".into(), "Wardrobe".into()],
                asset_type: "clothing".into(),
                inventory_type: "wearable".into(),
                kind: "Clothing".into(),
                creator_id: None,
                owner_mask: None,
                next_owner_mask: None,
                created_at: Some(if index % 5 == 0 {
                    now.saturating_sub(60)
                } else {
                    1_700_000_000
                }),
                is_link: false,
                linked_target_id: None,
                is_favorite: false,
                thumbnail_id: None,
            })
            .collect();
        let synthetic_snapshot = Snapshot {
            format_version: 5,
            avatar_id: avatar_id.into(),
            categories: vec![Category {
                id: folder_id.into(),
                parent_id: "00000000-0000-0000-0000-000000000000".into(),
                name: "Wardrobe".into(),
                preferred_type: "clothing".into(),
            }],
            items,
        };

        let summary = import_snapshot(&mut connection, synthetic_snapshot).unwrap();
        assert_eq!(summary.item_count, 200_000);
        assert_eq!(summary.candidate_counts.demo_like_name, 200);
        assert_eq!(summary.recent_count, 40_000);

        let first_page = search_page_in(
            &connection,
            avatar_id.into(),
            "collection".into(),
            None,
            None,
            false,
            Some("Relevance"),
            0,
            100,
        )
        .unwrap();
        assert_eq!(first_page.items.len(), 100);
        assert!(first_page.has_more);
        let second_page = search_page_in(
            &connection,
            avatar_id.into(),
            "collection".into(),
            None,
            None,
            false,
            Some("Relevance"),
            100,
            100,
        )
        .unwrap();
        assert_eq!(second_page.items.len(), 100);
        assert!(second_page.has_more);
        assert_ne!(first_page.items[0].id, second_page.items[0].id);

        let typo_started = std::time::Instant::now();
        let typo_page = search_page_in(
            &connection,
            avatar_id.into(),
            "colletion".into(),
            None,
            None,
            false,
            Some("Relevance"),
            0,
            100,
        )
        .unwrap();
        eprintln!(
            "200k typo-search fallback: {:?} for first page",
            typo_started.elapsed()
        );
        assert_eq!(typo_page.items.len(), 100);
        assert!(typo_page.has_more);
        assert!(typo_page.items[0]
            .name
            .to_lowercase()
            .contains("collection"));
    }

    #[test]
    fn existing_local_index_gets_candidate_labels_during_schema_upgrade() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE inventory_items (
                   avatar_id TEXT NOT NULL, item_id TEXT NOT NULL, parent_id TEXT NOT NULL, name TEXT NOT NULL,
                   description TEXT NOT NULL, path_json TEXT NOT NULL, path_text TEXT NOT NULL, kind TEXT NOT NULL,
                   asset_type TEXT NOT NULL, inventory_type TEXT NOT NULL, creator_id TEXT, created_at INTEGER,
                   is_link INTEGER NOT NULL, PRIMARY KEY(avatar_id, item_id)
                 );
                 INSERT INTO inventory_items VALUES
                   ('upgrade-avatar', 'demo-1', 'folder-1', 'Try On Jacket', '',
                    '[\"Clothing\",\"Demo Room\"]', 'Clothing / Demo Room', 'Clothing', 'clothing',
                    'wearable', NULL, 1700000000, 0),
                   ('upgrade-avatar', 'demo-2', 'folder-1', 'Try On Jacket', '',
                    '[\"Clothing\",\"Demo Room\"]', 'Clothing / Demo Room', 'Clothing', 'clothing',
                    'wearable', NULL, 1700000000, 0),
                   ('upgrade-avatar', 'fit-1', 'folder-2', 'Harbor jacket — Jake fit', '',
                    '[\"Clothing\",\"Tops\"]', 'Clothing / Tops', 'Clothing', 'clothing',
                    'wearable', NULL, 1700000000, 0);",
            )
            .unwrap();

        initialize_schema(&connection).unwrap();
        initialize_schema(&connection).unwrap();
        let label: String = connection
            .query_row(
                "SELECT candidate_label FROM inventory_items WHERE avatar_id='upgrade-avatar' AND item_id='demo-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(label, "Demo-like name");
        let counts = candidate_counts(&connection, "upgrade-avatar").unwrap();
        assert_eq!(counts.demo_like_name, 2);
        assert_eq!(counts.body_fit_variant, 1);
        assert_eq!(counts.possible_duplicate, 2);
        let duplicate_flags: Vec<i64> = connection
            .prepare("SELECT possible_duplicate FROM inventory_items WHERE avatar_id='upgrade-avatar' ORDER BY item_id")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(duplicate_flags, vec![1, 1, 0]);
    }

    #[test]
    fn body_fit_clues_stay_textual_and_wearable_scoped() {
        let clothing = vec!["Clothing".to_string(), "Archive".to_string()];
        let notes = vec!["Notecards".to_string()];
        assert_eq!(
            candidate_label("Harbor jacket — Jake fit", &clothing, "Clothing"),
            Some("Possible body-fit variant")
        );
        assert_eq!(
            candidate_label("Wool coat, size M", &clothing, "Object"),
            Some("Possible body-fit variant")
        );
        assert_eq!(
            candidate_label("Fit notes for Maitreya", &notes, "Notecard"),
            Some("Notecard")
        );
        assert_eq!(candidate_label("Winter coat", &clothing, "Clothing"), None);
    }

    #[test]
    fn unix_timestamp_format_is_stable() {
        assert_eq!(timestamp_iso(0), "1970-01-01T00:00:00Z");
        assert_eq!(timestamp_iso(1_700_000_000), "2023-11-14T22:13:20Z");
    }

    #[test]
    fn fts_query_is_prefix_based_and_sanitized() {
        assert_eq!(
            fts_query("wint*r north"),
            Some("\"wint\"* AND \"r\"* AND \"north\"*".into())
        );
        assert_eq!(fts_query("  /!? "), None);
    }

    #[test]
    fn sqlite_fts_searches_names_paths_and_tags() {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch(
            "CREATE VIRTUAL TABLE inventory_fts USING fts5(
               avatar_id UNINDEXED, item_id UNINDEXED, name, path, description, tags,
               tokenize='unicode61 remove_diacritics 2'
             );
             INSERT INTO inventory_fts VALUES ('avatar', 'item-1', 'Harbor coat', 'Clothing / Winter set', '', 'legacy warm');
             INSERT INTO inventory_fts VALUES ('avatar', 'item-2', 'Blue gloves', 'Clothing / Harbor set', '', 'winter');"
        ).unwrap();
        let sql = "SELECT item_id FROM inventory_fts WHERE inventory_fts MATCH ?1 ORDER BY rank";
        let mut statement = connection.prepare(sql).unwrap();
        let hits = statement
            .query_map([fts_query("winter legacy").unwrap()], |row| {
                row.get::<_, String>(0)
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(hits, vec!["item-1"]);
    }

    #[test]
    fn profile_tag_restore_rebuilds_fts_for_one_avatar_only() {
        let mut connection = Connection::open_in_memory().unwrap();
        initialize_schema(&connection).unwrap();
        import_snapshot(
            &mut connection,
            snapshot("avatar-a@secondlife", "coat-a", "Winter wool coat"),
        )
        .unwrap();
        import_snapshot(
            &mut connection,
            snapshot("avatar-b@secondlife", "coat-b", "Winter wool coat"),
        )
        .unwrap();

        let original_tags = HashMap::from([("coat-a".into(), vec!["old label".into()])]);
        replace_avatar_tags_in(&mut connection, "avatar-a@secondlife", &original_tags).unwrap();
        let other_avatar_tags = HashMap::from([("coat-b".into(), vec!["keepsake".into()])]);
        replace_avatar_tags_in(&mut connection, "avatar-b@secondlife", &other_avatar_tags).unwrap();

        let restored_tags =
            HashMap::from([("coat-a".into(), vec!["legacy".into(), "winter".into()])]);
        assert!(
            replace_avatar_tags_in(&mut connection, "avatar-a@secondlife", &restored_tags).unwrap()
        );
        assert_eq!(
            indexed_ids(&connection, "avatar-a@secondlife", "legacy"),
            vec!["coat-a"]
        );
        assert_eq!(
            indexed_ids(&connection, "avatar-a@secondlife", "old"),
            Vec::<String>::new()
        );
        assert_eq!(
            indexed_ids(&connection, "avatar-b@secondlife", "keepsake"),
            vec!["coat-b"]
        );
        assert!(!replace_avatar_tags_in(&mut connection, "not-imported", &restored_tags).unwrap());
    }

    #[test]
    fn typo_fallback_matches_transpositions_in_names_and_tags() {
        assert!(distance_within_one("jackte", "jacket"));
        assert!(distance_within_one("Jacket", "jacket"));
        assert!(!distance_within_one("jacket", "skirt"));
        assert!(spelling_variants("jackte").contains(&"jacket".into()));
        assert!(spelling_variants("jackit").contains(&"jacket".into()));
        assert!(spelling_variants("jackett").contains(&"jacket".into()));
        assert!(spelling_variants("jacket").contains(&"jackett".into()));
        assert_eq!(
            fuzzy_fields_score(
                &["jackte".into()],
                "Harbor jacket",
                "Clothing / Winter",
                "legacy",
                ""
            ),
            Some(3)
        );
        assert_eq!(
            fuzzy_fields_score(
                &["legayc".into()],
                "Harbor jacket",
                "Clothing / Winter",
                "legacy",
                ""
            ),
            Some(2)
        );
    }

    #[test]
    fn typo_fallback_scans_only_the_requested_avatar() {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch(
            "CREATE TABLE inventory_items (
               avatar_id TEXT, item_id TEXT, name TEXT, description TEXT, path_json TEXT, path_text TEXT,
               kind TEXT, asset_type TEXT, inventory_type TEXT, creator_id TEXT, created_at INTEGER, is_link INTEGER,
               candidate_label TEXT NOT NULL DEFAULT '', owner_mask INTEGER, next_owner_mask INTEGER,
               linked_target_id TEXT, is_favorite INTEGER NOT NULL DEFAULT 0, parent_id TEXT NOT NULL,
               possible_duplicate INTEGER NOT NULL DEFAULT 0
             );
             CREATE TABLE inventory_tags (avatar_id TEXT, item_id TEXT, tag TEXT);
             INSERT INTO inventory_items (avatar_id, item_id, name, description, path_json, path_text, kind, asset_type,
               inventory_type, creator_id, created_at, is_link, candidate_label, owner_mask, next_owner_mask,
               linked_target_id, is_favorite, parent_id) VALUES
               ('avatar-a', 'item-a', 'Soft wool jacket', '', '[\"Clothing\",\"Winter\"]', 'Clothing / Winter', 'Clothing', 'clothing', 'wearable', NULL, 1700000000, 0, '', NULL, NULL, NULL, 0, 'folder-a'),
               ('avatar-b', 'item-b', 'Soft wool jacket', '', '[\"Clothing\"]', 'Clothing', 'Clothing', 'clothing', 'wearable', NULL, 1700000000, 0, '', NULL, NULL, NULL, 0, 'folder-b');
             INSERT INTO inventory_tags VALUES ('avatar-a', 'item-a', 'outerwear');
             CREATE VIRTUAL TABLE inventory_fts USING fts5(
               avatar_id UNINDEXED, item_id UNINDEXED, name, path, description, tags,
               tokenize='unicode61 remove_diacritics 2'
             );
             CREATE VIRTUAL TABLE inventory_fts_vocab USING fts5vocab(inventory_fts, 'row');
             INSERT INTO inventory_fts VALUES
               ('avatar-a', 'item-a', 'Soft wool jacket', 'Clothing / Winter', '', 'outerwear'),
               ('avatar-b', 'item-b', 'Soft wool jacket', 'Clothing', '', '');"
        ).unwrap();
        let hits = fuzzy_search_page(
            &connection,
            "avatar-a",
            "jackte",
            None,
            None,
            false,
            "Relevance",
            i64::MIN,
            i64::MAX,
            0,
            300,
        )
        .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "item-a");

        let tag_hits = fuzzy_search_page(
            &connection,
            "avatar-a",
            "outerwer",
            None,
            None,
            false,
            "Relevance",
            i64::MIN,
            i64::MAX,
            0,
            300,
        )
        .unwrap();
        assert_eq!(tag_hits.len(), 1);
        assert_eq!(tag_hits[0].id, "item-a");
    }
}
