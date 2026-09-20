//! Deciding what is a game, what is noise, and how sure we are.
//!
//! The governing rule is that the scanner is never silently wrong. Every directory it
//! looks at produces a verdict *and* an itemised list of the signals behind it, and
//! anything it rejects still gets a row in the database so the user can see it and
//! overrule it. A game quietly dropped is far worse than a folder shown for review.
//!
//! Hard negatives are evaluated first and short-circuit. They are written to be
//! precise rather than broad: an OS installer image and a game archive look similar by
//! size and extension, so the exclusions lean on names and structural markers that
//! only ever appear on one side.

use crate::scan::archive_set::{ArchiveSet, FileEntry};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::sync::LazyLock;

/// What a catalogued item physically is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemKind {
    /// One or more archive volumes holding a game.
    ArchiveSet,
    /// An extracted / installed game directory.
    InstalledGame,
    /// A disc image.
    Iso,
    /// Photos, video, documents.
    Media,
    /// A software project, including the user's own game-engine projects.
    DevProject,
    /// A tool, firmware image or installer that is not a game.
    Utility,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Game,
    NeedsReview,
    NotGame,
}

/// One scored signal. Stored as `reasons_json` and rendered verbatim in the review UI
/// so a verdict is always explainable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Reason {
    pub rule: String,
    pub delta: f32,
    pub note: String,
}

impl Reason {
    fn new(rule: &str, delta: f32, note: impl Into<String>) -> Self {
        Self { rule: rule.to_string(), delta, note: note.into() }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Classification {
    pub verdict: Verdict,
    pub kind: ItemKind,
    pub confidence: f32,
    pub reasons: Vec<Reason>,
}

/// Tunables the user can change in Settings. Defaults are deliberately conservative:
/// it is cheaper to review a folder than to lose one.
#[derive(Debug, Clone, Copy)]
pub struct ClassifyConfig {
    /// Below this size, something with no engine markers is not treated as a game.
    /// Indie games can be tiny, which is exactly why this is adjustable.
    pub min_game_bytes: u64,
    pub game_threshold: f32,
    pub review_threshold: f32,
}

impl Default for ClassifyConfig {
    fn default() -> Self {
        Self {
            min_game_bytes: 200 * 1024 * 1024,
            game_threshold: 0.75,
            review_threshold: 0.40,
        }
    }
}

/// Everything the classifier needs about one directory. Gathered by the walker so the
/// classifier itself stays pure and testable.
#[derive(Debug, Clone, Default)]
pub struct DirFacts {
    pub name: String,
    pub rel_path: String,
    /// Files directly inside this directory.
    pub files: Vec<FileEntry>,
    /// Immediate subdirectory names.
    pub subdirs: Vec<String>,
    /// Recursive size and count.
    pub total_bytes: u64,
    pub total_files: u64,
    /// Lowercased, `/`-separated relative paths of files found beneath this directory,
    /// depth-limited by the walker. Engine detection reads these.
    pub marker_paths: Vec<String>,
    /// Archive sets found among `files`.
    pub archive_sets: Vec<ArchiveSet>,
    /// Name of the parent directory, lowercased. Some exclusions key on it.
    pub parent_name: String,
}

impl DirFacts {
    fn has_file(&self, predicate: impl Fn(&str) -> bool) -> bool {
        self.files.iter().any(|f| predicate(&f.name.to_lowercase()))
    }
    fn has_subdir(&self, name: &str) -> bool {
        self.subdirs.iter().any(|d| d.eq_ignore_ascii_case(name))
    }
    fn marker_matches(&self, predicate: impl Fn(&str) -> bool) -> bool {
        self.marker_paths.iter().any(|p| predicate(p))
    }
}

// ---------------------------------------------------------------------------
// Hard negatives
// ---------------------------------------------------------------------------

/// Operating-system and distro images. Matched on the *name*, deliberately before any
/// release-tag stripping, so `Windows 7 ... [FileCR].iso` is caught despite carrying a
/// bracket tag that would otherwise read like a repack.
static RE_OS_IMAGE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(win(dows)?[ _.-]?(7|8|8\.1|10|11|xp|vista|server)|win\d+_\d+h\d|winpe|win[ _.-]?aio|\baio\b|zorin|ubuntu|debian|fedora|linux[ _-]?mint|kali|manjaro|arch[ _-]?linux|pop[ _!-]?os|elementary[ _-]?os|centos|opensuse|macos|hiren|bootable)\b",
    )
    .unwrap()
});

