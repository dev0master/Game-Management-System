//! Console game formats: reading identity from inside the container.

pub mod generic;
pub mod group;
pub mod index;
pub mod ps3;
pub mod ps4;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Where a cached cover came from.
///
/// Ranked so a later pass can never replace better artwork with worse: embedded art read
/// out of the game file is region-correct and certain, while an online match is a guess.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CoverSource {
    /// An image file beside the package, matched to it by name.
    Sibling,
    /// `icon0.png` read out of the package itself.
    Embedded,
}

impl CoverSource {
    /// The tag recorded alongside the cover, and shown to the user as a badge.
    pub fn as_str(self) -> &'static str {
        match self {
            CoverSource::Embedded => "pkg_icon0",
            CoverSource::Sibling => "pkg_sibling",
        }
    }

    /// Precedence against the other sources: IGDB is 1, no cover at all is 0.
    pub fn rank(self) -> u8 {
        match self {
            CoverSource::Embedded => 3,
            CoverSource::Sibling => 2,
        }
    }
}

/// One file in a directory, as the console layer needs it.
#[derive(Debug, Clone)]
pub struct DirFile {
    pub name: String,
    pub size_bytes: u64,
    pub mtime_unix: i64,
}

/// List one directory: its files, and the names of its subdirectories.
///
/// Read once. Every caller here needs both halves, and reading the directory twice over
/// USB is the kind of waste that turns a fast window into a slow one.
pub fn list_dir(dir: &Path) -> std::io::Result<(Vec<DirFile>, Vec<String>)> {
    let mut files = Vec::new();
    let mut dirs = Vec::new();

    for entry in std::fs::read_dir(dir)? {
        let Ok(entry) = entry else { continue };
        let Ok(ft) = entry.file_type() else { continue };
        let name = entry.file_name().to_string_lossy().into_owned();
        if ft.is_dir() {
            dirs.push(name);
            continue;
        }
        if !ft.is_file() {
            continue;
        }
        let (size_bytes, mtime_unix) = match entry.metadata() {
            Ok(m) => (
                m.len(),
                m.modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0),
            ),
            Err(_) => (0, 0),
        };
        files.push(DirFile { name, size_bytes, mtime_unix });
    }

    files.sort_by(|a, b| a.name.cmp(&b.name));
    dirs.sort();
    Ok((files, dirs))
}

/// True for an image file that could serve as a cover.
pub fn is_cover_image(name_lower: &str) -> bool {
    name_lower.ends_with(".jpg") || name_lower.ends_with(".jpeg") || name_lower.ends_with(".png")
}

/// Probe the packages among an already-listed set of files.
///
/// Strictly read-only: each package is opened, a few hundred bytes are read from its
/// header, and it is closed. Nothing on the scanned drive is written, moved or removed.
///
/// `cached` is consulted first, keyed by filename; a file whose size and mtime match a
/// cached probe is never reopened. `fresh` collects the probes that had to be done, for
/// the caller to store.
pub fn probe_listed(
    dir: &Path,
    files: &[DirFile],
    cached: &HashMap<String, ProbeRow>,
    fresh: &mut Vec<ProbeRow>,
) -> (Vec<group::ProbedFile>, Vec<(String, u64)>) {
    let mut probed = Vec::new();
    let mut covers = Vec::new();

    for f in files {
        let lower = f.name.to_lowercase();
        if is_cover_image(&lower) {
            covers.push((f.name.clone(), f.size_bytes));
            continue;
        }
        if !lower.ends_with(".pkg") {
            continue;
        }

        // A file the cache already knows, unchanged on disk.
        if let Some(row) = cached.get(&f.name) {
            if row.size_bytes == f.size_bytes && row.mtime_unix == f.mtime_unix {
                probed.push(row.to_probed());
                continue;
            }
        }

        let (status, info) = match std::fs::File::open(dir.join(&f.name)) {
            Ok(mut file) => match ps4::parse(&mut file, f.size_bytes) {
                Ok(i) => (group::ProbeStatus::Ok, Some(i)),
                // A parse failure is data, not an error: it is how a continuation part
                // and a truncated copy are told apart from a healthy package.
                Err(_) => (group::ProbeStatus::NotPkg, None),
            },
            Err(_) => (group::ProbeStatus::Unreadable, None),
        };

        let p = group::ProbedFile {
            name: f.name.clone(),
            size_bytes: f.size_bytes,
            mtime_unix: f.mtime_unix,
            status,
            info,
        };
        fresh.push(ProbeRow::of(&p));
        probed.push(p);
    }

    probed.sort_by(|a, b| a.name.cmp(&b.name));
    covers.sort();
    (probed, covers)
}

