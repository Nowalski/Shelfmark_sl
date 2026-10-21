use llsd_rs::{autodetect, notation, Llsd};
use serde::Serialize;
use serde_json::{json, Map as JsonMap, Value as JsonValue};
use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const MAX_LEAP_PACKET: usize = 64 * 1024 * 1024;
const MAX_LOCAL_JSON_LINE: usize = 64 * 1024 * 1024;
const MAX_LOCAL_HANDSHAKE_LINE: usize = 4 * 1024;
const LOCAL_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
const APP_RPC_TIMEOUT: Duration = Duration::from_secs(30);
const VIEWER_RPC_TIMEOUT: Duration = Duration::from_secs(20);

macro_rules! leap_log {
    ($($arg:tt)*) => {{
        // A GUI-subsystem LEAP child may have no stderr handle. Keep diagnostic
        // writes best-effort so a missing console can never interrupt LEAP.
        let _ = writeln!(io::stderr().lock(), $($arg)*);
    }};
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LeapStatus {
    pub phase: String,
    pub connected: bool,
    pub native_bridge_enabled: bool,
    pub avatar_id: Option<String>,
    pub api_names: Vec<String>,
    pub inventory_window_paths: Vec<String>,
    pub inventory_paths_scanned: bool,
    pub message: String,
}

impl Default for LeapStatus {
    fn default() -> Self {
        Self {
            phase: "idle".into(),
            connected: false,
            native_bridge_enabled: false,
            avatar_id: None,
            api_names: Vec::new(),
            inventory_window_paths: Vec::new(),
            inventory_paths_scanned: false,
            message: "Stock Firestorm LEAP is not connected.".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LeapInspection {
    pub avatar_id: String,
    pub api_names: Vec<String>,
    pub inventory_window_paths: Vec<String>,
    pub inventory_paths_scanned: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LeapInventoryItem {
    pub id: String,
    pub name: String,
    pub parent_id: String,
    pub asset_type: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LeapMoveResult {
    pub item: LeapInventoryItem,
    pub previous_parent_id: String,
    pub previous_parent_name: String,
    pub destination_name: String,
    pub changed: bool,
}

#[derive(Default, Clone)]
pub struct LeapController {
    shared: Arc<Mutex<LeapShared>>,
    move_lock: Arc<Mutex<()>>,
}

#[derive(Default)]
struct LeapShared {
    status: LeapStatus,
    firestorm_executable: Option<PathBuf>,
    native_bridge_enabled: bool,
    token: Option<String>,
    outgoing: Option<Sender<JsonValue>>,
    pending: HashMap<String, Sender<Result<JsonValue, String>>>,
}

impl LeapController {
    pub fn status(&self) -> LeapStatus {
        self.shared
            .lock()
            .map(|shared| shared.status.clone())
            .unwrap_or_else(|_| LeapStatus {
                phase: "error".into(),
                message: "The LEAP connection state is unavailable.".into(),
                ..LeapStatus::default()
            })
    }

    pub fn start_stock_firestorm(
        &self,
        executable_path: &str,
        native_bridge_enabled: bool,
    ) -> Result<LeapStatus, String> {
        if native_bridge_enabled && !cfg!(windows) {
            return Err("The Firestorm Mini bridge is available on Windows only.".into());
        }
        if crate::discovery::firestorm_is_running() == Some(true) {
            return Err("Close every Firestorm window first. Firestorm reads --leap only at startup; Shelfmark will not close the viewer for you.".into());
        }

        let executable = Path::new(executable_path)
            .canonicalize()
            .map_err(|error| format!("Could not open that Firestorm executable: {error}"))?;
        if !executable.is_file()
            || !executable
                .extension()
                .and_then(std::ffi::OsStr::to_str)
                .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
            || !executable
                .file_stem()
                .and_then(std::ffi::OsStr::to_str)
                .is_some_and(|name| name.to_ascii_lowercase().starts_with("firestorm"))
        {
            return Err(if native_bridge_enabled {
                "Choose the patched Firestorm Mini .exe file.".into()
            } else {
                "Choose the stock Firestorm .exe file.".into()
            });
        }

        let listener = TcpListener::bind(("127.0.0.1", 0)).map_err(|error| {
            format!("Could not prepare Shelfmark's local LEAP connection: {error}")
        })?;
        listener
            .set_nonblocking(true)
            .map_err(|error| format!("Could not prepare the LEAP listener: {error}"))?;
        let address = listener
            .local_addr()
            .map_err(|error| format!("Could not read the local LEAP address: {error}"))?;
        let token = uuid::Uuid::new_v4().simple().to_string();
        {
            let mut shared = self
                .shared
                .lock()
                .map_err(|_| "The LEAP connection state is unavailable.".to_string())?;
            if shared.status.connected || shared.status.phase == "starting" {
                return Err("A Shelfmark LEAP session is already active.".into());
            }
            shared.status = LeapStatus {
                phase: "starting".into(),
                native_bridge_enabled,
                message: if native_bridge_enabled {
                    "Starting the patched Firestorm Mini build with Shelfmark's LEAP helper…".into()
                } else {
                    "Starting stock Firestorm with the Shelfmark LEAP helper…".into()
                },
                ..LeapStatus::default()
            };
            shared.firestorm_executable = Some(executable.clone());
            shared.native_bridge_enabled = native_bridge_enabled;
            shared.token = Some(token.clone());
            shared.outgoing = None;
        }

        let helper_path = match std::env::current_exe() {
            Ok(path) => path,
            Err(error) => {
                self.set_error(format!(
                    "Could not locate the Shelfmark desktop app: {error}"
                ));
                return Err(format!(
                    "Could not locate the Shelfmark desktop app: {error}"
                ));
            }
        };
        let helper_path = helper_path.to_string_lossy().replace('\\', "/");
        // LEAP parses its own command string after Firestorm's Windows argv
        // parsing. Use LEAP's single-quote syntax so the executable boundary
        // survives both parsers, including when the installed path has spaces.
        let plugin_command = format!("'{helper_path}' --leap-child {address} {token}");
        if let Err(error) = Command::new(&executable)
            .current_dir(executable.parent().unwrap_or_else(|| Path::new(".")))
            .arg("--leap")
            .arg(plugin_command)
            .arg("--console")
            .arg("false")
            .spawn()
        {
            let viewer_name = if native_bridge_enabled {
                "Firestorm Mini"
            } else {
                "stock Firestorm"
            };
            self.set_error(format!("Could not start {viewer_name}: {error}"));
            return Err(format!("Could not start {viewer_name}: {error}"));
        }

        let shared = self.shared.clone();
        thread::spawn(move || accept_helper(listener, address, token, shared));
        Ok(self.status())
    }

    pub fn inspect_viewer(&self) -> Result<LeapInspection, String> {
        let api_response = self.request("command", json!({ "op": "getAPIs" }))?;
        let api_names = collect_api_names(&api_response);
        let avatar_id = self.active_avatar_id()?;

        // LLWindow.getPaths serializes every descendant into one potentially large
        // reply. Keep initial connection inspection focused on the APIs and avatar;
        // query targeted UI paths separately once needed.
        let inventory_window_paths = Vec::new();

        if let Ok(mut shared) = self.shared.lock() {
            shared.status.avatar_id = Some(avatar_id.clone());
            shared.status.api_names = api_names.clone();
            shared.status.inventory_window_paths = inventory_window_paths.clone();
            shared.status.inventory_paths_scanned = false;
            shared.status.message = if shared.native_bridge_enabled {
                "Connected to patched Firestorm Mini. LEAP reads and native Trash actions are ready.".into()
            } else {
                "Connected to stock Firestorm. LEAP APIs and active avatar were read.".into()
            };
        }
        Ok(LeapInspection {
            avatar_id,
            api_names,
            inventory_window_paths,
            inventory_paths_scanned: false,
        })
    }

    pub fn active_avatar_id(&self) -> Result<String, String> {
        let agent_response = self.request("LLAgent", json!({ "op": "getID" }))?;
        let avatar_id = find_first_string(&agent_response, &["id", "avatar_id"])
            .or_else(|| agent_response.as_str().map(str::to_owned))
            .ok_or_else(|| format!("Firestorm did not return an avatar UUID: {agent_response}"))?;
        let parsed = uuid::Uuid::parse_str(&avatar_id)
            .map_err(|_| "Firestorm returned an invalid avatar UUID.".to_string())?;
        Ok(parsed.hyphenated().to_string())
    }

    pub fn scan_inventory_window_paths(&self) -> Result<Vec<String>, String> {
        let is_exposed = {
            let shared = self
                .shared
                .lock()
                .map_err(|_| "The LEAP connection state is unavailable.".to_string())?;
            shared.status.connected
                && shared
                    .status
                    .api_names
                    .iter()
                    .any(|name| name == "LLWindow")
        };
        if !is_exposed {
            return Err(
                "Inspect the connected Firestorm viewer first and confirm it exposes LLWindow."
                    .into(),
            );
        }

        // getPaths serializes every descendant into one potentially large
        // reply. Probe the named controls from Firestorm's installed inventory
        // XUI with getInfo, which returns each control's canonical path.
        let mut paths = Vec::new();
        let mut failure = None;
        for path in [
            "//floater_my_inventory",
            "//sidepanel_inventory_panel",
            "//main inventory panel",
            "//inventory search editor",
            "//inventory filter tabs",
            "//All Items",
        ] {
            match self.request("LLWindow", json!({ "op": "getInfo", "path": path })) {
                Ok(info) => match info.get("path").and_then(JsonValue::as_str) {
                    Some(canonical) if !paths.iter().any(|found| found == canonical) => {
                        paths.push(canonical.to_owned());
                    }
                    Some(_) => {}
                    None => {
                        failure = Some(
                            "Firestorm returned inventory view info without a path.".to_string(),
                        );
                        break;
                    }
                },
                Err(error) if is_invalid_view_path(&error) => {}
                Err(error) => {
                    failure = Some(error);
                    break;
                }
            }
        }

        if paths.is_empty() || failure.is_some() {
            let error = failure.unwrap_or_else(|| {
                "Firestorm could not find its inventory views. Open the Inventory floater and scan again.".to_string()
            });
            if let Ok(mut shared) = self.shared.lock() {
                shared.status.message = format!("Inventory UI scan failed: {error}");
                shared.status.inventory_window_paths.clear();
                shared.status.inventory_paths_scanned = false;
            }
            return Err(error);
        }

        if let Ok(mut shared) = self.shared.lock() {
            shared.status.inventory_window_paths = paths.clone();
            shared.status.inventory_paths_scanned = true;
            shared.status.message = format!(
                "Located {} inventory UI controls in Firestorm.",
                paths.len()
            );
        }
        Ok(paths)
    }

    pub fn inspect_inventory_view_by_name(&self, name: &str) -> Result<JsonValue, String> {
        if name.trim().is_empty() || name.chars().count() > 160 {
            return Err(
                "Enter an exact Firestorm inventory item name (up to 160 characters).".into(),
            );
        }
        let is_exposed = {
            let shared = self
                .shared
                .lock()
                .map_err(|_| "The LEAP connection state is unavailable.".to_string())?;
            shared.status.connected && shared.status.api_names.iter().any(|api| api == "LLWindow")
        };
        if !is_exposed {
            return Err(
                "Inspect the connected Firestorm viewer first and confirm it exposes LLWindow."
                    .into(),
            );
        }

        // LLUI separates path components on '/'; escape literal slashes and
        // percent signs in an inventory item's display name.
        let component = name.replace('%', "%25").replace('/', "%2F");
        self.request(
            "LLWindow",
            json!({ "op": "getInfo", "path": format!("//All Items//{component}") }),
        )
    }

    pub fn inspect_inventory_item(&self, item_id: &str) -> Result<LeapInventoryItem, String> {
        let item_id = normalize_uuid(item_id, "item")?;
        self.read_inventory_item(&item_id)
    }

    pub fn move_inventory_item(
        &self,
        avatar_id: &str,
        item_id: &str,
        source_parent_id: &str,
        destination_folder_id: Option<&str>,
    ) -> Result<LeapMoveResult, String> {
        let native_bridge_enabled = self
            .shared
            .lock()
            .map(|shared| shared.native_bridge_enabled)
            .unwrap_or(false);
        if native_bridge_enabled && destination_folder_id.is_some() {
            return Err("Firestorm Mini currently supports moves to Trash only.".into());
        }
        log_move_diagnostic(if native_bridge_enabled {
            "move.native_bridge_started"
        } else {
            "move.started"
        });
        let result = if native_bridge_enabled {
            self.move_inventory_item_native(avatar_id, item_id, source_parent_id)
        } else {
            self.move_inventory_item_inner(
                avatar_id,
                item_id,
                source_parent_id,
                destination_folder_id,
            )
        };
        match &result {
            Ok(move_result) if move_result.changed => {
                log_move_diagnostic("move.verified_parent_changed");
            }
            Ok(_) => log_move_diagnostic("move.noop_already_in_destination"),
            Err(error) => log_move_diagnostic(move_failure_stage(&error)),
        }
        result
    }

    fn move_inventory_item_native(
        &self,
        avatar_id: &str,
        item_id: &str,
        source_parent_id: &str,
    ) -> Result<LeapMoveResult, String> {
        if !cfg!(windows) {
            return Err("The Firestorm Mini bridge is available on Windows only.".into());
        }
        let _move = self
            .move_lock
            .lock()
            .map_err(|_| "The Firestorm Mini move is unavailable.".to_string())?;
        let (expected_avatar_id, _) = crate::normalized_inventory_move_avatar(avatar_id)?;
        let item_id = normalize_uuid(item_id, "item")?;
        let source_parent_id = normalize_uuid(source_parent_id, "source folder")?;
        let active_avatar_id = self.active_avatar_id()?;
        if !active_avatar_id.eq_ignore_ascii_case(&expected_avatar_id) {
            return Err(format!(
                "Firestorm is logged in as {active_avatar_id}, but this snapshot belongs to {expected_avatar_id}."
            ));
        }

        let before = self.read_inventory_item(&item_id)?;
        if !before.parent_id.eq_ignore_ascii_case(&source_parent_id) {
            return Err(
                "The item moved since this snapshot was imported. Refresh it and try again.".into(),
            );
        }
        let previous_parent = self.read_inventory_category(&before.parent_id)?;
        let previous_parent_name = previous_parent
            .get("name")
            .and_then(JsonValue::as_str)
            .unwrap_or("Source folder")
            .to_owned();

        let trash_response = self.request(
            "LLInventory",
            json!({ "op": "getBasicFolderID", "ft_name": "trash" }),
        )?;
        let destination_id = trash_response
            .get("id")
            .and_then(JsonValue::as_str)
            .ok_or_else(|| "Firestorm did not return its Trash folder UUID.".to_string())?;
        let destination_id = normalize_uuid(destination_id, "Trash folder")?;
        let destination = self.read_inventory_category(&destination_id)?;
        if destination.get("type").and_then(JsonValue::as_str) != Some("trash") {
            return Err("Firestorm did not identify that folder as Trash.".into());
        }
        let destination_name = destination
            .get("name")
            .and_then(JsonValue::as_str)
            .unwrap_or("Trash")
            .to_owned();
        if before.parent_id.eq_ignore_ascii_case(&destination_id) {
            return Ok(LeapMoveResult {
                item: before.clone(),
                previous_parent_id: before.parent_id,
                previous_parent_name,
                destination_name,
                changed: false,
            });
        }

        let (executable, native_bridge_enabled) = self
            .shared
            .lock()
            .map(|shared| {
                (
                    shared.firestorm_executable.clone(),
                    shared.native_bridge_enabled,
                )
            })
            .map_err(|_| "The Firestorm Mini connection state is unavailable.".to_string())?;
        if !native_bridge_enabled {
            return Err("Launch the patched Firestorm Mini build from Shelfmark first.".into());
        }
        let executable = executable.ok_or_else(|| {
            "Shelfmark has no Firestorm Mini executable for this session.".to_string()
        })?;

        let request_id = uuid::Uuid::new_v4().hyphenated().to_string();
        let uri = format!(
            "secondlife:///app/shelfmark/v1/move/{expected_avatar_id}/{item_id}/{source_parent_id}/trash?grid=agni&request={request_id}"
        );
        Command::new(&executable)
            .current_dir(executable.parent().unwrap_or_else(|| Path::new(".")))
            .arg("--url")
            .arg(uri)
            .spawn()
            .map_err(|error| {
                format!("Could not send the move request to Firestorm Mini: {error}")
            })?;

        // The viewer asks for confirmation. Keep checking its live inventory
        // until the user responds or the request times out.
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            thread::sleep(Duration::from_millis(450));
            let current = self.read_inventory_item(&item_id)?;
            if current.parent_id.eq_ignore_ascii_case(&destination_id) {
                // Read once more after the viewer has had time to process the
                // inventory update, so an immediate optimistic change is not
                // presented as a completed operation.
                thread::sleep(Duration::from_millis(900));
                let settled = self.read_inventory_item(&item_id)?;
                if !settled.parent_id.eq_ignore_ascii_case(&destination_id) {
                    return Err("Firestorm did not keep the item in Trash. Check its current location in the viewer.".into());
                }
                return Ok(LeapMoveResult {
                    item: settled,
                    previous_parent_id: before.parent_id,
                    previous_parent_name,
                    destination_name,
                    changed: true,
                });
            }
            if !current.parent_id.eq_ignore_ascii_case(&source_parent_id) {
                return Err("The item changed folders while Shelfmark was waiting. Check it in Firestorm before trying again.".into());
            }
        }
        Err("Firestorm Mini did not confirm the Trash move. Accept the confirmation in Firestorm, then check the item there.".into())
    }

    fn move_inventory_item_inner(
        &self,
        avatar_id: &str,
        item_id: &str,
        source_parent_id: &str,
        destination_folder_id: Option<&str>,
    ) -> Result<LeapMoveResult, String> {
        if !cfg!(debug_assertions) {
            return Err("Live Firestorm inventory changes are disabled in release builds until the Delete-based Trash route passes repeated live validation.".into());
        }
        let _move = self
            .move_lock
            .lock()
            .map_err(|_| "The stock Firestorm move is unavailable.".to_string())?;
        let (expected_avatar_id, _) = crate::normalized_inventory_move_avatar(avatar_id)?;
        let item_id = normalize_uuid(item_id, "item")?;
        let source_parent_id = normalize_uuid(source_parent_id, "source folder")?;
        let active_avatar = self.request("LLAgent", json!({ "op": "getID" }))?;
        let active_avatar_id = find_first_string(&active_avatar, &["id", "avatar_id"])
            .or_else(|| active_avatar.as_str().map(str::to_owned))
            .ok_or_else(|| "Firestorm did not return its active avatar UUID.".to_string())?;
        if !active_avatar_id.eq_ignore_ascii_case(&expected_avatar_id) {
            return Err(format!(
                "Firestorm is logged in as {active_avatar_id}, but this snapshot belongs to {expected_avatar_id}."
            ));
        }
        log_move_diagnostic("move.avatar_matched_snapshot");

        log_move_diagnostic("move.live_item_info_request");
        let before = self.read_inventory_item(&item_id)?;
        log_move_diagnostic("move.live_item_info_loaded");
        if !before.parent_id.eq_ignore_ascii_case(&source_parent_id) {
            return Err(
                "The item moved since this snapshot was imported. Refresh it and try again.".into(),
            );
        }
        let previous_parent = self.read_inventory_category(&before.parent_id)?;
        log_move_diagnostic("move.source_folder_info_loaded");
        let previous_parent_name = previous_parent
            .get("name")
            .and_then(JsonValue::as_str)
            .unwrap_or("Source folder")
            .to_owned();
        let destination_id = match destination_folder_id {
            Some(id) => normalize_uuid(id, "destination folder")?,
            None => {
                let response = self.request(
                    "LLInventory",
                    json!({ "op": "getBasicFolderID", "ft_name": "trash" }),
                )?;
                let id = response
                    .get("id")
                    .and_then(JsonValue::as_str)
                    .ok_or_else(|| "Firestorm did not return its Trash folder UUID.".to_string())?;
                normalize_uuid(id, "Trash folder")?
            }
        };
        log_move_diagnostic("move.destination_folder_id_resolved");
        let destination = self.read_inventory_category(&destination_id)?;
        log_move_diagnostic("move.destination_folder_info_loaded");
        if destination_folder_id.is_none()
            && destination.get("type").and_then(JsonValue::as_str) != Some("trash")
        {
            return Err("Firestorm did not identify that folder as Trash.".into());
        }
        let destination_name = destination
            .get("name")
            .and_then(JsonValue::as_str)
            .unwrap_or("Destination folder")
            .to_owned();
        let destination_is_trash =
            destination.get("type").and_then(JsonValue::as_str) == Some("trash");
        log_move_diagnostic("move.live_item_and_destination_loaded");
        if before.parent_id == destination_id {
            return Ok(LeapMoveResult {
                item: before.clone(),
                previous_parent_id: before.parent_id,
                previous_parent_name,
                destination_name,
                changed: false,
            });
        }

        if destination_folder_id.is_some() && !destination_is_trash {
            log_move_diagnostic("move.ordinary_folder_clipboard_paused");
            return Err("Moves to ordinary folders are temporarily paused. Stock Firestorm LEAP does not confirm which item is on the inventory clipboard after Cut, so Shelfmark cannot safely prevent an older clipboard item from being pasted. Move this item inside Firestorm; Move to Trash remains available through Firestorm's Delete action.".into());
        }

        let all_items = self.request(
            "LLWindow",
            json!({ "op": "getInfo", "path": "//All Items" }),
        )?;
        if all_items.get("visible_chain").and_then(JsonValue::as_bool) != Some(true) {
            return Err("Open Firestorm's Inventory window and choose the All Items tab before moving an item.".into());
        }
        let search = self.request(
            "UI",
            json!({ "op": "getValue", "path": "//inventory search editor" }),
        )?;
        if search
            .get("value")
            .and_then(JsonValue::as_str)
            .is_some_and(|value| !value.trim().is_empty())
        {
            return Err("Clear the Firestorm inventory search field before moving an item.".into());
        }

        // The UUID command selects the exact row. Focus the inventory tree
        // directly when sending Delete; mouseDown/mouseUp visibly move the
        // viewer cursor and do not reliably give the tree keyboard focus.
        let inventory_tree_path = "//All Items/Inventory Scroller";
        let inventory_tree = self.request(
            "LLWindow",
            json!({ "op": "getInfo", "path": inventory_tree_path }),
        )?;
        if inventory_tree
            .get("visible_chain")
            .and_then(JsonValue::as_bool)
            != Some(true)
            || inventory_tree.get("available").and_then(JsonValue::as_bool) != Some(true)
        {
            return Err(
                "Firestorm's All Items tree is not available to receive keyboard input.".into(),
            );
        }

        self.select_inventory_uuid(&item_id)?;
        let item_path = self.inventory_object_path(&before.id, &before.parent_id, &before.name)?;
        self.wait_for_inventory_row(&item_path, &before.name)?;
        log_move_diagnostic("move.source_row_selected_by_uuid");
        // This command now handles Trash only. Ordinary-folder destinations
        // are rejected above so there is no Cut/Paste path to reuse stale
        // clipboard contents.
        self.press_inventory_key("Del", inventory_tree_path)?;
        log_move_diagnostic("move.trash_delete_shortcut_sent");

        // Read the viewer's inventory model again instead of treating a
        // delivered key as a completed move.
        for _ in 0..12 {
            thread::sleep(Duration::from_millis(250));
            let after = self.read_inventory_item(&item_id)?;
            if after.parent_id == destination_id {
                log_move_diagnostic("move.live_parent_verification_succeeded");
                return Ok(LeapMoveResult {
                    item: after,
                    previous_parent_id: before.parent_id,
                    previous_parent_name,
                    destination_name,
                    changed: true,
                });
            }
        }
        log_move_diagnostic("move.verification_timed_out");
        Err(format!("Firestorm did not move the item to Trash. It may be active, not removable, or waiting for viewer attention. Check its inventory panel before retrying. Its existing clipboard was not pasted (folder {}).", before.parent_id))
    }

    fn read_inventory_item(&self, item_id: &str) -> Result<LeapInventoryItem, String> {
        let response = self.request(
            "LLInventory",
            json!({ "op": "getItemsInfo", "item_ids": [item_id] }),
        )?;
        let item = response
            .get("items")
            .and_then(|items| items.get(item_id))
            .ok_or_else(|| {
                format!("Item {item_id} is not loaded in the active Firestorm inventory.")
            })?;
        Ok(LeapInventoryItem {
            id: item_id.to_owned(),
            name: required_string(item, "name", "inventory item")?,
            parent_id: required_string(item, "parent_id", "inventory item")?,
            asset_type: required_string(item, "asset_type", "inventory item")?,
        })
    }

    fn read_inventory_category(&self, category_id: &str) -> Result<JsonValue, String> {
        let response = self.request(
            "LLInventory",
            json!({ "op": "getItemsInfo", "item_ids": [category_id] }),
        )?;
        response
            .get("categories")
            .and_then(|categories| categories.get(category_id))
            .cloned()
            .ok_or_else(|| {
                format!("Folder {category_id} is not loaded in the active Firestorm inventory.")
            })
    }

    fn inventory_object_path(
        &self,
        item_id: &str,
        parent_id: &str,
        item_name: &str,
    ) -> Result<String, String> {
        self.ensure_unique_sibling_name(parent_id, item_name, item_id)?;
        let parent_path = self.inventory_folder_path(parent_id)?;
        Ok(format!(
            "{parent_path}/{}",
            escape_inventory_path_component(item_name)
        ))
    }

    fn inventory_folder_path(&self, folder_id: &str) -> Result<String, String> {
        let root_response = self.request(
            "LLInventory",
            json!({ "op": "getBasicFolderID", "ft_name": "root_inv" }),
        )?;
        let root_id = root_response
            .get("id")
            .and_then(JsonValue::as_str)
            .ok_or_else(|| "Firestorm did not return its My Inventory folder UUID.".to_string())?;
        let root_id = normalize_uuid(root_id, "My Inventory folder")?;
        let folder_id = normalize_uuid(folder_id, "destination folder")?;

        let mut names = Vec::new();
        let mut current_id = folder_id;
        for _ in 0..64 {
            let category = self.read_inventory_category(&current_id)?;
            names.push(required_string(&category, "name", "inventory folder")?);
            if current_id == root_id {
                names.reverse();
                let components = names
                    .iter()
                    .map(|name| escape_inventory_path_component(name))
                    .collect::<Vec<_>>()
                    .join("/");
                return Ok(format!("//All Items/Inventory Scroller//{components}"));
            }
            current_id = normalize_uuid(
                &required_string(&category, "parent_id", "inventory folder")?,
                "parent folder",
            )?;
        }
        Err("Firestorm could not resolve this folder back to My Inventory.".into())
    }

    fn ensure_unique_sibling_name(
        &self,
        parent_id: &str,
        expected_name: &str,
        expected_id: &str,
    ) -> Result<(), String> {
        let response = self.request(
            "LLInventory",
            json!({ "op": "getDirectDescendants", "folder_id": parent_id }),
        )?;
        for kind in ["items", "categories"] {
            if let Some(objects) = response.get(kind).and_then(JsonValue::as_object) {
                if objects.iter().any(|(id, object)| {
                    id != expected_id
                        && object.get("name").and_then(JsonValue::as_str) == Some(expected_name)
                }) {
                    return Err(format!(
                        "Firestorm has more than one item named {expected_name} in the same folder, and its UI path cannot distinguish them. Rename one temporarily or move it in Firestorm."
                    ));
                }
            }
        }
        Ok(())
    }

    fn select_inventory_uuid(&self, id: &str) -> Result<(), String> {
        self.request_without_viewer_reply(
            "LLCommandDispatcher",
            json!({
                "op": "dispatch",
                "cmd": "inventory",
                "params": [id, "select"],
                "query": {},
                "trusted": true
            }),
        )?;
        Ok(())
    }

    fn wait_for_inventory_row(&self, path: &str, name: &str) -> Result<(), String> {
        for _ in 0..12 {
            thread::sleep(Duration::from_millis(150));
            match self.request("LLWindow", json!({ "op": "getInfo", "path": path })) {
                Ok(info)
                    if info.get("visible_chain").and_then(JsonValue::as_bool) == Some(true)
                        && info.get("available").and_then(JsonValue::as_bool) == Some(true) =>
                {
                    return Ok(());
                }
                Ok(_) => {}
                Err(error) if is_invalid_view_path(&error) => {}
                Err(error) => return Err(error),
            }
        }
        Err(format!(
            "Firestorm did not show {name} in the All Items inventory tree. Clear its filters and try again."
        ))
    }

    fn press_inventory_key(&self, key: &str, focus_path: &str) -> Result<(), String> {
        self.press_inventory_key_with_mask(key, focus_path, &[])
    }

    fn press_inventory_key_with_mask(
        &self,
        key: &str,
        focus_path: &str,
        mask: &[&str],
    ) -> Result<(), String> {
        self.request(
            "LLWindow",
            json!({ "op": "keyDown", "keysym": key, "mask": mask, "path": focus_path }),
        )?;
        self.request(
            "LLWindow",
            json!({ "op": "keyUp", "keysym": key, "mask": mask, "path": focus_path }),
        )?;
        Ok(())
    }

    pub fn describe_api(&self, api: &str) -> Result<JsonValue, String> {
        let is_exposed = {
            let shared = self
                .shared
                .lock()
                .map_err(|_| "The LEAP connection state is unavailable.".to_string())?;
            shared.status.connected && shared.status.api_names.iter().any(|name| name == api)
        };
        if !is_exposed {
            return Err("That API is not in the current Firestorm session's exposed API list. Inspect the viewer again first.".into());
        }

        self.request("command", json!({ "op": "getAPI", "api": api }))
    }

    fn request(&self, api: &str, event: JsonValue) -> Result<JsonValue, String> {
        self.request_with_reply_mode(api, event, true)
    }

    fn request_without_viewer_reply(
        &self,
        api: &str,
        event: JsonValue,
    ) -> Result<JsonValue, String> {
        self.request_with_reply_mode(api, event, false)
    }

    fn request_with_reply_mode(
        &self,
        api: &str,
        event: JsonValue,
        viewer_reply_expected: bool,
    ) -> Result<JsonValue, String> {
        let request_id = uuid::Uuid::new_v4().simple().to_string();
        let (response_tx, response_rx) = mpsc::channel();
        let outgoing = {
            let mut shared = self
                .shared
                .lock()
                .map_err(|_| "The LEAP connection state is unavailable.".to_string())?;
            if !shared.status.connected {
                return Err("Start stock Firestorm with the Shelfmark LEAP helper first.".into());
            }
            shared.pending.insert(request_id.clone(), response_tx);
            shared.outgoing.clone()
        };
        let Some(outgoing) = outgoing else {
            if let Ok(mut shared) = self.shared.lock() {
                shared.pending.remove(&request_id);
            }
            return Err("The Firestorm LEAP helper is disconnected.".into());
        };
        if outgoing
            .send(json!({ "id": request_id.clone(), "api": api, "event": event, "viewerReplyExpected": viewer_reply_expected }))
            .is_err()
        {
            if let Ok(mut shared) = self.shared.lock() {
                shared.pending.remove(&request_id);
            }
            return Err("The Firestorm LEAP helper is disconnected.".into());
        }
        leap_log!("Shelfmark LEAP app: queued {api} request {request_id}.");
        match response_rx.recv_timeout(APP_RPC_TIMEOUT) {
            Ok(result) => result,
            Err(_) => {
                if let Ok(mut shared) = self.shared.lock() {
                    shared.pending.remove(&request_id);
                }
                leap_log!(
                    "Shelfmark LEAP app: timed out waiting for helper response {request_id}."
                );
                Err("Shelfmark did not receive a response from its LEAP helper in time. Check the latest Firestorm.log for the helper's final diagnostic line.".into())
            }
        }
    }

    fn set_error(&self, message: String) {
        if let Ok(mut shared) = self.shared.lock() {
            shared.firestorm_executable = None;
            shared.native_bridge_enabled = false;
            shared.status = LeapStatus {
                phase: "error".into(),
                message,
                ..LeapStatus::default()
            };
        }
    }
}

fn accept_helper(
    listener: TcpListener,
    address: SocketAddr,
    token: String,
    shared: Arc<Mutex<LeapShared>>,
) {
    let deadline = Instant::now() + Duration::from_secs(90);
    while Instant::now() < deadline {
        match listener.accept() {
            Ok((stream, _peer)) => {
                if let Err(error) = accept_authenticated(stream, &token, &shared) {
                    let already_connected = shared
                        .lock()
                        .map(|state| state.status.connected)
                        .unwrap_or(false);
                    if already_connected {
                        set_shared_error(&shared, error);
                        return;
                    }
                    leap_log!("Shelfmark LEAP app: rejected local connection before authentication: {error}");
                } else {
                    return;
                }
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(120));
            }
            Err(error) => {
                set_shared_error(&shared, format!("Local LEAP listener stopped: {error}"));
                return;
            }
        }
    }
    let still_starting = shared
        .lock()
        .map(|state| state.status.phase == "starting")
        .unwrap_or(false);
    if still_starting {
        set_shared_error(
            &shared,
            format!("Stock Firestorm did not start its Shelfmark LEAP helper at {address}."),
        );
    }
}

fn accept_authenticated(
    stream: TcpStream,
    expected_token: &str,
    shared: &Arc<Mutex<LeapShared>>,
) -> Result<(), String> {
    // The listener is nonblocking so accept_helper can poll for a connection.
    // On Windows an accepted socket inherits the listener's mode; switch this
    // connection back to blocking before BufReader::read_line starts waiting.
    stream
        .set_nonblocking(false)
        .map_err(|error| format!("Could not configure the local LEAP connection: {error}"))?;
    stream
        .set_read_timeout(Some(LOCAL_HANDSHAKE_TIMEOUT))
        .map_err(|error| {
            format!("Could not configure the local LEAP handshake timeout: {error}")
        })?;
    stream
        .set_write_timeout(Some(LOCAL_HANDSHAKE_TIMEOUT))
        .map_err(|error| {
            format!("Could not configure the local LEAP handshake timeout: {error}")
        })?;
    let writer = stream
        .try_clone()
        .map_err(|error| format!("Could not open local LEAP writer: {error}"))?;
    let mut reader = BufReader::new(stream);
    let greeting = read_handshake_line(&mut reader, MAX_LOCAL_HANDSHAKE_LINE)
        .map_err(|error| format!("Could not read LEAP helper greeting: {error}"))?
        .ok_or_else(|| "The local LEAP helper closed before sending its greeting.".to_string())?;
    let greeting: JsonValue = serde_json::from_str(&greeting)
        .map_err(|error| format!("Invalid local LEAP helper greeting: {error}"))?;
    if greeting.get("token").and_then(JsonValue::as_str) != Some(expected_token) {
        return Err("The local LEAP helper token did not match.".into());
    }
    let mut writer = writer;
    serde_json::to_writer(&mut writer, &json!({ "accepted": true }))
        .map_err(|error| format!("Could not accept the LEAP helper: {error}"))?;
    writer
        .write_all(b"\n")
        .and_then(|_| writer.flush())
        .map_err(|error| format!("Could not finish the LEAP handshake: {error}"))?;
    reader
        .get_ref()
        .set_read_timeout(None)
        .map_err(|error| format!("Could not configure the local LEAP session: {error}"))?;
    writer
        .set_write_timeout(None)
        .map_err(|error| format!("Could not configure the local LEAP session: {error}"))?;

    let (outgoing_tx, outgoing_rx) = mpsc::channel::<JsonValue>();
    if let Ok(mut state) = shared.lock() {
        state.status.phase = "connected".into();
        state.status.connected = true;
        state.status.native_bridge_enabled = state.native_bridge_enabled;
        state.status.message = if state.native_bridge_enabled {
            "Patched Firestorm Mini connected. Shelfmark can request confirmed Trash moves through the native bridge.".into()
        } else {
            "Stock Firestorm connected. Querying its LEAP APIs is ready.".into()
        };
        state.outgoing = Some(outgoing_tx);
    }
    thread::spawn(move || {
        let mut writer = writer;
        for message in outgoing_rx {
            if serde_json::to_writer(&mut writer, &message).is_err()
                || writer.write_all(b"\n").is_err()
                || writer.flush().is_err()
            {
                break;
            }
        }
    });

    loop {
        let Some(line) = read_bounded_line(&mut reader, MAX_LOCAL_JSON_LINE)
            .map_err(|error| format!("Lost the Firestorm LEAP connection: {error}"))?
        else {
            break;
        };
        let line_length = line.len();
        let Ok(message) = serde_json::from_str::<JsonValue>(&line) else {
            leap_log!(
                "Shelfmark LEAP app: ignored malformed helper response ({line_length} bytes)."
            );
            continue;
        };
        let Some(request_id) = message.get("id").and_then(JsonValue::as_str) else {
            leap_log!("Shelfmark LEAP app: ignored helper response without a string ID.");
            continue;
        };
        leap_log!("Shelfmark LEAP app: read helper response {request_id} ({line_length} bytes).");
        let pending = shared.lock().ok().and_then(|mut state| {
            let pending = state.pending.remove(request_id);
            if pending.is_none() {
                leap_log!(
                    "Shelfmark LEAP app: no pending request matched helper response {request_id}."
                );
            }
            pending
        });
        if let Some(pending) = pending {
            let result = if message.get("ok").and_then(JsonValue::as_bool) == Some(true) {
                Ok(message.get("result").cloned().unwrap_or(JsonValue::Null))
            } else {
                Err(message
                    .get("error")
                    .and_then(JsonValue::as_str)
                    .unwrap_or("Firestorm LEAP request failed.")
                    .to_owned())
            };
            if pending.send(result).is_err() {
                leap_log!(
                    "Shelfmark LEAP app: request receiver was already closed for {request_id}."
                );
            } else {
                leap_log!("Shelfmark LEAP app: delivered helper response {request_id}.");
            }
        }
    }
    if let Ok(mut state) = shared.lock() {
        state.status.connected = false;
        state.status.phase = "disconnected".into();
        state.status.message = if state.native_bridge_enabled {
            "Firestorm Mini disconnected from the Shelfmark LEAP helper.".into()
        } else {
            "Stock Firestorm disconnected from the Shelfmark LEAP helper.".into()
        };
        state.outgoing = None;
        for (_, pending) in state.pending.drain() {
            let _ = pending.send(Err("Stock Firestorm disconnected.".into()));
        }
    }
    Ok(())
}

fn set_shared_error(shared: &Arc<Mutex<LeapShared>>, message: String) {
    if let Ok(mut state) = shared.lock() {
        state.outgoing = None;
        for (_, pending) in state.pending.drain() {
            let _ = pending.send(Err(message.clone()));
        }
        state.status = LeapStatus {
            phase: "error".into(),
            native_bridge_enabled: state.native_bridge_enabled,
            message,
            ..LeapStatus::default()
        };
    }
}

pub fn run_child(args: &[std::ffi::OsString]) -> Result<(), String> {
    if args.len() != 2 {
        return Err("Expected the local Shelfmark LEAP address and session token.".into());
    }
    let address = args[0]
        .to_str()
        .ok_or_else(|| "The local LEAP address is not valid text.".to_string())?;
    let token = args[1]
        .to_str()
        .ok_or_else(|| "The local LEAP token is not valid text.".to_string())?;
    let stdin = io::stdin();
    let intro = read_leap_packet(&mut stdin.lock()).map_err(|error| error.to_string())?;
    let reply_pump = take_llsd_string(intro.get("pump"), "LEAP reply pump")?;
    let command_pump = take_llsd_string(
        intro.get("data").and_then(|data| data.get("command")),
        "LEAP command pump",
    )?;
    leap_log!("Shelfmark LEAP helper: read viewer introduction.");

    let stream = TcpStream::connect(address)
        .map_err(|error| format!("Could not connect to Shelfmark's local listener: {error}"))?;
    stream
        .set_nodelay(true)
        .map_err(|error| format!("Could not tune the local LEAP connection: {error}"))?;
    let reader_stream = stream
        .try_clone()
        .map_err(|error| format!("Could not open the local LEAP reader: {error}"))?;
    let mut app_writer = stream
        .try_clone()
        .map_err(|error| format!("Could not open the local LEAP writer: {error}"))?;
    serde_json::to_writer(&mut app_writer, &json!({ "token": token }))
        .map_err(|error| format!("Could not greet Shelfmark: {error}"))?;
    app_writer
        .write_all(b"\n")
        .and_then(|_| app_writer.flush())
        .map_err(|error| format!("Could not send the LEAP greeting: {error}"))?;
    let mut app_reader = BufReader::new(reader_stream);
    app_reader
        .get_ref()
        .set_read_timeout(Some(LOCAL_HANDSHAKE_TIMEOUT))
        .map_err(|error| {
            format!("Could not configure the local LEAP handshake timeout: {error}")
        })?;
    let accepted = read_handshake_line(&mut app_reader, MAX_LOCAL_HANDSHAKE_LINE)
        .map_err(|error| format!("Shelfmark did not accept the LEAP session: {error}"))?
        .ok_or_else(|| "Shelfmark closed before accepting the LEAP session.".to_string())?;
    if serde_json::from_str::<JsonValue>(&accepted)
        .ok()
        .and_then(|value| value.get("accepted").and_then(JsonValue::as_bool))
        != Some(true)
    {
        return Err("Shelfmark rejected the local LEAP session.".into());
    }
    app_reader
        .get_ref()
        .set_read_timeout(None)
        .map_err(|error| format!("Could not configure the local LEAP session: {error}"))?;
    leap_log!("Shelfmark LEAP helper: local Shelfmark connection accepted.");

    let (viewer_tx, viewer_rx) = mpsc::channel::<Result<Llsd, String>>();
    thread::spawn(move || {
        let stdin = io::stdin();
        let mut reader = stdin.lock();
        loop {
            let packet = read_leap_packet(&mut reader);
            match &packet {
                Ok(packet) => {
                    let pump = packet
                        .get("pump")
                        .and_then(llsd_scalar_string)
                        .unwrap_or_else(|| "(no pump)".into());
                    let reqid = packet
                        .get("data")
                        .and_then(|data| data.get("reqid"))
                        .and_then(llsd_scalar_string)
                        .unwrap_or_else(|| "(no reqid)".into());
                    leap_log!("Shelfmark LEAP helper: received viewer event (pump {pump}, reqid {reqid}).");
                }
                Err(error) => leap_log!("Shelfmark LEAP helper: viewer event read failed: {error}"),
            }
            // A complete length-framed packet can fail LLSD decoding without
            // desynchronizing the next packet. Keep the reader alive for later
            // requests; EOF and invalid length prefixes remain fatal.
            let recoverable = packet.as_ref().err().is_some_and(|error| {
                error.kind() == io::ErrorKind::InvalidData
                    && error.to_string().starts_with("Invalid LLSD packet")
            });
            let finished = packet.is_err() && !recoverable;
            if viewer_tx
                .send(packet.map_err(|error| error.to_string()))
                .is_err()
                || finished
            {
                break;
            }
        }
    });

    let mut app_reader = app_reader;
    let stdout = io::stdout();
    let mut leap_writer = stdout.lock();
    loop {
        let Some(line) = read_bounded_line(&mut app_reader, MAX_LOCAL_JSON_LINE)
            .map_err(|error| format!("Lost Shelfmark's local LEAP connection: {error}"))?
        else {
            return Ok(());
        };
        let request: JsonValue = serde_json::from_str(&line)
            .map_err(|error| format!("Invalid local LEAP request: {error}"))?;
        let request_id = request
            .get("id")
            .and_then(JsonValue::as_str)
            .ok_or_else(|| "The local LEAP request has no ID.".to_string())?;
        let api = request
            .get("api")
            .and_then(JsonValue::as_str)
            .ok_or_else(|| "The local LEAP request has no API name.".to_string())?;
        let event = request
            .get("event")
            .cloned()
            .filter(JsonValue::is_object)
            .ok_or_else(|| "The local LEAP request has no event map.".to_string())?;
        let viewer_reply_expected = request
            .get("viewerReplyExpected")
            .and_then(JsonValue::as_bool)
            .unwrap_or(true);
        let operation = event
            .get("op")
            .and_then(JsonValue::as_str)
            .unwrap_or("(no op)")
            .to_owned();
        leap_log!(
            "Shelfmark LEAP helper: forwarding {api}.{operation} request (local id {request_id})."
        );
        let result = call_viewer_api(
            &mut leap_writer,
            &viewer_rx,
            api,
            event,
            &reply_pump,
            &command_pump,
            viewer_reply_expected,
        );
        let response = match result {
            Ok(value) => {
                leap_log!("Shelfmark LEAP helper: viewer API returned for {api}.{operation}; replying to Shelfmark (local id {request_id}).");
                json!({ "id": request_id, "ok": true, "result": llsd_to_json(&value) })
            }
            Err(error) => {
                leap_log!("Shelfmark LEAP helper: {api}.{operation} failed: {error}; replying to Shelfmark (local id {request_id}).");
                json!({ "id": request_id, "ok": false, "error": error })
            }
        };
        serde_json::to_writer(&mut app_writer, &response)
            .map_err(|error| format!("Could not return the LEAP response: {error}"))?;
        app_writer
            .write_all(b"\n")
            .and_then(|_| app_writer.flush())
            .map_err(|error| format!("Could not finish the LEAP response: {error}"))?;
        leap_log!("Shelfmark LEAP helper: local response flushed for id {request_id}.");
    }
}

fn read_bounded_line<R: BufRead>(reader: &mut R, max_bytes: usize) -> io::Result<Option<String>> {
    read_bounded_line_before_each_read(reader, max_bytes, || Ok(()))
}

fn read_handshake_line(
    reader: &mut BufReader<TcpStream>,
    max_bytes: usize,
) -> io::Result<Option<String>> {
    let deadline = Instant::now() + LOCAL_HANDSHAKE_TIMEOUT;
    let stream = reader.get_ref().try_clone()?;
    read_bounded_line_before_each_read(reader, max_bytes, || {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Local LEAP handshake exceeded its time limit",
            ));
        }
        stream.set_read_timeout(Some(remaining))
    })
}

fn read_bounded_line_before_each_read<R: BufRead>(
    reader: &mut R,
    max_bytes: usize,
    mut before_read: impl FnMut() -> io::Result<()>,
) -> io::Result<Option<String>> {
    let mut bytes = Vec::new();
    loop {
        before_read()?;
        let (consumed, complete) = {
            let available = reader.fill_buf()?;
            if available.is_empty() {
                break;
            }
            let newline = available.iter().position(|byte| *byte == b'\n');
            let consumed = newline.map_or(available.len(), |index| index + 1);
            if bytes.len().saturating_add(consumed) > max_bytes {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Local LEAP JSON line exceeds the supported size limit",
                ));
            }
            bytes.extend_from_slice(&available[..consumed]);
            (consumed, newline.is_some())
        };
        reader.consume(consumed);
        if complete {
            break;
        }
    }
    if bytes.is_empty() {
        return Ok(None);
    }
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