static RE_DISK_IMAGE_EXT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\.(iso|img|wim|esd|vhdx?|dmg)$").unwrap());

/// Tools, firmware and installers. Only applied under a size ceiling, because the
/// words are generic enough to appear in real game titles.
static RE_UTILITY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(testdisk|winaio|rufus|ventoy|imgburn|driverpack|driver[ _-]?booster|firmware|ps3hen|ps3xploit|ps[34][ _-]?hen|bios|activator|\bkms\b|office[ _-]?20\d\d|adobe|photoshop|ccleaner|winrar|7-?zip|nero|daemon[ _-]?tools)\b",
    )
    .unwrap()
});

/// Directory names that mean "this holds OS images", used to catch generically-named
/// `.iso` files sitting inside them.
const OS_PARENT_DIRS: &[&str] = &["windows", "linux", "os", "iso", "isos", "bootable", "distro"];

/// Steam subdirectories under `steamapps\common` that are not games.
static RE_STEAM_NON_GAME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(steamworks shared|steam controller configs|steamvr|proton.*|steamlinuxruntime.*)$")
        .unwrap()
});

const MEDIA_EXTS: &[&str] = &[
    "mov", "mp4", "avi", "mkv", "thm", "jpg", "jpeg", "png", "heic", "gif", "mp3", "wav",
    "flac", "m4a", "docx", "doc", "pdf", "xlsx", "pptx", "txt", "epub",
];

/// Files that identify a source project rather than a shipped game.
const DEV_PROJECT_FILES: &[&str] = &[
    ".uproject", ".sln", ".csproj", ".vcxproj", ".pyproj", "cargo.toml", "package.json",
    "pom.xml", "build.gradle", "cmakelists.txt", "makefile", "go.mod", "requirements.txt",
];

/// Directories that only ever exist in a working tree, never in a shipped build.
const DEV_PROJECT_DIRS: &[&str] = &[
    ".git", ".vs", ".idea", "node_modules", "derivedatacache", "deriveddatacache",
    "intermediate", "__pycache__", ".venv", "obj",
];

/// Apply the hard negatives. `Some` short-circuits all further scoring.
fn hard_negative(f: &DirFacts, _cfg: &ClassifyConfig) -> Option<Classification> {
    // 1. Source projects. A shipped Unreal game carries `Engine/Binaries` but never a
    //    `.uproject`, and never `Intermediate/` or `DerivedDataCache/` — those exist
    //    only in a working tree. That asymmetry is what makes this safe to apply
    //    before the positive engine markers.
    let project_file = DEV_PROJECT_FILES.iter().find(|ext| {
        f.has_file(|n| {
            if ext.starts_with('.') { n.ends_with(*ext) } else { n == **ext }
        })
    });
    let project_dir = DEV_PROJECT_DIRS
        .iter()
        .find(|d| f.has_subdir(d));
    if let Some(hit) = project_file.or(project_dir) {
        return Some(Classification {
            verdict: Verdict::NotGame,
            kind: ItemKind::DevProject,
            confidence: 0.97,
            reasons: vec![Reason::new(
                "dev_project",
                -0.97,
                format!("source project marker '{hit}' — a shipped game never contains one"),
            )],
        });
    }
    // Unity source projects: Assets/ beside ProjectSettings/.
    if f.has_subdir("Assets") && f.has_subdir("ProjectSettings") {
        return Some(Classification {
            verdict: Verdict::NotGame,
            kind: ItemKind::DevProject,
            confidence: 0.95,
            reasons: vec![Reason::new(
                "dev_project",
                -0.95,
                "Unity source project (Assets/ + ProjectSettings/)",
            )],
        });
    }

    // 2. Steam scaffolding directories that sit alongside real games.
    if f.parent_name == "common" && RE_STEAM_NON_GAME.is_match(&f.name) {
        return Some(Classification {
            verdict: Verdict::NotGame,
            kind: ItemKind::Utility,
            confidence: 0.92,
            reasons: vec![Reason::new(
                "steam_scaffolding",
                -0.92,
                format!("'{}' is Steam support content, not a game", f.name),
            )],
        });
    }

    None
}

