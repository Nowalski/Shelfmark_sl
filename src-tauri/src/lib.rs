mod discovery;
mod inventory;
mod leap;
mod llsd;
mod store;
mod texture_preview;

use serde::Serialize;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};
use tauri::{AppHandle, Emitter, State};
use tauri_plugin_opener::OpenerExt;

const IMPORT_PROGRESS_EVENT: &str = "shelfmark-import-progress";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ImportProgress {
    stage: String,
    completed: u64,
    total: Option<u64>,
}

#[derive(Default)]
struct ImportController {
    active: Mutex<Option<Arc<AtomicBool>>>,
}

impl ImportController {
    fn begin(&self) -> Result<Arc<AtomicBool>, String> {
        let mut active = self
            .active
            .lock()
            .map_err(|_| "The import controller is unavailable.".to_string())?;
        if active.is_some() {
            return Err("An inventory import is already running.".into());
        }
        let cancelled = Arc::new(AtomicBool::new(false));
        *active = Some(cancelled.clone());
        Ok(cancelled)
    }

    fn cancel(&self) -> Result<bool, String> {
        let active = self
            .active
            .lock()
            .map_err(|_| "The import controller is unavailable.".to_string())?;
        if let Some(cancelled) = active.as_ref() {
            cancelled.store(true, Ordering::Relaxed);
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn finish(&self, finished: &Arc<AtomicBool>) {
        if let Ok(mut active) = self.active.lock() {
            if active
                .as_ref()
                .is_some_and(|current| Arc::ptr_eq(current, finished))
            {
                *active = None;
            }
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ImportResult {
    avatar_id: String,
    cache_version: u32,
    category_count: usize,
    item_count: usize,
    snapshot_at: String,
    recent_count: usize,
    empty_folder_count: usize,
    candidate_counts: store::CandidateCounts,
    inventory_cache_path: Option<String>,
    cache_modified_at_ms: Option<i64>,
}

#[tauri::command]
async fn import_inventory_cache(
    app: AppHandle,
    controller: State<'_, ImportController>,
    path: String,
) -> Result<ImportResult, String> {
    let cancelled = controller.begin()?;
    let blocking_cancelled = cancelled.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        import_inventory_cache_blocking(app, path, blocking_cancelled)
    })
    .await;
    controller.finish(&cancelled);
    match result {
        Ok(import_result) => import_result,
        Err(error) => Err(format!(
            "The inventory import stopped unexpectedly: {error}"
        )),
    }
}

#[tauri::command]
async fn open_inventory_entry_in_firestorm(
    app: AppHandle,
    avatar_id: String,
    inventory_id: String,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let inventory_uuid = uuid::Uuid::parse_str(&inventory_id)
            .map_err(|_| "This snapshot entry does not have a valid inventory UUID.".to_string())?;
        if inventory_uuid.is_nil() {
            return Err("This snapshot entry does not have a valid inventory UUID.".into());
        }
        let inventory_id = inventory_uuid.hyphenated().to_string();
        if !store::inventory_entry_exists(&app, &avatar_id, &inventory_id)? {
            return Err(
                "This entry is not present in the selected avatar's local snapshot. Refresh the snapshot and try again.".into(),
            );
        }
        let uri = inventory_select_uri(&inventory_id)?;
        app.opener().open_url(uri, None::<&str>).map_err(|error| {
            format!("Could not open the inventory entry in the system viewer: {error}")
        })
    })
    .await
    .map_err(|error| format!("Could not open the Firestorm inventory entry: {error}"))?
}

#[tauri::command]
async fn start_stock_firestorm_leap(
    controller: State<'_, leap::LeapController>,
    executable_path: String,
    native_bridge_enabled: bool,
) -> Result<leap::LeapStatus, String> {
    let controller = controller.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        controller.start_stock_firestorm(&executable_path, native_bridge_enabled)
    })
    .await
    .map_err(|error| format!("Could not start Firestorm: {error}"))?
}