fn call_viewer_api(
    leap_writer: &mut impl Write,
    viewer_rx: &mpsc::Receiver<Result<Llsd, String>>,
    api: &str,
    event: JsonValue,
    reply_pump: &str,
    command_pump: &str,
    viewer_reply_expected: bool,
) -> Result<Llsd, String> {
    let request_id = uuid::Uuid::new_v4().simple().to_string();
    let mut event_map = json_to_llsd(event)
        .as_map()
        .cloned()
        .ok_or_else(|| "The LEAP event must be a map.".to_string())?;
    event_map.insert("reply".into(), Llsd::String(reply_pump.to_owned()));
    event_map.insert("reqid".into(), Llsd::String(request_id.clone()));
    let destination = if api == "command" { command_pump } else { api };
    let packet = Llsd::Map(HashMap::from([
        ("pump".into(), Llsd::String(destination.to_owned())),
        ("data".into(), Llsd::Map(event_map)),
    ]));
    let bytes = notation::to_vec(&packet, &notation::FormatterContext::default())
        .map_err(|error| format!("Could not serialize a LEAP request: {error}"))?;
    write!(leap_writer, "{}:", bytes.len())
        .and_then(|_| leap_writer.write_all(&bytes))
        .and_then(|_| leap_writer.flush())
        .map_err(|error| format!("Could not send a LEAP request to Firestorm: {error}"))?;
    leap_log!("Shelfmark LEAP helper: sent {api} request to pump {destination} ({request_id}).");

    if !viewer_reply_expected {
        return Ok(Llsd::Undefined);
    }

    let deadline = Instant::now() + VIEWER_RPC_TIMEOUT;
    while Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let packet = match viewer_rx.recv_timeout(remaining) {
            Ok(packet) => packet?,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                return Err("Firestorm did not answer the LEAP API request in time.".into())
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err("The Firestorm LEAP response stream closed unexpectedly.".into())
            }
        };
        let data = packet.get("data");
        let matches_request = data
            .and_then(|data| data.get("reqid"))
            .and_then(llsd_scalar_string)
            .is_some_and(|id| id == request_id);
        let received_reqid = data
            .and_then(|data| data.get("reqid"))
            .and_then(llsd_scalar_string)
            .unwrap_or_else(|| "(no reqid)".into());
        leap_log!("Shelfmark LEAP helper: response correlation for {api} (expected {request_id}, received {received_reqid}, match {matches_request}).");
        if matches_request {
            let response = data.cloned().unwrap_or(Llsd::Undefined);
            if let Some(error) = response.get("error").and_then(llsd_scalar_string) {
                return Err(error);
            }
            return Ok(response);
        }
    }
    Err("Firestorm did not answer the LEAP API request in time.".into())
}