/// Hard negatives for a standalone *file* (an archive or image not inside a folder of
/// its own).
fn hard_negative_file(name: &str, size: u64, parent: &str, cfg: &ClassifyConfig) -> Option<Classification> {
    let lower = name.to_lowercase();

    // OS and distro images.
    if RE_DISK_IMAGE_EXT.is_match(&lower) {
        if RE_OS_IMAGE.is_match(&lower) {
            return Some(Classification {
                verdict: Verdict::NotGame,
                kind: ItemKind::Iso,
                confidence: 0.95,
                reasons: vec![Reason::new(
                    "os_image",
                    -0.95,
                    "operating-system or distro image (matched before tag stripping)",
                )],
            });
        }
        if OS_PARENT_DIRS.contains(&parent) {
            return Some(Classification {
                verdict: Verdict::NotGame,
                kind: ItemKind::Iso,
                confidence: 0.85,
                reasons: vec![Reason::new(
                    "os_image_by_location",
                    -0.85,
                    format!("disc image inside a '{parent}' folder"),
                )],
            });
        }
    }

    // Tools and firmware, under a size ceiling so a real game with a generic word in
    // its title is not caught.
    if RE_UTILITY.is_match(&lower) && size < 2 * 1024 * 1024 * 1024 {
        return Some(Classification {
            verdict: Verdict::NotGame,
            kind: ItemKind::Utility,
            confidence: 0.80,
            reasons: vec![Reason::new(
                "utility_or_firmware",
                -0.80,
                "tool/firmware name pattern under the 2 GB ceiling",
            )],
        });
    }

    // Size floor. Applied last so engine-bearing folders never reach it.
    if size < cfg.min_game_bytes {
        return Some(Classification {
            verdict: Verdict::NotGame,
            kind: ItemKind::Unknown,
            confidence: 0.70,
            reasons: vec![Reason::new(
                "below_size_floor",
                -0.70,
                format!(
                    "{} is below the {} minimum and carries no game markers",
                    human_bytes(size),
                    human_bytes(cfg.min_game_bytes)
                ),
            )],
        });
    }

    None
}

// ---------------------------------------------------------------------------
// Positive signals
// ---------------------------------------------------------------------------

/// Engine and storefront markers for an installed game. Additive.
fn installed_game_markers(f: &DirFacts) -> Vec<Reason> {
    let mut r = Vec::new();
    let path_lower = f.rel_path.to_lowercase().replace('\\', "/");

    if path_lower.contains("steamapps/common/") {
        r.push(Reason::new("steam_library", 0.60, "inside steamapps/common"));
    }
    if f.marker_matches(|p| p.ends_with("-win64-shipping.exe"))
        || f.marker_matches(|p| p.contains("engine/binaries/win64/"))
    {
        r.push(Reason::new("unreal_engine", 0.45, "Unreal Engine shipping build"));
    }
    if f.has_file(|n| n == "unityplayer.dll")
        || f.marker_matches(|p| p.ends_with("_data/globalgamemanagers"))
        || f.marker_matches(|p| p.ends_with("_data/resources.assets"))
    {
        r.push(Reason::new("unity_engine", 0.40, "Unity runtime layout"));
    }
    if f.has_file(|n| n.starts_with("goggame-")) {
        r.push(Reason::new("gog_install", 0.40, "GOG install manifest"));
    }
    if f.marker_matches(|p| p.ends_with("steam_api64.dll") || p.ends_with("steam_api.dll")) {
        r.push(Reason::new("steam_api", 0.35, "Steamworks API library present"));
    }
    if f.marker_matches(|p| p.contains("content/paks/") && p.ends_with(".pak")) {
        r.push(Reason::new("ue_paks", 0.35, "Unreal .pak content"));
    }
    if f.has_subdir("_CommonRedist") || f.has_subdir("Redist") || f.has_subdir("DirectX") {
        r.push(Reason::new("redist_folder", 0.20, "bundled redistributables"));
    }
    if f.marker_matches(|p| {
        p.ends_with("xlive.dll") || p.ends_with("galaxy64.dll") || p.ends_with("eossdk-win64-shipping.dll")
    }) {
        r.push(Reason::new("platform_sdk", 0.20, "games-for-windows / GOG / EOS SDK"));
    }
    if f.has_file(|n| n.starts_with("unins") && n.ends_with(".exe")) {
        r.push(Reason::new("uninstaller", 0.15, "installer-generated uninstaller"));
    }
    r
}