/// Read every package in one directory, with no cache.
///
/// The form the `--console` harness uses: one call, nothing remembered.
pub fn probe_ps4_dir(dir: &Path) -> std::io::Result<(Vec<group::ProbedFile>, Vec<(String, u64)>)> {
    let (files, _dirs) = list_dir(dir)?;
    let empty = HashMap::new();
    let mut discard = Vec::new();
    Ok(probe_listed(dir, &files, &empty, &mut discard))
}

/// A cached probe of one package, as `console_probe` stores it.
///
/// Probing costs about 106 ms per package over USB — 7.5 s for a drive holding 71 of them
/// the first time. Keyed on size and mtime so reopening the window re-reads nothing that
/// has not changed, which is what lets the folder be read live without feeling slow.
///
/// Deliberately carries no artwork location. The cover cache is keyed by title id on disk,
/// so a cached probe never needs to know where inside the package the icon was.
#[derive(Debug, Clone)]
pub struct ProbeRow {
    pub name: String,
    pub size_bytes: u64,
    pub mtime_unix: i64,
    pub platform: String,
    pub status: String,
    pub title_id: Option<String>,
    pub content_id: Option<String>,
    pub category: Option<String>,
    pub title: Option<String>,
    pub app_ver: Option<String>,
    pub system_ver: Option<u32>,
    pub declared_bytes: Option<u64>,
    pub content_type: Option<u32>,
}

impl ProbeRow {
    /// Capture a fresh probe for storage.
    pub fn of(p: &group::ProbedFile) -> Self {
        let info = p.info.as_ref();
        Self {
            name: p.name.clone(),
            size_bytes: p.size_bytes,
            mtime_unix: p.mtime_unix,
            platform: group::Platform::Ps4.as_slug().to_string(),
            status: p.status.as_str().to_string(),
            title_id: info.and_then(|i| i.title_id()),
            content_id: info.map(|i| i.content_id.clone()),
            category: info.and_then(|i| i.sfo.category.clone()),
            title: info.and_then(|i| i.sfo.title.clone()),
            app_ver: info.and_then(|i| i.sfo.app_ver.clone()),
            system_ver: info.and_then(|i| i.sfo.system_ver),
            declared_bytes: info.map(|i| i.declared_bytes),
            content_type: info.map(|i| i.content_type),
        }
    }

    /// Rebuild what the grouper needs, without touching the file.
    pub fn to_probed(&self) -> group::ProbedFile {
        let status = group::ProbeStatus::parse(&self.status);
        // Only a package that parsed has identity to restore. Anything else must come
        // back as it went in — headerless — or the grouper would treat a damaged file
        // as a healthy one.
        let info = (status == group::ProbeStatus::Ok).then(|| ps4::PkgInfo {
            content_id: self.content_id.clone().unwrap_or_default(),
            content_type: self.content_type.unwrap_or(0),
            declared_bytes: self.declared_bytes.unwrap_or(0),
            sfo: ps4::ParamSfo {
                category: self.category.clone(),
                title_id: self.title_id.clone(),
                title: self.title.clone(),
                app_ver: self.app_ver.clone(),
                system_ver: self.system_ver,
                extra: Vec::new(),
            },
            icon0: None,
            pic0: None,
        });
        group::ProbedFile {
            name: self.name.clone(),
            size_bytes: self.size_bytes,
            mtime_unix: self.mtime_unix,
            status,
            info,
        }
    }
}

