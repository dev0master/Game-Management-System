//! Reading a folder of console games live, for the Game Files window.
//!
//! This is the one screen that does not serve from the catalogue. The user asked for the
//! folder to be read when the window opens, so a package added five minutes ago shows up
//! without a scan, and one deleted is gone. The cost of that is paid down by
//! `console_probe`: a package whose size and mtime have not changed is never reopened, so
//! only the first read of a drive is slow.
//!
//! Nothing here writes to the scanned drive. The only writes are cover images, into the
//! local cache under `%LOCALAPPDATA%`, because the asset protocol will not serve an image
//! from anywhere else and artwork must survive the drive being unplugged.

use super::{
    cache_cover_file, cache_group_cover, cover_key_of, generic, group, list_dir, probe_listed,
    ps3, CoverSource, ProbeRow,
};
use crate::scan::console::ps4;
use crate::scan::walker;
use std::collections::HashMap;
use std::path::Path;

/// One file inside a game, as the flat view lists it.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ConsoleFileRow {
    pub name: String,
    pub size_bytes: u64,
    /// `pkg_game` · `pkg_update` · `pkg_dlc` · `pkg_part` · `cover`
    pub role: String,
    pub part_index: Option<u32>,
    pub app_ver: Option<String>,
    pub content_id: Option<String>,
    pub probe_status: String,
}

/// How many files of one role a game has, for the contents chips.
#[derive(Debug, Clone, serde::Serialize)]
pub struct RoleCount {
    pub role: String,
    pub count: usize,
}

/// One game as the window shows it.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ConsoleGameRow {
    pub platform: String,
    /// Absent for a disc image, whose id lives inside a filesystem nobody has walked yet.
    pub title_id: Option<String>,
    /// The game's name: read from inside the file when that was possible, otherwise taken
    /// from the filename.
    pub display_title: String,
    /// True when `display_title` and `title_id` were read from inside the file. False means
    /// the filename supplied the name, which is worth saying rather than implying.
    pub title_from_file: bool,
    /// True when the platform is an inference from the folder name, or not known at all.
    pub platform_guessed: bool,
    /// Directory holding the files, relative to the root that was read.
    pub rel_dir: String,
    /// The package that defines this game's identity, relative to the root.
    pub rel_path: String,
    pub total_bytes: u64,
    pub min_system_ver: Option<u32>,
    /// `min_system_ver` formatted. Done here because the packing is BCD and duplicating
    /// that in JavaScript would be a bug waiting to happen.
    pub firmware_label: Option<String>,
    /// Absolute path to the cached cover, ready for `convertFileSrc`.
    pub cover_path: Option<String>,
    /// `pkg_icon0` when the art came out of the game file itself, `pkg_sibling` when it
    /// came from an image beside it.
    pub cover_source: Option<String>,
    /// For an add-on with no base game of its own: the game it belongs to.
    pub base_title_id: Option<String>,
    pub link_reason: Option<String>,
    pub is_orphan: bool,
    pub roles: Vec<RoleCount>,
    pub files: Vec<ConsoleFileRow>,
}

/// A file that could not be read, or that no game claimed.
#[derive(Debug, Clone, serde::Serialize)]
pub struct StrayFileRow {
    pub name: String,
    pub rel_dir: String,
    pub size_bytes: u64,
    pub detail: String,
}

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct ReadStats {
    pub dirs_read: u64,
    /// Packages opened this time.
    pub probed: u64,
    /// Packages served from the probe cache without being opened.
    pub from_cache: u64,
    pub covers_embedded: u64,
    pub covers_sibling: u64,
    pub elapsed_ms: u64,
}

/// Progress while a folder is being read, for the window's status line.
///
/// Emitted after each directory rather than each file: the first read of a drive takes
/// seconds and the user needs to see it moving, but a per-file event on 71 packages would
/// be noise.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ConsoleProgress {
    pub dirs_read: u64,
    pub games: usize,
    pub probed: u64,
    pub from_cache: u64,
    /// The directory just read, relative to the root.
    pub current: String,
    pub finished: bool,
}

/// Everything one read of a folder produced.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct ConsoleRead {
    pub root: String,
    pub games: Vec<ConsoleGameRow>,
    /// Damaged or orphaned files. Surfaced, never dropped.
    pub damaged: Vec<StrayFileRow>,
    pub stats: ReadStats,
    /// Fresh probes for the caller to store. Not sent to the UI.
    #[serde(skip)]
    pub fresh_probes: Vec<(String, ProbeRow)>,
}

/// Read a folder tree and return the console games in it.
///
/// `cached` is keyed by the file's path relative to `root`. `cover_dir` receives any
/// artwork extracted from inside a package.
pub fn read_tree(
    root: &Path,
    cached: &HashMap<String, ProbeRow>,
    cover_dir: &Path,
    on_progress: &mut dyn FnMut(ConsoleProgress),
) -> ConsoleRead {
    let started = std::time::Instant::now();
    let mut out = ConsoleRead { root: root.to_string_lossy().into_owned(), ..Default::default() };
    visit(root, root, 0, cached, cover_dir, &mut out, on_progress);
    out.games.sort_by(|a, b| {
        b.total_bytes
            .cmp(&a.total_bytes)
            .then_with(|| a.display_title.cmp(&b.display_title))
    });
    out.stats.elapsed_ms = started.elapsed().as_millis() as u64;
    on_progress(ConsoleProgress {
        dirs_read: out.stats.dirs_read,
        games: out.games.len(),
        probed: out.stats.probed,
        from_cache: out.stats.from_cache,
        current: String::new(),
        finished: true,
    });
    out
}