#[tauri::command(async)]
fn get_stock_firestorm_leap_status(
    controller: State<'_, leap::LeapController>,
) -> leap::LeapStatus {
    controller.status()
}

#[tauri::command]
async fn inspect_stock_firestorm_leap(
    controller: State<'_, leap::LeapController>,
) -> Result<leap::LeapInspection, String> {
    let controller = controller.inner().clone();
    tauri::async_runtime::spawn_blocking(move || controller.inspect_viewer())
        .await
        .map_err(|error| format!("Could not run the Firestorm inspection: {error}"))?
}

#[tauri::command]
async fn scan_stock_firestorm_inventory_ui(
    controller: State<'_, leap::LeapController>,
) -> Result<Vec<String>, String> {
    let controller = controller.inner().clone();
    tauri::async_runtime::spawn_blocking(move || controller.scan_inventory_window_paths())
        .await
        .map_err(|error| format!("Could not scan Firestorm's inventory UI: {error}"))?
}

#[tauri::command]
async fn inspect_stock_firestorm_inventory_view(
    controller: State<'_, leap::LeapController>,
    name: String,
) -> Result<serde_json::Value, String> {
    let controller = controller.inner().clone();
    tauri::async_runtime::spawn_blocking(move || controller.inspect_inventory_view_by_name(&name))
        .await
        .map_err(|error| format!("Could not locate the Firestorm inventory view: {error}"))?
}

#[tauri::command]
async fn inspect_stock_firestorm_inventory_item(
    controller: State<'_, leap::LeapController>,
    item_id: String,
) -> Result<leap::LeapInventoryItem, String> {
    let controller = controller.inner().clone();
    tauri::async_runtime::spawn_blocking(move || controller.inspect_inventory_item(&item_id))
        .await
        .map_err(|error| format!("Could not read the live Firestorm item: {error}"))?
}

#[tauri::command]
async fn move_stock_firestorm_inventory_item(
    controller: State<'_, leap::LeapController>,
    avatar_id: String,
    item_id: String,
    source_parent_id: String,
    destination_folder_id: Option<String>,
) -> Result<leap::LeapMoveResult, String> {
    normalized_inventory_move_avatar(&avatar_id)?;
    let controller = controller.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        controller.move_inventory_item(
            &avatar_id,
            &item_id,
            &source_parent_id,
            destination_folder_id.as_deref(),
        )
    })
    .await
    .map_err(|error| format!("Could not finish the stock Firestorm move: {error}"))?
}

#[tauri::command]
async fn describe_stock_firestorm_leap_api(
    controller: State<'_, leap::LeapController>,
    api: String,
) -> Result<serde_json::Value, String> {
    let controller = controller.inner().clone();
    tauri::async_runtime::spawn_blocking(move || controller.describe_api(&api))
        .await
        .map_err(|error| format!("Could not describe the Firestorm API: {error}"))?
}

fn normalized_inventory_move_avatar(value: &str) -> Result<(String, String), String> {
    let (uuid_value, grid) = value.split_once('@').unwrap_or((value, "secondlife"));
    if !matches!(grid.to_ascii_lowercase().as_str(), "secondlife" | "agni") {
        return Err(
            "Stock Firestorm LEAP moves currently support the Second Life main grid only.".into(),
        );
    }
    let avatar_uuid = normalized_inventory_uuid(uuid_value, "avatar")?;
    let avatar_key = format!("{avatar_uuid}@secondlife");
    Ok((avatar_uuid, avatar_key))
}

fn normalized_inventory_uuid(value: &str, label: &str) -> Result<String, String> {
    let parsed = uuid::Uuid::parse_str(value)
        .map_err(|_| format!("The {label} does not have a valid inventory UUID."))?;
    if parsed.is_nil() {
        return Err(format!("The {label} UUID cannot be empty."));
    }
    Ok(parsed.hyphenated().to_string())
}

