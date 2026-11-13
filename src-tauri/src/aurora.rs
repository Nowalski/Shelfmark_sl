//! Aurora Viewer support.
//!
//! Aurora (github.com/Aurora-Viewer/Aurora-Viewer) is a Second Life viewer
//! written in Rust. It keeps its inventory snapshot in a sealed disk cache:
//!
//! * file: `<cache>/store/inventory/<name>.inv`, where `<name>` is the first
//!   128 bits (hex) of HMAC-SHA256(names key, agent UUID bytes);
//! * contents: AES-256-GCM in 64 KiB chunks under a per-file key (HKDF-SHA256
//!   of the installation key and the file's salt), see `aurora-assets/src/sealed.rs`;
//! * installation key: 32 random bytes in the OS credential store (service
//!   "Aurora Viewer cache", account "disk cache key v1"), or `config/cache.key`
//!   when no credential store is available (`aurora-viewer/src/cache_key.rs`);
//! * plain bytes: binary LLSD `{version: 3, folders: [...], items: [...]}`
//!   (`aurora-viewer/src/world/inventory.rs`, `Inventory::save_cache`).
//!
//! Shelfmark only ever *reads* the installation key: it never creates,
//! replaces or deletes it, and it never writes into Aurora's folders. The
//! decrypted inventory is translated to the Firestorm cache shape and goes
//! through the same import as a Firestorm snapshot.

use crate::discovery::CacheCandidate;
use crate::inventory::{self, Snapshot};
use crate::llsd::{self, Value};
use ring::{aead, hkdf, hmac};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

/// Grid label for catalogues imported from Aurora. Aurora's inventory cache is
/// keyed by agent only, so the grid is unknown; a separate label keeps these
/// catalogues apart from Firestorm imports and from Firestorm-only live actions.
pub const AURORA_GRID_ID: &str = "aurora";
pub const VIEWER_ID: &str = "aurora";

const MAGIC: &[u8; 4] = b"AUSL";
const SALT: usize = 16;
const HEADER: usize = 4 + SALT + 8;
const CHUNK: usize = 64 << 10;
const TAG: usize = 16;
const FILE_INFO: &[u8] = b"aurora cache file v1";
const NAMES_SALT: &[u8] = b"aurora cache names";
const NAMES_INFO: &[u8] = b"aurora cache names v1";
const NAME_LEN: usize = 32;

const KEY_SERVICE: &str = "Aurora Viewer cache";
const KEY_ACCOUNT: &str = "disk cache key v1";
const KEY_FILE: &str = "cache.key";
const CHOSEN_CACHE_ROOT: &str = "AuroraViewerCache";
const ASSET_STORE: &str = "store";
const MAX_SETTINGS_BYTES: u64 = 4 * 1024 * 1024;

/// The format version Aurora writes in its inventory cache today.
pub const SUPPORTED_AURORA_INVENTORY_VERSION: i32 = 3;

/// Aurora's installation key.
#[derive(Clone)]
pub struct InstallKey([u8; 32]);

impl std::fmt::Debug for InstallKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("InstallKey(..)")
    }
}

impl InstallKey {
    #[cfg(test)]
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    fn parse(bytes: &[u8]) -> Option<Self> {
        <[u8; 32]>::try_from(bytes).ok().map(Self)
    }

