//! The IPC surface between the window and the catalogue.
//!
//! Commands are deliberately thin: they translate arguments, take the database lock,
//! and hand off to the library. Anything that could take more than a moment — a scan —
//! runs on a blocking thread so the window never freezes.

pub mod meta;
pub mod transfer;

use crate::db::{Db, DriveRow, ItemFilter, ItemRow};
use crate::platform::volume::{self, VolumeInfo};
use crate::scan::classify::ClassifyConfig;
use crate::scan::walker;
use serde::Serialize;
use std::path::PathBuf;
use std::sync::Mutex;
use tauri::State;

/// Application state: one catalogue connection, serialised by a mutex.
///
/// SQLite in WAL mode handles concurrent readers, but a single connection behind a
/// mutex is simpler and entirely sufficient — the heavy work is filesystem I/O, which
/// happens outside the lock.
pub struct AppState {
    pub db: Mutex<Db>,
}

/// Errors crossing the IPC boundary, as a plain message the UI can show.
#[derive(Debug, Serialize)]
pub struct CmdError {
    message: String,
}

impl<E: std::fmt::Display> From<E> for CmdError {
    fn from(e: E) -> Self {
        Self { message: e.to_string() }
    }
}

pub(crate) type CmdResult<T> = std::result::Result<T, CmdError>;

/// A drive as the UI shows it: catalogue row plus live facts when attached.
#[derive(Debug, Serialize)]
pub struct DriveView {
    #[serde(flatten)]
    pub row: DriveRow,
    /// Present only while the drive is attached.
    pub live: Option<VolumeInfo>,
    pub is_external: bool,
}

/// Refresh the drive list against what is physically attached right now.
///
/// Attached volumes are upserted by GUID; everything else is flipped offline but keeps
/// all of its catalogued contents. This is the operation behind offline browsing.
#[tauri::command]
pub async fn refresh_drives(state: State<'_, AppState>) -> CmdResult<Vec<DriveView>> {
    let present = volume::enumerate();
    let guids: Vec<String> = present.iter().map(|v| v.volume_guid.clone()).collect();

    let mut db = state.db.lock().map_err(|e| CmdError::from(e.to_string()))?;
    for v in &present {
        // Volumes with no mount point and no label are system partitions the user
        // cannot browse; cataloguing them would only add noise.
        if v.mount_point.is_none() && v.label.is_empty() {
            continue;
        }
        db.upsert_drive(v)?;
    }
    db.mark_offline_except(&guids)?;

    let rows = db.list_drives()?;
    Ok(rows
        .into_iter()
        .map(|row| {
            let live = present
                .iter()
                .find(|v| v.volume_guid == row.volume_guid)
                .cloned();
            let is_external = live
                .as_ref()
                .map(|v| v.bus_type.is_removable_bus())
                .unwrap_or(row.bus_type == "usb" || row.bus_type == "sd");
            DriveView { row, live, is_external }
        })
        .collect())
}

#[derive(Debug, Serialize)]
pub struct ScanSummary {
    pub drive_id: i64,
    pub drive_label: String,
    pub dirs_seen: u64,
    pub files_seen: u64,
    pub bytes_seen: u64,
    pub items_found: usize,
    pub items_new: usize,
    pub items_missing: usize,
    pub elapsed_ms: u128,
}