fn read_leap_packet<R: Read>(reader: &mut R) -> io::Result<Llsd> {
    let mut prefix = Vec::with_capacity(12);
    loop {
        let mut byte = [0_u8; 1];
        reader.read_exact(&mut byte)?;
        if byte[0] == b':' {
            break;
        }
        if !byte[0].is_ascii_digit() || prefix.len() >= 10 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Invalid LEAP length prefix",
            ));
        }
        prefix.push(byte[0]);
    }
    let length = std::str::from_utf8(&prefix)
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|length| *length > 0 && *length <= MAX_LEAP_PACKET)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "LEAP packet length is out of range",
            )
        })?;
    let mut bytes = vec![0; length];
    reader.read_exact(&mut bytes)?;
    autodetect::from_slice(&bytes).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "Invalid LLSD packet ({length} bytes, {:?}): {error}",
                autodetect::detect_format(&bytes)
            ),
        )
    })
}

fn take_llsd_string(value: Option<&Llsd>, label: &str) -> Result<String, String> {
    value
        .and_then(llsd_scalar_string)
        .ok_or_else(|| format!("Firestorm intro omitted its {label}."))
}

fn llsd_scalar_string(value: &Llsd) -> Option<String> {
    match value {
        Llsd::String(value) => Some(value.clone()),
        Llsd::Uuid(value) => Some(value.hyphenated().to_string()),
        _ => None,
    }
}

