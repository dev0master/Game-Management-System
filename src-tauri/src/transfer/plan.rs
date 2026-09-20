//! Preflight: everything that must be true before a single byte moves.
//!
//! The checks here exist because the failure modes they prevent are expensive. A copy
//! that runs out of space halfway leaves a partial game; a copy onto FAT32 dies at the
//! 4 GiB mark after transferring gigabytes; a silent overwrite destroys a file the user
//! still wanted. Every one of these is cheap to detect beforehand and painful to
//! discover afterwards.

use crate::platform::volume::{self, VolumeInfo};
use serde::Serialize;
use std::path::{Path, PathBuf};

/// Copy leaves the source alone; move deletes it, but only after verification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferKind {
    Copy,
    Move,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerifyMode {
    /// Byte count only. Effectively free.
    Size,
    /// Size plus modification time, with a tolerance for filesystems that store
    /// coarse timestamps.
    SizeMtime,
    /// Full blake3 read-back. Roughly doubles the time of a job.
    Hash,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CollisionPolicy {
    /// Leave the existing file and skip the source.
    Skip,
    /// Write alongside as "name (2).ext". The default: nothing is ever destroyed
    /// without the user choosing it.
    Rename,
    /// Replace the existing file. Only ever set explicitly.
    Overwrite,
}

/// exFAT and FAT store modification times to a 2-second granularity, and FAT adds a
/// local-time offset on top. Comparing exactly against an NTFS source reports every
/// file as different, which is why robocopy has `/FFT`.
pub const FAT_TIME_TOLERANCE_SECS: u64 = 2;

/// What the user asked for.
#[derive(Debug, Clone)]
pub struct TransferRequest {
    pub kind: TransferKind,
    /// Absolute source paths. For a multi-volume archive this is every volume: the set
    /// is one unit and must never be split across a transfer.
    pub sources: Vec<PathBuf>,
    /// The item's root on disk. Every source path is recorded relative to this, so a
    /// nested game keeps its structure at the destination. `None` means the sources are
    /// loose siblings and only their filenames matter.
    pub source_root: Option<PathBuf>,
    /// Destination directory. Files land directly inside it.
    pub dst_dir: PathBuf,
    /// When the item is a folder, its name — recreated inside `dst_dir`.
    pub folder_name: Option<String>,
    /// Whether a completed move may remove the emptied source directory.
    ///
    /// Separate from `folder_name` on purpose: a flat PS4 group is given a destination
    /// folder name but its sources are loose files in the drive root, and the engine
    /// must never consider removing that.
    pub remove_empty_source_dir: bool,
    pub verify: VerifyMode,
    pub collision: CollisionPolicy,
    /// When set, a move is downgraded to a copy and nothing is ever deleted.
    /// Defaults to on.
    pub safe_mode: bool,
}

/// A condition that stops the transfer.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Problem {
    pub code: String,
    pub message: String,
}

/// A condition the user should know about but which does not stop the transfer.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Warning {
    pub code: String,
    pub message: String,
}