    /// The store file stem of `id`: 32 lowercase hex digits.
    pub fn file_stem(&self, id: &uuid::Uuid) -> String {
        let prk = hkdf::Salt::new(hkdf::HKDF_SHA256, NAMES_SALT).extract(&self.0);
        let okm = prk
            .expand(&[NAMES_INFO], hmac::HMAC_SHA256)
            .expect("one SHA-256 output");
        let key = hmac::Key::from(okm);
        let tag = hmac::sign(&key, id.as_bytes());
        tag.as_ref()[..NAME_LEN / 2]
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    fn file_key(&self, header: &[u8]) -> Result<aead::LessSafeKey, String> {
        let prk = hkdf::Salt::new(hkdf::HKDF_SHA256, &header[4..4 + SALT]).extract(&self.0);
        let okm = prk
            .expand(&[FILE_INFO], &aead::AES_256_GCM)
            .map_err(|_| "Aurora cache key derivation failed.".to_string())?;
        Ok(aead::LessSafeKey::new(aead::UnboundKey::from(okm)))
    }

    /// Decrypt a whole sealed Aurora cache file.
    pub fn open(&self, mut file: Vec<u8>) -> Result<Vec<u8>, String> {
        if !is_sealed(&file) {
            return Err("This file is not an Aurora Viewer cache file.".into());
        }
        let header: [u8; HEADER] = file[..HEADER].try_into().expect("header length");
        let len = u64::from_le_bytes(header[4 + SALT..].try_into().expect("u64"));
        let chunks = len.div_ceil(CHUNK as u64).max(1);
        let expected = (HEADER as u64)
            .checked_add(len)
            .and_then(|total| total.checked_add(chunks * TAG as u64));
        if expected != Some(file.len() as u64) {
            return Err(
                "The Aurora inventory cache is incomplete. Close Aurora and try again.".into(),
            );
        }
        let key = self.file_key(&header)?;
        let (mut read, mut write) = (HEADER, 0usize);
        for index in 0..chunks {
            let n = (len - index * CHUNK as u64).min(CHUNK as u64) as usize;
            key.open_in_place(
                nonce(index),
                aead::Aad::from(&header),
                &mut file[read..read + n + TAG],
            )
            .map_err(|_| {
                "Aurora's cache key does not open this file. It may belong to another Aurora installation or Windows account, or Aurora reset its cache key.".to_string()
            })?;
            file.copy_within(read..read + n, write);
            read += n + TAG;
            write += n;
        }
        file.truncate(write);
        Ok(file)
    }

    #[cfg(test)]
    pub fn seal(&self, plain: &[u8], salt: [u8; SALT]) -> Vec<u8> {
        let mut header = [0u8; HEADER];
        header[..4].copy_from_slice(MAGIC);
        header[4..4 + SALT].copy_from_slice(&salt);
        header[4 + SALT..].copy_from_slice(&(plain.len() as u64).to_le_bytes());
        let key = self.file_key(&header).unwrap();
        let mut out = header.to_vec();
        let mut parts: Vec<&[u8]> = plain.chunks(CHUNK).collect();
        if parts.is_empty() {
            parts.push(&[]);
        }
        for (index, part) in parts.into_iter().enumerate() {
            let start = out.len();
            out.extend_from_slice(part);
            let tag = key
                .seal_in_place_separate_tag(
                    nonce(index as u64),
                    aead::Aad::from(&header),
                    &mut out[start..],
                )
                .unwrap();
            out.extend_from_slice(tag.as_ref());
        }
        out
    }
}

fn nonce(chunk: u64) -> aead::Nonce {
    let mut bytes = [0u8; aead::NONCE_LEN];
    bytes[4..].copy_from_slice(&chunk.to_be_bytes());
    aead::Nonce::assume_unique_for_key(bytes)
}

/// Whether these bytes start with the sealed Aurora cache magic.
pub fn is_sealed_magic(head: &[u8]) -> bool {
    head.starts_with(MAGIC)
}

/// Whether these bytes start like a sealed Aurora cache file.
pub fn is_sealed(bytes: &[u8]) -> bool {
    bytes.len() >= HEADER && bytes.starts_with(MAGIC)
}

// ---------------------------------------------------------------- folders

/// Aurora's settings folder (`ProjectDirs::from("org", "Aurora", "AuroraViewer")`).
pub fn config_dir() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        return std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .map(|path| path.join("Aurora").join("AuroraViewer").join("config"));
    }
    #[cfg(target_os = "macos")]
    {
        return std::env::var_os("HOME")
            .map(PathBuf::from)
            .map(|path| path.join("Library/Application Support/org.Aurora.AuroraViewer"));
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        return std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
            .map(|path| path.join("auroraviewer"));
    }
    #[allow(unreachable_code)]
    None
}

/// Aurora's default cache folder.
pub fn default_cache_dir() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        return std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .map(|path| path.join("Aurora").join("AuroraViewer").join("cache"));
    }
    #[cfg(target_os = "macos")]
    {
        return std::env::var_os("HOME")
            .map(PathBuf::from)
            .map(|path| path.join("Library/Caches/org.Aurora.AuroraViewer"));
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        return std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
            .map(|path| path.join("auroraviewer"));
    }
    #[allow(unreachable_code)]
    None
}

/// The cache folders Aurora may be using: the folder it recorded on its last
/// run, the folder chosen in its preferences, and its default folder.
pub fn cache_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(config) = config_dir() {
        dirs.extend(cache_dirs_from_config(&config));
    }
    if let Some(default) = default_cache_dir() {
        dirs.push(default);
    }
    let mut seen = HashSet::new();
    dirs.retain(|dir| seen.insert(dir.to_string_lossy().to_ascii_lowercase()));
    dirs
}

