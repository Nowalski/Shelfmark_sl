use quick_xml::{escape::unescape, events::Event, Reader, XmlVersion};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
#[cfg(target_os = "windows")]
use std::process::Command;
use std::time::UNIX_EPOCH;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheCandidate {
    pub path: String,
    pub file_name: String,
    pub avatar_id: String,
    pub bytes: u64,
    pub modified_at_ms: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FirestormDiscovery {
    pub viewer_running: Option<bool>,
    pub cache_directories: Vec<String>,
    pub candidates: Vec<CacheCandidate>,
    pub avatar_names: HashMap<String, String>,
}

/// Find likely inventory snapshots in Firestorm's standard and configured cache directories.
/// This reads process names, directory entries and file metadata, then reads only matching names
/// from Firestorm's bounded local avatar-name cache. Inventory snapshot bytes remain unopened.
pub fn discover_firestorm_caches() -> FirestormDiscovery {
    let directories = cache_directories_to_check();
    let cache_directories = directories
        .iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect();
    let candidates = discover_in(directories.clone());
    let avatar_ids = candidates
        .iter()
        .map(|candidate| candidate.avatar_id.clone())
        .collect::<Vec<_>>();
    FirestormDiscovery {
        viewer_running: firestorm_is_running(),
        cache_directories,
        avatar_names: read_avatar_names(&directories, &avatar_ids),
        candidates,
    }
}

/// Resolve names for already-known avatar/grid IDs without scanning inventory directories.
pub fn resolve_firestorm_avatar_names(avatar_ids: &[String]) -> HashMap<String, String> {
    read_avatar_names(&cache_directories_to_check(), avatar_ids)
}

const MAX_AVATAR_NAME_CACHE_BYTES: u64 = 4 * 1024 * 1024;
const MAX_AVATAR_NAME_LOOKUPS: usize = 1_000;

fn read_avatar_names(directories: &[PathBuf], avatar_ids: &[String]) -> HashMap<String, String> {
    let mut requested: HashMap<String, Vec<String>> = HashMap::new();
    for avatar_id in avatar_ids.iter().take(MAX_AVATAR_NAME_LOOKUPS) {
        let Some((uuid, _grid)) = avatar_id.split_once('@') else {
            continue;
        };
        let Ok(uuid) = uuid::Uuid::parse_str(uuid) else {
            continue;
        };
        requested
            .entry(uuid.to_string())
            .or_default()
            .push(avatar_id.clone());
    }
    if requested.is_empty() {
        return HashMap::new();
    }
    let requested_uuids = requested.keys().cloned().collect::<HashSet<_>>();

    let mut resolved = HashMap::new();
    for directory in directories {
        if resolved.len() >= avatar_ids.len().min(MAX_AVATAR_NAME_LOOKUPS) {
            break;
        }
        let path = directory.join("avatar_name_cache.xml");
        let Ok(metadata) = std::fs::metadata(&path) else {
            continue;
        };
        if !metadata.is_file() || metadata.len() > MAX_AVATAR_NAME_CACHE_BYTES {
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        if bytes.len() as u64 > MAX_AVATAR_NAME_CACHE_BYTES {
            continue;
        }
        let Ok(xml) = std::str::from_utf8(&bytes) else {
            continue;
        };
        for (uuid, name) in parse_avatar_name_cache(xml, &requested_uuids) {
            if let Some(ids) = requested.get(&uuid) {
                for avatar_id in ids {
                    resolved
                        .entry(avatar_id.clone())
                        .or_insert_with(|| name.clone());
                }
            }
        }
    }
    resolved
}

#[derive(Default)]
struct AvatarNameParts {
    display_name: String,
    legacy_first_name: String,
    legacy_last_name: String,
    username: String,
}

enum NameCacheMapRole {
    Root,
    Agents,
    Avatar(String),
    Other,
}

struct NameCacheMap {
    role: NameCacheMapRole,
    pending_key: Option<String>,
    parts: AvatarNameParts,
}

struct NameCacheScalar {
    field: String,
    depth: usize,
    value: String,
}

fn parse_avatar_name_cache(xml: &str, requested_ids: &HashSet<String>) -> HashMap<String, String> {
    if requested_ids.is_empty() {
        return HashMap::new();
    }
    let parsed = (|| -> Option<HashMap<String, String>> {
        let mut reader = Reader::from_str(xml);
        reader.config_mut().trim_text(false);
        let mut maps: Vec<NameCacheMap> = Vec::new();
        let mut in_key = false;
        let mut key_text = String::new();
        let mut scalar: Option<NameCacheScalar> = None;
        let mut names = HashMap::new();

        loop {
            match reader.read_event().ok()? {
                Event::Start(element) => match element.local_name().as_ref() {
                    "map" => {
                        let role = if let Some(parent) = maps.last_mut() {
                            let key = parent.pending_key.take().unwrap_or_default();
                            match &parent.role {
                                NameCacheMapRole::Root if key == "agents" => {
                                    NameCacheMapRole::Agents
                                }
                                NameCacheMapRole::Agents => uuid::Uuid::parse_str(&key)
                                    .ok()
                                    .map(|id| id.to_string())
                                    .filter(|id| requested_ids.contains(id))
                                    .map(NameCacheMapRole::Avatar)
                                    .unwrap_or(NameCacheMapRole::Other),
                                _ => NameCacheMapRole::Other,
                            }
                        } else {
                            NameCacheMapRole::Root
                        };
                        maps.push(NameCacheMap {
                            role,
                            pending_key: None,
                            parts: AvatarNameParts::default(),
                        });
                    }
                    "key" => {
                        in_key = true;
                        key_text.clear();
                    }
                    "string" => {
                        let field = maps.last_mut().and_then(|map| {
                            let key = map.pending_key.take()?;
                            matches!(&map.role, NameCacheMapRole::Avatar(_))
                                .then_some(key)
                                .filter(|key| {
                                    matches!(
                                        key.as_str(),
                                        "display_name"
                                            | "legacy_first_name"
                                            | "legacy_last_name"
                                            | "username"
                                    )
                                })
                        });
                        if let Some(field) = field {
                            scalar = Some(NameCacheScalar {
                                field,
                                depth: maps.len(),
                                value: String::new(),
                            });
                        }
                    }
                    // A non-string scalar consumes its map key, so a later nested map cannot
                    // accidentally be interpreted as the value for that key.
                    "date" | "boolean" | "integer" | "real" | "uuid" | "uri" | "binary"
                    | "undef" | "array" => {
                        if let Some(map) = maps.last_mut() {
                            map.pending_key = None;
                        }
                    }
                    _ => {}
                },
                Event::Empty(element) => {
                    if element.local_name().as_ref() == "string" {
                        if let Some(map) = maps.last_mut() {
                            let field = map.pending_key.take().unwrap_or_default();
                            set_avatar_name_part(&mut map.parts, &field, "");
                        }
                    }
                }
                Event::Text(text) => {
                    let decoded = text.xml_content(XmlVersion::Implicit1_0);
                    let decoded = unescape(&decoded).ok()?;
                    if in_key {
                        key_text.push_str(&decoded);
                    } else if let Some(value) = scalar.as_mut() {
                        value.value.push_str(&decoded);
                    }
                }
                Event::CData(text) => {
                    let decoded = text.xml_content(XmlVersion::Implicit1_0);
                    if in_key {
                        key_text.push_str(&decoded);
                    } else if let Some(value) = scalar.as_mut() {
                        value.value.push_str(&decoded);
                    }
                }
                Event::GeneralRef(reference) => {
                    let entity = format!("&{};", reference.as_ref());
                    let decoded = unescape(&entity).ok()?;
                    if in_key {
                        key_text.push_str(&decoded);
                    } else if let Some(value) = scalar.as_mut() {
                        value.value.push_str(&decoded);
                    }
                }
                Event::End(element) => match element.local_name().as_ref() {
                    "key" if in_key => {
                        in_key = false;
                        if let Some(map) = maps.last_mut() {
                            map.pending_key = Some(key_text.trim().to_string());
                        }
                    }
                    "string" => {
                        if let Some(value) = scalar.take().filter(|value| value.depth == maps.len())
                        {
                            if let Some(map) = maps.last_mut() {
                                set_avatar_name_part(&mut map.parts, &value.field, &value.value);
                            }
                        }
                    }
                    "map" => {
                        let map = maps.pop()?;
                        if let NameCacheMapRole::Avatar(uuid) = map.role {
                            if let Some(name) = map.parts.label() {
                                names.insert(uuid, name);
                            }
                        }
                    }
                    _ => {}
                },
                Event::Eof => return Some(names),
                _ => {}
            }
        }
    })();
    parsed.unwrap_or_default()
}

impl AvatarNameParts {
    fn label(&self) -> Option<String> {
        let display = clean_avatar_name(&self.display_name);
        if display.is_some() {
            return display;
        }
        let legacy = format!(
            "{} {}",
            self.legacy_first_name.trim(),
            self.legacy_last_name.trim()
        );
        clean_avatar_name(&legacy).or_else(|| clean_avatar_name(&self.username))
    }
}

fn set_avatar_name_part(parts: &mut AvatarNameParts, field: &str, value: &str) {
    let value = value.to_string();
    match field {
        "display_name" => parts.display_name = value,
        "legacy_first_name" => parts.legacy_first_name = value,
        "legacy_last_name" => parts.legacy_last_name = value,
        "username" => parts.username = value,
        _ => {}
    }
}

fn clean_avatar_name(value: &str) -> Option<String> {
    let cleaned = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if cleaned.is_empty() || cleaned.chars().any(char::is_control) {
        return None;
    }
    Some(cleaned.chars().take(96).collect())
}

fn cache_directories_to_check() -> Vec<PathBuf> {
    let mut directories = default_cache_directories();
    if let Some(configured) = configured_cache_directory() {
        if configured.is_absolute()
            && !directories
                .iter()
                .any(|directory| same_directory(directory, &configured))
        {
            directories.push(configured);
        }
    }
    directories
}

fn same_directory(left: &Path, right: &Path) -> bool {
    let left = left.canonicalize().unwrap_or_else(|_| left.to_path_buf());
    let right = right.canonicalize().unwrap_or_else(|_| right.to_path_buf());
    #[cfg(target_os = "windows")]
    {
        left.to_string_lossy()
            .eq_ignore_ascii_case(&right.to_string_lossy())
    }
    #[cfg(not(target_os = "windows"))]
    {
        left == right
    }
}

/// Firestorm persists the user-selected CacheLocation in its LLSD settings file.
/// Read only this named string; the setting file can contain unrelated private preferences.
fn configured_cache_directory() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        let settings_file = PathBuf::from(std::env::var_os("APPDATA")?)
            .join("Firestorm_x64/user_settings/settings.xml");
        let metadata = std::fs::metadata(&settings_file).ok()?;
        if !metadata.is_file() || metadata.len() > 2 * 1024 * 1024 {
            return None;
        }
        let settings = std::fs::read_to_string(settings_file).ok()?;
        llsd_string_setting(&settings, "CacheLocation").map(PathBuf::from)
    }
    #[cfg(not(target_os = "windows"))]
    {
        None
    }
}