fn json_to_llsd(value: JsonValue) -> Llsd {
    match value {
        JsonValue::Null => Llsd::Undefined,
        JsonValue::Bool(value) => Llsd::Boolean(value),
        JsonValue::Number(value) => value
            .as_i64()
            .and_then(|value| i32::try_from(value).ok())
            .map(Llsd::Integer)
            .or_else(|| value.as_f64().map(Llsd::Real))
            .unwrap_or(Llsd::Undefined),
        JsonValue::String(value) => Llsd::String(value),
        JsonValue::Array(values) => Llsd::Array(values.into_iter().map(json_to_llsd).collect()),
        JsonValue::Object(values) => Llsd::Map(
            values
                .into_iter()
                .map(|(key, value)| (key, json_to_llsd(value)))
                .collect(),
        ),
    }
}

fn llsd_to_json(value: &Llsd) -> JsonValue {
    match value {
        Llsd::Undefined => JsonValue::Null,
        Llsd::Boolean(value) => json!(value),
        Llsd::Integer(value) => json!(value),
        Llsd::Real(value) if value.is_finite() => json!(value),
        Llsd::Real(_) => JsonValue::Null,
        Llsd::String(value) => json!(value),
        Llsd::Uri(value) => json!(value.as_str()),
        Llsd::Uuid(value) => json!(value.hyphenated().to_string()),
        Llsd::Date(value) => json!(value.to_rfc3339()),
        Llsd::Binary(value) => {
            use base64::Engine as _;
            json!(base64::engine::general_purpose::STANDARD.encode(value))
        }
        Llsd::Array(values) => JsonValue::Array(values.iter().map(llsd_to_json).collect()),
        Llsd::Map(values) => JsonValue::Object(
            values
                .iter()
                .map(|(key, value)| (key.clone(), llsd_to_json(value)))
                .collect::<JsonMap<_, _>>(),
        ),
    }
}

