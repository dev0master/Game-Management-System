//! The IPC surface between the window and the catalogue.
//!
//! Commands are deliberately thin: they translate arguments, take the database lock,
//! and hand off to the library. Anything that could take more than a moment — a scan —
//! runs on a blocking thread so the window never freezes.

pub mod console;
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
///
/// **Only removable drives are catalogued or returned.** This app manages game drives that
/// come and go over USB; the system disk is not one of them, and offering it only invites a
/// scan of somewhere with no games in it. Internal volumes are still enumerated — they have
/// to be, or every one of them would be flipped offline on each refresh — they are simply
/// never written to the catalogue and never shown.
#[tauri::command]
pub async fn refresh_drives(state: State<'_, AppState>) -> CmdResult<Vec<DriveView>> {
    let present = volume::enumerate();
    // Every present volume, removable or not: this list decides what stays marked online.
    let guids: Vec<String> = present.iter().map(|v| v.volume_guid.clone()).collect();

    let mut db = state.db.lock().map_err(|e| CmdError::from(e.to_string()))?;
    for v in &present {
        if !should_catalogue(v) {
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
                // Offline, so the stored bus type is all there is to go on.
                .unwrap_or(row.bus_type == "usb" || row.bus_type == "sd");
            DriveView { row, live, is_external }
        })
        // Drives catalogued by an earlier build, before this rule existed, are filtered
        // out here rather than deleted: their items stay in the catalogue until the user
        // clears it from Settings.
        .filter(|d| d.is_external)
        .collect())
}