fn llsd_string_setting(xml: &str, setting: &str) -> Option<String> {
    let mut reader = Reader::from_str(xml);
    let mut reading_key = false;
    let mut key = String::new();
    let mut expect_string = false;
    let mut reading_string = false;
    let mut value = String::new();

    loop {
        match reader.read_event().ok()? {
            Event::Start(element) => match element.local_name().as_ref() {
                "key" => {
                    reading_key = true;
                    key.clear();
                }
                "string" if expect_string => {
                    expect_string = false;
                    reading_string = true;
                    value.clear();
                }
                _ if expect_string => expect_string = false,
                _ => {}
            },
            Event::Empty(element) if expect_string => {
                if element.local_name().as_ref() == "string" {
                    return None;
                }
                expect_string = false;
            }
            Event::Text(text) => {
                let text = text.xml_content(XmlVersion::Implicit1_0);
                let text = unescape(&text).ok()?;
                if reading_key {
                    key.push_str(&text);
                } else if reading_string {
                    value.push_str(&text);
                }
            }
            Event::CData(text) => {
                if reading_key {
                    key.push_str(&text.xml_content(XmlVersion::Implicit1_0));
                } else if reading_string {
                    value.push_str(&text.xml_content(XmlVersion::Implicit1_0));
                }
            }
            Event::GeneralRef(reference) => {
                let entity = format!("&{};", reference.as_ref());
                let decoded = unescape(&entity).ok()?;
                if reading_key {
                    key.push_str(&decoded);
                } else if reading_string {
                    value.push_str(&decoded);
                }
            }
            Event::End(element) => match element.local_name().as_ref() {
                "key" if reading_key => {
                    reading_key = false;
                    expect_string = key.trim() == setting;
                }
                "string" if reading_string => {
                    let value = value.trim();
                    return (!value.is_empty()).then(|| value.to_string());
                }
                _ => {}
            },
            Event::Eof => return None,
            _ => {}
        }
    }
}

