//! Transfer commands.
//!
//! A transfer runs on its own thread and reports progress through Tauri events rather
//! than by returning; a 41 GB copy takes minutes, and the window has to stay live
//! throughout. Only the final outcome comes back through the command channel.

use super::{AppState, CmdError, CmdResult};
use crate::transfer::engine::{self, Control, TransferOutcome};
use crate::transfer::plan::{
    preflight, CollisionPolicy, Preflight, TransferKind, TransferRequest, VerifyMode,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use tauri::{AppHandle, Emitter, Manager, State};

/// Live transfers, so the UI can pause or cancel one after it has started.
static RUNNING: OnceLock<Mutex<HashMap<u64, Arc<Control>>>> = OnceLock::new();

fn running() -> &'static Mutex<HashMap<u64, Arc<Control>>> {
    RUNNING.get_or_init(|| Mutex::new(HashMap::new()))
}

fn next_job_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// What the UI sends to start or preview a transfer.
#[derive(Debug, Clone, Deserialize)]
pub struct TransferArgs {
    pub item_id: i64,
    /// Volume GUID of the destination drive. A GUID, not a letter, because letters are
    /// reassigned between sessions.
    pub dst_drive_guid: String,
    /// Folder on the destination drive to place the item in, e.g. `Games`.
    pub dst_subfolder: String,
    pub kind: TransferKind,
    pub verify: VerifyMode,
    pub collision: CollisionPolicy,
    pub safe_mode: bool,
}

/// Turn a catalogue item plus a destination into a concrete request.
fn build_request(
    state: &State<'_, AppState>,
    args: &TransferArgs,
) -> Result<(TransferRequest, i64, String), CmdError> {
    let db = state.db.lock().map_err(|e| CmdError::from(e.to_string()))?;
    let detail = db
        .get_item(args.item_id)?
        .ok_or_else(|| CmdError::from("item not found"))?;

    // A transfer needs the source drive attached. Offline browsing is fine; offline
    // copying is not.
    let src_mount = detail
        .drive_mount
        .clone()
        .ok_or_else(|| CmdError::from("the source drive is not connected"))?;

    let dst_info = crate::platform::volume::describe(&args.dst_drive_guid)
        .ok_or_else(|| CmdError::from("the destination drive is not connected"))?;
    let dst_mount = dst_info
        .mount_point
        .clone()
        .ok_or_else(|| CmdError::from("the destination drive has no drive letter"))?;

    let dst_drive_id = db
        .drive_id_for_guid(&args.dst_drive_guid)?
        .ok_or_else(|| CmdError::from("destination drive is not in the catalogue"))?;

    let item_abs = PathBuf::from(&src_mount).join(&detail.item.rel_path);
    let is_folder = detail.item.container == "folder";

    // The root each source path is recorded against. For a folder item that is the
    // folder itself, so its internal structure is recreated at the destination; for
    // loose files it is the directory they sit in, so they land side by side.
    let source_root = if is_folder {
        item_abs.clone()
    } else {
        item_abs.parent().map(PathBuf::from).unwrap_or_default()
    };

    // For a folder item every file inside it travels; for a loose archive set it is the
    // volumes named in the catalogue. Either way the set moves as one unit.
    let (sources, folder_name, dst_rel) = if is_folder {
        let names: Vec<PathBuf> = if detail.files.is_empty() {
            // Recursive: a game folder is a tree, and collecting only its top level
            // would silently transfer a fraction of the game.
            collect_files(&item_abs)
                .map_err(|e| CmdError::from(format!("cannot read {}: {e}", item_abs.display())))?
        } else {
            detail.files.iter().map(|f| item_abs.join(&f.rel_path)).collect()
        };
        let name = PathBuf::from(&detail.item.rel_path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| detail.item.clean_title.clone());
        let rel = PathBuf::from(&args.dst_subfolder).join(&name);
        (names, Some(name), rel.to_string_lossy().into_owned())
    } else {
        let parent = item_abs.parent().map(PathBuf::from).unwrap_or_default();
        let names: Vec<PathBuf> = if detail.files.is_empty() {
            vec![item_abs.clone()]
        } else {
            detail.files.iter().map(|f| parent.join(&f.rel_path)).collect()
        };
        let primary = item_abs
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let rel = PathBuf::from(&args.dst_subfolder).join(&primary);
        (names, None, rel.to_string_lossy().into_owned())
    };

    let dst_dir = PathBuf::from(&dst_mount).join(&args.dst_subfolder);

    Ok((
        TransferRequest {
            kind: args.kind,
            source_root: Some(source_root),
            sources,
            dst_dir,
            folder_name,
            remove_empty_source_dir: is_folder,
            verify: args.verify,
            collision: args.collision,
            safe_mode: args.safe_mode,
        },
        dst_drive_id,
        dst_rel,
    ))
}

/// Check a transfer without moving anything. Always shown to the user before they
/// commit, so problems surface before the first byte rather than partway through.
#[tauri::command]
pub async fn preflight_transfer(
    args: TransferArgs,
    state: State<'_, AppState>,
) -> CmdResult<Preflight> {
    let (req, _, _) = build_request(&state, &args)?;
    Ok(preflight(&req))
}

#[derive(Debug, Serialize, Clone)]
pub struct TransferStarted {
    pub job_id: u64,
    pub files: usize,
    pub total_bytes: u64,
}