#[allow(clippy::too_many_arguments)]
fn visit(
    dir: &Path,
    root: &Path,
    depth: usize,
    cached: &HashMap<String, ProbeRow>,
    cover_dir: &Path,
    out: &mut ConsoleRead,
    on_progress: &mut dyn FnMut(ConsoleProgress),
) {
    let Ok((files, subdirs)) = list_dir(dir) else { return };
    out.stats.dirs_read += 1;

    let rel_dir = rel(root, dir);
    let hint = generic::folder_hint(&rel_dir);

    // A PS3 game is a folder, and `PS3_GAME\USRDIR` below it holds hundreds of files that
    // are parts of this one game rather than games of their own. So emit it and stop: the
    // filesystem already did the grouping, and `group.rs` says to respect that.
    if let Some(game_dir) = ps3::ps3_game_dir(dir) {
        let row = ps3_row(dir, &game_dir, &rel_dir, cover_dir, &mut out.stats);
        out.games.push(row);
        emit(out, &rel_dir, on_progress);
        return;
    }

    // An Xbox game is likewise a folder, named by the folder and booted by `default.xbe`.
    // Same reasoning: the filesystem did the grouping, so take the folder whole.
    if let Some((boot, platform)) = generic::boot_executable(&files) {
        let row = folder_game_row(dir, &rel_dir, &boot, platform, &files, cover_dir, &mut out.stats);
        out.games.push(row);
        emit(out, &rel_dir, on_progress);
        return;
    }

    // The cache is keyed on the path relative to the root; the probe layer wants it keyed
    // on filename within one directory. Narrow it here so a filename in two folders can
    // never collide.
    let mut local: HashMap<String, ProbeRow> = HashMap::new();
    for f in &files {
        let key = join_rel(&rel_dir, &f.name);
        if let Some(row) = cached.get(&key) {
            local.insert(f.name.clone(), row.clone());
        }
    }

    let mut fresh = Vec::new();
    let (probed, cover_images) = probe_listed(dir, &files, &local, &mut fresh);

    if !probed.is_empty() {
        out.stats.probed += fresh.len() as u64;
        out.stats.from_cache += (probed.len() - fresh.len()) as u64;
        for row in fresh {
            out.fresh_probes.push((join_rel(&rel_dir, &row.name), row));
        }

        let result = group::group_ps4(&probed, &cover_images);
        for g in &result.groups {
            out.games.push(row_for(dir, &rel_dir, g, cover_dir, &mut out.stats));
        }
        for p in &result.ungrouped {
            // Before calling a package damaged, check whether it is simply a PS3 one. The
            // two share the extension and differ in four bytes, and a PS3 library listed
            // as "damaged" would be badly wrong.
            if let Some(platform) = ps3::peek_pkg_magic(&dir.join(&p.name))
                .and_then(ps3::platform_for_pkg_magic)
                .filter(|pl| *pl != group::Platform::Ps4)
            {
                out.games.push(loose_file_row(
                    dir,
                    &rel_dir,
                    &p.name,
                    p.size_bytes,
                    platform,
                    false,
                    &files,
                    cover_dir,
                    &mut out.stats,
                ));
                continue;
            }
            out.damaged.push(StrayFileRow {
                name: p.name.clone(),
                rel_dir: rel_dir.clone(),
                size_bytes: p.size_bytes,
                detail: match p.status {
                    group::ProbeStatus::Unreadable => "could not be opened".into(),
                    _ => "no package header, and no split set it belongs to".into(),
                },
            });
        }
    }

    // Everything else this folder holds that is a console game: disc images, Xbox
    // executables, Vita packages. Recognised from the outside, named by its filename.
    generic_rows(dir, &rel_dir, &files, hint, cover_dir, out);

    emit(out, &rel_dir, on_progress);

    if depth >= walker::MAX_DEPTH {
        return;
    }
    for name in subdirs {
        let child = dir.join(&name);
        if walker::is_pruned_dir(&child, &name.to_lowercase()) {
            continue;
        }
        visit(&child, root, depth + 1, cached, cover_dir, out, on_progress);
    }
}

fn emit(out: &ConsoleRead, rel_dir: &str, on_progress: &mut dyn FnMut(ConsoleProgress)) {
    on_progress(ConsoleProgress {
        dirs_read: out.stats.dirs_read,
        games: out.games.len(),
        probed: out.stats.probed,
        from_cache: out.stats.from_cache,
        current: rel_dir.to_string(),
        finished: false,
    });
}

fn count_cover(stats: &mut ReadStats, source: Option<CoverSource>) {
    match source {
        Some(CoverSource::Embedded) => stats.covers_embedded += 1,
        Some(CoverSource::Sibling) => stats.covers_sibling += 1,
        None => {}
    }
}

/// Total bytes of a folder tree, bounded so a pathological tree cannot stall a read.
fn folder_bytes(dir: &Path, depth: usize) -> u64 {
    if depth > walker::MAX_DEPTH {
        return 0;
    }
    let Ok((files, dirs)) = list_dir(dir) else { return 0 };
    let here: u64 = files.iter().map(|f| f.size_bytes).sum();
    here + dirs
        .iter()
        .map(|d| folder_bytes(&dir.join(d), depth + 1))
        .sum::<u64>()
}