/// Cache filename stem: platform and whatever identifies the game.
///
/// Sanitised rather than trusted. An id normally looks like `CUSA08948`, but it can come
/// from a corrupt `param.sfo` or from a filename, and this string becomes a path.
pub fn cover_key_of(platform: group::Platform, id: &str) -> String {
    let id: String = id
        .chars()
        .take(64)
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_uppercase() } else { '_' })
        .collect();
    format!("{}_{}", platform.as_slug(), id)
}

fn cover_key(g: &group::ConsoleGroup) -> String {
    cover_key_of(g.platform, &g.title_id)
}

/// Copy one image into the cover cache and report where it landed.
///
/// Used for artwork that is already a file — a PS3 `ICON0.PNG`, an image beside a disc
/// image. It is copied rather than referenced because the original lives on the game drive,
/// which the asset protocol will not serve and which may be unplugged at any moment.
pub fn cache_cover_file(
    src: &Path,
    key: &str,
    source: CoverSource,
    cover_dir: &Path,
) -> Option<(PathBuf, CoverSource)> {
    let tag = match source {
        CoverSource::Embedded => "icon0",
        CoverSource::Sibling => "sib",
    };
    let ext = Path::new(src)
        .extension()
        .and_then(|e| e.to_str())
        .filter(|e| e.len() <= 4 && e.chars().all(|c| c.is_ascii_alphanumeric()))
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_else(|| "png".into());

    let dest = cover_dir.join(format!("{key}.{tag}.{ext}"));
    if dest.is_file() {
        return Some((dest, source));
    }
    let bytes = std::fs::read(src).ok()?;
    write_cover(&dest, &bytes).ok()?;
    Some((dest, source))
}

/// Open the package that defines this group and read the artwork inside it.
///
/// One open, one parse, one read — and only for the primary package, which is why
/// `PkgInfo` carries entry *locations* rather than bytes.
fn read_primary_artwork(dir: &Path, g: &group::ConsoleGroup) -> Option<Vec<u8>> {
    let path = dir.join(&g.primary_file().name);
    let len = std::fs::metadata(&path).ok()?.len();
    let mut f = std::fs::File::open(&path).ok()?;
    let info = ps4::parse(&mut f, len).ok()?;
    ps4::read_artwork(&mut f, &info)
}

fn write_cover(dest: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(dest, bytes)
}

/// Put a group's cover into the local cache, preferring the artwork inside the package.
///
/// Writes **only** under `cover_dir`. Nothing on the scanned drive is written, moved or
/// removed. The copy has to be local for two reasons: the asset protocol is scoped to
/// the cache directory, and the library must still show art with the drive unplugged.
///
/// The source is encoded in the filename, so finding a cached file already there also
/// says where it came from — which is what makes the skip-if-present path free.
pub fn cache_group_cover(
    dir: &Path,
    g: &group::ConsoleGroup,
    cover_dir: &Path,
) -> Option<(PathBuf, CoverSource)> {
    let key = cover_key(g);

    // 1. The artwork the game file carries. This is the one that was asked for.
    let embedded = cover_dir.join(format!("{key}.icon0.png"));
    if embedded.is_file() {
        return Some((embedded, CoverSource::Embedded));
    }
    if let Some(bytes) = read_primary_artwork(dir, g) {
        if write_cover(&embedded, &bytes).is_ok() {
            return Some((embedded, CoverSource::Embedded));
        }
    }

    // 2. An image sitting beside the package, already matched to it by the grouper.
    let sibling = g.files.iter().find(|f| f.role == group::ConsoleRole::Cover)?;
    let ext = Path::new(&sibling.name)
        .extension()
        .and_then(|e| e.to_str())
        .filter(|e| e.len() <= 4 && e.chars().all(|c| c.is_ascii_alphanumeric()))
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_else(|| "jpg".into());
    let dest = cover_dir.join(format!("{key}.sib.{ext}"));
    if dest.is_file() {
        return Some((dest, CoverSource::Sibling));
    }
    let bytes = std::fs::read(dir.join(&sibling.name)).ok()?;
    write_cover(&dest, &bytes).ok()?;
    Some((dest, CoverSource::Sibling))
}