fn cache_dirs_from_config(config: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    // `cache_location.txt`: the cache folder of Aurora's last run.
    if let Some(text) = read_small(&config.join("cache_location.txt")) {
        let last = String::from_utf8_lossy(&text).trim().to_string();
        if !last.is_empty() {
            dirs.push(PathBuf::from(last));
        }
    }
    // `settings.json` `cache_location`: a chosen folder, used as `<folder>/AuroraViewerCache`.
    if let Some(bytes) = read_small(&config.join("settings.json")) {
        if let Ok(json) = serde_json::from_slice::<serde_json::Value>(&bytes) {
            if let Some(chosen) = json.get("cache_location").and_then(|v| v.as_str()) {
                let chosen = chosen.trim();
                if !chosen.is_empty() {
                    dirs.push(Path::new(chosen).join(CHOSEN_CACHE_ROOT));
                }
            }
        }
    }
    dirs
}

fn read_small(path: &Path) -> Option<Vec<u8>> {
    let metadata = std::fs::metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() > MAX_SETTINGS_BYTES {
        return None;
    }
    std::fs::read(path).ok()
}

/// Agent UUIDs that have logged in with Aurora (`config/accounts/<uuid>/`).
fn known_agents(config: Option<&Path>) -> Vec<uuid::Uuid> {
    let Some(config) = config else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(config.join("accounts")) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| {
            entry
                .file_name()
                .to_str()
                .and_then(|n| uuid::Uuid::parse_str(n).ok())
        })
        .filter(|id| !id.is_nil())
        .collect()
}

fn scoped_avatar_id(id: &uuid::Uuid) -> String {
    format!("{}@{AURORA_GRID_ID}", id.hyphenated())
}

// ---------------------------------------------------------------- key

/// Read Aurora's installation key. Read-only: a missing key is an error, never
/// replaced, because a new key would make Aurora discard its whole cache.
pub fn load_key() -> Result<InstallKey, String> {
    let mut store_error = None;
    match keyring::Entry::new(KEY_SERVICE, KEY_ACCOUNT) {
        Ok(entry) => match entry.get_secret() {
            Ok(bytes) => {
                if let Some(key) = InstallKey::parse(&bytes) {
                    return Ok(key);
                }
                store_error =
                    Some("the stored Aurora cache key has an unexpected size".to_string());
            }
            Err(keyring::Error::NoEntry) => {}
            Err(error) => store_error = Some(error.to_string()),
        },
        Err(error) => store_error = Some(error.to_string()),
    }
    if let Some(file) = config_dir().map(|dir| dir.join(KEY_FILE)) {
        if let Some(key) = read_small(&file).and_then(|bytes| InstallKey::parse(&bytes)) {
            return Ok(key);
        }
    }
    Err(match store_error {
        Some(error) => format!("Shelfmark could not read Aurora's cache key from the system credential store ({error}). Start Aurora once on this computer and account, then try again."),
        None => "Aurora's cache key was not found on this computer. Log in with Aurora once on this computer and account, then try again.".into(),
    })
}

// ---------------------------------------------------------------- discovery

#[derive(Debug, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuroraDiscovery {
    pub installed: bool,
    pub viewer_running: Option<bool>,
    pub cache_directories: Vec<String>,
    pub candidates: Vec<CacheCandidate>,
    /// Why candidates could not be matched to avatars, if they could not.
    pub key_error: Option<String>,
}

/// Find Aurora inventory caches. Matching a file to its avatar needs the
/// installation key (file names are keyed hashes); without it the files are
/// still listed, with the avatar left empty.
pub fn discover() -> AuroraDiscovery {
    let dirs = cache_dirs();
    let config = config_dir();
    let installed = config.as_ref().is_some_and(|c| c.is_dir())
        || dirs.iter().any(|dir| dir.join(ASSET_STORE).is_dir());
    let mut discovery = AuroraDiscovery {
        installed,
        viewer_running: aurora_is_running(),
        cache_directories: dirs
            .iter()
            .map(|d| d.to_string_lossy().into_owned())
            .collect(),
        ..Default::default()
    };
    let files: Vec<PathBuf> = dirs
        .iter()
        .filter_map(|dir| std::fs::read_dir(dir.join(ASSET_STORE).join("inventory")).ok())
        .flat_map(|entries| entries.flatten().map(|entry| entry.path()))
        .filter(|path| {
            path.extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("inv"))
        })
        .filter(|path| path.is_file())
        .collect();
    if files.is_empty() {
        return discovery;
    }
    let names: HashMap<String, uuid::Uuid> = match load_key() {
        Ok(key) => known_agents(config.as_deref())
            .into_iter()
            .map(|agent| (key.file_stem(&agent), agent))
            .collect(),
        Err(error) => {
            discovery.key_error = Some(error);
            HashMap::new()
        }
    };
    for path in files {
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        let avatar_id = names.get(&stem).map(scoped_avatar_id);
        if let Ok(candidate) = candidate_for(&path, avatar_id) {
            discovery.candidates.push(candidate);
        }
    }
    discovery
        .candidates
        .sort_by(|left, right| right.modified_at_ms.cmp(&left.modified_at_ms));
    discovery
}

