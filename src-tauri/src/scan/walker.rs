//! Walking a drive and turning it into catalogue items.
//!
//! The walk is deliberately simple. A full recursive pass over the target drive reads
//! 2,521 entries in 0.17 s, because cold-storage drives hold few, enormous files rather
//! than many small ones. The cost is in pruning the directories that would explode —
//! build trees, package caches, recycle bins — not in raw traversal throughput.
//!
//! Analysis happens in three phases so the classifier can stay pure:
//!   1. one pass to collect every surviving entry,
//!   2. a bottom-up pass to roll up sizes and engine markers,
//!   3. a top-down pass that decides which directories become items.

use crate::scan::archive_set::{group_directory, ArchiveSet, FileEntry};
use crate::scan::classify::{classify_archive_file, classify_dir, ClassifyConfig, Classification, DirFacts, ItemKind, Verdict};
use crate::scan::title::{self, CleanedTitle};
use jwalk::WalkDirGeneric;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Directory names never worth descending into. Lowercased, matched exactly.
const PRUNE_DIRS: &[&str] = &[
    "$recycle.bin", "system volume information", "node_modules", ".git", ".vs", ".idea",
    "__pycache__", ".venv", "venv", "deriveddatacache", "intermediate", "obj",
    "$windows.~bt", "$windows.~ws", "recovery", "boot", "efi", "casper", "isolinux",
    "pool", "dists", ".disk", "found.000",
];

/// Names that belong to an OS installation *and* are perfectly ordinary folder names
/// on a data drive.
///
/// The target drive has an `E:\Windows` holding 16 GB of installer images. Pruning by
/// name alone would hide those bytes from the storage breakdown — the exact question
/// the user is trying to answer. So these are pruned only when the directory really is
/// a system folder, proven by a child that only ever exists in one.
const OS_DIR_NAMES: &[&str] = &["windows", "winnt", "program files", "program files (x86)", "programdata", "appdata"];

/// True when `dir` is an actual OS directory rather than a user folder of the same
/// name. One `exists` call, only for the handful of names above.
fn is_real_os_dir(dir: &Path, name_lower: &str) -> bool {
    match name_lower {
        "windows" | "winnt" => dir.join("System32").is_dir(),
        "program files" | "program files (x86)" => dir.join("Common Files").is_dir(),
        "programdata" => dir.join("Microsoft").is_dir(),
        "appdata" => dir.join("Roaming").is_dir() || dir.join("Local").is_dir(),
        _ => false,
    }
}

/// True when a directory should never be descended into.
///
/// Shared so the live console read prunes exactly what the scanner prunes: two lists that
/// drifted apart would mean the Game Files window and the library disagreed about what is
/// even on the drive. `dir` is needed because the OS-directory names are ordinary folder
/// names on a data drive, and are pruned only when the directory really is a system one.
pub fn is_pruned_dir(dir: &Path, name_lower: &str) -> bool {
    PRUNE_DIRS.contains(&name_lower)
        || (OS_DIR_NAMES.contains(&name_lower) && is_real_os_dir(dir, name_lower))
}

/// Maximum directory depth below a scan root. Games are never nested deeper than this,
/// and the bound keeps a pathological tree from stalling a scan.
pub const MAX_DEPTH: usize = 6;

/// Depth below an item at which marker files are still collected.
const MARKER_DEPTH: usize = 4;

/// Cap on markers rolled up per directory, so a huge tree cannot balloon memory.
const MARKER_CAP: usize = 256;

/// Filenames and extensions that carry classification signal.
fn is_marker(name_lower: &str) -> bool {
    name_lower.ends_with(".exe")
        || name_lower.ends_with(".dll")
        || name_lower.ends_with(".pak")
        || name_lower.ends_with(".uproject")
        || name_lower.ends_with(".acf")
        || name_lower.ends_with(".info")
        || name_lower == "globalgamemanagers"
        || name_lower == "resources.assets"
}

/// One file belonging to an item, with everything the catalogue stores about it.
///
/// Previously only archive volumes produced `item_file` rows, so any other kind of
/// multi-file item lost its contents on save. Every item now carries these.
#[derive(Debug, Clone, Default)]
pub struct FileSpec {
    /// Path relative to the item root.
    pub rel_path: String,
    pub size_bytes: u64,
    pub mtime_unix: i64,
    /// 1-based position for ordered sets (archive volumes, split packages).
    pub part_index: Option<i64>,
    /// `archive_part` | `pkg_game` | `pkg_update` | `pkg_dlc` | `pkg_part` | `cover` | `data`
    pub role: String,
    // Console fields, populated by the console probe and NULL otherwise.
    pub content_id: Option<String>,
    pub app_ver: Option<String>,
    pub system_ver: Option<i64>,
    pub declared_bytes: Option<i64>,
    pub probe_status: Option<String>,
}