/// One planned file movement.
#[derive(Debug, Clone, PartialEq)]
pub struct PlannedFile {
    pub src: PathBuf,
    pub dst: PathBuf,
    /// Path of this file relative to the item root, e.g. `4E4D07D1/00007000/Data0003`.
    /// The destination is built from this, not from the bare filename.
    pub rel: PathBuf,
    pub size_bytes: u64,
    /// True when an existing destination file means this one is skipped.
    pub skipped: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Preflight {
    pub total_bytes: u64,
    pub file_count: usize,
    pub dst_free_bytes: u64,
    /// True when source and destination are the same volume, so the move is a rename.
    pub same_volume: bool,
    /// Whether the transfer may proceed.
    pub ok: bool,
    pub problems: Vec<Problem>,
    pub warnings: Vec<Warning>,
    /// Rough estimate, from a conservative sustained-throughput assumption.
    pub estimated_secs: u64,
    /// Extra seconds if hash verification is selected, so the cost is stated before
    /// the user opts in rather than appearing as an unexplained stall.
    pub hash_verify_extra_secs: u64,
    #[serde(skip)]
    pub files: Vec<PlannedFile>,
}

/// Conservative sustained throughput for a USB 3.0 mechanical drive. Used only to set
/// expectations; actual progress is reported from real byte counts.
const ASSUMED_BYTES_PER_SEC: u64 = 130 * 1024 * 1024;

/// Prefix a path for the Win32 wide APIs so it is not subject to MAX_PATH.
///
/// Applied even though long paths are enabled on this machine: the flag is per-machine
/// and the built exe may run elsewhere, and extracted game trees routinely exceed 260
/// characters.
pub fn long_path(p: &Path) -> PathBuf {
    let s = p.to_string_lossy();
    if s.starts_with("\\\\?\\") || s.starts_with("\\\\") {
        return p.to_path_buf();
    }
    // Only absolute paths can take the prefix.
    if s.len() >= 2 && s.as_bytes()[1] == b':' {
        PathBuf::from(format!("\\\\?\\{s}"))
    } else {
        p.to_path_buf()
    }
}

/// Produce a non-colliding destination name: `Game.rar` -> `Game (2).rar`.
pub fn dedupe_name(dir: &Path, name: &str) -> PathBuf {
    let candidate = dir.join(name);
    if !candidate.exists() {
        return candidate;
    }
    let path = Path::new(name);
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let ext = path.extension().map(|s| format!(".{}", s.to_string_lossy())).unwrap_or_default();
    for n in 2..10_000 {
        let next = dir.join(format!("{stem} ({n}){ext}"));
        if !next.exists() {
            return next;
        }
    }
    candidate
}

/// Longest single path component, which exFAT and NTFS both cap at 255 characters.
fn longest_component(p: &Path) -> usize {
    p.components()
        .filter_map(|c| c.as_os_str().to_str())
        .map(|s| s.chars().count())
        .max()
        .unwrap_or(0)
}

/// Check a transfer and produce its file plan.
pub fn preflight(req: &TransferRequest) -> Preflight {
    let mut problems = Vec::new();
    let mut warnings = Vec::new();

    let dst_root = match &req.folder_name {
        Some(name) => req.dst_dir.join(name),
        None => req.dst_dir.clone(),
    };

    // Source sizes. A source that has vanished is a hard failure: a partial set is
    // worse than no transfer.
    let mut total_bytes = 0u64;
    let mut sizes: Vec<u64> = Vec::new();
    for s in &req.sources {
        match std::fs::metadata(long_path(s)) {
            Ok(m) => {
                total_bytes += m.len();
                sizes.push(m.len());
            }
            Err(e) => {
                problems.push(Problem {
                    code: "source_unreadable".into(),
                    message: format!("{}: {e}", s.display()),
                });
                sizes.push(0);
            }
        }
    }

    if req.sources.is_empty() {
        problems.push(Problem {
            code: "no_sources".into(),
            message: "nothing selected to transfer".into(),
        });
    }

    // Destination volume facts.
    let dst_vol: Option<VolumeInfo> =
        volume::guid_for_path(&req.dst_dir).and_then(|g| volume::describe(&g));
    let src_vol: Option<VolumeInfo> = req
        .sources
        .first()
        .and_then(|s| volume::guid_for_path(s))
        .and_then(|g| volume::describe(&g));

    // Same-volume is decided by GUID, never by drive letter: letters get reassigned,
    // and a wrong answer here turns an instant rename into a full copy, or worse.
    let same_volume = match (&src_vol, &dst_vol) {
        (Some(a), Some(b)) => a.volume_guid == b.volume_guid,
        _ => false,
    };

    let dst_free = dst_vol.as_ref().map(|v| v.free_bytes).unwrap_or(0);

    if let Some(v) = &dst_vol {
        // Headroom: 2% plus 256 MB, so the destination is not driven to zero.
        let needed = total_bytes + total_bytes / 50 + 256 * 1024 * 1024;
        if !same_volume && dst_free < needed {
            problems.push(Problem {
                code: "insufficient_space".into(),
                message: format!(
                    "needs {} including headroom, {} free — short by {}",
                    human(needed),
                    human(dst_free),
                    human(needed.saturating_sub(dst_free))
                ),
            });
        }

        // FAT32 cannot hold a file of 4 GiB or more. The user's RAR volumes are 8.5 GB
        // each, so this would otherwise fail several gigabytes into the copy.
        if let Some(max) = v.max_file_bytes {
            for (s, size) in req.sources.iter().zip(&sizes) {
                if *size > max {
                    problems.push(Problem {
                        code: "file_too_large_for_filesystem".into(),
                        message: format!(
                            "{} is {} but {} cannot hold a file over {}",
                            s.file_name().unwrap_or_default().to_string_lossy(),
                            human(*size),
                            v.filesystem,
                            human(max)
                        ),
                    });
                }
            }
        }

        if v.is_unjournaled {
            warnings.push(Warning {
                code: "unjournaled_destination".into(),
                message: format!(
                    "{} has no journalling — do not unplug the drive during the transfer",
                    v.filesystem
                ),
            });
        }
    } else {
        problems.push(Problem {
            code: "destination_unknown".into(),
            message: format!("could not identify the volume for {}", req.dst_dir.display()),
        });
    }

    // Build the file plan and apply the collision policy.
    let mut files = Vec::new();
    for (src, size) in req.sources.iter().zip(&sizes) {
        // The path relative to the item root is what gets recreated at the destination.
        // Using only the filename would flatten a nested game — an Xbox title is
        // `<id>/<type>/<container>.data/Data0000..Data0047`, and dropping the structure
        // dumps 48 identically-shaped files into one folder and yields an unusable game.
        let rel = req
            .source_root
            .as_ref()
            .and_then(|root| src.strip_prefix(root).ok())
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(src.file_name().unwrap_or_else(|| std::ffi::OsStr::new("unnamed")))
            });