/// One PS3 game folder.
fn ps3_row(
    dir: &Path,
    game_dir: &Path,
    rel_dir: &str,
    cover_dir: &Path,
    stats: &mut ReadStats,
) -> ConsoleGameRow {
    let g = ps3::read(game_dir);
    let folder_name = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| rel_dir.to_string());

    // The title from inside PARAM.SFO is authoritative; the folder name is the fallback.
    let title_from_file = g.title.is_some();
    let display_title = g
        .title
        .clone()
        .unwrap_or_else(|| crate::scan::title::clean(&folder_name, true).clean_title);

    let key = cover_key_of(
        group::Platform::Ps3,
        g.title_id.as_deref().unwrap_or(&folder_name),
    );
    let cover = g
        .icon
        .as_ref()
        .and_then(|p| cache_cover_file(p, &key, CoverSource::Embedded, cover_dir));
    count_cover(stats, cover.as_ref().map(|(_, s)| *s));

    ConsoleGameRow {
        platform: group::Platform::Ps3.as_slug().to_string(),
        title_id: g.title_id.clone(),
        display_title,
        title_from_file,
        platform_guessed: false,
        rel_dir: parent_rel(rel_dir),
        rel_path: rel_dir.to_string(),
        total_bytes: folder_bytes(dir, 0),
        min_system_ver: None,
        // A PS3 sfo prints its firmware as text rather than packing it, so it is passed
        // through as written instead of being reformatted.
        firmware_label: g.system_ver.clone(),
        cover_path: cover.as_ref().map(|(p, _)| p.to_string_lossy().into_owned()),
        cover_source: cover.as_ref().map(|(_, s)| s.as_str().to_string()),
        base_title_id: None,
        link_reason: None,
        is_orphan: false,
        roles: vec![RoleCount { role: "pkg_game".into(), count: 1 }],
        files: vec![ConsoleFileRow {
            name: folder_name,
            size_bytes: folder_bytes(dir, 0),
            role: "pkg_game".into(),
            part_index: None,
            app_ver: g.app_ver.clone(),
            content_id: g.title_id.clone(),
            probe_status: "ok".into(),
        }],
    }
}

/// A game that is a folder rather than a file: an Xbox title booted by `default.xbe`.
///
/// The folder names it and the folder is measured, so a card shows "Halo Combat Evolved"
/// at its real size rather than "default" at the size of one executable.
#[allow(clippy::too_many_arguments)]
fn folder_game_row(
    dir: &Path,
    rel_dir: &str,
    boot: &str,
    platform: group::Platform,
    files: &[super::DirFile],
    cover_dir: &Path,
    stats: &mut ReadStats,
) -> ConsoleGameRow {
    let folder_name = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| rel_dir.to_string());
    let total = folder_bytes(dir, 0);

    // The executable does carry a title in its certificate, but reading that is a separate
    // job; until then the folder name is the honest answer.
    let key = cover_key_of(platform, &folder_name);
    let cover = files
        .iter()
        .find(|s| super::is_cover_image(&s.name.to_lowercase()))
        .and_then(|s| cache_cover_file(&dir.join(&s.name), &key, CoverSource::Sibling, cover_dir));
    count_cover(stats, cover.as_ref().map(|(_, s)| *s));

    ConsoleGameRow {
        platform: platform.as_slug().to_string(),
        title_id: None,
        display_title: crate::scan::title::clean(&folder_name, true).clean_title,
        title_from_file: false,
        // The extension named the console outright, so this is not a guess.
        platform_guessed: false,
        rel_dir: parent_rel(rel_dir),
        rel_path: join_rel(rel_dir, boot),
        total_bytes: total,
        min_system_ver: None,
        firmware_label: None,
        cover_path: cover.as_ref().map(|(p, _)| p.to_string_lossy().into_owned()),
        cover_source: cover.as_ref().map(|(_, s)| s.as_str().to_string()),
        base_title_id: None,
        link_reason: None,
        is_orphan: false,
        roles: vec![RoleCount { role: "pkg_game".into(), count: 1 }],
        files: vec![ConsoleFileRow {
            name: boot.to_string(),
            size_bytes: total,
            role: "pkg_game".into(),
            part_index: None,
            app_ver: None,
            content_id: None,
            probe_status: "ok".into(),
        }],
    }
}

/// The folder holding `rel_dir`, so a game folder's card points at its parent.
fn parent_rel(rel_dir: &str) -> String {
    rel_dir
        .rsplit_once('\\')
        .map(|(head, _)| head.to_string())
        .unwrap_or_default()
}

/// Rows for every console file in this folder that the PS4 pass did not claim.
///
/// A `.cue` takes its own tracks with it, so a two-file PS1 rip is one card rather than
/// two. Everything else stands alone.
fn generic_rows(
    dir: &Path,
    rel_dir: &str,
    files: &[super::DirFile],
    hint: Option<group::Platform>,
    cover_dir: &Path,
    out: &mut ConsoleRead,
) {
    let mut claimed: Vec<String> = Vec::new();

    // Cue sheets first, so their tracks are spoken for before the loose pass.
    for f in files {
        if !f.name.to_lowercase().ends_with(".cue") {
            continue;
        }
        let stem = generic::stem_of(&f.name).to_lowercase();
        let members: Vec<&super::DirFile> = files
            .iter()
            .filter(|m| generic::belongs_to_set(&stem, &m.name))
            .collect();
        if members.is_empty() {
            continue;
        }
        for m in &members {
            claimed.push(m.name.to_lowercase());
        }
        claimed.push(f.name.to_lowercase());

        let total = f.size_bytes + members.iter().map(|m| m.size_bytes).sum::<u64>();
        let mut row = loose_file_row(
            dir,
            rel_dir,
            &f.name,
            total,
            hint.unwrap_or(group::Platform::Unknown),
            // A cue sheet never states its console, so the platform is always inferred
            // here — from the folder when it names one, and otherwise not known at all.
            true,
            files,
            cover_dir,
            &mut out.stats,
        );
        row.roles = vec![RoleCount { role: "pkg_game".into(), count: 1 }];
        if !members.is_empty() {
            row.roles.push(RoleCount { role: "pkg_part".into(), count: members.len() });
        }
        for (i, m) in members.iter().enumerate() {
            row.files.push(ConsoleFileRow {
                name: m.name.clone(),
                size_bytes: m.size_bytes,
                role: "pkg_part".into(),
                part_index: Some(i as u32 + 1),
                app_ver: None,
                content_id: None,
                probe_status: "ok".into(),
            });
        }
        out.games.push(row);
    }

    for f in files {
        let lower = f.name.to_lowercase();
        // `.pkg` belongs to the PS4 pass, images are covers, and a cue's tracks are taken.
        if lower.ends_with(".pkg") || super::is_cover_image(&lower) || claimed.contains(&lower) {
            continue;
        }
        let Some(r) = generic::recognise(&f.name, hint) else { continue };
        out.games.push(loose_file_row(
            dir,
            rel_dir,
            &f.name,
            f.size_bytes,
            r.platform,
            r.confidence != generic::Confidence::Certain,
            files,
            cover_dir,
            &mut out.stats,
        ));
    }
}