fn inventory_select_uri(inventory_id: &str) -> Result<String, String> {
    let inventory_uuid = uuid::Uuid::parse_str(inventory_id)
        .map_err(|_| "This snapshot entry does not have a valid inventory UUID.".to_string())?;
    if inventory_uuid.is_nil() {
        return Err("This snapshot entry does not have a valid inventory UUID.".into());
    }
    Ok(format!(
        "secondlife:///app/inventory/{}/select",
        inventory_uuid.hyphenated()
    ))
}

#[tauri::command]
fn cancel_inventory_import(controller: State<'_, ImportController>) -> Result<bool, String> {
    controller.cancel()
}

fn import_inventory_cache_blocking(
    app: AppHandle,
    path: String,
    cancelled: Arc<AtomicBool>,
) -> Result<ImportResult, String> {
    let cache_path = std::path::PathBuf::from(&path);
    if !cache_path.is_file() {
        return Err("Choose a Firestorm inventory cache file.".into());
    }
    let before = std::fs::metadata(&cache_path)
        .map_err(|e| format!("Could not inspect the selected file: {e}"))?;
    if before.len() > inventory::MAX_COMPRESSED_BYTES {
        return Err("The selected file is larger than the supported 1 GB limit.".into());
    }
    let before_modified = before.modified().unwrap_or(SystemTime::UNIX_EPOCH);
    let cache_modified_at_ms = before_modified
        .duration_since(SystemTime::UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or_default();
    let total_bytes = before.len();
    let mut file = std::fs::File::open(&cache_path)
        .map_err(|e| format!("Could not read the selected file: {e}"))?;
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 64 * 1024];
    let mut last_stage = String::new();
    let mut last_emit = Instant::now();
    let mut report = |stage: &str, completed: u64, total: Option<u64>| {
        if stage != last_stage || last_emit.elapsed() >= Duration::from_millis(120) {
            let _ = app.emit(
                IMPORT_PROGRESS_EVENT,
                ImportProgress {
                    stage: stage.to_string(),
                    completed,
                    total,
                },
            );
            last_stage.clear();
            last_stage.push_str(stage);
            last_emit = Instant::now();
        }
    };
    let mut bytes_read = 0_u64;
    report("reading_cache", 0, Some(total_bytes));
    loop {
        if cancelled.load(Ordering::Relaxed) {
            return Err(crate::llsd::CANCELLED_MESSAGE.into());
        }
        let read = file
            .read(&mut buffer)
            .map_err(|e| format!("Could not read the selected file: {e}"))?;
        if read == 0 {
            break;
        }
        if bytes_read.saturating_add(read as u64) > inventory::MAX_COMPRESSED_BYTES {
            return Err("The selected file grew beyond the supported 1 GB limit while Shelfmark was reading it. The original file was not changed.".into());
        }
        bytes.extend_from_slice(&buffer[..read]);
        bytes_read += read as u64;
        report("reading_cache", bytes_read, Some(total_bytes));
    }
    let after = std::fs::metadata(&cache_path)
        .map_err(|e| format!("Could not verify the selected file after reading: {e}"))?;
    if before.len() != after.len()
        || before_modified != after.modified().unwrap_or(SystemTime::UNIX_EPOCH)
    {
        return Err("The cache changed while Shelfmark was reading it. Wait for Firestorm to finish writing, then import again. The original file was not changed.".into());
    }
    let is_cancelled = || cancelled.load(Ordering::Relaxed);
    report("decompressing", 0, None);
    let mut snapshot = inventory::parse_cache_with_progress(&bytes, &is_cancelled, &mut report)?;
    if is_cancelled() {
        return Err(crate::llsd::CANCELLED_MESSAGE.into());
    }
    inventory::apply_cache_namespace(&mut snapshot, &cache_path)?;
    let cache_dir = cache_path
        .parent()
        .ok_or("The selected cache needs a parent folder so Shelfmark can locate local Firestorm thumbnails.")?;
    let result = store::import_with_progress(
        &app,
        snapshot,
        cache_dir,
        &cache_path,
        cache_modified_at_ms,
        &is_cancelled,
        &mut report,
    )?;
    Ok(ImportResult {
        avatar_id: result.avatar_id,
        cache_version: result.cache_version,
        category_count: result.category_count,
        item_count: result.item_count,
        snapshot_at: result.snapshot_at,
        recent_count: result.recent_count,
        empty_folder_count: result.empty_folder_count,
        candidate_counts: result.candidate_counts,
        inventory_cache_path: result.inventory_cache_path,
        cache_modified_at_ms: result.cache_modified_at_ms,
    })
}