#[cfg(target_os = "windows")]
pub fn firestorm_is_running() -> Option<bool> {
    use std::os::windows::process::CommandExt;

    let tasklist = PathBuf::from(std::env::var_os("SystemRoot")?).join("System32/tasklist.exe");
    let output = Command::new(tasklist)
        .args(["/FO", "CSV", "/NH"])
        .creation_flags(0x08000000) // CREATE_NO_WINDOW
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let listing = String::from_utf8_lossy(&output.stdout);
    Some(listing.lines().any(is_firestorm_process_row))
}

#[cfg(target_os = "windows")]
fn is_firestorm_process_row(line: &str) -> bool {
    let process_name = line
        .trim_start()
        .strip_prefix('"')
        .and_then(|line| line.split_once("\",\"").map(|(name, _)| name))
        .unwrap_or_default()
        .to_ascii_lowercase();
    process_name.starts_with("firestorm")
        && (process_name.ends_with(".exe")
            || process_name == "firestorm"
            || process_name == "firestorm-bin")
}

#[cfg(not(target_os = "windows"))]
pub fn firestorm_is_running() -> Option<bool> {
    None
}

fn discover_in(directories: Vec<PathBuf>) -> Vec<CacheCandidate> {
    let mut candidates = Vec::new();
    for directory in directories {
        let Ok(entries) = std::fs::read_dir(directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if let Some(path_string) = path.to_str() {
                if let Ok(candidate) = inspect_cache_file(path_string) {
                    candidates.push(candidate);
                }
            }
        }
    }
    candidates.sort_by(|left, right| right.modified_at_ms.cmp(&left.modified_at_ms));
    candidates.truncate(100);
    candidates
}