        let name = rel
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "unnamed".into());

        if name.chars().count() > 255 {
            problems.push(Problem {
                code: "name_too_long".into(),
                message: format!("'{name}' exceeds the 255-character filename limit"),
            });
        }

        let target = dst_root.join(&rel);
        let exists = target.exists();
        let (dst, skipped) = match (exists, req.collision) {
            (false, _) => (target, false),
            (true, CollisionPolicy::Skip) => (target, true),
            (true, CollisionPolicy::Overwrite) => (target, false),
            (true, CollisionPolicy::Rename) => {
                // Dedupe within the file's own destination directory, so a renamed
                // copy stays beside its siblings rather than jumping to the root.
                let dir = target.parent().map(PathBuf::from).unwrap_or_else(|| dst_root.clone());
                (dedupe_name(&dir, &name), false)
            }
        };

        if exists {
            warnings.push(Warning {
                code: "collision".into(),
                message: match req.collision {
                    CollisionPolicy::Skip => format!("{name} already exists — skipping"),
                    CollisionPolicy::Overwrite => format!("{name} already exists — will be replaced"),
                    CollisionPolicy::Rename => format!(
                        "{name} already exists — writing as {}",
                        dst.file_name().unwrap_or_default().to_string_lossy()
                    ),
                },
            });
        }

        if longest_component(&dst) > 255 {
            problems.push(Problem {
                code: "path_component_too_long".into(),
                message: format!("a folder name in {} exceeds 255 characters", dst.display()),
            });
        }

        files.push(PlannedFile { src: src.clone(), dst, rel, size_bytes: *size, skipped });
    }

    // Moving into a subdirectory of the source would recurse forever.
    if let Some(first) = req.sources.first() {
        if let Some(parent) = first.parent() {
            if dst_root.starts_with(parent) && dst_root != parent && req.folder_name.is_none() {
                warnings.push(Warning {
                    code: "nested_destination".into(),
                    message: "the destination is inside the source folder".into(),
                });
            }
            if dst_root == parent {
                problems.push(Problem {
                    code: "same_location".into(),
                    message: "source and destination are the same folder".into(),
                });
            }
        }
    }

    if req.kind == TransferKind::Move && req.safe_mode {
        warnings.push(Warning {
            code: "safe_mode".into(),
            message: "Safe mode is on: this will copy, and the source will be kept".into(),
        });
    }

    let moving_bytes: u64 = files.iter().filter(|f| !f.skipped).map(|f| f.size_bytes).sum();
    let estimated_secs = if same_volume && req.kind == TransferKind::Move {
        0 // A same-volume move is a metadata rename.
    } else {
        moving_bytes / ASSUMED_BYTES_PER_SEC
    };

    Preflight {
        total_bytes,
        file_count: files.len(),
        dst_free_bytes: dst_free,
        same_volume,
        ok: problems.is_empty(),
        problems,
        warnings,
        estimated_secs,
        // Hash verification re-reads the destination, so it costs roughly another pass.
        hash_verify_extra_secs: moving_bytes / ASSUMED_BYTES_PER_SEC,
        files,
    }
}