/// One card for a single console file, named by its filename.
#[allow(clippy::too_many_arguments)]
fn loose_file_row(
    dir: &Path,
    rel_dir: &str,
    name: &str,
    total_bytes: u64,
    platform: group::Platform,
    platform_guessed: bool,
    siblings: &[super::DirFile],
    cover_dir: &Path,
    stats: &mut ReadStats,
) -> ConsoleGameRow {
    let stem = generic::stem_of(name);
    let display_title = crate::scan::title::clean(stem, false).clean_title;

    // An image beside the file, matched by name, is the only artwork available without
    // reading inside the container.
    let key = cover_key_of(platform, stem);
    let cover = siblings
        .iter()
        .find(|s| {
            let l = s.name.to_lowercase();
            super::is_cover_image(&l) && generic::stem_of(&l) == stem.to_lowercase()
        })
        .and_then(|s| {
            cache_cover_file(&dir.join(&s.name), &key, CoverSource::Sibling, cover_dir)
        });
    count_cover(stats, cover.as_ref().map(|(_, s)| *s));

    ConsoleGameRow {
        platform: platform.as_slug().to_string(),
        title_id: None,
        display_title,
        title_from_file: false,
        platform_guessed,
        rel_dir: rel_dir.to_string(),
        rel_path: join_rel(rel_dir, name),
        total_bytes,
        min_system_ver: None,
        firmware_label: None,
        cover_path: cover.as_ref().map(|(p, _)| p.to_string_lossy().into_owned()),
        cover_source: cover.as_ref().map(|(_, s)| s.as_str().to_string()),
        base_title_id: None,
        link_reason: None,
        is_orphan: false,
        roles: vec![RoleCount { role: "pkg_game".into(), count: 1 }],
        files: vec![ConsoleFileRow {
            name: name.to_string(),
            size_bytes: total_bytes,
            role: "pkg_game".into(),
            part_index: None,
            app_ver: None,
            content_id: None,
            probe_status: "ok".into(),
        }],
    }
}

fn row_for(
    dir: &Path,
    rel_dir: &str,
    g: &group::ConsoleGroup,
    cover_dir: &Path,
    stats: &mut ReadStats,
) -> ConsoleGameRow {
    let cover = cache_group_cover(dir, g, cover_dir);
    match cover.as_ref().map(|(_, s)| *s) {
        Some(CoverSource::Embedded) => stats.covers_embedded += 1,
        Some(CoverSource::Sibling) => stats.covers_sibling += 1,
        None => {}
    }

    let roles = [
        group::ConsoleRole::Game,
        group::ConsoleRole::Update,
        group::ConsoleRole::Dlc,
        group::ConsoleRole::Part,
    ]
    .into_iter()
    .filter_map(|r| {
        let count = g.count_role(r);
        (count > 0).then(|| RoleCount { role: r.as_db_role().to_string(), count })
    })
    .collect();

    let files = g
        .files
        .iter()
        .map(|f| ConsoleFileRow {
            name: f.name.clone(),
            size_bytes: f.size_bytes,
            role: f.role.as_db_role().to_string(),
            part_index: f.part_index,
            app_ver: f.app_ver.clone(),
            content_id: f.content_id.clone(),
            probe_status: f.status.as_str().to_string(),
        })
        .collect();

    ConsoleGameRow {
        platform: g.platform.as_slug().to_string(),
        title_id: Some(g.title_id.clone()),
        display_title: g.display_title.clone(),
        // Both came out of the package's own param.sfo, which is the whole point of it.
        title_from_file: true,
        platform_guessed: false,
        rel_dir: rel_dir.to_string(),
        rel_path: join_rel(rel_dir, &g.primary_file().name),
        total_bytes: g.total_bytes,
        min_system_ver: g.min_system_ver,
        firmware_label: g.min_system_ver.map(ps4::firmware_label),
        cover_path: cover
            .as_ref()
            .map(|(p, _)| p.to_string_lossy().into_owned()),
        cover_source: cover.as_ref().map(|(_, s)| s.as_str().to_string()),
        base_title_id: g.linked_base.clone(),
        link_reason: g.link_reason.clone(),
        is_orphan: g.is_orphan,
        roles,
        files,
    }
}

/// A path relative to the read root, using backslashes. Empty for the root itself.
fn rel(root: &Path, p: &Path) -> String {
    p.strip_prefix(root)
        .unwrap_or(p)
        .to_string_lossy()
        .into_owned()
}