/// Signals that an archive set holds a game.
fn archive_signals(sets: &[ArchiveSet], title: &crate::scan::title::CleanedTitle, total: u64) -> Vec<Reason> {
    let mut r = Vec::new();

    if let Some(rp) = &title.repacker {
        r.push(Reason::new("repacker_tag", 0.30, format!("repacker '{rp}'")));
    } else if let Some(g) = &title.release_group {
        r.push(Reason::new("release_group", 0.30, format!("release group '{g}'")));
    }
    if total >= 2 * 1024 * 1024 * 1024 {
        r.push(Reason::new("large_archive", 0.25, format!("{} archive", human_bytes(total))));
    }
    // Past roughly 15 GB the only things stored as a single archive are games and disc
    // images, and images are excluded by name before scoring ever runs.
    if total >= 15 * 1024 * 1024 * 1024 {
        r.push(Reason::new("very_large_archive", 0.10, "well beyond non-game archive sizes"));
    }
    if let Some(set) = sets.first() {
        if set.is_multipart() && set.complete {
            // Splitting into fixed-size volumes is something people do to distribute
            // games, not to store documents or media. A complete, uniformly-sized set
            // is therefore strong evidence in its own right.
            r.push(Reason::new(
                "complete_multipart_set",
                0.20,
                format!("{} volumes, contiguous and uniformly sized", set.parts.len()),
            ));
        }
        if let Some(reason) = &set.incomplete_reason {
            r.push(Reason::new("incomplete_set", -0.20, reason.clone()));
        }
    }
    r
}

// ---------------------------------------------------------------------------
// Entry points
// ---------------------------------------------------------------------------

/// Classify a directory.
pub fn classify_dir(f: &DirFacts, cfg: &ClassifyConfig) -> Classification {
    if let Some(neg) = hard_negative(f, cfg) {
        return neg;
    }

    let mut reasons = Vec::new();

    // Media dumps: judged by share of bytes, not file count, so one stray document in
    // a game folder cannot tip it over.
    let media_bytes: u64 = f
        .files
        .iter()
        .filter(|x| {
            x.name
                .rsplit('.')
                .next()
                .is_some_and(|e| MEDIA_EXTS.contains(&e.to_lowercase().as_str()))
        })
        .map(|x| x.size_bytes)
        .sum();
    let immediate_bytes: u64 = f.files.iter().map(|x| x.size_bytes).sum();
    if immediate_bytes > 0 && f.subdirs.is_empty() {
        let share = media_bytes as f64 / immediate_bytes as f64;
        if share >= 0.70 {
            return Classification {
                verdict: Verdict::NotGame,
                kind: ItemKind::Media,
                confidence: 0.88,
                reasons: vec![Reason::new(
                    "media_directory",
                    -0.88,
                    format!("{:.0}% of bytes are media or documents", share * 100.0),
                )],
            };
        }
    }

    let markers = installed_game_markers(f);
    let marker_score: f32 = markers.iter().map(|m| m.delta).sum();
    reasons.extend(markers);

    // The single most important structural rule: a directory whose entire content is
    // one archive set, with no meaningful subdirectories, *is* the game. This is the
    // Assassins Creed Mirage case — five volumes, no executable anywhere — which every
    // exe-hunting scanner misses completely.
    let archive_only = !f.archive_sets.is_empty()
        && f.subdirs.is_empty()
        && f.files.len()
            == f.archive_sets.iter().map(|s| s.parts.len()).sum::<usize>();

    if archive_only && f.archive_sets.len() == 1 {
        let set = &f.archive_sets[0];
        // The contents still have to clear the file-level exclusions. A folder holding
        // nothing but `Win11_25H2_English_x64.iso` is an ISO folder, not a game —
        // without this the archive-only rule below would happily catalogue it.
        if let Some(neg) = hard_negative_file(
            &set.parts[0].file_name,
            set.total_bytes,
            &f.name.to_lowercase(),
            cfg,
        ) {
            return neg;
        }
        let title = crate::scan::title::clean(&f.name, false);
        reasons.push(Reason::new(
            "archive_only_folder",
            0.15,
            "folder contains exactly one archive set and nothing else",
        ));
        reasons.extend(archive_signals(&f.archive_sets, &title, set.total_bytes));
        if set.total_bytes >= 2 * 1024 * 1024 * 1024 {
            // A multi-gigabyte archive in a folder named after it is a game with very
            // high probability; nothing else is stored that way.
            reasons.push(Reason::new(
                "named_archive_folder",
                0.30,
                format!("folder '{}' names a {} archive", f.name, human_bytes(set.total_bytes)),
            ));
        }
        return finish(ItemKind::ArchiveSet, reasons, cfg, set.total_bytes, 0.0);
    }

    if marker_score > 0.0 {
        return finish(ItemKind::InstalledGame, reasons, cfg, f.total_bytes, marker_score);
    }

    // Nothing positive and nothing disqualifying: below the floor it is noise, above it
    // the user decides.
    if f.total_bytes < cfg.min_game_bytes {
        reasons.push(Reason::new(
            "below_size_floor",
            -0.70,
            format!("{} with no game markers", human_bytes(f.total_bytes)),
        ));
    }
    finish(ItemKind::Unknown, reasons, cfg, f.total_bytes, 0.0)
}