fn human(b: u64) -> String {
    const U: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = b as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 { format!("{b} B") } else { format!("{v:.1} {}", U[i]) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmp(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("gv_plan_{tag}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    fn req(sources: Vec<PathBuf>, dst: PathBuf) -> TransferRequest {
        TransferRequest {
            kind: TransferKind::Copy,
            sources,
            source_root: None,
            dst_dir: dst,
            folder_name: None,
            remove_empty_source_dir: false,
            verify: VerifyMode::Size,
            collision: CollisionPolicy::Rename,
            safe_mode: true,
        }
    }

    #[test]
    fn long_path_prefix_is_added_once_and_only_to_absolute_paths() {
        assert_eq!(
            long_path(Path::new("C:\\a\\b")),
            PathBuf::from("\\\\?\\C:\\a\\b")
        );
        // Already prefixed stays as it is.
        assert_eq!(
            long_path(Path::new("\\\\?\\C:\\a")),
            PathBuf::from("\\\\?\\C:\\a")
        );
        // UNC paths are left alone.
        assert_eq!(
            long_path(Path::new("\\\\server\\share")),
            PathBuf::from("\\\\server\\share")
        );
    }

    /// The default must never destroy an existing file.
    #[test]
    fn a_collision_renames_rather_than_overwriting() {
        let dir = tmp("dedupe");
        fs::write(dir.join("Game.rar"), b"old").unwrap();
        let picked = dedupe_name(&dir, "Game.rar");
        assert_eq!(picked.file_name().unwrap(), "Game (2).rar");
        // The original is untouched.
        assert_eq!(fs::read(dir.join("Game.rar")).unwrap(), b"old");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn dedupe_keeps_counting_past_the_second_copy() {
        let dir = tmp("dedupe2");
        fs::write(dir.join("a.rar"), b"x").unwrap();
        fs::write(dir.join("a (2).rar"), b"x").unwrap();
        assert_eq!(dedupe_name(&dir, "a.rar").file_name().unwrap(), "a (3).rar");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_source_is_a_blocking_problem() {
        let dir = tmp("missing");
        let p = preflight(&req(vec![dir.join("nope.rar")], dir.clone()));
        assert!(!p.ok);
        assert!(p.problems.iter().any(|x| x.code == "source_unreadable"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn copying_into_the_source_folder_is_refused() {
        let dir = tmp("same");
        let f = dir.join("a.rar");
        fs::write(&f, b"data").unwrap();
        let p = preflight(&req(vec![f], dir.clone()));
        assert!(!p.ok);
        assert!(p.problems.iter().any(|x| x.code == "same_location"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_empty_selection_is_refused() {
        let dir = tmp("empty");
        let p = preflight(&req(vec![], dir.clone()));
        assert!(!p.ok);
        assert!(p.problems.iter().any(|x| x.code == "no_sources"));
        let _ = fs::remove_dir_all(&dir);
    }

    /// Safe mode has to be visible in the plan, or a user who expects a move will be
    /// surprised to find the source still there.
    #[test]
    fn safe_mode_on_a_move_is_reported_as_a_warning() {
        let dir = tmp("safemode");
        let src = dir.join("src");
        let dst = dir.join("dst");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&dst).unwrap();
        let f = src.join("a.rar");
        fs::write(&f, b"data").unwrap();

        let mut r = req(vec![f], dst);
        r.kind = TransferKind::Move;
        r.safe_mode = true;
        let p = preflight(&r);
        assert!(p.warnings.iter().any(|w| w.code == "safe_mode"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn hash_verification_cost_is_stated_up_front() {
        let dir = tmp("estimate");
        let src = dir.join("src");
        let dst = dir.join("dst");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&dst).unwrap();
        let f = src.join("big.rar");
        fs::write(&f, vec![0u8; 4096]).unwrap();

        let p = preflight(&req(vec![f], dst));
        // Both estimates exist so the UI can show "+N s to verify" before opting in.
        assert_eq!(p.file_count, 1);
        assert!(p.hash_verify_extra_secs == p.estimated_secs);
        let _ = fs::remove_dir_all(&dir);
    }

    /// Regression: destinations were built from the bare filename, which flattened a
    /// nested game. An Xbox 360 title keeps its data in
    /// `<id>/<type>/<container>.data/Data0000…`, so flattening dropped 48 files into one
    /// folder and produced an unusable game. It also silently broke every installed PC
    /// game transfer.
    #[test]
    fn a_nested_game_keeps_its_structure_at_the_destination() {
        let dir = tmp("nested");
        let src = dir.join("ACE COMBAT 6");
        let deep = src.join("4E4D07D1").join("00007000").join("container.data");
        fs::create_dir_all(&deep).unwrap();
        fs::write(src.join("desktop.ini"), b"x").unwrap();
        fs::write(deep.join("Data0000"), vec![0u8; 512]).unwrap();
        fs::write(deep.join("Data0001"), vec![0u8; 512]).unwrap();
        let dst = dir.join("dst");
        fs::create_dir_all(&dst).unwrap();

        let mut r = req(
            vec![
                src.join("desktop.ini"),
                deep.join("Data0000"),
                deep.join("Data0001"),
            ],
            dst.clone(),
        );
        r.source_root = Some(src.clone());
        r.folder_name = Some("ACE COMBAT 6".into());
        let p = preflight(&r);

        let dests: Vec<String> = p
            .files
            .iter()
            .map(|f| {
                f.dst
                    .strip_prefix(&dst)
                    .unwrap_or(&f.dst)
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect();
        assert!(
            dests.contains(&"ACE COMBAT 6/4E4D07D1/00007000/container.data/Data0000".to_string()),
            "structure was flattened: {dests:?}"
        );
        assert!(dests.contains(&"ACE COMBAT 6/desktop.ini".to_string()), "{dests:?}");
        // The two Data files must not collide into one destination.
        let mut uniq = dests.clone();
        uniq.sort();
        uniq.dedup();
        assert_eq!(uniq.len(), dests.len(), "destinations collided: {dests:?}");
        let _ = fs::remove_dir_all(&dir);
    }

    /// Loose sibling files have no structure to preserve and must land side by side.
    #[test]
    fn loose_files_land_flat_in_the_destination() {
        let dir = tmp("loose");
        let src = dir.join("src");
        let dst = dir.join("dst");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&dst).unwrap();
        let a = src.join("Game.part1.rar");
        let b = src.join("Game.part2.rar");
        fs::write(&a, vec![0u8; 16]).unwrap();
        fs::write(&b, vec![0u8; 16]).unwrap();

        let mut r = req(vec![a, b], dst.clone());
        r.source_root = Some(src.clone());
        let p = preflight(&r);
        for f in &p.files {
            assert_eq!(f.dst.parent(), Some(dst.as_path()), "{:?}", f.dst);
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn every_volume_of_a_set_is_planned_as_one_unit() {
        let dir = tmp("set");
        let src = dir.join("src");
        let dst = dir.join("dst");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&dst).unwrap();
        let mut sources = Vec::new();
        for n in 1..=5 {
            let f = src.join(format!("Game.part{n}.rar"));
            fs::write(&f, vec![0u8; 1024]).unwrap();
            sources.push(f);
        }
        let p = preflight(&req(sources, dst));
        assert_eq!(p.file_count, 5, "all volumes must travel together");
        assert_eq!(p.total_bytes, 5 * 1024);
        let _ = fs::remove_dir_all(&dir);
    }
}