#[tauri::command(async)]
fn list_avatars(app: AppHandle) -> Result<Vec<store::AvatarSummary>, String> {
    store::list_avatars(&app)
}

#[tauri::command]
async fn get_inventory_thumbnail_preview(
    app: AppHandle,
    avatar_id: String,
    item_id: String,
) -> Result<Option<String>, String> {
    let preview =
        tauri::async_runtime::spawn_blocking(move || -> Result<Option<String>, String> {
            let Some((cache_dir, thumbnail_id)) =
                store::thumbnail_source(&app, &avatar_id, &item_id)?
            else {
                return Ok(None);
            };
            Ok(texture_preview::load_data_url(&cache_dir, &thumbnail_id))
        })
        .await
        .map_err(|error| format!("Could not prepare this local thumbnail: {error}"))??;
    Ok(preview)
}

#[tauri::command]
async fn discover_firestorm_caches() -> Result<discovery::FirestormDiscovery, String> {
    tauri::async_runtime::spawn_blocking(|| Ok(discovery::discover_firestorm_caches()))
        .await
        .map_err(|error| format!("Could not discover Firestorm caches: {error}"))?
}

#[tauri::command(async)]
fn discover_stock_firestorm_executable() -> Option<String> {
    discovery::discover_stock_firestorm_executable()
}

#[tauri::command]
async fn resolve_firestorm_avatar_names(
    avatar_ids: Vec<String>,
) -> Result<std::collections::HashMap<String, String>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        Ok(discovery::resolve_firestorm_avatar_names(&avatar_ids))
    })
    .await
    .map_err(|error| format!("Could not read Firestorm avatar names: {error}"))?
}

#[tauri::command]
async fn inspect_firestorm_cache(path: String) -> Result<discovery::CacheCandidate, String> {
    tauri::async_runtime::spawn_blocking(move || discovery::inspect_cache_file(&path))
        .await
        .map_err(|error| format!("Could not inspect the Firestorm cache: {error}"))?
}

#[tauri::command(async)]
fn search_inventory(
    app: AppHandle,
    avatar_id: String,
    query: String,
    kind: Option<String>,
) -> Result<Vec<store::DbItem>, String> {
    store::search(&app, avatar_id, query, kind, Some(300))
}

#[tauri::command(async)]
#[allow(clippy::too_many_arguments)]
fn search_inventory_page(
    app: AppHandle,
    avatar_id: String,
    query: String,
    kind: Option<String>,
    candidate_filter: Option<String>,
    recent_only: bool,
    sort: Option<String>,
    offset: u32,
    limit: u32,
) -> Result<store::InventoryPage, String> {
    store::search_page(
        &app,
        avatar_id,
        query,
        kind,
        candidate_filter,
        recent_only,
        sort.as_deref(),
        offset,
        limit,
    )
}

#[tauri::command(async)]
fn get_collection_items(
    app: AppHandle,
    avatar_id: String,
    item_ids: Vec<String>,
) -> Result<Vec<store::DbItem>, String> {
    store::items_by_ids(&app, avatar_id, item_ids)
}

#[tauri::command(async)]
fn get_inventory_link_target(
    app: AppHandle,
    avatar_id: String,
    target_id: String,
) -> Result<Option<store::LinkTarget>, String> {
    store::inventory_link_target(&app, avatar_id, target_id)
}