/// What artwork a group has available. Reads, but writes nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtworkStatus {
    /// Bytes read and confirmed to be a PNG, with their length.
    Embedded(usize),
    /// The table claims an image, but says its bytes are encrypted.
    Encrypted,
    /// An image entry exists and is readable, but the bytes are not a PNG.
    NotAnImage,
    /// No image entry in the package at all.
    NoEntry,
}

/// Report a group's embedded artwork without touching the cover cache.
///
/// This exists for the `--console` harness, which answers the question the whole feature
/// rests on — do these packages really carry a readable `icon0.png`? — before any of it is
/// wired to the window.
pub fn inspect_artwork(dir: &Path, g: &group::ConsoleGroup) -> ArtworkStatus {
    let path = dir.join(&g.primary_file().name);
    let Ok(len) = std::fs::metadata(&path).map(|m| m.len()) else {
        return ArtworkStatus::NoEntry;
    };
    let Ok(mut f) = std::fs::File::open(&path) else {
        return ArtworkStatus::NoEntry;
    };
    let Ok(info) = ps4::parse(&mut f, len) else {
        return ArtworkStatus::NoEntry;
    };
    if info.icon0.is_none() && info.pic0.is_none() {
        return ArtworkStatus::NoEntry;
    }
    if !info.has_artwork() {
        return ArtworkStatus::Encrypted;
    }
    match ps4::read_artwork(&mut f, &info) {
        Some(b) => ArtworkStatus::Embedded(b.len()),
        None => ArtworkStatus::NotAnImage,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::console::ps4::tests::{png_bytes, PkgBuilder};
    use std::fs;

    fn tmp(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "gv_console_{tag}_{}_{}",
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

    /// The whole chain on real files: a directory of packages becomes games, and the
    /// artwork inside the primary package lands in the cover cache.
    ///
    /// This is the path the Game Files window depends on, so it is tested against bytes
    /// on disk rather than an in-memory cursor.
    #[test]
    fn a_directory_of_packages_yields_games_with_covers_from_inside_the_files() {
        let dir = tmp("chain");
        let covers = dir.join("_covers");
        let icon = png_bytes(0x42);

        // A base game carrying its own artwork, plus its update — one game, two files.
        fs::write(
            dir.join("Metro Exodus[USA11407].pkg"),
            PkgBuilder::new("EP4062-CUSA11407_00-METROEXODUS00000")
                .text("CATEGORY", "gd")
                .text("TITLE_ID", "CUSA11407")
                .text("TITLE", "Metro Exodus")
                .int("SYSTEM_VER", 0x0470_0000)
                .icon(icon.clone())
                .build(),
        )
        .unwrap();
        fs::write(
            dir.join("Metro Exodus Update 1.05.pkg"),
            PkgBuilder::new("EP4062-CUSA11407_00-METROEXODUSPATCH")
                .text("CATEGORY", "gp")
                .text("TITLE_ID", "CUSA11407")
                .text("TITLE", "Metro Exodus")
                .build(),
        )
        .unwrap();

        // A second game with no embedded artwork but an image file beside it.
        fs::write(
            dir.join("SOMA.pkg"),
            PkgBuilder::new("EP1188-CUSA03200_00-HPL3SOMAGAME2015")
                .text("CATEGORY", "gd")
                .text("TITLE_ID", "CUSA03200")
                .text("TITLE", "SOMA")
                .build(),
        )
        .unwrap();
        fs::write(dir.join("SOMA.jpg"), b"jpeg-bytes-stand-in").unwrap();

        let (probed, sibling_images) = probe_ps4_dir(&dir).expect("probe");
        assert_eq!(probed.len(), 3, "three packages read");
        assert_eq!(sibling_images.len(), 1, "one image collected");

        let result = group::group_ps4(&probed, &sibling_images);
        assert_eq!(result.groups.len(), 2, "two title ids, two games");
        assert!(result.ungrouped.is_empty(), "nothing damaged");

        let metro = result
            .groups
            .iter()
            .find(|g| g.title_id == "CUSA11407")
            .expect("metro grouped by title id");
        assert_eq!(metro.display_title, "Metro Exodus");
        assert_eq!(metro.count_role(group::ConsoleRole::Update), 1);

        // The cover comes out of the package itself.
        let (path, source) = cache_group_cover(&dir, metro, &covers).expect("metro cover");
        assert_eq!(source, CoverSource::Embedded);
        assert_eq!(fs::read(&path).unwrap(), icon, "the bytes are the embedded icon");
        assert!(
            path.starts_with(&covers),
            "covers are written only to the cache, never to the scanned drive"
        );
        assert_eq!(path.file_name().unwrap(), "ps4_CUSA11407.icon0.png");

        // With no embedded artwork, the sibling image is copied instead.
        let soma = result.groups.iter().find(|g| g.title_id == "CUSA03200").unwrap();
        let (soma_path, soma_source) = cache_group_cover(&dir, soma, &covers).expect("soma cover");
        assert_eq!(soma_source, CoverSource::Sibling);
        assert_eq!(fs::read(&soma_path).unwrap(), b"jpeg-bytes-stand-in");
        assert_eq!(soma_path.file_name().unwrap(), "ps4_CUSA03200.sib.jpg");

        // Embedded beats sibling, so a game with both keeps the art from its own file.
        assert!(source > soma_source);

        // The scanned directory still holds exactly what it did before.
        let mut left: Vec<String> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n != "_covers")
            .collect();
        left.sort();
        assert_eq!(
            left,
            vec![
                "Metro Exodus Update 1.05.pkg",
                "Metro Exodus[USA11407].pkg",
                "SOMA.jpg",
                "SOMA.pkg",
            ]
        );

        let _ = fs::remove_dir_all(&dir);
    }

    /// A second pass must not re-extract: the cached file is reused, and its name alone
    /// says where it came from.
    #[test]
    fn a_cached_cover_is_reused_rather_than_rewritten() {
        let dir = tmp("cached");
        let covers = dir.join("_covers");
        fs::write(
            dir.join("game.pkg"),
            PkgBuilder::new("EP0000-CUSA00001_00-X")
                .text("CATEGORY", "gd")
                .text("TITLE_ID", "CUSA00001")
                .text("TITLE", "Anything")
                .icon(png_bytes(7))
                .build(),
        )
        .unwrap();

        let (probed, imgs) = probe_ps4_dir(&dir).unwrap();
        let result = group::group_ps4(&probed, &imgs);
        let g = &result.groups[0];

        let (first, _) = cache_group_cover(&dir, g, &covers).unwrap();
        // Mark the cached file, then ask again: the mark must survive.
        fs::write(&first, b"\x89PNG\r\n\x1a\nmarked").unwrap();
        let (second, source) = cache_group_cover(&dir, g, &covers).unwrap();

        assert_eq!(first, second);
        assert_eq!(source, CoverSource::Embedded);
        assert_eq!(fs::read(&second).unwrap(), b"\x89PNG\r\n\x1a\nmarked");

        let _ = fs::remove_dir_all(&dir);
    }

    /// `inspect_artwork` is what the harness reports, so its verdicts must be distinct.
    #[test]
    fn artwork_status_distinguishes_the_reasons_there_is_no_cover() {
        let dir = tmp("status");

        fs::write(
            dir.join("with.pkg"),
            PkgBuilder::new("EP0000-CUSA00001_00-A")
                .text("CATEGORY", "gd")
                .text("TITLE_ID", "CUSA00001")
                .icon(png_bytes(9))
                .build(),
        )
        .unwrap();
        fs::write(
            dir.join("without.pkg"),
            PkgBuilder::new("EP0000-CUSA00002_00-B")
                .text("CATEGORY", "gd")
                .text("TITLE_ID", "CUSA00002")
                .build(),
        )
        .unwrap();

        let (probed, imgs) = probe_ps4_dir(&dir).unwrap();
        let result = group::group_ps4(&probed, &imgs);

        let with = result.groups.iter().find(|g| g.title_id == "CUSA00001").unwrap();
        let without = result.groups.iter().find(|g| g.title_id == "CUSA00002").unwrap();

        assert!(matches!(inspect_artwork(&dir, with), ArtworkStatus::Embedded(n) if n > 8));
        assert_eq!(inspect_artwork(&dir, without), ArtworkStatus::NoEntry);

        let _ = fs::remove_dir_all(&dir);
    }
}