fn is_invalid_view_path(error: &str) -> bool {
    let message = error.to_ascii_lowercase();
    message.contains("invalid") && message.contains("path")
}

fn escape_inventory_path_component(name: &str) -> String {
    name.replace('%', "%25").replace('/', "%2F")
}

fn log_move_diagnostic(stage: &str) {
    let path = move_diagnostic_path();
    if let Some(parent) = path.parent() {
        if std::fs::create_dir_all(parent).is_err() {
            return;
        }
    }
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let epoch_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_millis())
            .unwrap_or_default();
        let _ = writeln!(file, "{epoch_ms} {stage}");
    }
}

fn move_diagnostic_path() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("Shelfmark")
        .join("logs")
        .join("move-test.log")
}

fn move_failure_stage(error: &str) -> &'static str {
    let message = error.to_ascii_lowercase();
    if message.contains("disabled in release builds") {
        "move.failed.release_build_action_disabled"
    } else if message.contains("clipboard") {
        "move.failed.ordinary_folder_clipboard_paused"
    } else if message.contains("logged in as") {
        "move.failed.avatar_mismatch"
    } else if message.contains("not loaded in the active firestorm inventory") {
        "move.failed.live_item_or_folder_not_loaded"
    } else if message.contains("more than one item named") {
        "move.failed.duplicate_sibling_name"
    } else if message.contains("inventory search field") || message.contains("filters") {
        "move.failed.inventory_filter_active"
    } else if message.contains("inventory window") || message.contains("all items tab") {
        "move.failed.inventory_window_not_ready"
    } else if message.contains("did not show") || message.contains("not visible") {
        "move.failed.inventory_row_not_visible"
    } else if message.contains("did not handle") {
        "move.failed.inventory_row_click_unhandled"
    } else if message.contains("did not answer") || message.contains("timed out") {
        "move.failed.leap_request_timeout"
    } else if message.contains("did not move the item to trash") {
        "move.failed.trash_delete_not_verified"
    } else if message.contains("folder") && message.contains("not loaded") {
        "move.failed.live_folder_not_loaded"
    } else {
        "move.failed.unknown_stage_check_app_error"
    }
}

