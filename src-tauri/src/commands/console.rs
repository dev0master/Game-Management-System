//! The Game Files window's one command.
//!
//! Unlike every other screen, this one reads the filesystem rather than the catalogue —
//! that is what the user asked for, so a package added a minute ago is simply there. The
//! catalogue is still used, but only as a probe cache, so the second read of a drive costs
//! nothing.
//!
//! Lock discipline follows the rest of the module: two short windows around the database,
//! with the walking, probing and icon extraction outside both of them.

use super::{AppState, CmdError, CmdResult};
use crate::commands::meta::cover_dir;
use crate::platform::volume;
use crate::scan::console::index::{self, ConsoleRead};
use crate::scan::console::ProbeRow;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Emitter, State};

/// Where the probe cache lives for one read: which drive, and where under it we are.
///
/// `None` when the path is on something with no resolvable volume — a network share, say.
/// The read still works; it just re-probes every time.
struct CacheScope {
    drive_id: i64,
    /// The read root, relative to the drive root. Empty when reading a whole drive.
    prefix: String,
}

/// Read a folder and return the console games in it.
///
/// Read-only against the folder. The only writes are cover images into the local cache,
/// and probe rows into the catalogue.
#[tauri::command]
pub async fn read_console_dir(
    root: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> CmdResult<ConsoleRead> {
    let root_path = PathBuf::from(&root);
    if !root_path.is_dir() {
        return Err(CmdError::from(format!("folder not found: {root}")));
    }

    // Window 1: register the drive and load its cached probes. One lock, one connection —
    // the volume lookup happens outside it because it talks to Win32, not to SQLite.
    let located = locate(&root_path);
    let (scope, cached) = match located {
        Some((info, prefix)) => {
            let mut db = state.db.lock().map_err(|e| CmdError::from(e.to_string()))?;
            // The drive row has to exist before a probe can reference it, and it is what
            // gives the read a stable identity rather than a letter.
            let drive_id = db.upsert_drive(&info)?;
            let all = db.load_console_probes(drive_id)?;
            // Stored keys are relative to the drive root; the reader works relative to the
            // folder being read. Narrow to this subtree and rebase.
            let cached = strip_prefix_keys(all, &prefix);
            (Some(CacheScope { drive_id, prefix }), cached)
        }
        None => (None, HashMap::new()),
    };

    // No lock held for any of this: it is all filesystem work.
    let covers = cover_dir();
    let progress_app = app.clone();
    let read = tauri::async_runtime::spawn_blocking(move || {
        index::read_tree(&root_path, &cached, &covers, &mut |p| {
            let _ = progress_app.emit("console://progress", &p);
        })
    })
    .await
    .map_err(|e| CmdError::from(e.to_string()))?;

    // Window 2: store what had to be probed, so the next read is instant.
    if let Some(s) = &scope {
        if !read.fresh_probes.is_empty() {
            let keyed: Vec<(String, ProbeRow)> = read
                .fresh_probes
                .iter()
                .map(|(rel, row)| (join_rel(&s.prefix, rel), row.clone()))
                .collect();
            let mut db = state.db.lock().map_err(|e| CmdError::from(e.to_string()))?;
            db.save_console_probes(s.drive_id, &keyed)?;
        }
    }

    Ok(read)
}

/// Work out which volume `root` sits on, and where under it.
///
/// Failure is not an error: it only means this read cannot be cached, which a network
/// share or an unusual mount will hit. The folder is still read.
fn locate(root: &Path) -> Option<(volume::VolumeInfo, String)> {
    let guid = volume::guid_for_path(root)?;
    let info = volume::describe(&guid)?;
    let mount = info.mount_point.clone()?;
    let prefix = root
        .strip_prefix(Path::new(&mount))
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    Some((info, prefix))
}

/// Keep the cached rows under `prefix` and rebase their keys onto the read root.
fn strip_prefix_keys(
    all: HashMap<String, ProbeRow>,
    prefix: &str,
) -> HashMap<String, ProbeRow> {
    if prefix.is_empty() {
        return all;
    }
    let head = format!("{prefix}\\");
    all.into_iter()
        .filter_map(|(k, v)| k.strip_prefix(&head).map(|rest| (rest.to_string(), v)))
        .collect()
}

fn join_rel(prefix: &str, rel: &str) -> String {
    if prefix.is_empty() {
        rel.to_string()
    } else {
        format!("{prefix}\\{rel}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(name: &str) -> ProbeRow {
        ProbeRow {
            name: name.into(),
            size_bytes: 1,
            mtime_unix: 0,
            platform: "ps4".into(),
            status: "ok".into(),
            title_id: None,
            content_id: None,
            category: None,
            title: None,
            app_ver: None,
            system_ver: None,
            declared_bytes: None,
            content_type: None,
        }
    }

    /// Reading `E:\Games` must reuse the probes stored while reading `E:\`, and must not
    /// pick up rows from a sibling folder.
    #[test]
    fn cache_keys_rebase_onto_the_folder_being_read() {
        let mut all = HashMap::new();
        all.insert("Games\\a.pkg".to_string(), row("a.pkg"));
        all.insert("Games\\PS4\\b.pkg".to_string(), row("b.pkg"));
        all.insert("Other\\c.pkg".to_string(), row("c.pkg"));

        let scoped = strip_prefix_keys(all, "Games");
        let mut keys: Vec<&String> = scoped.keys().collect();
        keys.sort();
        assert_eq!(keys, vec!["PS4\\b.pkg", "a.pkg"]);
    }

    /// Reading a whole drive keeps every key as stored.
    #[test]
    fn an_empty_prefix_keeps_every_key() {
        let mut all = HashMap::new();
        all.insert("Games\\a.pkg".to_string(), row("a.pkg"));
        assert_eq!(strip_prefix_keys(all, "").len(), 1);
    }

    /// Keys go back exactly as they came, so a rebase followed by a save round-trips.
    #[test]
    fn saved_keys_are_relative_to_the_drive_again() {
        assert_eq!(join_rel("Games", "PS4\\b.pkg"), "Games\\PS4\\b.pkg");
        assert_eq!(join_rel("", "b.pkg"), "b.pkg");
        assert_eq!(strip_prefix_keys(
            HashMap::from([(join_rel("Games", "PS4\\b.pkg"), row("b.pkg"))]),
            "Games",
        )
        .contains_key("PS4\\b.pkg"), true);
    }
}