pub fn inspect_cache_file(path: &str) -> Result<CacheCandidate, String> {
    let path = PathBuf::from(path);
    if !path.is_file() {
        return Err("Choose a Firestorm inventory cache file.".into());
    }
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("The selected file does not have a readable Firestorm cache filename.")?;
    let avatar_id = cache_avatar_id(file_name)
        .ok_or("The selected filename is not a recognized Firestorm inventory cache.")?;
    let metadata = std::fs::metadata(&path)
        .map_err(|error| format!("Could not inspect the selected file: {error}"))?;
    let modified_at_ms = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or_default();
    Ok(CacheCandidate {
        path: path.to_string_lossy().into_owned(),
        file_name: file_name.to_string(),
        avatar_id,
        bytes: metadata.len(),
        modified_at_ms,
    })
}

fn default_cache_directories() -> Vec<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        return std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .map(|path| vec![path.join("Firestorm_x64")])
            .unwrap_or_default();
    }
    #[cfg(target_os = "macos")]
    {
        return std::env::var_os("HOME")
            .map(PathBuf::from)
            .map(|path| vec![path.join("Library/Caches/Firestorm_x64")])
            .unwrap_or_default();
    }
    #[cfg(target_os = "linux")]
    {
        return std::env::var_os("HOME")
            .map(PathBuf::from)
            .map(|path| vec![path.join(".firestorm_x64/cache")])
            .unwrap_or_default();
    }
    #[allow(unreachable_code)]
    Vec::new()
}

/// Locate a normally installed Firestorm executable without scanning build trees.
pub fn discover_stock_firestorm_executable() -> Option<String> {
    #[cfg(target_os = "windows")]
    {
        let roots = [
            std::env::var_os("ProgramFiles").map(PathBuf::from),
            std::env::var_os("ProgramFiles(x86)").map(PathBuf::from),
            std::env::var_os("LOCALAPPDATA").map(PathBuf::from),
        ];
        let install_dirs = [
            "Firestorm-Releasex64",
            "FirestormOS-Releasex64",
            "Firestorm_x64",
            "FirestormOS_x64",
        ];
        let executable_names = [
            "Firestorm-Releasex64.exe",
            "FirestormOS-Releasex64.exe",
            "firestorm-bin.exe",
        ];
        for root in roots.into_iter().flatten() {
            for install_dir in install_dirs {
                for executable_name in executable_names {
                    let candidate = root.join(install_dir).join(executable_name);
                    if candidate.is_file() {
                        return Some(
                            candidate
                                .canonicalize()
                                .unwrap_or(candidate)
                                .to_string_lossy()
                                .into_owned(),
                        );
                    }
                }
            }
        }
    }
    None
}