/// Classify a standalone archive file that is not alone in its folder — the
/// `E:\Game\Sekiro ... .rar` case, where the parent directory is a shelf holding
/// several unrelated games.
pub fn classify_archive_file(
    set: &ArchiveSet,
    parent_name: &str,
    cfg: &ClassifyConfig,
) -> Classification {
    let primary = &set.parts[0].file_name;
    if let Some(neg) =
        hard_negative_file(primary, set.total_bytes, &parent_name.to_lowercase(), cfg)
    {
        return neg;
    }
    let title = crate::scan::title::clean(primary, true);
    let reasons = archive_signals(std::slice::from_ref(set), &title, set.total_bytes);
    finish(ItemKind::ArchiveSet, reasons, cfg, set.total_bytes, 0.0)
}

/// Convert accumulated signals into a verdict.
///
/// The base of 0.30 reflects that something large enough to reach this point is more
/// likely than not to be worth showing; the thresholds then decide whether it is shown
/// as a game or queued for review.
fn finish(
    kind: ItemKind,
    reasons: Vec<Reason>,
    cfg: &ClassifyConfig,
    total_bytes: u64,
    _marker_score: f32,
) -> Classification {
    let base = 0.30_f32;
    let sum: f32 = reasons.iter().map(|r| r.delta).sum();
    let confidence = (base + sum).clamp(0.0, 1.0);

    let verdict = if confidence >= cfg.game_threshold {
        Verdict::Game
    } else if confidence >= cfg.review_threshold {
        Verdict::NeedsReview
    } else {
        Verdict::NotGame
    };

    let _ = total_bytes;
    Classification { verdict, kind, confidence, reasons }
}