fn normalize_uuid(value: &str, label: &str) -> Result<String, String> {
    let parsed =
        uuid::Uuid::parse_str(value).map_err(|_| format!("The {label} UUID is invalid."))?;
    if parsed.is_nil() {
        return Err(format!("The {label} UUID is invalid."));
    }
    Ok(parsed.hyphenated().to_string())
}

fn required_string(value: &JsonValue, field: &str, label: &str) -> Result<String, String> {
    value
        .get(field)
        .and_then(JsonValue::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("Firestorm omitted {label} {field}."))
}

fn collect_api_names(value: &JsonValue) -> Vec<String> {
    let value = value.get("apis").unwrap_or(value);
    let mut names = if let Some(map) = value.as_object() {
        map.keys()
            .filter(|key| *key != "reqid" && *key != "reply")
            .cloned()
            .collect()
    } else if let Some(array) = value.as_array() {
        array
            .iter()
            .filter_map(|entry| {
                entry.as_str().map(str::to_owned).or_else(|| {
                    entry
                        .get("name")
                        .and_then(JsonValue::as_str)
                        .map(str::to_owned)
                })
            })
            .collect()
    } else {
        Vec::new()
    };
    names.sort_unstable();
    names
}

fn find_first_string(value: &JsonValue, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        value
            .get(key)
            .and_then(JsonValue::as_str)
            .map(str::to_owned)
    })
}