fn cache_avatar_id(file_name: &str) -> Option<String> {
    let lower = file_name.to_ascii_lowercase();
    let mut stem = lower.as_str();
    if let Some(value) = stem.strip_suffix(".gz") {
        stem = value;
    }
    if let Some(value) = stem.strip_suffix(".inv.llsd") {
        stem = value;
    } else if let Some(value) = stem.strip_suffix(".inv") {
        stem = value;
    } else {
        return None;
    }

    let (owner, grid) = stem.split_once('.').unwrap_or((stem, "secondlife"));
    let owner = uuid::Uuid::parse_str(owner).ok()?.to_string();
    if grid.is_empty()
        || grid.len() > 64
        || !grid.chars().all(|character| {
            character.is_ascii_alphanumeric() || character == '-' || character == '_'
        })
    {
        return None;
    }
    Some(format!("{owner}@{}", grid.to_ascii_lowercase()))
}

#[cfg(test)]
mod tests {
    use super::{cache_avatar_id, discover_in, inspect_cache_file, llsd_string_setting};
    use std::path::PathBuf;

    #[test]
    fn reads_only_the_requested_llsd_string_setting() {
        let settings = r#"<llsd><map>
            <key>WindowWidth</key><integer>1280</integer>
            <key>CacheLocation</key><string>D:\SL &amp; Stuff\Cache</string>
            <key>OtherPreference</key><string>private value</string>
        </map></llsd>"#;
        assert_eq!(
            llsd_string_setting(settings, "CacheLocation"),
            Some(r"D:\SL & Stuff\Cache".into())
        );
        assert_eq!(
            llsd_string_setting(settings, "OtherPreference"),
            Some("private value".into())
        );
    }

    #[test]
    fn ignores_empty_non_string_and_missing_settings() {
        assert_eq!(
            llsd_string_setting(
                r#"<llsd><map><key>CacheLocation</key><string /></map></llsd>"#,
                "CacheLocation"
            ),
            None
        );
        assert_eq!(
            llsd_string_setting(
                r#"<llsd><map><key>CacheLocation</key><integer>0</integer><key>Other</key><string>ignored</string></map></llsd>"#,
                "CacheLocation"
            ),
            None
        );
    }

    #[test]
    fn reads_only_requested_firestorm_avatar_names_and_prefers_display_name() {
        let first = "20000000-0000-0000-0000-000000000002";
        let second = "20000000-0000-0000-0000-000000000003";
        let other = "20000000-0000-0000-0000-000000000004";
        let xml = format!(
            r#"<llsd><map><key>agents</key><map>
            <key>{first}</key><map><key>display_name</key><string>Test &amp; Friend</string>
            <key>legacy_first_name</key><string>Legacy</string><key>legacy_last_name</key><string>Name</string>
            <key>username</key><string>test.friend</string></map>
            <key>{second}</key><map><key>legacy_first_name</key><string>Legacy</string>
            <key>legacy_last_name</key><string>Resident</string><key>username</key><string>legacy.resident</string></map>
            <key>{other}</key><map><key>display_name</key><string>Not requested</string></map>
            </map></map></llsd>"#
        );
        let requested = [first.to_string(), second.to_string()]
            .into_iter()
            .collect();

        let names = super::parse_avatar_name_cache(&xml, &requested);

        assert_eq!(names.len(), 2);
        assert_eq!(names.get(first).map(String::as_str), Some("Test & Friend"));
        assert_eq!(
            names.get(second).map(String::as_str),
            Some("Legacy Resident")
        );
        assert!(!names.contains_key(other));
    }

    #[test]
    fn avatar_name_cache_uses_username_as_last_resort_and_ignores_invalid_xml() {
        let requested = ["20000000-0000-0000-0000-000000000002".to_string()]
            .into_iter()
            .collect();
        let xml = r#"<llsd><map><key>agents</key><map>
            <key>20000000-0000-0000-0000-000000000002</key><map>
            <key>display_name</key><string>  </string>
            <key>username</key><string>fallback.name</string>
            </map></map></map></llsd>"#;

        let names = super::parse_avatar_name_cache(xml, &requested);

        assert_eq!(
            names
                .get("20000000-0000-0000-0000-000000000002")
                .map(String::as_str),
            Some("fallback.name")
        );
        assert!(super::parse_avatar_name_cache("<llsd><map>", &requested).is_empty());
    }