fn candidate_for(path: &Path, avatar_id: Option<String>) -> Result<CacheCandidate, String> {
    let metadata = std::fs::metadata(path).map_err(|e| e.to_string())?;
    Ok(CacheCandidate {
        path: path.to_string_lossy().into_owned(),
        file_name: path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_string(),
        avatar_id: avatar_id.unwrap_or_default(),
        bytes: metadata.len(),
        modified_at_ms: metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_millis().min(i64::MAX as u128) as i64)
            .unwrap_or_default(),
        viewer: VIEWER_ID.into(),
    })
}

/// Inspect a file the user picked: it must be a sealed Aurora cache file.
pub fn inspect_cache_file(path: &Path) -> Result<CacheCandidate, String> {
    let mut head = [0u8; HEADER];
    let mut file =
        std::fs::File::open(path).map_err(|e| format!("Could not read the selected file: {e}"))?;
    std::io::Read::read_exact(&mut file, &mut head).map_err(|_| {
        "The selected file is too short to be an Aurora inventory cache.".to_string()
    })?;
    if !is_sealed(&head) {
        return Err("The selected file is not an Aurora Viewer cache file.".into());
    }
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let key = load_key()?;
    let mut avatar = known_agents(config_dir().as_deref())
        .into_iter()
        .find(|agent| key.file_stem(agent) == stem)
        .map(|id| scoped_avatar_id(&id));
    if avatar.is_none() {
        // Not a known account's file name: open it to find its owner.
        let bytes =
            std::fs::read(path).map_err(|e| format!("Could not read the selected file: {e}"))?;
        let plain = key.open(bytes)?;
        let snapshot = parse_plain_inventory(&plain, None, &|| false, &mut |_, _, _| {})?;
        avatar = Some(snapshot.avatar_id);
    }
    candidate_for(path, avatar)
}

#[cfg(target_os = "windows")]
pub fn aurora_is_running() -> Option<bool> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let output = std::process::Command::new("tasklist")
        .args(["/FI", "IMAGENAME eq aurora-viewer.exe", "/FO", "CSV", "/NH"])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let listing = String::from_utf8_lossy(&output.stdout).to_ascii_lowercase();
    Some(
        listing
            .lines()
            .any(|line| line.trim_start().starts_with("\"aurora-viewer.exe\"")),
    )
}

#[cfg(not(target_os = "windows"))]
pub fn aurora_is_running() -> Option<bool> {
    None
}

// ---------------------------------------------------------------- parsing

/// Decrypt and parse a sealed Aurora inventory cache into a snapshot whose
/// avatar id is scoped as `<agent>@aurora`.
pub fn parse_sealed_inventory(
    sealed: Vec<u8>,
    path: &Path,
    key: &InstallKey,
    is_cancelled: &dyn Fn() -> bool,
    report: &mut dyn FnMut(&str, u64, Option<u64>),
) -> Result<Snapshot, String> {
    report("decompressing", 0, None);
    let plain = key.open(sealed)?;
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let agent = known_agents(config_dir().as_deref())
        .into_iter()
        .find(|agent| key.file_stem(agent) == stem);
    parse_plain_inventory(&plain, agent, is_cancelled, report)
}