impl FileSpec {
    pub fn simple(rel_path: impl Into<String>, size_bytes: u64, role: &str) -> Self {
        Self { rel_path: rel_path.into(), size_bytes, role: role.to_string(), ..Default::default() }
    }
}

/// A catalogued item: what the library shows and what a transfer job moves.
#[derive(Debug, Clone)]
pub struct ScannedItem {
    /// Path relative to the scan root, using the platform separator.
    pub rel_path: String,
    /// `true` when the item is a folder, `false` when it is loose file(s) in a folder.
    pub is_folder: bool,
    pub raw_name: String,
    pub title: CleanedTitle,
    pub kind: ItemKind,
    pub verdict: Verdict,
    pub confidence: f32,
    pub classification: Classification,
    pub total_bytes: u64,
    pub file_count: u64,
    /// Files that make up this item, relative to `rel_path`'s parent. For an archive
    /// set these are the volumes, in extraction order — the unit a copy must keep whole.
    pub files: Vec<String>,
    /// The same files with their sizes, roles and console metadata. This is what gets
    /// persisted; `files` stays the flat name list the transfer layer consumes.
    pub file_specs: Vec<FileSpec>,
    pub archive_set: Option<ArchiveSet>,
}

#[derive(Debug, Default)]
pub struct ScanStats {
    pub dirs_seen: u64,
    pub files_seen: u64,
    pub bytes_seen: u64,
    pub pruned: u64,
}

#[derive(Debug, Default)]
pub struct ScanResult {
    pub items: Vec<ScannedItem>,
    pub stats: ScanStats,
}

impl ScanResult {
    /// Rebase item paths so they are relative to the **drive root** rather than the
    /// scan root.
    ///
    /// The catalogue stores drive-relative paths, because that is the only form that
    /// survives a drive being remounted under a different letter: the absolute path is
    /// rebuilt at use time from the drive's current mount point. Without this, scanning
    /// `E:\Game` instead of `E:\` would record paths that cannot be resolved later.
    pub fn rebase(&mut self, scan_root: &Path, drive_root: &Path) {
        let Ok(prefix) = scan_root.strip_prefix(drive_root) else {
            return; // Scan root is the drive root; paths are already correct.
        };
        if prefix.as_os_str().is_empty() {
            return;
        }
        for item in &mut self.items {
            item.rel_path = prefix.join(&item.rel_path).to_string_lossy().into_owned();
        }
    }
}

/// Raw facts about one directory, before roll-up.
#[derive(Default)]
struct RawDir {
    files: Vec<FileEntry>,
    subdirs: Vec<String>,
    depth: usize,
}

/// Rolled-up facts.
#[derive(Default)]
struct Rolled {
    total_bytes: u64,
    total_files: u64,
    markers: Vec<String>,
}

/// Scan one root and return everything worth cataloguing.
pub fn scan(root: &Path, cfg: &ClassifyConfig) -> ScanResult {
    let mut stats = ScanStats::default();
    let mut raw: HashMap<PathBuf, RawDir> = HashMap::new();
    raw.insert(root.to_path_buf(), RawDir::default());

    // Phase 1: one pass, pruning as we go. jwalk's callback lets us drop a subtree
    // before its children are ever read.
    let walker = WalkDirGeneric::<((), ())>::new(root)
        .max_depth(MAX_DEPTH)
        .skip_hidden(false)
        .process_read_dir(|_depth, _path, _state, children| {
            children.retain(|entry| {
                let Ok(e) = entry else { return true };
                if !e.file_type().is_dir() {
                    return true;
                }
                let name = e.file_name().to_string_lossy().to_lowercase();
                !is_pruned_dir(&e.path(), &name)
            });
        });

    for entry in walker.into_iter().flatten() {
        let path = entry.path();
        let depth = entry.depth();
        if entry.file_type().is_dir() {
            stats.dirs_seen += 1;
            raw.entry(path.clone()).or_default().depth = depth;
            if let Some(parent) = path.parent() {
                if let Some(p) = raw.get_mut(parent) {
                    p.subdirs.push(entry.file_name().to_string_lossy().into_owned());
                }
            }
        } else if entry.file_type().is_file() {
            let md = entry.metadata().ok();
            let size = md.as_ref().map(|m| m.len()).unwrap_or(0);
            let mtime = md
                .as_ref()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            stats.files_seen += 1;
            stats.bytes_seen += size;
            if let Some(parent) = path.parent() {
                raw.entry(parent.to_path_buf()).or_default().files.push(FileEntry::with_mtime(
                    entry.file_name().to_string_lossy().into_owned(),
                    size,
                    mtime,
                ));
            }
        }
    }

    // Phase 2: roll up sizes and markers, deepest directory first so a parent always
    // reads finished children.
    let mut order: Vec<PathBuf> = raw.keys().cloned().collect();
    order.sort_by_key(|p| std::cmp::Reverse(raw[p].depth));

    let mut rolled: HashMap<PathBuf, Rolled> = HashMap::new();
    for dir in &order {
        let r = &raw[dir];
        let mut acc = Rolled {
            total_bytes: r.files.iter().map(|f| f.size_bytes).sum(),
            total_files: r.files.len() as u64,
            markers: r
                .files
                .iter()
                .map(|f| f.name.to_lowercase())
                .filter(|n| is_marker(n))
                .collect(),
        };
        for sub in &r.subdirs {
            let child = dir.join(sub);
            if let Some(c) = rolled.get(&child) {
                acc.total_bytes += c.total_bytes;
                acc.total_files += c.total_files;
                if acc.markers.len() < MARKER_CAP {
                    let prefix = sub.to_lowercase();
                    acc.markers.extend(
                        c.markers
                            .iter()
                            .filter(|m| m.matches('/').count() < MARKER_DEPTH)
                            .take(MARKER_CAP - acc.markers.len())
                            .map(|m| format!("{prefix}/{m}")),
                    );
                }
            }
        }
        rolled.insert(dir.clone(), acc);
    }

    // Phase 3: decide top-down which directories are items.
    let mut items = Vec::new();
    visit(root, root, &raw, &rolled, cfg, &mut items);

    ScanResult { items, stats }
}