fn human_bytes(b: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = b as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 { format!("{b} B") } else { format!("{v:.1} {}", UNITS[i]) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::archive_set::group_directory;

    const MB: u64 = 1024 * 1024;
    const GB: u64 = 1024 * MB;

    fn facts(name: &str) -> DirFacts {
        DirFacts { name: name.to_string(), ..Default::default() }
    }

    /// E:\Game\Assassins Creed Mirage — five RAR volumes, 41 GB, no executable.
    /// The headline case: every exe-scanning library manager finds nothing here.
    #[test]
    fn folder_of_only_rar_volumes_is_a_game() {
        let files = vec![
            FileEntry::new("Assassins Creed Mirage.part1.rar", 8704 * MB),
            FileEntry::new("Assassins Creed Mirage.part2.rar", 8704 * MB),
            FileEntry::new("Assassins Creed Mirage.part3.rar", 8704 * MB),
            FileEntry::new("Assassins Creed Mirage.part4.rar", 8704 * MB),
            FileEntry::new("Assassins Creed Mirage.part5.rar", 7330 * MB),
        ];
        let sets = group_directory(&files).sets;
        let f = DirFacts {
            name: "Assassins Creed Mirage".into(),
            rel_path: "Game\\Assassins Creed Mirage".into(),
            total_bytes: files.iter().map(|x| x.size_bytes).sum(),
            total_files: 5,
            files,
            archive_sets: sets,
            parent_name: "game".into(),
            ..Default::default()
        };
        let c = classify_dir(&f, &ClassifyConfig::default());
        assert_eq!(c.verdict, Verdict::Game, "{c:#?}");
        assert_eq!(c.kind, ItemKind::ArchiveSet);
        assert!(c.confidence >= 0.75, "confidence {} too low", c.confidence);
    }

    /// E:\first game\Clozr — the user's own Unreal project. Must never be catalogued
    /// as a game, despite living on the games drive and being large.
    #[test]
    fn unreal_source_project_is_rejected() {
        let f = DirFacts {
            name: "Clozr".into(),
            files: vec![FileEntry::new("Clozr.uproject", 1024)],
            subdirs: vec![
                "Content".into(),
                "Config".into(),
                "Intermediate".into(),
                "Saved".into(),
                "DerivedDataCache".into(),
            ],
            total_bytes: 8 * GB,
            ..Default::default()
        };
        let c = classify_dir(&f, &ClassifyConfig::default());
        assert_eq!(c.verdict, Verdict::NotGame);
        assert_eq!(c.kind, ItemKind::DevProject);
        assert!(c.reasons[0].note.contains("uproject"));
    }

    /// A shipped Unreal game has Engine/Binaries but no .uproject — the asymmetry the
    /// dev-project rule depends on. Guards against over-broad exclusion.
    #[test]
    fn shipped_unreal_game_is_not_mistaken_for_a_source_project() {
        let f = DirFacts {
            name: "Ghost of Tsushima DIRECTORS CUT".into(),
            files: vec![FileEntry::new("GhostOfTsushima.exe", 80 * MB)],
            subdirs: vec!["Engine".into(), "Content".into()],
            marker_paths: vec![
                "engine/binaries/win64/ghostoftsushima-win64-shipping.exe".into(),
                "content/paks/pakchunk0.pak".into(),
            ],
            total_bytes: 60 * GB,
            ..Default::default()
        };
        let c = classify_dir(&f, &ClassifyConfig::default());
        assert_eq!(c.verdict, Verdict::Game, "{c:#?}");
        assert_eq!(c.kind, ItemKind::InstalledGame);
    }

    #[test]
    fn dotnet_and_node_projects_are_rejected() {
        for marker in ["GameZone.csproj", "package.json", "Cargo.toml"] {
            let f = DirFacts {
                name: "proj".into(),
                files: vec![FileEntry::new(marker, 2048)],
                total_bytes: 3 * GB,
                ..Default::default()
            };
            assert_eq!(
                classify_dir(&f, &ClassifyConfig::default()).kind,
                ItemKind::DevProject,
                "{marker} should be a dev project"
            );
        }
    }

    /// The bracket tag on this real filename would read like a repack to a naive
    /// tag-stripper, so the OS rule has to run first.
    #[test]
    fn windows_iso_with_a_release_tag_is_still_rejected() {
        let c = hard_negative_file(
            "Windows 7 Professional SP1 Multilingual Preactivated January 2026 [FileCR].iso",
            5 * GB,
            "windows",
            &ClassifyConfig::default(),
        )
        .expect("should be excluded");
        assert_eq!(c.kind, ItemKind::Iso);
        assert!(c.confidence >= 0.9);
    }

    #[test]
    fn linux_iso_is_rejected() {
        let c = hard_negative_file("Zorin-OS-18.1-Core-64-bit.iso", 4 * GB, "linux", &ClassifyConfig::default())
            .expect("should be excluded");
        assert_eq!(c.kind, ItemKind::Iso);
    }

    /// A generically-named image inside a folder called "windows" is still an OS image.
    #[test]
    fn generic_iso_inside_an_os_folder_is_rejected_by_location() {
        let c = hard_negative_file("Win 7 X64.iso", 3 * GB, "windows", &ClassifyConfig::default())
            .expect("should be excluded");
        assert_eq!(c.kind, ItemKind::Iso);
    }

    #[test]
    fn tools_and_firmware_are_rejected() {
        let cfg = ClassifyConfig::default();
        for (name, size) in [
            ("testdisk-7.3-WIP.win.zip", 29 * MB),
            ("PS3HEN_3.5.0_4.92_FW.zip", 22 * MB),
            ("WinAIO Maker Professional.7z", 3 * MB),
        ] {
            let c = hard_negative_file(name, size, "", &cfg).expect("should be excluded");
            assert_eq!(c.verdict, Verdict::NotGame, "{name}");
        }
    }

    /// Small personal archives on the games drive must not become library entries.
    #[test]
    fn small_archives_fall_below_the_size_floor() {
        let cfg = ClassifyConfig::default();
        let c = hard_negative_file("خاص بي فقط.rar", 74 * MB, "", &cfg).expect("below floor");
        assert_eq!(c.verdict, Verdict::NotGame);
        assert_eq!(c.reasons[0].rule, "below_size_floor");
    }

    /// The floor is configurable precisely so that small indie games can be kept.
    #[test]
    fn lowering_the_size_floor_admits_small_games() {
        let cfg = ClassifyConfig { min_game_bytes: 10 * MB, ..Default::default() };
        assert!(hard_negative_file("Tiny Indie Game.zip", 74 * MB, "", &cfg).is_none());
    }

    /// E:\سفرة إيران — a camera dump. Also checks Arabic names survive the path.
    #[test]
    fn media_directory_is_rejected() {
        let f = DirFacts {
            name: "سفرة إيران".into(),
            files: vec![
                FileEntry::new("MVI_0001.MOV", 900 * MB),
                FileEntry::new("MVI_0001.THM", 2 * MB),
                FileEntry::new("MVI_0002.MOV", 850 * MB),
            ],
            total_bytes: 1752 * MB,
            ..Default::default()
        };
        let c = classify_dir(&f, &ClassifyConfig::default());
        assert_eq!(c.verdict, Verdict::NotGame);
        assert_eq!(c.kind, ItemKind::Media);
    }

    /// E:\Game\Sekiro ... .rar and Mad Max — archives sharing a shelf directory.
    #[test]
    fn standalone_repack_archives_are_games() {
        let cfg = ClassifyConfig::default();
        let files = vec![
            FileEntry::new("Sekiro Shadows Die Twice by xatab.rar", 12_270 * MB),
            FileEntry::new("Mad Max  - [DODI Repack].zip", 3_890 * MB),
        ];
        let sets = group_directory(&files).sets;
        assert_eq!(sets.len(), 2);
        for s in &sets {
            let c = classify_archive_file(s, "Game", &cfg);
            assert_eq!(c.verdict, Verdict::Game, "{} -> {c:#?}", s.stem);
        }
    }

    #[test]
    fn steam_support_folders_are_rejected() {
        for name in ["Steamworks Shared", "Steam Controller Configs"] {
            let f = DirFacts {
                name: name.into(),
                parent_name: "common".into(),
                total_bytes: 500 * MB,
                ..Default::default()
            };
            assert_eq!(classify_dir(&f, &ClassifyConfig::default()).verdict, Verdict::NotGame);
        }
    }

    /// An incomplete set is still surfaced, but flagged and scored down rather than
    /// presented as a healthy game.
    #[test]
    fn incomplete_set_is_penalised_and_explained() {
        let files = vec![
            FileEntry::new("Big Game.part1.rar", 8 * GB),
            FileEntry::new("Big Game.part3.rar", 8 * GB),
        ];
        let sets = group_directory(&files).sets;
        let c = classify_archive_file(&sets[0], "Game", &ClassifyConfig::default());
        assert!(c.reasons.iter().any(|r| r.rule == "incomplete_set"));
        assert!(c.reasons.iter().any(|r| r.note.contains("part 2")));
    }

    /// Every verdict must be explainable — the review UI renders these directly.
    #[test]
    fn every_classification_carries_reasons() {
        let f = facts("Whatever");
        let c = classify_dir(&f, &ClassifyConfig::default());
        assert!(!c.reasons.is_empty(), "a verdict with no reasons is not reviewable");
    }
}