#[tauri::command(async)]
fn list_inventory_folders(
    app: AppHandle,
    avatar_id: String,
) -> Result<Vec<store::DbFolder>, String> {
    store::list_folders(&app, avatar_id)
}

#[tauri::command(async)]
fn list_empty_inventory_folders(
    app: AppHandle,
    avatar_id: String,
    offset: u32,
    limit: u32,
) -> Result<store::EmptyFoldersPage, String> {
    store::empty_folders_page(&app, avatar_id, offset, limit)
}

#[tauri::command(async)]
fn get_inventory_folder_contents(
    app: AppHandle,
    avatar_id: String,
    folder_id: Option<String>,
    offset: u32,
    limit: u32,
) -> Result<store::FolderContents, String> {
    store::folder_contents(&app, avatar_id, folder_id, offset, limit)
}

#[tauri::command(async)]
fn save_item_tags(
    app: AppHandle,
    avatar_id: String,
    item_id: String,
    tags: Vec<String>,
) -> Result<(), String> {
    store::update_tags(&app, avatar_id, item_id, tags)
}

#[tauri::command(async)]
fn replace_avatar_tags(
    app: AppHandle,
    avatar_id: String,
    item_tags: std::collections::HashMap<String, Vec<String>>,
) -> Result<bool, String> {
    store::replace_avatar_tags(&app, avatar_id, item_tags)
}

#[tauri::command(async)]
fn replace_profile_tags(
    app: AppHandle,
    profiles: Vec<store::AvatarTagProfile>,
) -> Result<(), String> {
    store::replace_profile_tags(&app, profiles)
}

pub fn run() {
    let args = std::env::args_os().skip(1).collect::<Vec<_>>();
    if args.first().is_some_and(|arg| arg == "--leap-child") {
        let result = leap::run_child(&args[1..]);
        if let Err(error) = result {
            let _ = writeln!(
                std::io::stderr().lock(),
                "Shelfmark LEAP helper stopped: {error}"
            );
        }
        return;
    }

    tauri::Builder::default()
        .manage(ImportController::default())
        .manage(leap::LeapController::default())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .invoke_handler(tauri::generate_handler![
            import_inventory_cache,
            open_inventory_entry_in_firestorm,
            start_stock_firestorm_leap,
            get_stock_firestorm_leap_status,
            inspect_stock_firestorm_leap,
            scan_stock_firestorm_inventory_ui,
            inspect_stock_firestorm_inventory_view,
            inspect_stock_firestorm_inventory_item,
            move_stock_firestorm_inventory_item,
            describe_stock_firestorm_leap_api,
            cancel_inventory_import,
            discover_firestorm_caches,
            discover_stock_firestorm_executable,
            resolve_firestorm_avatar_names,
            inspect_firestorm_cache,
            list_avatars,
            get_inventory_thumbnail_preview,
            search_inventory,
            search_inventory_page,
            get_collection_items,
            get_inventory_link_target,
            list_inventory_folders,
            list_empty_inventory_folders,
            get_inventory_folder_contents,
            save_item_tags,
            replace_avatar_tags,
            replace_profile_tags
        ])
        .run(tauri::generate_context!())
        .expect("Shelfmark failed to start");
}

#[cfg(test)]
mod inventory_identity_tests {
    #[test]
    fn inventory_selection_uses_only_a_valid_non_null_uuid() {
        assert_eq!(
            super::inventory_select_uri("30000000-0000-0000-0000-000000000003").unwrap(),
            "secondlife:///app/inventory/30000000-0000-0000-0000-000000000003/select"
        );
        assert!(super::inventory_select_uri("sample-item").is_err());
        assert!(super::inventory_select_uri("00000000-0000-0000-0000-000000000000").is_err());
    }