pub fn parse_plain_inventory(
    plain: &[u8],
    agent: Option<uuid::Uuid>,
    is_cancelled: &dyn Fn() -> bool,
    report: &mut dyn FnMut(&str, u64, Option<u64>),
) -> Result<Snapshot, String> {
    report("cache_ready", plain.len() as u64, Some(plain.len() as u64));
    let mut payload = plain;
    const BINARY_HEADER: &[u8] = b"<? LLSD/Binary ?>";
    if let Some(rest) = payload.strip_prefix(BINARY_HEADER) {
        payload = rest.trim_ascii_start();
    }
    let (root, _) = llsd::parse_with_progress(payload, is_cancelled, &mut |offset| {
        report("decoding", offset as u64, Some(payload.len() as u64));
    })
    .map_err(|error| {
        if error == llsd::CANCELLED_MESSAGE {
            error
        } else {
            format!("The Aurora inventory could not be decoded: {error}.")
        }
    })?;
    let Value::Map(map) = &root else {
        return Err("The Aurora inventory cache does not contain an inventory map.".into());
    };
    let version = match map.get("version") {
        Some(Value::Integer(v)) => *v,
        _ => 0,
    };
    if version != SUPPORTED_AURORA_INVENTORY_VERSION {
        return Err(format!(
            "This Aurora inventory cache is format version {version}; Shelfmark reads version {SUPPORTED_AURORA_INVENTORY_VERSION}. Update Shelfmark, or report this on GitHub."
        ));
    }
    let translated = translate(map, agent)?;
    let mut snapshot =
        inventory::snapshot_from_root(&translated, version as u32, is_cancelled, report)?;
    let scoped = format!(
        "{}@{AURORA_GRID_ID}",
        snapshot.avatar_id.to_ascii_lowercase()
    );
    snapshot.avatar_id = scoped.clone();
    for item in &mut snapshot.items {
        item.avatar_id = scoped.clone();
    }
    Ok(snapshot)
}

fn uuid_of(map: &BTreeMap<String, Value>, key: &str) -> Option<String> {
    match map.get(key)? {
        Value::Uuid(value) | Value::String(value) => uuid::Uuid::parse_str(value.trim())
            .ok()
            .map(|id| id.hyphenated().to_string()),
        _ => None,
    }
}

fn int_of(map: &BTreeMap<String, Value>, key: &str) -> Option<i64> {
    match map.get(key)? {
        Value::Integer(v) => Some(*v as i64),
        Value::Real(v) => Some(*v as i64),
        Value::Date(v) => Some(*v as i64),
        Value::Boolean(v) => Some(*v as i64),
        _ => None,
    }
}

fn string_of(map: &BTreeMap<String, Value>, key: &str) -> Option<String> {
    match map.get(key)? {
        Value::String(v) => Some(v.clone()),
        _ => None,
    }
}

fn bool_of(map: &BTreeMap<String, Value>, key: &str) -> bool {
    matches!(
        map.get(key),
        Some(Value::Boolean(true)) | Some(Value::Integer(1))
    )
}

const NIL: &str = "00000000-0000-0000-0000-000000000000";