    #[test]
    fn reads_a_bounded_name_file_for_matching_avatar_grid_keys_only() {
        let directory =
            std::env::temp_dir().join(format!("shelfmark-avatar-names-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&directory).unwrap();
        let id = "20000000-0000-0000-0000-000000000002";
        let xml = format!(
            r#"<llsd><map><key>agents</key><map><key>{id}</key><map><key>display_name</key><string>Local Fixture</string></map></map></map></llsd>"#
        );
        std::fs::write(directory.join("avatar_name_cache.xml"), xml).unwrap();
        let requested = [format!("{id}@secondlife"), format!("{id}@test_grid")];

        let names = super::read_avatar_names(std::slice::from_ref(&directory), &requested);

        assert_eq!(names.len(), 2);
        assert_eq!(
            names.get(&requested[0]).map(String::as_str),
            Some("Local Fixture")
        );
        assert_eq!(
            names.get(&requested[1]).map(String::as_str),
            Some("Local Fixture")
        );
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn recognizes_default_and_alternate_grid_inventory_cache_names() {
        assert_eq!(
            cache_avatar_id("20000000-0000-0000-0000-000000000002.inv.llsd.gz"),
            Some("20000000-0000-0000-0000-000000000002@secondlife".into())
        );
        assert_eq!(
            cache_avatar_id("20000000-0000-0000-0000-000000000002.grid_test.inv.llsd.gz"),
            Some("20000000-0000-0000-0000-000000000002@grid_test".into())
        );
        assert_eq!(
            cache_avatar_id("20000000-0000-0000-0000-000000000002.inv"),
            Some("20000000-0000-0000-0000-000000000002@secondlife".into())
        );
    }

    #[test]
    fn ignores_non_inventory_and_malformed_cache_names() {
        assert_eq!(cache_avatar_id("texture.cache.gz"), None);
        assert_eq!(cache_avatar_id("not-an-avatar.inv.llsd.gz"), None);
        assert_eq!(
            cache_avatar_id("20000000-0000-0000-0000-000000000002.bad%grid.inv"),
            None
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn recognizes_firestorm_viewer_process_rows() {
        assert!(super::is_firestorm_process_row(
            "\"FirestormOS-Releasex64.exe\",\"1234\",\"Console\""
        ));
        assert!(super::is_firestorm_process_row(
            "\"firestorm-bin.exe\",\"1234\",\"Console\""
        ));
        assert!(!super::is_firestorm_process_row(
            "\"Shelfmark.exe\",\"1234\",\"Console\""
        ));
    }

    #[test]
    fn discovery_lists_names_and_metadata_without_parsing_cache_bytes() {
        let directory =
            std::env::temp_dir().join(format!("shelfmark-discovery-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&directory).unwrap();
        let expected = directory.join("20000000-0000-0000-0000-000000000002.inv.llsd.gz");
        std::fs::write(&expected, b"intentionally not a cache").unwrap();
        std::fs::write(directory.join("not-an-inventory.gz"), b"ignored").unwrap();

        let listed = discover_in(vec![PathBuf::from(&directory)]);
        assert_eq!(listed.len(), 1);
        assert_eq!(
            listed[0].avatar_id,
            "20000000-0000-0000-0000-000000000002@secondlife"
        );
        assert_eq!(listed[0].bytes, b"intentionally not a cache".len() as u64);
        assert_eq!(
            inspect_cache_file(expected.to_str().unwrap())
                .unwrap()
                .file_name,
            expected.file_name().unwrap().to_string_lossy()
        );

        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn discovery_combines_default_and_configured_cache_directories() {
        let root =
            std::env::temp_dir().join(format!("shelfmark-cache-paths-{}", uuid::Uuid::new_v4()));
        let standard = root.join("standard");
        let configured = root.join("configured");
        std::fs::create_dir_all(&standard).unwrap();
        std::fs::create_dir_all(&configured).unwrap();
        std::fs::write(
            standard.join("20000000-0000-0000-0000-000000000002.inv.llsd.gz"),
            b"standard cache fixture",
        )
        .unwrap();
        std::fs::write(
            configured.join("20000000-0000-0000-0000-000000000003.inv.llsd.gz"),
            b"configured cache fixture",
        )
        .unwrap();

        let listed = discover_in(vec![standard, configured]);
        let avatars = listed
            .into_iter()
            .map(|candidate| candidate.avatar_id)
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(avatars.len(), 2);
        assert!(avatars.contains("20000000-0000-0000-0000-000000000002@secondlife"));
        assert!(avatars.contains("20000000-0000-0000-0000-000000000003@secondlife"));

        std::fs::remove_dir_all(root).unwrap();
    }
}