    #[test]
    fn move_avatar_keeps_snapshot_grid_key_and_limits_bridge_to_agni() {
        let avatar = "20000000-0000-0000-0000-000000000002";
        assert_eq!(
            super::normalized_inventory_move_avatar(&format!("{avatar}@secondlife")).unwrap(),
            (avatar.to_string(), format!("{avatar}@secondlife"))
        );
        assert_eq!(
            super::normalized_inventory_move_avatar(&format!("{avatar}@agni")).unwrap(),
            (avatar.to_string(), format!("{avatar}@secondlife"))
        );
        assert!(super::normalized_inventory_move_avatar(&format!("{avatar}@grid_test")).is_err());
    }
}

#[cfg(test)]
mod stress_tests {
    use flate2::{write::GzEncoder, Compression};
    use std::io::Write;

    fn write_u32(out: &mut Vec<u8>, value: u32) {
        out.extend(value.to_be_bytes());
    }
    fn key(out: &mut Vec<u8>, value: &str) {
        out.push(b'k');
        write_u32(out, value.len() as u32);
        out.extend(value.as_bytes());
    }
    fn string(out: &mut Vec<u8>, value: &str) {
        out.push(b's');
        write_u32(out, value.len() as u32);
        out.extend(value.as_bytes());
    }
    fn uuid(out: &mut Vec<u8>, value: &str) {
        out.push(b'u');
        for part in value
            .split('-')
            .flat_map(|chunk| (0..chunk.len()).step_by(2).map(move |i| &chunk[i..i + 2]))
        {
            out.push(u8::from_str_radix(part, 16).unwrap());
        }
    }
    fn map_start(out: &mut Vec<u8>, count: u32) {
        out.push(b'{');
        write_u32(out, count);
    }
    fn array_start(out: &mut Vec<u8>, count: u32) {
        out.push(b'[');
        write_u32(out, count);
    }

    fn large_cache(item_count: u32) -> Vec<u8> {
        let mut raw = Vec::with_capacity(item_count as usize * 120);
        raw.extend(5u32.to_be_bytes());
        map_start(&mut raw, 2);
        key(&mut raw, "categories");
        array_start(&mut raw, 1);
        map_start(&mut raw, 4);
        key(&mut raw, "folder_id");
        uuid(&mut raw, "10000000-0000-0000-0000-000000000001");
        key(&mut raw, "parent_id");
        uuid(&mut raw, "00000000-0000-0000-0000-000000000000");
        key(&mut raw, "owner_id");
        uuid(&mut raw, "20000000-0000-0000-0000-000000000002");
        key(&mut raw, "name");
        string(&mut raw, "Wardrobe");
        raw.push(b'}');
        raw.push(b']');
        key(&mut raw, "items");
        array_start(&mut raw, item_count);
        for i in 0..item_count {
            map_start(&mut raw, 5);
            key(&mut raw, "item_id");
            let id = format!("30000000-0000-0000-0000-{i:012x}");
            uuid(&mut raw, &id);
            key(&mut raw, "parent_id");
            uuid(&mut raw, "10000000-0000-0000-0000-000000000001");
            key(&mut raw, "type");
            string(&mut raw, if i % 4 == 0 { "notecard" } else { "clothing" });
            key(&mut raw, "name");
            string(&mut raw, &format!("Coat collection piece {i:06}"));
            key(&mut raw, "creation_date");
            raw.push(b'i');
            raw.extend((1_700_000_000 + i as i32).to_be_bytes());
            raw.push(b'}');
        }
        raw.push(b']');
        raw.push(b'}');
        raw.push(b'\n');
        let mut gzip = GzEncoder::new(Vec::new(), Compression::fast());
        gzip.write_all(&raw).unwrap();
        gzip.finish().unwrap()
    }

    #[test]
    fn parses_a_200k_item_inventory_cache() {
        let cache = large_cache(200_000);
        let snapshot = crate::inventory::parse_cache(&cache).unwrap();
        assert_eq!(snapshot.items.len(), 200_000);
        assert_eq!(snapshot.categories.len(), 1);
        assert_eq!(snapshot.items[199_999].name, "Coat collection piece 199999");
    }
}