#[derive(Debug, Serialize, Clone)]
pub struct ProgressEvent {
    pub job_id: u64,
    #[serde(flatten)]
    pub progress: engine::Progress,
}

#[derive(Debug, Serialize, Clone)]
pub struct FinishedEvent {
    pub job_id: u64,
    pub outcome: TransferOutcome,
}

/// Start a transfer. Returns as soon as the job is running; progress arrives as
/// `transfer://progress` events and the result as `transfer://finished`.
#[tauri::command]
pub async fn start_transfer(
    args: TransferArgs,
    app: AppHandle,
    state: State<'_, AppState>,
) -> CmdResult<TransferStarted> {
    let (req, dst_drive_id, dst_rel) = build_request(&state, &args)?;
    let plan = preflight(&req);
    if !plan.ok {
        let first = plan
            .problems
            .first()
            .map(|p| p.message.clone())
            .unwrap_or_else(|| "preflight failed".into());
        return Err(CmdError::from(first));
    }

    let job_id = next_job_id();
    let ctl = Control::new();
    running()
        .lock()
        .map_err(|e| CmdError::from(e.to_string()))?
        .insert(job_id, ctl.clone());

    let started = TransferStarted {
        job_id,
        files: plan.files.len(),
        total_bytes: plan.total_bytes,
    };

    let item_id = args.item_id;
    let kind = args.kind;
    let safe_mode = args.safe_mode;

    std::thread::spawn(move || {
        let app2 = app.clone();
        let outcome = engine::execute(&req, &plan, &ctl, move |p| {
            let _ = app2.emit("transfer://progress", ProgressEvent { job_id, progress: p });
        });

        // Keep the catalogue in step with what actually happened on disk. A move that
        // fully verified re-points the item; a copy adds a second row. Anything that
        // failed leaves the catalogue untouched, so the next scan reconciles it.
        if let Some(state) = app.try_state::<AppState>() {
            if let Ok(mut db) = state.db.lock() {
                if outcome.all_verified && !outcome.cancelled {
                    let moved = kind == TransferKind::Move && !safe_mode;
                    let r = if moved {
                        db.relocate_item(item_id, dst_drive_id, &dst_rel)
                    } else {
                        db.note_copy(item_id, dst_drive_id, &dst_rel)
                    };
                    if let Err(e) = r {
                        eprintln!("catalogue update after transfer failed: {e}");
                    }
                }
            }
        }

        running().lock().ok().map(|mut m| m.remove(&job_id));
        let _ = app.emit("transfer://finished", FinishedEvent { job_id, outcome });
    });

    Ok(started)
}

#[tauri::command]
pub async fn cancel_transfer(job_id: u64) -> CmdResult<bool> {
    let map = running().lock().map_err(|e| CmdError::from(e.to_string()))?;
    match map.get(&job_id) {
        Some(c) => {
            c.cancel();
            Ok(true)
        }
        None => Ok(false),
    }
}

#[tauri::command]
pub async fn pause_transfer(job_id: u64, paused: bool) -> CmdResult<bool> {
    let map = running().lock().map_err(|e| CmdError::from(e.to_string()))?;
    match map.get(&job_id) {
        Some(c) => {
            c.set_paused(paused);
            Ok(true)
        }
        None => Ok(false),
    }
}

/// Full detail for one item, including its file list.
#[tauri::command]
pub async fn get_item(
    item_id: i64,
    state: State<'_, AppState>,
) -> CmdResult<Option<crate::db::ItemDetail>> {
    let db = state.db.lock().map_err(|e| CmdError::from(e.to_string()))?;
    Ok(db.get_item(item_id)?)
}

/// Show an item in File Explorer, selecting it.
#[tauri::command]
pub async fn reveal_item(item_id: i64, state: State<'_, AppState>) -> CmdResult<()> {
    let path = {
        let db = state.db.lock().map_err(|e| CmdError::from(e.to_string()))?;
        let detail = db
            .get_item(item_id)?
            .ok_or_else(|| CmdError::from("item not found"))?;
        let mount = detail
            .drive_mount
            .ok_or_else(|| CmdError::from("the drive is not connected"))?;
        PathBuf::from(mount).join(&detail.item.rel_path)
    };

    // Arguments are passed as an argv array, never as a command string: the real
    // library contains names like "Mad Max  - [DODI Repack].zip", and shell quoting
    // around brackets and spaces is a reliable source of bugs.
    std::process::Command::new("explorer.exe")
        .arg("/select,")
        .arg(&path)
        .spawn()
        .map_err(|e| CmdError::from(format!("could not open Explorer: {e}")))?;
    Ok(())
}

/// Every file beneath `root`, recursively, in a stable order.
///
/// The catalogue normally supplies the file list, so this is the fallback for a folder
/// item scanned before per-file rows existed. It must recurse: an Xbox 360 title keeps
/// its data in `<id>/<type>/<container>.data/Data0000…`, and a top-level-only listing
/// would transfer the header and none of the game.
fn collect_files(root: &std::path::Path) -> std::io::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            let ft = entry.file_type()?;
            if ft.is_dir() {
                stack.push(entry.path());
            } else if ft.is_file() {
                out.push(entry.path());
            }
            // Symlinks and reparse points are skipped: following them could walk out of
            // the item, and duplicating a link's target is not what a copy means here.
        }
    }
    out.sort();
    Ok(out)
}