/// Translate Aurora's `{folders, items}` into Firestorm's `{categories, items}`,
/// keeping only the agent's own inventory tree (Aurora also saves fetched
/// Library folders, which belong to another owner).
fn translate(map: &BTreeMap<String, Value>, agent: Option<uuid::Uuid>) -> Result<Value, String> {
    let empty = Vec::new();
    let folders = match map.get("folders") {
        Some(Value::Array(v)) => v,
        _ => &empty,
    };
    let items = match map.get("items") {
        Some(Value::Array(v)) => v,
        _ => &empty,
    };

    // Folder tree: the root of each folder.
    let mut parent_of: HashMap<String, String> = HashMap::new();
    for folder in folders {
        if let Value::Map(m) = folder {
            if let Some(id) = uuid_of(m, "id") {
                parent_of.insert(id, uuid_of(m, "parent").unwrap_or_else(|| NIL.into()));
            }
        }
    }
    let root_of = |start: &str| -> String {
        let mut current = start.to_string();
        let mut seen = HashSet::new();
        while let Some(parent) = parent_of.get(&current) {
            if parent == NIL || !parent_of.contains_key(parent) || !seen.insert(current.clone()) {
                break;
            }
            current = parent.clone();
        }
        current
    };
    // Roots of the agent's own inventory hold a Trash folder; the Library does not.
    let mut trash_roots = HashSet::new();
    for folder in folders {
        if let Value::Map(m) = folder {
            if int_of(m, "type") == Some(14) {
                if let Some(id) = uuid_of(m, "id") {
                    trash_roots.insert(root_of(&id));
                }
            }
        }
    }

    // The owner: the account the file name belongs to, else the most common
    // owner of items under a root with Trash, else of all items.
    let owner = agent.map(|id| id.hyphenated().to_string()).or_else(|| {
        let mut counts: BTreeMap<String, (usize, usize)> = BTreeMap::new();
        for item in items {
            if let Value::Map(m) = item {
                if let Some(owner) = uuid_of(m, "owner").filter(|o| o != NIL) {
                    let in_own_tree = uuid_of(m, "parent")
                        .is_some_and(|parent| trash_roots.contains(&root_of(&parent)));
                    let entry = counts.entry(owner).or_default();
                    entry.0 += usize::from(in_own_tree);
                    entry.1 += 1;
                }
            }
        }
        counts
            .into_iter()
            .max_by(|(left_id, left), (right_id, right)| {
                left.cmp(right).then(right_id.cmp(left_id))
            })
            .map(|(owner, _)| owner)
    });
    let owner = owner.ok_or("The Aurora inventory cache does not name its owner.")?;

    let mut owner_roots = HashSet::new();
    for item in items {
        if let Value::Map(m) = item {
            if uuid_of(m, "owner").as_deref() == Some(owner.as_str()) {
                if let Some(parent) = uuid_of(m, "parent") {
                    owner_roots.insert(root_of(&parent));
                }
            }
        }
    }

    let mut categories = Vec::with_capacity(folders.len());
    for folder in folders {
        let Value::Map(m) = folder else { continue };
        let Some(id) = uuid_of(m, "id") else { continue };
        if !owner_roots.is_empty() && !owner_roots.contains(&root_of(&id)) {
            continue;
        }
        let mut out = BTreeMap::new();
        out.insert("cat_id".into(), Value::Uuid(id));
        out.insert(
            "parent_id".into(),
            Value::Uuid(uuid_of(m, "parent").unwrap_or_else(|| NIL.into())),
        );
        out.insert("owner_id".into(), Value::Uuid(owner.clone()));
        out.insert(
            "name".into(),
            Value::String(string_of(m, "name").unwrap_or_else(|| "Unnamed folder".into())),
        );
        let folder_type = int_of(m, "type").unwrap_or(-1);
        out.insert(
            "preferred_type".into(),
            Value::String(folder_type_name(folder_type)),
        );
        categories.push(Value::Map(out));
    }

    let mut translated_items = Vec::with_capacity(items.len());
    for item in items {
        let Value::Map(m) = item else { continue };
        let Some(id) = uuid_of(m, "id") else { continue };
        if uuid_of(m, "owner")
            .as_deref()
            .is_some_and(|o| o != owner && o != NIL)
        {
            continue;
        }
        let asset_type = int_of(m, "asset_type").unwrap_or(-1);
        let inv_type = int_of(m, "inv_type").unwrap_or(-1);
        let mut out = BTreeMap::new();
        out.insert("item_id".into(), Value::Uuid(id));
        out.insert(
            "parent_id".into(),
            Value::Uuid(uuid_of(m, "parent").unwrap_or_else(|| NIL.into())),
        );
        out.insert(
            "name".into(),
            Value::String(string_of(m, "name").unwrap_or_else(|| "Unnamed item".into())),
        );
        out.insert(
            "desc".into(),
            Value::String(string_of(m, "desc").unwrap_or_default()),
        );
        out.insert("type".into(), Value::String(asset_type_name(asset_type)));
        out.insert(
            "inv_type".into(),
            Value::String(inventory_type_name(inv_type)),
        );
        if let Some(asset) = uuid_of(m, "asset_id") {
            out.insert("asset_id".into(), Value::Uuid(asset));
        }
        if let Some(created) = int_of(m, "created_at") {
            out.insert("created_at".into(), Value::Integer(created as i32));
        }
        let mut permissions = BTreeMap::new();
        permissions.insert("owner_id".into(), Value::Uuid(owner.clone()));
        if let Some(creator) = uuid_of(m, "creator") {
            permissions.insert("creator_id".into(), Value::Uuid(creator));
        }
        for (from, to) in [
            ("owner_mask", "owner_mask"),
            ("next_owner_mask", "next_owner_mask"),
        ] {
            if let Some(mask) = int_of(m, from) {
                // Aurora writes u32 masks as i32; Firestorm's shape carries them unsigned.
                permissions.insert(to.into(), Value::Real(f64::from(mask as i32 as u32)));
            }
        }
        out.insert("permissions".into(), Value::Map(permissions));
        if bool_of(m, "favorite") {
            let mut favorite = BTreeMap::new();
            favorite.insert("toggled".into(), Value::Boolean(true));
            out.insert("favorite".into(), Value::Map(favorite));
        }
        if let Some(thumbnail) = uuid_of(m, "thumbnail").filter(|t| t != NIL) {
            let mut thumb = BTreeMap::new();
            thumb.insert("asset_id".into(), Value::Uuid(thumbnail));
            out.insert("thumbnail".into(), Value::Map(thumb));
        }
        translated_items.push(Value::Map(out));
    }

    let mut root = BTreeMap::new();
    root.insert("categories".into(), Value::Array(categories));
    root.insert("items".into(), Value::Array(translated_items));
    Ok(Value::Map(root))
}