/// Is this a volume the app manages?
///
/// Two rules. A volume with neither a mount point nor a label is a system partition the
/// user cannot browse, and cataloguing it would only add noise. A volume on a fixed bus is
/// not a game drive: this app exists to track drives that come and go, and the system disk
/// never does.
fn should_catalogue(v: &VolumeInfo) -> bool {
    if v.mount_point.is_none() && v.label.is_empty() {
        return false;
    }
    v.bus_type.is_removable_bus()
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

/* ------------------------------------------------------------ resetting */

/// What a reset removed.
#[derive(Debug, Default, Serialize)]
pub struct ResetSummary {
    pub catalog_removed: bool,
    pub credentials_removed: bool,
    pub covers_removed: usize,
    pub bytes_freed: u64,
}

/// The token the window must send. A reset is irreversible, so it cannot be reachable by
/// an accidental or malformed `invoke`.
const RESET_TOKEN: &str = "RESET-EVERYTHING";

/// Everything this app has ever written, and nothing else.
///
/// Listed explicitly rather than deleting the folder wholesale: a recursive delete of a
/// directory path assembled at runtime is the kind of code that removes the wrong thing
/// once, and once is enough.
fn app_data_files(dir: &std::path::Path) -> [PathBuf; 4] {
    [
        dir.join("catalog.db"),
        dir.join("catalog.db-wal"),
        dir.join("catalog.db-shm"),
        dir.join("secrets.bin"),
    ]
}

/// Erase everything the app has stored and start over empty.
///
/// **Nothing on any drive is touched.** Every path removed here is inside
/// `%LOCALAPPDATA%\GameVault`: the catalogue, the cover cache, and the saved API key. The
/// games themselves are never opened for writing by this command — the catalogue is a
/// description of those drives, and deleting a description cannot delete what it describes.
///
/// The catalogue is reopened empty before returning, so the window keeps working.
#[tauri::command]
pub async fn reset_app_data(
    confirm: String,
    state: State<'_, AppState>,
) -> CmdResult<ResetSummary> {
    if confirm != RESET_TOKEN {
        return Err(CmdError::from("reset not confirmed"));
    }

    let catalog = Db::default_path();
    let dir = catalog
        .parent()
        .ok_or_else(|| CmdError::from("cannot locate the application data folder"))?
        .to_path_buf();
    let covers = crate::commands::meta::cover_dir();

    // Refuse to act outside our own folder, whatever the environment says.
    if !covers.starts_with(&dir) {
        return Err(CmdError::from("cover cache is outside the application data folder"));
    }

    // Close the catalogue before deleting it: on Windows an open handle makes the file
    // undeletable, and the WAL would otherwise be replayed into a resurrected database.
    let mut db = state.db.lock().map_err(|e| CmdError::from(e.to_string()))?;
    *db = Db::open_in_memory()?;

    let summary = erase_stored_data(&dir, &covers);

    // Back to a real, empty catalogue so the window is usable immediately.
    *db = Db::open(&catalog)?;
    Ok(summary)
}

/// Delete the app's own files under `dir`, and every cover in `covers`.
///
/// Separated from the command so it can be exercised against a fixture directory. This is
/// the only code in the project that deletes something the user did not point at, so it is
/// written to be provably incapable of straying: a fixed list of filenames, and one
/// non-recursive pass over the cover folder.
fn erase_stored_data(dir: &std::path::Path, covers: &std::path::Path) -> ResetSummary {
    let mut summary = ResetSummary::default();

    for path in app_data_files(dir) {
        let Ok(meta) = std::fs::metadata(&path) else { continue };
        if std::fs::remove_file(&path).is_ok() {
            summary.bytes_freed += meta.len();
            if path.ends_with("catalog.db") {
                summary.catalog_removed = true;
            }
            if path.ends_with("secrets.bin") {
                summary.credentials_removed = true;
            }
        }
    }

    // Covers are deleted one at a time, and only files: a directory in there is left alone
    // rather than recursively removed.
    if let Ok(entries) = std::fs::read_dir(covers) {
        for entry in entries.flatten() {
            if !entry.file_type().is_ok_and(|t| t.is_file()) {
                continue;
            }
            let len = entry.metadata().map(|m| m.len()).unwrap_or(0);
            if std::fs::remove_file(entry.path()).is_ok() {
                summary.covers_removed += 1;
                summary.bytes_freed += len;
            }
        }
    }

    summary
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::volume::BusType;
    use std::fs;

    fn tmp(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "gv_reset_{tag}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .subsec_nanos()
        ));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    fn vol(mount: Option<&str>, label: &str, bus: BusType) -> VolumeInfo {
        VolumeInfo {
            volume_guid: "{g}".into(),
            volume_serial: "S".into(),
            mount_point: mount.map(str::to_string),
            label: label.into(),
            filesystem: "exFAT".into(),
            bus_type: bus,
            total_bytes: 1,
            free_bytes: 1,
            is_unjournaled: false,
            max_file_bytes: None,
        }
    }

    /// The app manages drives that come and go. The system disk is not one of them, and
    /// offering it invited a scan of somewhere with no games on it.
    #[test]
    fn only_removable_drives_are_catalogued() {
        assert!(should_catalogue(&vol(Some("E:\\"), "GAMES", BusType::Usb)));
        assert!(should_catalogue(&vol(Some("F:\\"), "SD CARD", BusType::Sd)));

        assert!(!should_catalogue(&vol(Some("C:\\"), "Windows", BusType::Nvme)));
        assert!(!should_catalogue(&vol(Some("D:\\"), "New Volume", BusType::Nvme)));
        assert!(!should_catalogue(&vol(Some("G:\\"), "Data", BusType::Sata)));
        // A partition with neither mount point nor label cannot be browsed at all.
        assert!(!should_catalogue(&vol(None, "", BusType::Usb)));
    }

    /// A reset clears what the app stored — and provably nothing else.
    #[test]
    fn a_reset_removes_only_the_apps_own_files() {
        let root = tmp("scope");
        let dir = root.join("GameVault");
        let covers = dir.join("covers");
        fs::create_dir_all(&covers).unwrap();

        fs::write(dir.join("catalog.db"), vec![0u8; 4096]).unwrap();
        fs::write(dir.join("catalog.db-wal"), vec![0u8; 1024]).unwrap();
        fs::write(dir.join("catalog.db-shm"), vec![0u8; 512]).unwrap();
        fs::write(dir.join("secrets.bin"), vec![0u8; 64]).unwrap();
        fs::write(covers.join("steam412020.jpg"), vec![0u8; 2048]).unwrap();
        fs::write(covers.join("ps4_CUSA11407.icon0.png"), vec![0u8; 1024]).unwrap();

        // Things that must survive: a game drive beside the data folder, and a file the
        // app did not write sitting in its own folder.
        let drive = root.join("E_drive");
        fs::create_dir_all(&drive).unwrap();
        fs::write(drive.join("Metro Exodus.pkg"), vec![0u8; 8192]).unwrap();
        fs::write(dir.join("notes-from-the-user.txt"), b"keep me").unwrap();
        // A directory inside covers must not be recursed into.
        fs::create_dir_all(covers.join("subfolder")).unwrap();
        fs::write(covers.join("subfolder").join("keep.jpg"), vec![0u8; 16]).unwrap();

        let s = erase_stored_data(&dir, &covers);

        assert!(s.catalog_removed);
        assert!(s.credentials_removed);
        assert_eq!(s.covers_removed, 2);
        assert_eq!(s.bytes_freed, 4096 + 1024 + 512 + 64 + 2048 + 1024);

        for gone in ["catalog.db", "catalog.db-wal", "catalog.db-shm", "secrets.bin"] {
            assert!(!dir.join(gone).exists(), "{gone} should be gone");
        }
        assert!(!covers.join("steam412020.jpg").exists());

        // The point of the whole feature.
        assert!(
            drive.join("Metro Exodus.pkg").exists(),
            "a reset must never touch anything on a drive"
        );
        assert_eq!(fs::read(dir.join("notes-from-the-user.txt")).unwrap(), b"keep me");
        assert!(covers.join("subfolder").join("keep.jpg").exists(), "no recursive delete");

        let _ = fs::remove_dir_all(&root);
    }

    /// Resetting twice is not an error: the second pass simply finds nothing.
    #[test]
    fn resetting_an_already_empty_folder_is_harmless() {
        let root = tmp("empty");
        let covers = root.join("covers");
        fs::create_dir_all(&covers).unwrap();

        let s = erase_stored_data(&root, &covers);
        assert!(!s.catalog_removed);
        assert!(!s.credentials_removed);
        assert_eq!(s.covers_removed, 0);
        assert_eq!(s.bytes_freed, 0);

        let _ = fs::remove_dir_all(&root);
    }

    /// Only the exact filenames the app writes are removed.
    #[test]
    fn the_deletion_list_is_fixed_and_named() {
        let files = app_data_files(std::path::Path::new("X"));
        let names: Vec<String> = files
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            names,
            vec!["catalog.db", "catalog.db-wal", "catalog.db-shm", "secrets.bin"]
        );
    }
}