/// Scan one path and fold the results into the catalogue.
///
/// The walk runs on a blocking thread: it is filesystem-bound, and holding the async
/// runtime for the duration would stall every other command.
#[tauri::command]
pub async fn scan_path(path: String, state: State<'_, AppState>) -> CmdResult<ScanSummary> {
    let root = PathBuf::from(&path);
    if !root.exists() {
        return Err(CmdError::from(format!("path not found: {path}")));
    }

    // Resolve the letter to stable identity immediately, so nothing downstream ever
    // stores a drive letter.
    let guid = volume::guid_for_path(&root)
        .ok_or_else(|| CmdError::from(format!("no volume found for {path}")))?;
    let info = volume::describe(&guid)
        .ok_or_else(|| CmdError::from("volume disappeared during scan setup"))?;

    let started = std::time::Instant::now();
    let root2 = root.clone();
    let drive_root = PathBuf::from(info.mount_point.clone().unwrap_or_default());
    let result = tauri::async_runtime::spawn_blocking(move || {
        let mut r = walker::scan(&root2, &ClassifyConfig::default());
        // Store paths relative to the drive root, not the scanned subfolder.
        r.rebase(&root2, &drive_root);
        r
    })
    .await
    .map_err(|e| CmdError::from(e.to_string()))?;
    let elapsed_ms = started.elapsed().as_millis();

    let mut db = state.db.lock().map_err(|e| CmdError::from(e.to_string()))?;
    let drive_id = db.upsert_drive(&info)?;
    let (found, new, missing) = db.record_scan(drive_id, &result.items)?;

    Ok(ScanSummary {
        drive_id,
        drive_label: info.display_name(),
        dirs_seen: result.stats.dirs_seen,
        files_seen: result.stats.files_seen,
        bytes_seen: result.stats.bytes_seen,
        items_found: found,
        items_new: new,
        items_missing: missing,
        elapsed_ms,
    })
}

#[tauri::command]
pub async fn list_items(filter: ItemFilter, state: State<'_, AppState>) -> CmdResult<Vec<ItemRow>> {
    let db = state.db.lock().map_err(|e| CmdError::from(e.to_string()))?;
    Ok(db.list_items(&filter)?)
}

/// Record the user's decision. The only writer of `user_verdict`; no rescan will
/// overwrite what this sets.
#[tauri::command]
pub async fn set_verdict(
    item_id: i64,
    verdict: Option<String>,
    state: State<'_, AppState>,
) -> CmdResult<()> {
    let mut db = state.db.lock().map_err(|e| CmdError::from(e.to_string()))?;
    db.set_user_verdict(item_id, verdict.as_deref())?;
    Ok(())
}

#[tauri::command]
pub async fn set_title(
    item_id: i64,
    title: Option<String>,
    state: State<'_, AppState>,
) -> CmdResult<()> {
    let mut db = state.db.lock().map_err(|e| CmdError::from(e.to_string()))?;
    db.set_user_title(item_id, title.as_deref())?;
    Ok(())
}

#[derive(Debug, Serialize)]
pub struct DuplicateGroup {
    pub title_key: String,
    pub copies: i64,
    pub total_bytes: i64,
    pub items: Vec<ItemRow>,
}

/// Titles present on more than one drive, with the reclaimable size.
#[tauri::command]
pub async fn list_duplicates(state: State<'_, AppState>) -> CmdResult<Vec<DuplicateGroup>> {
    let db = state.db.lock().map_err(|e| CmdError::from(e.to_string()))?;
    let groups = db.duplicates()?;
    let all = db.list_items(&ItemFilter { limit: Some(5000), ..Default::default() })?;

    Ok(groups
        .into_iter()
        .map(|(key, copies, bytes)| {
            let items: Vec<ItemRow> = all
                .iter()
                .filter(|i| crate::scan::title::make_key(&i.clean_title) == key)
                .cloned()
                .collect();
            DuplicateGroup { title_key: key, copies, total_bytes: bytes, items }
        })
        .collect())
}

#[derive(Debug, Serialize)]
pub struct StorageSlice {
    pub kind: String,
    pub count: i64,
    pub bytes: i64,
}

#[tauri::command]
pub async fn storage_breakdown(
    drive_id: i64,
    state: State<'_, AppState>,
) -> CmdResult<Vec<StorageSlice>> {
    let db = state.db.lock().map_err(|e| CmdError::from(e.to_string()))?;
    Ok(db
        .storage_breakdown(drive_id)?
        .into_iter()
        .map(|(kind, count, bytes)| StorageSlice { kind, count, bytes })
        .collect())
}

/// Where the catalogue lives, shown in the UI so the user knows what to back up.
#[tauri::command]
pub async fn catalog_path() -> CmdResult<String> {
    Ok(Db::default_path().to_string_lossy().into_owned())
}
