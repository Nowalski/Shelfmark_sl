use crate::llsd::{self, Value};
use flate2::read::GzDecoder;
use serde::Serialize;
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::Path;

pub const SUPPORTED_FIRESTORM_CACHE_VERSION: u32 = 5;
pub const MAX_COMPRESSED_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_DECOMPRESSED_BYTES: u64 = 512 * 1024 * 1024;
const MAX_FOLDER_DEPTH: usize = 256;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Category {
    pub id: String,
    pub parent_id: String,
    pub name: String,
    pub preferred_type: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InventoryItem {
    pub id: String,
    pub avatar_id: String,
    pub parent_id: String,
    pub name: String,
    pub description: String,
    pub path: Vec<String>,
    pub asset_type: String,
    pub inventory_type: String,
    pub kind: String,
    pub creator_id: Option<String>,
    pub owner_mask: Option<u32>,
    pub next_owner_mask: Option<u32>,
    pub created_at: Option<i64>,
    pub is_link: bool,
    pub linked_target_id: Option<String>,
    pub is_favorite: bool,
    pub thumbnail_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Snapshot {
    pub format_version: u32,
    pub avatar_id: String,
    pub categories: Vec<Category>,
    pub items: Vec<InventoryItem>,
}

#[cfg(test)]
pub fn parse_cache(input: &[u8]) -> Result<Snapshot, String> {
    parse_cache_with_progress(input, &|| false, &mut |_, _, _| {})
}

pub fn parse_cache_with_progress(
    input: &[u8],
    is_cancelled: &dyn Fn() -> bool,
    report: &mut dyn FnMut(&str, u64, Option<u64>),
) -> Result<Snapshot, String> {
    check_cancelled(is_cancelled)?;
    if input.len() as u64 > MAX_COMPRESSED_BYTES {
        return Err("The selected cache is larger than the supported 1 GB input limit.".into());
    }
    let decoded = if input.starts_with(&[0x1f, 0x8b]) {
        let mut decoder = GzDecoder::new(input);
        let mut out = Vec::new();
        let mut buffer = [0_u8; 64 * 1024];
        loop {
            check_cancelled(is_cancelled)?;
            let read = decoder.read(&mut buffer).map_err(|_| {
                "The gzip cache could not be read. It may be incomplete or damaged.".to_string()
            })?;
            if read == 0 {
                break;
            }
            out.extend_from_slice(&buffer[..read]);
            report("decompressing", out.len() as u64, None);
            if out.len() as u64 > MAX_DECOMPRESSED_BYTES {
                return Err(
                    "The expanded inventory cache is larger than the supported 512 MB limit."
                        .into(),
                );
            }
        }
        if out.len() as u64 > MAX_DECOMPRESSED_BYTES {
            return Err(
                "The expanded inventory cache is larger than the supported 512 MB limit.".into(),
            );
        }
        Cow::Owned(out)
    } else {
        Cow::Borrowed(input)
    };
    check_cancelled(is_cancelled)?;
    report(
        "cache_ready",
        decoded.len() as u64,
        Some(decoded.len() as u64),
    );

    if decoded.len() < 5 {
        return Err("The selected file is too short to be a Firestorm inventory cache.".into());
    }
    let format_version = u32::from_be_bytes(decoded[0..4].try_into().unwrap());
    if format_version != SUPPORTED_FIRESTORM_CACHE_VERSION {
        return Err(format!(
            "This Firestorm inventory cache format is version {format_version}; Shelfmark currently reads version {SUPPORTED_FIRESTORM_CACHE_VERSION}. The file has not been changed."
        ));
    }

    let payload = &decoded[4..];
    let (root, consumed) = llsd::parse_with_progress(payload, is_cancelled, &mut |offset| {
        report("decoding", offset as u64, Some(payload.len() as u64));
    })
    .map_err(|error| {
        if error == llsd::CANCELLED_MESSAGE {
            error
        } else {
            format!("The inventory metadata could not be decoded: {error}. The file has not been changed.")
        }
    })?;
    if decoded[4 + consumed..]
        .iter()
        .any(|byte| !byte.is_ascii_whitespace())
    {
        return Err(
            "Unexpected data follows the inventory cache. The file has not been changed.".into(),
        );
    }
    let root = as_map(&root).ok_or("The decoded cache does not contain an inventory map.")?;
    let category_values = as_array(root.get("categories")).unwrap_or_default();
    let item_values = as_array(root.get("items")).unwrap_or_default();
    if category_values.is_empty() && item_values.is_empty() {
        return Err("No inventory records were found in this cache.".into());
    }

    let mut categories = Vec::with_capacity(category_values.len());
    let mut owners = HashSet::new();
    report("reading_folders", 0, Some(category_values.len() as u64));
    for (index, record) in category_values.iter().enumerate() {
        if index % 512 == 0 {
            check_cancelled(is_cancelled)?;
            report(
                "reading_folders",
                index as u64,
                Some(category_values.len() as u64),
            );
        }
        let Some(map) = as_map(record) else { continue };
        let id = field_string(map, "folder_id")
            .or_else(|| field_string(map, "category_id"))
            .or_else(|| field_string(map, "cat_id"));
        let Some(id) = id.filter(|value| !is_null_uuid(value)) else {
            continue;
        };
        let parent_id = field_string(map, "parent_id").unwrap_or_else(null_uuid);
        if let Some(owner) = field_string(map, "owner_id") {
            if !is_null_uuid(&owner) {
                owners.insert(owner.to_ascii_lowercase());
            }
        }
        categories.push(Category {
            id,
            parent_id,
            name: field_string(map, "name").unwrap_or_else(|| "Unnamed folder".into()),
            preferred_type: field_string(map, "preferred_type").unwrap_or_default(),
        });
    }

    let mut raw_items = Vec::with_capacity(item_values.len());
    report("reading_items", 0, Some(item_values.len() as u64));
    for (index, record) in item_values.iter().enumerate() {
        if index % 512 == 0 {
            check_cancelled(is_cancelled)?;
            report(
                "reading_items",
                index as u64,
                Some(item_values.len() as u64),
            );
        }
        let Some(map) = as_map(record) else { continue };
        let Some(id) = field_string(map, "item_id").filter(|value| !is_null_uuid(value)) else {
            continue;
        };
        let parent_id = field_string(map, "parent_id").unwrap_or_else(null_uuid);
        let permissions = map.get("permissions").and_then(as_map);
        if let Some(owner) = permissions.and_then(|p| field_string(p, "owner_id")) {
            if !is_null_uuid(&owner) {
                owners.insert(owner.to_ascii_lowercase());
            }
        }
        let asset_type = field_string(map, "type")
            .or_else(|| field_string(map, "asset_type"))
            .unwrap_or_default();
        let inventory_type = field_string(map, "inv_type")
            .or_else(|| field_string(map, "inventory_type"))
            .unwrap_or_default();
        let is_link = inventory_type.eq_ignore_ascii_case("link")
            || inventory_type.eq_ignore_ascii_case("link_folder")
            || inventory_type == "24"
            || inventory_type == "25"
            || asset_type.to_ascii_lowercase().starts_with("link");
        let linked_target_id = if is_link {
            field_string(map, "linked_id")
                .or_else(|| field_string(map, "asset_id"))
                .filter(|value| !is_null_uuid(value))
        } else {
            None
        };
        let is_favorite = map
            .get("favorite")
            .and_then(as_map)
            .and_then(|favorite| favorite.get("toggled"))
            .and_then(as_bool)
            .unwrap_or(false);
        let thumbnail_id = map
            .get("thumbnail")
            .and_then(as_map)
            .or_else(|| {
                map.get("metadata")
                    .and_then(as_map)
                    .and_then(|metadata| metadata.get("thumbnail"))
                    .and_then(as_map)
            })
            .and_then(|thumbnail| field_string(thumbnail, "asset_id"))
            .and_then(|value| uuid::Uuid::parse_str(&value).ok())
            .filter(|value| !value.is_nil())
            .map(|value| value.hyphenated().to_string());
        if let Some(owner) = permissions.and_then(|p| field_string(p, "owner_id")) {
            if !is_null_uuid(&owner) {
                owners.insert(owner.to_ascii_lowercase());
            }
        }
        raw_items.push(InventoryItem {
            id,
            avatar_id: String::new(),
            parent_id,
            name: field_string(map, "name").unwrap_or_else(|| "Unnamed item".into()),
            description: field_string(map, "desc").unwrap_or_default(),
            path: Vec::new(),
            asset_type: asset_type.clone(),
            inventory_type: inventory_type.clone(),
            kind: item_kind(&asset_type),
            creator_id: permissions.and_then(|p| field_string(p, "creator_id")),
            owner_mask: permissions
                .and_then(|p| p.get("owner_mask"))
                .and_then(as_i64)
                .and_then(|mask| u32::try_from(mask).ok()),
            next_owner_mask: permissions
                .and_then(|p| p.get("next_owner_mask"))
                .and_then(as_i64)
                .and_then(|mask| u32::try_from(mask).ok()),
            created_at: map
                .get("created_at")
                .or_else(|| map.get("creation_date"))
                .and_then(as_i64),
            is_link,
            linked_target_id,
            is_favorite,
            thumbnail_id,
        });
    }

    if owners.len() > 1 {
        return Err("The cache contains records belonging to more than one inventory owner. Choose a single-avatar Firestorm cache file.".into());
    }
    let avatar_id = owners.into_iter().next().ok_or(
        "The cache did not include an owner identifier, so Shelfmark cannot keep this avatar separate safely.",
    )?;

    let by_id: HashMap<String, &Category> = categories
        .iter()
        .map(|category| (category.id.to_ascii_lowercase(), category))
        .collect();
    let mut paths: HashMap<String, Vec<String>> = HashMap::new();
    let raw_item_count = raw_items.len();
    report("building_paths", 0, Some(raw_item_count as u64));
    for (index, item) in raw_items.iter_mut().enumerate() {
        if index % 512 == 0 {
            check_cancelled(is_cancelled)?;
            report("building_paths", index as u64, Some(raw_item_count as u64));
        }
        item.avatar_id = avatar_id.clone();
        item.path = folder_path(&item.parent_id, &by_id, &mut paths);
    }
    check_cancelled(is_cancelled)?;

    Ok(Snapshot {
        format_version,
        avatar_id,
        categories,
        items: raw_items,
    })
}

fn check_cancelled(is_cancelled: &dyn Fn() -> bool) -> Result<(), String> {
    if is_cancelled() {
        Err(llsd::CANCELLED_MESSAGE.into())
    } else {
        Ok(())
    }
}

pub fn apply_cache_namespace(snapshot: &mut Snapshot, path: &Path) -> Result<(), String> {
    let filename = path.file_name().and_then(|name| name.to_str()).ok_or(
        "The selected cache needs its original Firestorm filename to keep grids separate.",
    )?;
    let filename_lower = filename.to_ascii_lowercase();
    let mut stem = filename_lower.as_str();
    if let Some(without_gzip) = stem.strip_suffix(".gz") {
        stem = without_gzip;
    }
    if let Some(without_cache_extension) = stem.strip_suffix(".inv.llsd") {
        stem = without_cache_extension;
    } else if let Some(without_cache_extension) = stem.strip_suffix(".inv") {
        stem = without_cache_extension;
    }

    let owner_id = snapshot.avatar_id.to_ascii_lowercase();
    let grid_id = if stem.eq_ignore_ascii_case(&owner_id) {
        "secondlife".to_string()
    } else {
        let suffix = stem.strip_prefix(&owner_id)
            .and_then(|tail| tail.strip_prefix('.'))
            .ok_or("This file name does not match its inventory owner. Keep the original Firestorm cache name and import again.")?;
        if suffix.is_empty()
            || suffix.len() > 64
            || !suffix
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
        {
            return Err("The Firestorm grid name in this cache file is not recognized. The cache has not been changed.".into());
        }
        suffix.to_ascii_lowercase()
    };
    let scoped_id = format!("{owner_id}@{grid_id}");
    snapshot.avatar_id = scoped_id.clone();
    for item in &mut snapshot.items {
        item.avatar_id = scoped_id.clone();
    }
    Ok(())
}

fn folder_path(
    id: &str,
    categories: &HashMap<String, &Category>,
    memo: &mut HashMap<String, Vec<String>>,
) -> Vec<String> {
    if is_null_uuid(id) {
        return Vec::new();
    }
    let key = id.to_ascii_lowercase();
    if let Some(path) = memo.get(&key) {
        return path.clone();
    }
    let mut segments = Vec::new();
    let mut chain = Vec::new();
    let mut current = key;
    let mut seen = HashSet::new();
    while !is_null_uuid(&current) && chain.len() < MAX_FOLDER_DEPTH {
        if !seen.insert(current.clone()) {
            segments.push("Folder cycle".into());
            break;
        }
        if let Some(category) = categories.get(&current) {
            chain.push((current.clone(), category.name.clone()));
            current = category.parent_id.to_ascii_lowercase();
        } else {
            segments.push("Folder missing from cache".into());
            break;
        }
    }
    if chain.len() == MAX_FOLDER_DEPTH {
        segments.push("Folder path too deep".into());
    }
    chain.reverse();
    segments.extend(chain.into_iter().map(|(_, name)| name));
    memo.insert(id.to_ascii_lowercase(), segments.clone());
    segments
}

fn item_kind(asset_type: &str) -> String {
    match asset_type.to_ascii_lowercase().as_str() {
        "clothing" | "bodypart" | "wearable" => "Clothing",
        "object" => "Object",
        "notecard" => "Notecard",
        "landmark" => "Landmark",
        "category" | "root" | "trash" => "Folder",
        _ => "Other",
    }
    .into()
}

fn field_string(map: &std::collections::BTreeMap<String, Value>, key: &str) -> Option<String> {
    match map.get(key)? {
        Value::String(value) | Value::Uuid(value) | Value::Uri(value) => Some(value.clone()),
        Value::Integer(value) => Some(value.to_string()),
        Value::Binary(value) => Some(value.iter().map(|byte| format!("{byte:02x}")).collect()),
        _ => None,
    }
}

fn as_map(value: &Value) -> Option<&std::collections::BTreeMap<String, Value>> {
    if let Value::Map(map) = value {
        Some(map)
    } else {
        None
    }
}

fn as_bool(value: &Value) -> Option<bool> {
    match value {
        Value::Boolean(value) => Some(*value),
        _ => None,
    }
}

fn as_array(value: Option<&Value>) -> Option<&[Value]> {
    match value? {
        Value::Array(values) => Some(values),
        _ => None,
    }
}

fn as_i64(value: &Value) -> Option<i64> {
    match value {
        Value::Integer(value) => Some(*value as i64),
        Value::Real(value) => Some(*value as i64),
        Value::Date(value) => Some(*value as i64),
        Value::String(value) => value.parse().ok(),
        _ => None,
    }
}

fn null_uuid() -> String {
    "00000000-0000-0000-0000-000000000000".into()
}
fn is_null_uuid(value: &str) -> bool {
    value == "00000000-0000-0000-0000-000000000000"
}

#[cfg(test)]
pub(crate) fn sample_cache_for_store_tests() -> Vec<u8> {
    tests::sample_cache(SUPPORTED_FIRESTORM_CACHE_VERSION)
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{write::GzEncoder, Compression};
    use std::io::Write;

    fn write_u32(out: &mut Vec<u8>, value: u32) {
        out.extend(value.to_be_bytes());
    }
    fn string(out: &mut Vec<u8>, value: &str) {
        out.push(b's');
        write_u32(out, value.len() as u32);
        out.extend(value.as_bytes());
    }
    fn key(out: &mut Vec<u8>, value: &str) {
        out.push(b'k');
        write_u32(out, value.len() as u32);
        out.extend(value.as_bytes());
    }
    fn map_start(out: &mut Vec<u8>, count: u32) {
        out.push(b'{');
        write_u32(out, count);
    }
    fn array_start(out: &mut Vec<u8>, count: u32) {
        out.push(b'[');
        write_u32(out, count);
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

    pub(super) fn sample_cache(version: u32) -> Vec<u8> {
        let mut out = version.to_be_bytes().to_vec();
        map_start(&mut out, 2);
        key(&mut out, "categories");
        array_start(&mut out, 2);
        map_start(&mut out, 4);
        key(&mut out, "cat_id");
        uuid(&mut out, "10000000-0000-0000-0000-000000000001");
        key(&mut out, "parent_id");
        uuid(&mut out, "00000000-0000-0000-0000-000000000000");
        key(&mut out, "owner_id");
        uuid(&mut out, "20000000-0000-0000-0000-000000000002");
        key(&mut out, "name");
        string(&mut out, "Closet");
        out.push(b'}');
        map_start(&mut out, 4);
        key(&mut out, "folder_id");
        uuid(&mut out, "10000000-0000-0000-0000-000000000002");
        key(&mut out, "parent_id");
        uuid(&mut out, "10000000-0000-0000-0000-000000000001");
        key(&mut out, "owner_id");
        uuid(&mut out, "20000000-0000-0000-0000-000000000002");
        key(&mut out, "name");
        string(&mut out, "Winter pieces");
        out.push(b'}');
        out.push(b']');
        key(&mut out, "items");
        array_start(&mut out, 2);
        map_start(&mut out, 8);
        key(&mut out, "item_id");
        uuid(&mut out, "30000000-0000-0000-0000-000000000003");
        key(&mut out, "parent_id");
        uuid(&mut out, "10000000-0000-0000-0000-000000000002");
        key(&mut out, "type");
        string(&mut out, "clothing");
        key(&mut out, "name");
        string(&mut out, "Rain coat");
        key(&mut out, "created_at");
        out.push(b'i');
        out.extend(1_700_000_000i32.to_be_bytes());
        key(&mut out, "favorite");
        map_start(&mut out, 1);
        key(&mut out, "toggled");
        out.push(b'1');
        out.push(b'}');
        key(&mut out, "permissions");
        map_start(&mut out, 4);
        key(&mut out, "owner_id");
        uuid(&mut out, "20000000-0000-0000-0000-000000000002");
        key(&mut out, "creator_id");
        uuid(&mut out, "40000000-0000-0000-0000-000000000004");
        key(&mut out, "owner_mask");
        out.push(b'i');
        out.extend(0x0008_ffffi32.to_be_bytes());
        key(&mut out, "next_owner_mask");
        out.push(b'i');
        out.extend(0x0008_a000i32.to_be_bytes());
        out.push(b'}');
        key(&mut out, "thumbnail");
        map_start(&mut out, 1);
        key(&mut out, "asset_id");
        uuid(&mut out, "50000000-0000-0000-0000-000000000005");
        out.push(b'}');
        out.push(b'}');
        map_start(&mut out, 6);
        key(&mut out, "item_id");
        uuid(&mut out, "30000000-0000-0000-0000-000000000004");
        key(&mut out, "parent_id");
        uuid(&mut out, "10000000-0000-0000-0000-000000000002");
        key(&mut out, "type");
        string(&mut out, "link");
        key(&mut out, "inv_type");
        string(&mut out, "link");
        key(&mut out, "asset_id");
        uuid(&mut out, "30000000-0000-0000-0000-000000000003");
        key(&mut out, "name");
        string(&mut out, "Rain coat link");
        out.push(b'}');
        out.push(b']');
        out.push(b'}');
        out.push(b'\n');
        out
    }

    #[test]
    fn parses_firestorm_v5_metadata_and_folder_path() {
        let snapshot = parse_cache(&sample_cache(5)).unwrap();
        assert_eq!(snapshot.format_version, 5);
        assert_eq!(snapshot.avatar_id, "20000000-0000-0000-0000-000000000002");
        assert_eq!(snapshot.categories.len(), 2);
        assert_eq!(snapshot.items[0].path, vec!["Closet", "Winter pieces"]);
        assert_eq!(snapshot.items[0].kind, "Clothing");
        assert_eq!(snapshot.items[0].created_at, Some(1_700_000_000));
        assert_eq!(
            snapshot.items[0].creator_id.as_deref(),
            Some("40000000-0000-0000-0000-000000000004")
        );
        assert_eq!(snapshot.items[0].owner_mask, Some(0x0008_ffff));
        assert_eq!(snapshot.items[0].next_owner_mask, Some(0x0008_a000));
        assert!(snapshot.items[0].is_favorite);
        assert_eq!(
            snapshot.items[0].thumbnail_id.as_deref(),
            Some("50000000-0000-0000-0000-000000000005")
        );
        assert!(snapshot.items[1].is_link);
        assert_eq!(
            snapshot.items[1].linked_target_id.as_deref(),
            Some("30000000-0000-0000-0000-000000000003")
        );
    }

    #[test]
    fn accepts_gzip_and_rejects_unknown_or_corrupt_formats() {
        let mut gzip = GzEncoder::new(Vec::new(), Compression::fast());
        gzip.write_all(&sample_cache(5)).unwrap();
        let gzip = gzip.finish().unwrap();
        assert_eq!(parse_cache(&gzip).unwrap().items.len(), 2);
        assert!(parse_cache(&sample_cache(99))
            .unwrap_err()
            .contains("format is version 99"));
        assert!(parse_cache(b"not an inventory cache").is_err());
    }

    #[test]
    fn scopes_inventory_by_firestorm_grid_filename() {
        let mut main_grid = parse_cache(&sample_cache(5)).unwrap();
        apply_cache_namespace(
            &mut main_grid,
            Path::new("20000000-0000-0000-0000-000000000002.inv.llsd.gz"),
        )
        .unwrap();
        assert_eq!(
            main_grid.avatar_id,
            "20000000-0000-0000-0000-000000000002@secondlife"
        );
        assert_eq!(main_grid.items[0].avatar_id, main_grid.avatar_id);

        let mut alternate_grid = parse_cache(&sample_cache(5)).unwrap();
        apply_cache_namespace(
            &mut alternate_grid,
            Path::new("20000000-0000-0000-0000-000000000002.grid_test.inv.llsd.gz"),
        )
        .unwrap();
        assert_eq!(
            alternate_grid.avatar_id,
            "20000000-0000-0000-0000-000000000002@grid_test"
        );
    }

    #[test]
    fn accepts_second_life_llsd_dates() {
        assert_eq!(as_i64(&Value::Date(1_700_000_000.75)), Some(1_700_000_000));
    }
}