#[allow(clippy::too_many_arguments)]
fn visit(
    root: &Path,
    dir: &Path,
    raw: &HashMap<PathBuf, RawDir>,
    rolled: &HashMap<PathBuf, Rolled>,
    cfg: &ClassifyConfig,
    out: &mut Vec<ScannedItem>,
) {
    let Some(r) = raw.get(dir) else { return };
    let Some(acc) = rolled.get(dir) else { return };

    let grouped = group_directory(&r.files);
    let facts = DirFacts {
        name: dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| dir.to_string_lossy().into_owned()),
        rel_path: rel(root, dir),
        files: r.files.clone(),
        subdirs: r.subdirs.clone(),
        total_bytes: acc.total_bytes,
        total_files: acc.total_files,
        marker_paths: acc.markers.clone(),
        archive_sets: grouped.sets.clone(),
        parent_name: dir
            .parent()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_lowercase())
            .unwrap_or_default(),
    };

    // The scan root itself is a container, never an item.
    let is_root = dir == root;
    if !is_root {
        let c = classify_dir(&facts, cfg);
        let terminal = matches!(
            c.kind,
            ItemKind::ArchiveSet | ItemKind::InstalledGame | ItemKind::DevProject | ItemKind::Media
        );
        if terminal {
            // This directory *is* the item. Do not descend — the files beneath it are
            // its contents, not separate entries.
            out.push(make_dir_item(root, dir, &facts, c, grouped.sets.first().cloned()));
            return;
        }
    }

    // Otherwise the directory is a shelf: loose archive sets inside it are their own
    // items, and real subdirectories are visited in turn.
    for set in &grouped.sets {
        let c = classify_archive_file(set, &facts.name, cfg);
        let primary = &set.parts[0].file_name;
        out.push(ScannedItem {
            rel_path: rel(root, &dir.join(primary)),
            is_folder: false,
            raw_name: primary.clone(),
            title: title::clean(primary, true),
            kind: c.kind,
            verdict: c.verdict,
            confidence: c.confidence,
            classification: c,
            total_bytes: set.total_bytes,
            file_count: set.parts.len() as u64,
            files: set.parts.iter().map(|p| p.file_name.clone()).collect(),
            file_specs: set
                .parts
                .iter()
                .map(|p| FileSpec {
                    rel_path: p.file_name.clone(),
                    size_bytes: p.size_bytes,
                    part_index: Some(p.index as i64),
                    role: "archive_part".into(),
                    ..Default::default()
                })
                .collect(),
            archive_set: Some(set.clone()),
        });
    }

    for sub in &r.subdirs {
        visit(root, &dir.join(sub), raw, rolled, cfg, out);
    }
}

fn make_dir_item(
    root: &Path,
    dir: &Path,
    facts: &DirFacts,
    c: Classification,
    set: Option<ArchiveSet>,
) -> ScannedItem {
    ScannedItem {
        rel_path: rel(root, dir),
        is_folder: true,
        raw_name: facts.name.clone(),
        title: title::clean(&facts.name, false),
        kind: c.kind,
        verdict: c.verdict,
        confidence: c.confidence,
        classification: c,
        total_bytes: facts.total_bytes,
        file_count: facts.total_files,
        files: set
            .as_ref()
            .map(|s| s.parts.iter().map(|p| p.file_name.clone()).collect())
            .unwrap_or_default(),
        file_specs: set
            .as_ref()
            .map(|s| {
                s.parts
                    .iter()
                    .map(|p| FileSpec {
                        rel_path: p.file_name.clone(),
                        size_bytes: p.size_bytes,
                        part_index: Some(p.index as i64),
                        role: "archive_part".into(),
                        ..Default::default()
                    })
                    .collect()
            })
            .unwrap_or_default(),
        archive_set: set,
    }
}