fn join_rel(rel_dir: &str, name: &str) -> String {
    if rel_dir.is_empty() {
        name.to_string()
    } else {
        format!("{rel_dir}\\{name}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::console::ps4::tests::{png_bytes, PkgBuilder};
    use std::fs;
    use std::path::PathBuf;

    fn tmp(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "gv_index_{tag}_{}_{}",
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

    fn base(cid: &str, tid: &str, title: &str) -> PkgBuilder {
        PkgBuilder::new(cid)
            .text("CATEGORY", "gd")
            .text("TITLE_ID", tid)
            .text("TITLE", title)
    }

    /// A PS3 `PARAM.SFO` — the same `\0PSF` container a PS4 package carries, which is why
    /// one parser reads both.
    fn ps3_sfo(title_id: &str, title: &str, fw: &str) -> Vec<u8> {
        use crate::scan::console::ps4::SfoValue;
        let fields: Vec<(&str, SfoValue)> = vec![
            ("CATEGORY", SfoValue::Text("DG".into())),
            ("TITLE_ID", SfoValue::Text(title_id.into())),
            ("TITLE", SfoValue::Text(title.into())),
            ("PS3_SYSTEM_VER", SfoValue::Text(fw.into())),
        ];
        let n = fields.len();
        let mut keys: Vec<u8> = Vec::new();
        let mut key_offs: Vec<u16> = Vec::new();
        for (k, _) in &fields {
            key_offs.push(keys.len() as u16);
            keys.extend_from_slice(k.as_bytes());
            keys.push(0);
        }
        while keys.len() % 4 != 0 {
            keys.push(0);
        }
        let mut vals: Vec<u8> = Vec::new();
        let mut meta: Vec<(u16, u32, u32)> = Vec::new();
        for (_, v) in &fields {
            let off = vals.len() as u32;
            match v {
                SfoValue::Text(t) => {
                    let mut b = t.as_bytes().to_vec();
                    b.push(0);
                    meta.push((0x0204, b.len() as u32, off));
                    vals.extend_from_slice(&b);
                }
                SfoValue::Int(i) => {
                    meta.push((0x0404, 4, off));
                    vals.extend_from_slice(&i.to_le_bytes());
                }
            }
        }
        let key_table = (0x14 + n * 16) as u32;
        let data_table = key_table + keys.len() as u32;

        let mut out = b"\x00PSF".to_vec();
        out.extend_from_slice(&0x0101_0000u32.to_le_bytes());
        out.extend_from_slice(&key_table.to_le_bytes());
        out.extend_from_slice(&data_table.to_le_bytes());
        out.extend_from_slice(&(n as u32).to_le_bytes());
        for i in 0..n {
            let (fmt, len, doff) = meta[i];
            out.extend_from_slice(&key_offs[i].to_le_bytes());
            out.extend_from_slice(&fmt.to_le_bytes());
            out.extend_from_slice(&len.to_le_bytes());
            out.extend_from_slice(&len.to_le_bytes());
            out.extend_from_slice(&doff.to_le_bytes());
        }
        out.extend_from_slice(&keys);
        out.extend_from_slice(&vals);
        out
    }

    /// A PS3 rip is one card, named from its own PARAM.SFO and carrying its own ICON0.PNG —
    /// and the files under USRDIR are not mistaken for games of their own.
    #[test]
    fn a_ps3_folder_is_one_game_with_its_own_icon() {
        let root = tmp("ps3");
        let covers = root.join("_covers");
        let game = root.join("The Last of Us").join("PS3_GAME");
        fs::create_dir_all(game.join("USRDIR")).unwrap();
        fs::write(game.join("PARAM.SFO"), ps3_sfo("BLES01925", "The Last of Us", "04.2500")).unwrap();
        fs::write(game.join("ICON0.PNG"), png_bytes(8)).unwrap();
        fs::write(game.join("USRDIR").join("EBOOT.BIN"), vec![0u8; 2048]).unwrap();
        fs::write(game.join("USRDIR").join("data.psarc"), vec![0u8; 4096]).unwrap();

        let read = read_tree(&root, &HashMap::new(), &covers, &mut |_| {});

        assert_eq!(
            read.games.len(),
            1,
            "one folder, one game: {:?}",
            read.games.iter().map(|g| &g.display_title).collect::<Vec<_>>()
        );
        let g = &read.games[0];
        assert_eq!(g.platform, "ps3");
        assert_eq!(g.title_id.as_deref(), Some("BLES01925"));
        assert_eq!(g.display_title, "The Last of Us");
        assert!(g.title_from_file, "the name came from PARAM.SFO");
        assert!(!g.platform_guessed);
        assert_eq!(g.firmware_label.as_deref(), Some("04.2500"), "PS3 prints firmware as text");
        assert_eq!(g.cover_source.as_deref(), Some("pkg_icon0"));
        assert!(g.total_bytes > 6000, "the whole folder is measured: {}", g.total_bytes);
        assert!(read.damaged.is_empty());

        let _ = fs::remove_dir_all(&root);
    }

    /// A disc image takes its platform from the folder it sits in, and is marked as a guess.
    #[test]
    fn a_disc_image_is_listed_with_the_platform_its_folder_names() {
        let root = tmp("disc");
        let covers = root.join("_covers");
        let ps2 = root.join("PS2");
        fs::create_dir_all(&ps2).unwrap();
        fs::write(ps2.join("Shadow of the Colossus.iso"), vec![0u8; 5000]).unwrap();
        fs::write(ps2.join("Shadow of the Colossus.jpg"), b"jpeg stand-in").unwrap();

        let read = read_tree(&root, &HashMap::new(), &covers, &mut |_| {});
        assert_eq!(read.games.len(), 1);
        let g = &read.games[0];
        assert_eq!(g.platform, "ps2");
        assert!(g.platform_guessed, "the folder named it, the file did not");
        assert!(!g.title_from_file);
        assert_eq!(g.title_id, None, "nothing has walked the disc filesystem");
        assert_eq!(g.display_title, "Shadow of the Colossus");
        // A PS2 disc carries no artwork, so the image beside it becomes the cover.
        assert_eq!(g.cover_source.as_deref(), Some("pkg_sibling"));

        let _ = fs::remove_dir_all(&root);
    }

    /// A cue sheet and its tracks are one card, and a similarly named game is not swallowed.
    #[test]
    fn a_cue_sheet_and_its_tracks_are_one_card() {
        let root = tmp("cue");
        let covers = root.join("_covers");
        let ps1 = root.join("PS1");
        fs::create_dir_all(&ps1).unwrap();
        fs::write(ps1.join("Final Fantasy VII.cue"), b"FILE BINARY").unwrap();
        fs::write(ps1.join("Final Fantasy VII (Track 1).bin"), vec![0u8; 3000]).unwrap();
        fs::write(ps1.join("Final Fantasy VII (Track 2).bin"), vec![0u8; 2000]).unwrap();
        fs::write(ps1.join("Final Fantasy VIII.bin"), vec![0u8; 1000]).unwrap();

        let read = read_tree(&root, &HashMap::new(), &covers, &mut |_| {});
        let titles: Vec<&str> = read.games.iter().map(|g| g.display_title.as_str()).collect();
        assert_eq!(read.games.len(), 2, "one set plus one separate game: {titles:?}");

        let set = read
            .games
            .iter()
            .find(|g| g.rel_path.ends_with(".cue"))
            .expect("the cue sheet is the primary file");
        assert_eq!(set.platform, "ps1");
        assert_eq!(set.files.len(), 3, "the sheet plus its two tracks");
        assert!(set.total_bytes >= 5000);
        assert!(
            read.games.iter().any(|g| g.display_title.contains("VIII")),
            "the eighth game must not be swallowed by the seventh: {titles:?}"
        );

        let _ = fs::remove_dir_all(&root);
    }

    /// An Xbox executable is certain from its extension alone, with no folder hint needed.
    #[test]
    fn an_xbox_executable_is_attributed_from_its_extension_alone() {
        let root = tmp("xbox");
        let covers = root.join("_covers");
        fs::write(root.join("Halo.xbe"), vec![0u8; 900]).unwrap();
        fs::write(root.join("Gears.xex"), vec![0u8; 900]).unwrap();

        let read = read_tree(&root, &HashMap::new(), &covers, &mut |_| {});
        assert!(read.games.iter().any(|g| g.platform == "xbox"), "original Xbox from .xbe");
        assert!(read.games.iter().any(|g| g.platform == "xbox360"), "Xbox 360 from .xex");
        for g in &read.games {
            assert!(!g.platform_guessed, "the extension is certain: {}", g.platform);
        }

        let _ = fs::remove_dir_all(&root);
    }

    /// An Xbox game is its folder. `default.xbe` is the console's naming convention, so a
    /// card reading "default" at the size of one executable would be useless.
    #[test]
    fn an_xbox_game_folder_is_named_and_measured_by_its_folder() {
        let root = tmp("xboxfolder");
        let covers = root.join("_covers");
        let game = root.join("Xbox").join("Halo Combat Evolved");
        fs::create_dir_all(game.join("media")).unwrap();
        fs::write(game.join("default.xbe"), vec![0u8; 900]).unwrap();
        fs::write(game.join("cover.jpg"), b"jpeg stand-in").unwrap();
        fs::write(game.join("media").join("audio.wav"), vec![0u8; 5000]).unwrap();

        let read = read_tree(&root, &HashMap::new(), &covers, &mut |_| {});

        assert_eq!(
            read.games.len(),
            1,
            "the folder is one game, and `media` is not another: {:?}",
            read.games.iter().map(|g| &g.display_title).collect::<Vec<_>>()
        );
        let g = &read.games[0];
        assert_eq!(g.display_title, "Halo Combat Evolved", "named by its folder, not by default.xbe");
        assert_eq!(g.platform, "xbox");
        assert!(!g.platform_guessed, ".xbe names the console outright");
        assert!(g.total_bytes >= 5900, "the whole folder is measured: {}", g.total_bytes);
        assert_eq!(g.cover_source.as_deref(), Some("pkg_sibling"));

        let _ = fs::remove_dir_all(&root);
    }

    /// A PS3 package must not be filed as damaged merely for not being a PS4 one.
    #[test]
    fn a_ps3_package_is_recognised_rather_than_called_damaged() {
        let root = tmp("ps3pkg");
        let covers = root.join("_covers");
        // Same extension as a PS4 package, different magic.
        let mut pkg = b"\x7FPKG".to_vec();
        pkg.extend_from_slice(&vec![0u8; 0x500]);
        fs::write(root.join("Some PS3 Game.pkg"), pkg).unwrap();

        let read = read_tree(&root, &HashMap::new(), &covers, &mut |_| {});
        assert!(read.damaged.is_empty(), "not damaged: {:?}", read.damaged);
        assert_eq!(read.games.len(), 1);
        assert_eq!(read.games[0].platform, "ps3");
        assert_eq!(read.games[0].display_title, "Some PS3 Game");

        let _ = fs::remove_dir_all(&root);
    }

    /// Files that are not console games must never become cards.
    #[test]
    fn ordinary_files_do_not_become_games() {
        let root = tmp("plain");
        let covers = root.join("_covers");
        for n in ["notes.txt", "setup.exe", "movie.mkv", "Backup.rar", "art.png"] {
            fs::write(root.join(n), vec![0u8; 100]).unwrap();
        }

        let read = read_tree(&root, &HashMap::new(), &covers, &mut |_| {});
        assert!(
            read.games.is_empty(),
            "{:?}",
            read.games.iter().map(|g| &g.display_title).collect::<Vec<_>>()
        );
        assert!(read.damaged.is_empty());

        let _ = fs::remove_dir_all(&root);
    }

    /// Packages in nested folders are all found, and each game carries the cover that was
    /// read out of its own file.
    #[test]
    fn a_folder_tree_of_packages_reads_into_games_with_their_own_artwork() {
        let root = tmp("tree");
        let covers = root.join("_covers");
        fs::create_dir_all(root.join("PS4").join("Games")).unwrap();

        fs::write(
            root.join("PS4").join("Games").join("metro.pkg"),
            base("EP4062-CUSA11407_00-METROEXODUS00000", "CUSA11407", "Metro Exodus")
                .int("SYSTEM_VER", 0x0470_0000)
                .icon(png_bytes(1))
                .build(),
        )
        .unwrap();
        fs::write(
            root.join("soma.pkg"),
            base("EP1188-CUSA03200_00-HPL3SOMAGAME2015", "CUSA03200", "SOMA").build(),
        )
        .unwrap();

        let read = read_tree(&root, &HashMap::new(), &covers, &mut |_| {});

        assert_eq!(read.games.len(), 2, "both packages became games");
        assert_eq!(read.stats.probed, 2);
        assert_eq!(read.stats.from_cache, 0);
        assert_eq!(read.stats.covers_embedded, 1);

        let metro = read.games.iter().find(|g| g.title_id.as_deref() == Some("CUSA11407")).unwrap();
        assert_eq!(metro.display_title, "Metro Exodus");
        assert_eq!(metro.rel_dir, "PS4\\Games");
        assert_eq!(metro.rel_path, "PS4\\Games\\metro.pkg");
        assert_eq!(metro.platform, "ps4");
        assert_eq!(metro.firmware_label.as_deref(), Some("04.70"));
        assert_eq!(metro.cover_source.as_deref(), Some("pkg_icon0"));
        assert!(metro.cover_path.as_ref().is_some_and(|p| p.ends_with(".icon0.png")));

        // No artwork anywhere for SOMA, so the card falls back to a gradient.
        let soma = read.games.iter().find(|g| g.title_id.as_deref() == Some("CUSA03200")).unwrap();
        assert_eq!(soma.cover_path, None);
        assert_eq!(soma.cover_source, None);

        let _ = fs::remove_dir_all(&root);
    }

    /// The second read must not reopen anything, which is what makes reading live
    /// affordable. The probe keys are relative to the root, so nesting is preserved.
    #[test]
    fn a_second_read_serves_every_package_from_the_probe_cache() {
        let root = tmp("cache");
        let covers = root.join("_covers");
        fs::create_dir_all(root.join("sub")).unwrap();
        fs::write(
            root.join("sub").join("a.pkg"),
            base("EP0000-CUSA00001_00-A", "CUSA00001", "Game A").build(),
        )
        .unwrap();
        fs::write(
            root.join("b.pkg"),
            base("EP0000-CUSA00002_00-B", "CUSA00002", "Game B").build(),
        )
        .unwrap();

        let first = read_tree(&root, &HashMap::new(), &covers, &mut |_| {});
        assert_eq!(first.stats.probed, 2);
        assert_eq!(first.fresh_probes.len(), 2);

        let cache: HashMap<String, ProbeRow> = first.fresh_probes.iter().cloned().collect();
        assert!(cache.contains_key("sub\\a.pkg"), "keys are relative to the root: {cache:?}");

        let second = read_tree(&root, &cache, &covers, &mut |_| {});
        assert_eq!(second.stats.probed, 0, "nothing was reopened");
        assert_eq!(second.stats.from_cache, 2);
        assert_eq!(second.games.len(), 2);
        // The identity survives the round trip through the cache.
        let mut titles: Vec<&str> = second.games.iter().map(|g| g.display_title.as_str()).collect();
        titles.sort();
        assert_eq!(titles, vec!["Game A", "Game B"]);

        let _ = fs::remove_dir_all(&root);
    }

    /// A file whose size changed on disk must be reprobed, or the window would show a
    /// stale title for a package the user replaced.
    #[test]
    fn a_changed_file_is_reprobed_rather_than_trusted() {
        let root = tmp("changed");
        let covers = root.join("_covers");
        let path = root.join("g.pkg");
        fs::write(&path, base("EP0000-CUSA00001_00-A", "CUSA00001", "Before").build()).unwrap();

        let first = read_tree(&root, &HashMap::new(), &covers, &mut |_| {});
        let mut cache: HashMap<String, ProbeRow> = first.fresh_probes.iter().cloned().collect();

        // Replace the package, and pretend the cache still holds the old size.
        fs::write(&path, base("EP0000-CUSA00001_00-A", "CUSA00001", "After").build()).unwrap();
        if let Some(row) = cache.get_mut("g.pkg") {
            row.size_bytes += 1;
        }

        let second = read_tree(&root, &cache, &covers, &mut |_| {});
        assert_eq!(second.stats.probed, 1, "the changed file was reopened");
        assert_eq!(second.games[0].display_title, "After");

        let _ = fs::remove_dir_all(&root);
    }

    /// A damaged package must reach the window rather than vanishing from it.
    #[test]
    fn a_damaged_package_is_reported_not_dropped() {
        let root = tmp("damaged");
        let covers = root.join("_covers");
        fs::write(
            root.join("good.pkg"),
            base("EP0000-CUSA00001_00-A", "CUSA00001", "Good").build(),
        )
        .unwrap();
        fs::write(root.join("junk.pkg"), vec![0u8; 0x60]).unwrap();

        let read = read_tree(&root, &HashMap::new(), &covers, &mut |_| {});
        assert_eq!(read.games.len(), 1);
        assert_eq!(read.damaged.len(), 1);
        assert_eq!(read.damaged[0].name, "junk.pkg");
        assert!(!read.damaged[0].detail.is_empty());

        let _ = fs::remove_dir_all(&root);
    }

    /// Pruned directories are skipped here exactly as the scanner skips them.
    #[test]
    fn pruned_directories_are_not_descended() {
        let root = tmp("prune");
        let covers = root.join("_covers");
        let bin = root.join("$RECYCLE.BIN");
        fs::create_dir_all(&bin).unwrap();
        fs::write(
            bin.join("deleted.pkg"),
            base("EP0000-CUSA09999_00-Z", "CUSA09999", "Deleted").build(),
        )
        .unwrap();

        let read = read_tree(&root, &HashMap::new(), &covers, &mut |_| {});
        assert!(read.games.is_empty(), "a recycle bin is not a game library");

        let _ = fs::remove_dir_all(&root);
    }
}

#[cfg(test)]
mod contract {
    use super::*;
    use crate::scan::console::ps4::tests::{png_bytes, PkgBuilder};
    use std::fs;

    /// The JSON field names the Game Files screen reads, exactly as `ui/gamefiles.js`
    /// spells them.
    ///
    /// Serde derives these from the Rust field names, so a rename here is silent: the
    /// screen would simply render blank chips and `undefined`. This pins the contract on
    /// the Rust side, where the rename would happen.
    #[test]
    fn the_json_sent_to_the_window_has_the_field_names_it_reads() {
        let dir = std::env::temp_dir().join(format!("gv_contract_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("game.pkg"),
            PkgBuilder::new("EP4062-CUSA11407_00-METROEXODUS00000")
                .text("CATEGORY", "gd")
                .text("TITLE_ID", "CUSA11407")
                .text("TITLE", "Metro Exodus")
                .text("APP_VER", "01.00")
                .int("SYSTEM_VER", 0x0470_0000)
                .icon(png_bytes(3))
                .build(),
        )
        .unwrap();
        fs::write(dir.join("junk.pkg"), vec![0u8; 0x60]).unwrap();

        let read = read_tree(&dir, &HashMap::new(), &dir.join("_covers"), &mut |_| {});
        let v: serde_json::Value = serde_json::to_value(&read).expect("serialises");

        let keys = |val: &serde_json::Value| -> Vec<String> {
            val.as_object().expect("object").keys().cloned().collect()
        };

        let mut top = keys(&v);
        top.sort();
        assert_eq!(top, vec!["damaged", "games", "root", "stats"]);

        let mut stats = keys(&v["stats"]);
        stats.sort();
        assert_eq!(
            stats,
            vec![
                "covers_embedded",
                "covers_sibling",
                "dirs_read",
                "elapsed_ms",
                "from_cache",
                "probed",
            ]
        );

        let game = &v["games"][0];
        let mut g = keys(game);
        g.sort();
        assert_eq!(
            g,
            vec![
                "base_title_id",
                "cover_path",
                "cover_source",
                "display_title",
                "files",
                "firmware_label",
                "is_orphan",
                "link_reason",
                "min_system_ver",
                "platform",
                "platform_guessed",
                "rel_dir",
                "rel_path",
                "roles",
                "title_from_file",
                "title_id",
                "total_bytes",
            ]
        );

        let mut f = keys(&game["files"][0]);
        f.sort();
        assert_eq!(
            f,
            vec![
                "app_ver",
                "content_id",
                "name",
                "part_index",
                "probe_status",
                "role",
                "size_bytes",
            ]
        );

        let mut r = keys(&game["roles"][0]);
        r.sort();
        assert_eq!(r, vec!["count", "role"]);

        let mut d = keys(&v["damaged"][0]);
        d.sort();
        assert_eq!(d, vec!["detail", "name", "rel_dir", "size_bytes"]);

        // `fresh_probes` is backend bookkeeping and must never cross to the window.
        assert!(v.get("fresh_probes").is_none(), "probe rows must not be sent to the UI");

        // The values the window actually shows, not just the names.
        assert_eq!(game["platform"], "ps4");
        assert_eq!(game["title_id"], "CUSA11407");
        assert_eq!(game["display_title"], "Metro Exodus");
        assert_eq!(game["firmware_label"], "04.70");
        assert_eq!(game["cover_source"], "pkg_icon0");
        assert!(game["cover_path"].as_str().unwrap().ends_with("ps4_CUSA11407.icon0.png"));
        assert_eq!(game["roles"][0]["role"], "pkg_game");
        // A PS4 package states its own name, so nothing here is a guess.
        assert_eq!(game["title_from_file"], true);
        assert_eq!(game["platform_guessed"], false);

        // The progress payload the status line reads.
        let p = ConsoleProgress {
            dirs_read: 1,
            games: 2,
            probed: 3,
            from_cache: 4,
            current: "PS4".into(),
            finished: false,
        };
        let mut pk = keys(&serde_json::to_value(&p).unwrap());
        pk.sort();
        assert_eq!(
            pk,
            vec!["current", "dirs_read", "finished", "from_cache", "games", "probed"]
        );

        let _ = fs::remove_dir_all(&dir);
    }
}