/// LLAssetType names, as Firestorm writes them in its cache.
fn asset_type_name(value: i64) -> String {
    match value {
        0 => "texture",
        1 => "sound",
        2 => "callcard",
        3 => "landmark",
        5 => "clothing",
        6 => "object",
        7 => "notecard",
        8 => "category",
        10 => "lsltext",
        11 => "lslbyte",
        13 => "bodypart",
        20 => "animatn",
        21 => "gesture",
        24 => "link",
        25 => "link_f",
        49 => "mesh",
        56 => "settings",
        57 => "material",
        other => return other.to_string(),
    }
    .into()
}

/// LLInventoryType names.
fn inventory_type_name(value: i64) -> String {
    match value {
        0 => "texture",
        1 => "sound",
        2 => "callcard",
        3 => "landmark",
        6 => "object",
        7 => "notecard",
        8 => "category",
        9 => "root",
        10 => "script",
        15 => "snapshot",
        17 => "attach",
        18 => "wearable",
        19 => "animation",
        20 => "gesture",
        22 => "mesh",
        25 => "settings",
        26 => "material",
        other => return other.to_string(),
    }
    .into()
}

/// LLFolderType names, which Shelfmark uses to recognise system folders.
fn folder_type_name(value: i64) -> String {
    match value {
        -1 => "-1",
        0 => "texture",
        1 => "sound",
        2 => "callcard",
        3 => "landmark",
        5 => "clothing",
        6 => "object",
        7 => "notecard",
        8 => "root_inv",
        10 => "lsltext",
        13 => "bodypart",
        14 => "trash",
        15 => "snapshots",
        16 => "lstndfnd",
        20 => "animatn",
        21 => "gesture",
        23 => "favorite",
        46 => "current",
        47 => "outfit",
        48 => "my_otfts",
        49 => "mesh",
        50 => "inbox",
        51 => "outbox",
        52 => "basic_rt",
        53 => "merchant",
        54 => "stock",
        55 => "version",
        56 => "settings",
        57 => "material",
        other => return other.to_string(),
    }
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Binary LLSD writer matching aurora-llsd's `to_binary`.
    enum L {
        I(i32),
        S(&'static str),
        U(&'static str),
        B(bool),
        A(Vec<L>),
        M(Vec<(&'static str, L)>),
    }
    fn write(v: &L, out: &mut Vec<u8>) {
        match v {
            L::I(i) => {
                out.push(b'i');
                out.extend(i.to_be_bytes());
            }
            L::S(s) => {
                out.push(b's');
                out.extend((s.len() as u32).to_be_bytes());
                out.extend(s.as_bytes());
            }
            L::U(u) => {
                out.push(b'u');
                out.extend(uuid::Uuid::parse_str(u).unwrap().as_bytes());
            }
            L::B(b) => out.push(if *b { b'1' } else { b'0' }),
            L::A(a) => {
                out.push(b'[');
                out.extend((a.len() as u32).to_be_bytes());
                a.iter().for_each(|e| write(e, out));
                out.push(b']');
            }
            L::M(m) => {
                out.push(b'{');
                out.extend((m.len() as u32).to_be_bytes());
                for (k, e) in m {
                    out.push(b'k');
                    out.extend((k.len() as u32).to_be_bytes());
                    out.extend(k.as_bytes());
                    write(e, out);
                }
                out.push(b'}');
            }
        }
    }

    const AGENT: &str = "20000000-0000-0000-0000-000000000002";
    const LIBRARY_OWNER: &str = "ba2a564a-f0f1-4b82-9c61-b7520bfcd09f";

    fn sample() -> Vec<u8> {
        let doc = L::M(vec![
            ("version", L::I(3)),
            (
                "folders",
                L::A(vec![
                    L::M(vec![
                        ("id", L::U("10000000-0000-0000-0000-000000000001")),
                        ("parent", L::U(NIL)),
                        ("name", L::S("My Inventory")),
                        ("type", L::I(8)),
                    ]),
                    L::M(vec![
                        ("id", L::U("10000000-0000-0000-0000-000000000002")),
                        ("parent", L::U("10000000-0000-0000-0000-000000000001")),
                        ("name", L::S("Trash")),
                        ("type", L::I(14)),
                    ]),
                    L::M(vec![
                        ("id", L::U("30000000-0000-0000-0000-000000000001")),
                        ("parent", L::U(NIL)),
                        ("name", L::S("Library")),
                        ("type", L::I(8)),
                    ]),
                ]),
            ),
            (
                "items",
                L::A(vec![
                    L::M(vec![
                        ("id", L::U("40000000-0000-0000-0000-000000000001")),
                        ("parent", L::U("10000000-0000-0000-0000-000000000001")),
                        ("name", L::S("Wool coat")),
                        ("desc", L::S("Winter")),
                        ("asset_type", L::I(5)),
                        ("inv_type", L::I(18)),
                        ("owner", L::U(AGENT)),
                        ("creator", L::U("50000000-0000-0000-0000-000000000005")),
                        ("owner_mask", L::I(-1)),
                        ("next_owner_mask", L::I(0x0008_2000)),
                        ("created_at", L::I(1_700_000_000)),
                        ("favorite", L::B(true)),
                    ]),
                    L::M(vec![
                        ("id", L::U("40000000-0000-0000-0000-000000000002")),
                        ("parent", L::U("30000000-0000-0000-0000-000000000001")),
                        ("name", L::S("Library thing")),
                        ("asset_type", L::I(6)),
                        ("inv_type", L::I(6)),
                        ("owner", L::U(LIBRARY_OWNER)),
                    ]),
                ]),
            ),
        ]);
        let mut out = Vec::new();
        write(&doc, &mut out);
        out
    }

    #[test]
    fn sealed_round_trip_and_wrong_key() {
        let key = InstallKey::from_bytes([7; 32]);
        let plain: Vec<u8> = (0..(CHUNK * 2 + 123)).map(|i| (i % 251) as u8).collect();
        let sealed = key.seal(&plain, [3; SALT]);
        assert!(is_sealed(&sealed));
        assert_eq!(key.open(sealed.clone()).unwrap(), plain);
        assert!(InstallKey::from_bytes([8; 32])
            .open(sealed.clone())
            .is_err());
        let mut truncated = sealed;
        truncated.pop();
        assert!(key.open(truncated).is_err());
        let empty = key.seal(&[], [1; SALT]);
        assert_eq!(key.open(empty).unwrap(), Vec::<u8>::new());
    }

    #[test]
    fn file_stem_is_32_hex_and_key_dependent() {
        let agent = uuid::Uuid::parse_str(AGENT).unwrap();
        let a = InstallKey::from_bytes([1; 32]).file_stem(&agent);
        let b = InstallKey::from_bytes([2; 32]).file_stem(&agent);
        assert_eq!(a.len(), 32);
        assert!(a
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
        assert_ne!(a, b);
    }

    #[test]
    fn parses_aurora_inventory_and_drops_library() {
        let key = InstallKey::from_bytes([9; 32]);
        let plain = key.open(key.seal(&sample(), [5; SALT])).unwrap();
        let snapshot = parse_plain_inventory(&plain, None, &|| false, &mut |_, _, _| {}).unwrap();
        assert_eq!(snapshot.avatar_id, format!("{AGENT}@aurora"));
        assert_eq!(snapshot.items.len(), 1);
        assert_eq!(snapshot.categories.len(), 2);
        let item = &snapshot.items[0];
        assert_eq!(item.name, "Wool coat");
        assert_eq!(item.kind, "Clothing");
        assert_eq!(item.path, vec!["My Inventory".to_string()]);
        assert_eq!(item.owner_mask, Some(u32::MAX));
        assert_eq!(item.next_owner_mask, Some(0x0008_2000));
        assert_eq!(item.created_at, Some(1_700_000_000));
        assert!(item.is_favorite);
        assert!(snapshot
            .categories
            .iter()
            .any(|c| c.name == "Trash" && c.preferred_type == "trash"));
    }

    #[test]
    fn rejects_unknown_version() {
        let mut doc = Vec::new();
        write(&L::M(vec![("version", L::I(99))]), &mut doc);
        let error = parse_plain_inventory(&doc, None, &|| false, &mut |_, _, _| {}).unwrap_err();
        assert!(error.contains("version 99"));
    }

    #[test]
    fn reads_cache_location_settings() {
        let dir = std::env::temp_dir().join(format!("shelfmark-aurora-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("settings.json"), r#"{"cache_location":"D:\\SL"}"#).unwrap();
        std::fs::write(
            dir.join("cache_location.txt"),
            "E:\\last\\AuroraViewerCache\n",
        )
        .unwrap();
        let dirs = cache_dirs_from_config(&dir);
        assert_eq!(dirs[0], PathBuf::from("E:\\last\\AuroraViewerCache"));
        assert_eq!(dirs[1], Path::new("D:\\SL").join("AuroraViewerCache"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