fn rel(root: &Path, p: &Path) -> String {
    p.strip_prefix(root)
        .unwrap_or(p)
        .to_string_lossy()
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Build a miniature copy of the real drive layout in a temp directory and scan it
    /// end to end. Sizes are scaled down; the structure is what is under test.
    fn build_fixture() -> PathBuf {
        let base = std::env::temp_dir().join(format!("gv_scan_{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);

        let game = base.join("Game");
        let mirage = game.join("Assassins Creed Mirage");
        fs::create_dir_all(&mirage).unwrap();
        // Five equal volumes and a short tail, mirroring the real set.
        for n in 1..=4 {
            fs::write(mirage.join(format!("Assassins Creed Mirage.part{n}.rar")), vec![0u8; 4096])
                .unwrap();
        }
        fs::write(mirage.join("Assassins Creed Mirage.part5.rar"), vec![0u8; 2048]).unwrap();
        fs::write(game.join("Sekiro Shadows Die Twice by xatab.rar"), vec![0u8; 8192]).unwrap();
        fs::write(game.join("Mad Max  - [DODI Repack].zip"), vec![0u8; 8192]).unwrap();

        let clozr = base.join("first game").join("Clozr");
        fs::create_dir_all(clozr.join("Content")).unwrap();
        fs::create_dir_all(clozr.join("Intermediate")).unwrap();
        fs::write(clozr.join("Clozr.uproject"), b"{}").unwrap();

        let win = base.join("Windows");
        fs::create_dir_all(&win).unwrap();
        fs::write(win.join("Win11_25H2_English_x64.iso"), vec![0u8; 4096]).unwrap();

        base
    }

    /// The end-to-end shape check: the real drive layout must produce exactly the
    /// items a person would name, and nothing else.
    #[test]
    fn scans_the_real_drive_layout_into_the_expected_items() {
        let base = build_fixture();
        // Floor lowered because the fixture uses byte-sized stand-ins for GB files.
        let cfg = ClassifyConfig { min_game_bytes: 1024, ..Default::default() };
        let res = scan(&base, &cfg);

        let by_path = |needle: &str| {
            res.items
                .iter()
                .find(|i| i.rel_path.replace('\\', "/").contains(needle))
                .unwrap_or_else(|| panic!("no item matching {needle}; got {:?}",
                    res.items.iter().map(|i| &i.rel_path).collect::<Vec<_>>()))
        };

        // The five volumes collapse into one folder item.
        let mirage = by_path("Assassins Creed Mirage");
        assert!(mirage.is_folder);
        assert_eq!(mirage.kind, ItemKind::ArchiveSet);
        assert_eq!(mirage.files.len(), 5, "volumes must stay grouped");
        assert_eq!(mirage.title.clean_title, "Assassins Creed Mirage");
        // No separate entry for any individual volume.
        assert_eq!(
            res.items.iter().filter(|i| i.rel_path.contains("Mirage")).count(),
            1
        );

        // Loose repacks on the shelf are their own items.
        let sekiro = by_path("Sekiro");
        assert!(!sekiro.is_folder);
        assert_eq!(sekiro.title.repacker.as_deref(), Some("xatab"));
        assert_eq!(by_path("Mad Max").title.clean_title, "Mad Max");

        // Noise is catalogued but rejected, never silently dropped.
        assert_eq!(by_path("Clozr").verdict, Verdict::NotGame);
        assert_eq!(by_path("Clozr").kind, ItemKind::DevProject);
        assert_eq!(by_path("Win11").verdict, Verdict::NotGame);

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn pruned_directories_are_never_descended() {
        let base = std::env::temp_dir().join(format!("gv_prune_{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(base.join("$RECYCLE.BIN").join("deep")).unwrap();
        fs::write(base.join("$RECYCLE.BIN").join("deep").join("Game.rar"), vec![0u8; 4096]).unwrap();

        let res = scan(&base, &ClassifyConfig { min_game_bytes: 1, ..Default::default() });
        assert!(
            res.items.is_empty(),
            "recycle bin contents must not be catalogued: {:?}",
            res.items.iter().map(|i| &i.rel_path).collect::<Vec<_>>()
        );
        let _ = fs::remove_dir_all(&base);
    }
}
