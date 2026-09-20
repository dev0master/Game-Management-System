//! Turning a flat pile of packages into the games a person recognises.
//!
//! The drive this was written against holds 71 `.pkg` files loose in its root, which
//! are really 31 games: a base package plus its updates, its DLC, occasional alternate
//! builds, and split continuation parts. Grouping is by title id read from inside each
//! file, so a broken or missing filename changes nothing.
//!
//! The governing rule mirrors the rest of the scanner: group where the filesystem gives
//! no grouping, and respect the filesystem's grouping where it does. A flat directory
//! of packages gets grouped here; an Xbox game folder is already the unit the user
//! browses and transfers, so it is left alone.

use super::ps4::{self, PkgInfo};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::LazyLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Platform {
    Ps4,
    Xbox360,
    XboxOriginal,
}

/// What one file contributes to its game.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConsoleRole {
    /// Base game (`CATEGORY = gd`).
    Game,
    /// Update or patch (`gp`).
    Update,
    /// Additional content (`ac`).
    Dlc,
    /// A continuation part of a split package — carries no header of its own.
    Part,
    /// A sibling image used as the cover.
    Cover,
    Unknown,
}

impl ConsoleRole {
    pub fn as_db_role(self) -> &'static str {
        match self {
            ConsoleRole::Game => "pkg_game",
            ConsoleRole::Update => "pkg_update",
            ConsoleRole::Dlc => "pkg_dlc",
            ConsoleRole::Part => "pkg_part",
            ConsoleRole::Cover => "cover",
            ConsoleRole::Unknown => "data",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeStatus {
    Ok,
    /// Has no package header. Either a continuation part or a damaged file.
    NotPkg,
    /// Confirmed continuation of a split set.
    Continuation,
    Unreadable,
}

/// One file after probing, before grouping.
#[derive(Debug, Clone)]
pub struct ProbedFile {
    pub name: String,
    pub size_bytes: u64,
    pub mtime_unix: i64,
    pub status: ProbeStatus,
    pub info: Option<PkgInfo>,
}

impl ProbedFile {
    pub fn new(name: impl Into<String>, size_bytes: u64, info: Option<PkgInfo>) -> Self {
        let status = if info.is_some() { ProbeStatus::Ok } else { ProbeStatus::NotPkg };
        Self { name: name.into(), size_bytes, mtime_unix: 0, status, info }
    }
}

/// A member of a grouped game.
#[derive(Debug, Clone)]
pub struct GroupFile {
    pub name: String,
    pub size_bytes: u64,
    pub mtime_unix: i64,
    pub role: ConsoleRole,
    /// Position within a split package, 1-based. `None` for a whole file.
    pub part_index: Option<u32>,
    pub content_id: Option<String>,
    pub app_ver: Option<String>,
    pub system_ver: Option<u32>,
    pub declared_bytes: Option<u64>,
    pub status: ProbeStatus,
}

/// One game: everything sharing a title id.
#[derive(Debug, Clone)]
pub struct ConsoleGroup {
    pub platform: Platform,
    pub title_id: String,
    /// Name from the primary package, verbatim.
    pub display_title: String,
    pub files: Vec<GroupFile>,
    /// Index into `files` of the package that defines this game's identity.
    pub primary: usize,
    pub total_bytes: u64,
    /// Lowest firmware that yields a playable base game, when known.
    pub min_system_ver: Option<u32>,
    /// For an add-on with no base of its own: the game it belongs to.
    pub linked_base: Option<String>,
    pub link_reason: Option<String>,
    /// True when the group has no base game — only updates or DLC.
    pub is_orphan: bool,
}

impl ConsoleGroup {
    pub fn primary_file(&self) -> &GroupFile {
        // `primary` is always a valid index: it is chosen from `files`, which is never
        // empty for a constructed group.
        self.files.get(self.primary).unwrap_or(&self.files[0])
    }

    pub fn count_role(&self, role: ConsoleRole) -> usize {
        self.files.iter().filter(|f| f.role == role).count()
    }

    /// Whether a base game here can install on `firmware`.
    ///
    /// A file with no stated requirement counts as installable: DLC packages carry no
    /// `SYSTEM_VER`, and absence must never read as "blocked".
    pub fn playable_on(&self, firmware: u32) -> bool {
        self.files
            .iter()
            .filter(|f| f.role == ConsoleRole::Game)
            .any(|f| f.system_ver.is_none_or(|v| v <= firmware))
    }
}

/// `Name_1.pkg`, `Name_2.pkg` — a continuation of `Name_0.pkg`.
static RE_SPLIT_PART: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^(?P<stem>.+)_(?P<n>[1-9]\d*)\.pkg$").unwrap());

/// `Name_0.pkg` — the head of a split set.
static RE_SPLIT_HEAD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^(?P<stem>.+)_0\.pkg$").unwrap());

/// Any `CUSAnnnnn` appearing in a filename.
static RE_CUSA: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)CUSA\d{5}").unwrap());

fn strip_ext(name: &str) -> &str {
    name.rsplit_once('.').map(|(s, _)| s).unwrap_or(name)
}

fn role_of(info: &PkgInfo) -> ConsoleRole {
    match info.sfo.category.as_deref() {
        Some("gd") => ConsoleRole::Game,
        Some("gp") => ConsoleRole::Update,
        Some("ac") => ConsoleRole::Dlc,
        // No category: fall back to the header's content type.
        _ => match info.content_type {
            ps4::CONTENT_TYPE_ADDON => ConsoleRole::Dlc,
            ps4::CONTENT_TYPE_GAME => ConsoleRole::Game,
            _ => ConsoleRole::Unknown,
        },
    }
}

/// Result of grouping one directory of packages.
#[derive(Debug, Default)]
pub struct GroupResult {
    pub groups: Vec<ConsoleGroup>,
    /// Files that belong to no group — damaged packages, chiefly.
    pub ungrouped: Vec<ProbedFile>,
}

/// Group probed packages, attaching sibling images as covers.
///
/// `covers` is `(filename, size)` for every image in the same directory; one is
/// attached to a group when its name without extension matches a member package.
pub fn group_ps4(probed: &[ProbedFile], covers: &[(String, u64)]) -> GroupResult {
    // Pass 1: fold split continuations into the head package that owns them.
    //
    // A `_N` file with no header is a continuation when a `_0` sibling parsed cleanly.
    // Without this the three SOMA parts read as one game and two corrupt files.
    let heads: Vec<&ProbedFile> = probed
        .iter()
        .filter(|p| p.info.is_some() && RE_SPLIT_HEAD.is_match(&p.name))
        .collect();

    let mut continuation_of: BTreeMap<usize, (String, u32)> = BTreeMap::new();
    for (i, p) in probed.iter().enumerate() {
        if p.info.is_some() {
            continue;
        }
        let Some(c) = RE_SPLIT_PART.captures(&p.name) else { continue };
        let stem = c.name("stem").map(|m| m.as_str()).unwrap_or("");
        let n: u32 = c.name("n").and_then(|m| m.as_str().parse().ok()).unwrap_or(0);
        let head_name = format!("{stem}_0.pkg");
        if let Some(h) = heads.iter().find(|h| h.name.eq_ignore_ascii_case(&head_name)) {
            continuation_of.insert(i, (h.name.clone(), n));
        }
    }

    // Pass 2: bucket by title id.
    let mut buckets: BTreeMap<String, Vec<GroupFile>> = BTreeMap::new();
    let mut ungrouped: Vec<ProbedFile> = Vec::new();
    // head filename -> title id, so continuations can join their head's bucket.
    let mut head_title: BTreeMap<String, String> = BTreeMap::new();

    for p in probed.iter() {
        let Some(info) = &p.info else { continue };
        let Some(tid) = info.title_id() else { continue };
        head_title.insert(p.name.clone(), tid.clone());
        buckets.entry(tid).or_default().push(GroupFile {
            name: p.name.clone(),
            size_bytes: p.size_bytes,
            mtime_unix: p.mtime_unix,
            role: role_of(info),
            part_index: RE_SPLIT_HEAD.is_match(&p.name).then_some(1),
            content_id: Some(info.content_id.clone()),
            app_ver: info.sfo.app_ver.clone(),
            system_ver: info.sfo.system_ver,
            declared_bytes: Some(info.declared_bytes),
            status: ProbeStatus::Ok,
        });
    }

    for (i, p) in probed.iter().enumerate() {
        if p.info.is_some() {
            continue;
        }
        match continuation_of.get(&i) {
            Some((head, n)) => {
                if let Some(tid) = head_title.get(head) {
                    buckets.entry(tid.clone()).or_default().push(GroupFile {
                        name: p.name.clone(),
                        size_bytes: p.size_bytes,
                        mtime_unix: p.mtime_unix,
                        role: ConsoleRole::Part,
                        part_index: Some(n + 1),
                        content_id: None,
                        app_ver: None,
                        system_ver: None,
                        declared_bytes: None,
                        status: ProbeStatus::Continuation,
                    });
                }
            }
            // No header and no head to belong to: a damaged file. Surfaced, never dropped.
            None => ungrouped.push(p.clone()),
        }
    }

    // Pass 3: build the groups.
    let mut groups: Vec<ConsoleGroup> = Vec::new();
    for (tid, mut files) in buckets {
        if files.is_empty() {
            continue;
        }
        // Covers attach by matching a member's name without its extension.
        for (cover_name, cover_size) in covers {
            let stem = strip_ext(cover_name);
            if files.iter().any(|f| strip_ext(&f.name).eq_ignore_ascii_case(stem)) {
                files.push(GroupFile {
                    name: cover_name.clone(),
                    size_bytes: *cover_size,
                    mtime_unix: 0,
                    role: ConsoleRole::Cover,
                    part_index: None,
                    content_id: None,
                    app_ver: None,
                    system_ver: None,
                    declared_bytes: None,
                    status: ProbeStatus::Ok,
                });
            }
        }

        let primary = pick_primary(&files);
        let display_title = files
            .get(primary)
            .and_then(|_| primary_title(probed, &files, primary))
            .unwrap_or_else(|| tid.clone());

        let has_base = files.iter().any(|f| f.role == ConsoleRole::Game);
        let min_system_ver = files
            .iter()
            .filter(|f| f.role == ConsoleRole::Game)
            .filter_map(|f| f.system_ver)
            .min();

        groups.push(ConsoleGroup {
            platform: Platform::Ps4,
            title_id: tid,
            display_title,
            total_bytes: files.iter().map(|f| f.size_bytes).sum(),
            min_system_ver,
            primary,
            files,
            linked_base: None,
            link_reason: None,
            is_orphan: !has_base,
        });
    }

    link_orphans(&mut groups);
    GroupResult { groups, ungrouped }
}

/// The package that defines a group's identity.
///
/// Must be deterministic across rescans, because it supplies the item's path and
/// therefore its identity in the catalogue.
fn pick_primary(files: &[GroupFile]) -> usize {
    let mut best = 0usize;
    let mut best_key: Option<(u8, String, u64, String)> = None;
    for (i, f) in files.iter().enumerate() {
        if f.role == ConsoleRole::Cover || f.role == ConsoleRole::Part {
            continue;
        }
        // Base game first, then update, then anything else.
        let rank = match f.role {
            ConsoleRole::Game => 0u8,
            ConsoleRole::Update => 1,
            _ => 2,
        };
        let key = (
            rank,
            f.app_ver.clone().unwrap_or_default(),
            f.size_bytes,
            f.name.clone(),
        );
        let better = match &best_key {
            None => true,
            Some(cur) => {
                // Lower rank wins; then higher version, larger size, and finally the
                // name, so the choice never depends on directory order.
                (key.0, std::cmp::Reverse((key.1.clone(), key.2)), key.3.clone())
                    < (cur.0, std::cmp::Reverse((cur.1.clone(), cur.2)), cur.3.clone())
            }
        };
        if better {
            best = i;
            best_key = Some(key);
        }
    }
    best
}

fn primary_title(probed: &[ProbedFile], files: &[GroupFile], primary: usize) -> Option<String> {
    let name = &files.get(primary)?.name;
    probed
        .iter()
        .find(|p| &p.name == name)
        .and_then(|p| p.info.as_ref())
        .and_then(|i| i.sfo.title.clone())
        .filter(|t| !t.trim().is_empty())
}

/// Attach add-on-only groups to the game they belong to.
///
/// Sony ships some DLC under its own title id — `Watch Dogs: Legion Bloodline` is
/// CUSA13034 while the game is CUSA13115 — so a strict title-id grouping leaves a
/// zero-byte ghost entry. The filename usually names the real base, which is an exact
/// signal; title similarity is the fallback. Either way the link is recorded with its
/// reason so the user can see and undo it.
fn link_orphans(groups: &mut [ConsoleGroup]) {
    let bases: Vec<(String, String)> = groups
        .iter()
        .filter(|g| !g.is_orphan)
        .map(|g| (g.title_id.clone(), g.display_title.clone()))
        .collect();

    for g in groups.iter_mut().filter(|g| g.is_orphan) {
        // 1. Another known title id written in one of the filenames — exact.
        let mut found = None;
        'outer: for f in &g.files {
            for m in RE_CUSA.find_iter(&f.name) {
                let cand = m.as_str().to_uppercase();
                if cand != g.title_id && bases.iter().any(|(t, _)| *t == cand) {
                    found = Some((cand.clone(), format!("title id {cand} appears in the filename")));
                    break 'outer;
                }
            }
        }

        // 2. Otherwise the closest base title, if it is close enough to be safe.
        if found.is_none() {
            let mut best: Option<(f64, String, String)> = None;
            for (tid, title) in &bases {
                if title.trim().is_empty() {
                    continue;
                }
                let s = rapidfuzz::fuzz::ratio(
                    g.display_title.to_lowercase().chars(),
                    title.to_lowercase().chars(),
                ) * 100.0;
                let prefix = g
                    .display_title
                    .to_lowercase()
                    .starts_with(&title.to_lowercase());
                if (s >= 85.0 || prefix) && best.as_ref().is_none_or(|(bs, _, _)| s > *bs) {
                    best = Some((s, tid.clone(), title.clone()));
                }
            }
            if let Some((s, tid, title)) = best {
                found = Some((tid, format!("title matches \"{title}\" ({s:.0}%)")));
            }
        }

        if let Some((base, reason)) = found {
            g.linked_base = Some(base);
            g.link_reason = Some(reason);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::console::ps4::ParamSfo;

    fn info(cid: &str, cat: &str, tid: &str, title: &str, ver: &str, fw: Option<u32>) -> PkgInfo {
        PkgInfo {
            content_id: cid.into(),
            content_type: if cat == "ac" { ps4::CONTENT_TYPE_ADDON } else { ps4::CONTENT_TYPE_GAME },
            declared_bytes: 0,
            sfo: ParamSfo {
                category: Some(cat.into()),
                title_id: Some(tid.into()),
                title: Some(title.into()),
                app_ver: (!ver.is_empty()).then(|| ver.to_string()),
                system_ver: fw,
                extra: vec![],
            },
        }
    }

    const GB: u64 = 1024 * 1024 * 1024;

    /// Metro Exodus on the real drive: base + 2 DLC + 1 update, four files, one game.
    #[test]
    fn a_game_and_its_updates_and_dlc_become_one_entry() {
        let p = vec![
            ProbedFile::new("Metro Exodus[USA11407].pkg", 48 * GB,
                Some(info("EP4062-CUSA11407_00-METROEXODUS00000", "gd", "CUSA11407", "Metro Exodus", "01.00", Some(0x0600_0000)))),
            ProbedFile::new("Metro Exodus[USA11407][Update v1.09][FIX 6.72].pkg", 19 * GB,
                Some(info("EP4062-CUSA11407_00-METROEXODUS00000", "gp", "CUSA11407", "Metro Exodus", "01.09", Some(0x0600_0000)))),
            ProbedFile::new("Metro Exodus[USA11407][Sams Story DLC].pkg", 6 * GB,
                Some(info("EP4062-CUSA11407_00-METROEXODUSDLC02", "ac", "CUSA11407", "Sam's Story", "", None))),
            ProbedFile::new("Metro Exodus[USA11407][The Two Colonels DLC].pkg", 1 * GB,
                Some(info("EP4062-CUSA11407_00-METROEXODUSDLC01", "ac", "CUSA11407", "The Two Colonels", "", None))),
        ];
        let r = group_ps4(&p, &[]);
        assert_eq!(r.groups.len(), 1, "four files are one game");
        let g = &r.groups[0];
        assert_eq!(g.title_id, "CUSA11407");
        assert_eq!(g.display_title, "Metro Exodus", "the base game names the group");
        assert_eq!(g.count_role(ConsoleRole::Game), 1);
        assert_eq!(g.count_role(ConsoleRole::Update), 1);
        assert_eq!(g.count_role(ConsoleRole::Dlc), 2);
        assert_eq!(g.total_bytes, 74 * GB);
        assert!(!g.is_orphan);
    }

    /// SOMA: `_0` parses, `_1` and `_2` are raw continuations. One game, three parts —
    /// not one game and two corrupt files.
    #[test]
    fn split_continuation_parts_join_their_head_package() {
        let p = vec![
            ProbedFile::new("EP1188-CUSA03200_00-HPL3SOMAGAME2015_0.pkg", 4_294_967_296,
                Some(info("EP1188-CUSA03200_00-HPL3SOMAGAME2015", "gd", "CUSA03200", "SOMA", "01.00", Some(0x0257_0000)))),
            ProbedFile::new("EP1188-CUSA03200_00-HPL3SOMAGAME2015_1.pkg", 4_294_967_296, None),
            ProbedFile::new("EP1188-CUSA03200_00-HPL3SOMAGAME2015_2.pkg", 2_651_783_168, None),
        ];
        let r = group_ps4(&p, &[]);
        assert_eq!(r.groups.len(), 1);
        assert!(r.ungrouped.is_empty(), "continuations are not damaged files");
        let g = &r.groups[0];
        assert_eq!(g.display_title, "SOMA");
        assert_eq!(g.files.len(), 3);
        assert_eq!(g.count_role(ConsoleRole::Part), 2);
        // The three parts sum to the size the head package declares.
        assert_eq!(g.total_bytes, 11_241_717_760);
    }

    /// A headerless file with no `_0` sibling is damage, and must be surfaced.
    #[test]
    fn a_damaged_package_is_reported_not_swallowed() {
        let p = vec![
            ProbedFile::new("Good[CUSA00001].pkg", GB,
                Some(info("EP0000-CUSA00001_00-X", "gd", "CUSA00001", "Good Game", "01.00", None))),
            ProbedFile::new("Truncated copy - نسخة.pkg", 29_919_019_008, None),
        ];
        let r = group_ps4(&p, &[]);
        assert_eq!(r.groups.len(), 1);
        assert_eq!(r.ungrouped.len(), 1);
        assert_eq!(r.ungrouped[0].name, "Truncated copy - نسخة.pkg");
    }

    /// CUSA18740 really does hold three base packages. None may be hidden.
    #[test]
    fn multiple_base_packages_are_all_kept() {
        let p = vec![
            ProbedFile::new("A[GAME].pkg", 80 * GB,
                Some(info("EP0177-CUSA18740_00-X", "gd", "CUSA18740", "eFootball", "01.00", Some(0x0750_0000)))),
            ProbedFile::new("B[GAME].pkg", 80 * GB,
                Some(info("EP0177-CUSA18740_00-X", "gd", "CUSA18740", "eFootball", "01.00", Some(0x0750_0000)))),
            ProbedFile::new("C[PATCH].pkg", 5 * GB,
                Some(info("EP0177-CUSA18740_00-X", "gp", "CUSA18740", "eFootball", "01.02", Some(0x0750_0000)))),
        ];
        let r = group_ps4(&p, &[]);
        assert_eq!(r.groups.len(), 1);
        assert_eq!(r.groups[0].count_role(ConsoleRole::Game), 2, "both builds kept");
        assert_eq!(r.groups[0].total_bytes, 165 * GB);
    }

    /// Watch Dogs: Legion Bloodline ships under its own title id. The base game's id
    /// appears in the filename, which is an exact link.
    #[test]
    fn an_orphan_dlc_links_to_the_base_named_in_its_filename() {
        let p = vec![
            ProbedFile::new("Watch Dogs Legion[CUSA13115].pkg", 40 * GB,
                Some(info("EP0001-CUSA13115_00-WDL", "gd", "CUSA13115", "Watch Dogs®: Legion", "01.00", Some(0x0750_0000)))),
            ProbedFile::new("Watch Dogs Legion[CUSA13115][Bloodline DLC].pkg", 7 * GB,
                Some(info("EP0001-CUSA13034_00-ORWBL", "ac", "CUSA13034", "Watch Dogs®: Legion Bloodline", "", None))),
        ];
        let r = group_ps4(&p, &[]);
        let orphan = r.groups.iter().find(|g| g.title_id == "CUSA13034").expect("orphan group");
        assert!(orphan.is_orphan);
        assert_eq!(orphan.linked_base.as_deref(), Some("CUSA13115"));
        assert!(orphan.link_reason.as_ref().unwrap().contains("filename"), "{:?}", orphan.link_reason);
    }

    /// With no id in the name, a close title still links — but visibly, with a reason.
    #[test]
    fn an_orphan_dlc_falls_back_to_title_similarity() {
        let p = vec![
            ProbedFile::new("base.pkg", 40 * GB,
                Some(info("EP0001-CUSA13115_00-WDL", "gd", "CUSA13115", "Watch Dogs Legion", "01.00", None))),
            ProbedFile::new("extra.pkg", 7 * GB,
                Some(info("EP0001-CUSA13034_00-ORWBL", "ac", "CUSA13034", "Watch Dogs Legion Bloodline", "", None))),
        ];
        let r = group_ps4(&p, &[]);
        let orphan = r.groups.iter().find(|g| g.title_id == "CUSA13034").unwrap();
        assert_eq!(orphan.linked_base.as_deref(), Some("CUSA13115"));
        assert!(orphan.link_reason.as_ref().unwrap().contains("matches"));
    }

    /// An unrelated add-on must stay unlinked rather than be attached to a wrong game.
    #[test]
    fn an_unrelated_orphan_is_left_unlinked() {
        let p = vec![
            ProbedFile::new("base.pkg", 40 * GB,
                Some(info("EP0001-CUSA00001_00-A", "gd", "CUSA00001", "Some Racing Game", "01.00", None))),
            ProbedFile::new("mystery.pkg", GB,
                Some(info("EP0002-CUSA99999_00-B", "ac", "CUSA99999", "Zzz Unrelated Pack", "", None))),
        ];
        let r = group_ps4(&p, &[]);
        let orphan = r.groups.iter().find(|g| g.title_id == "CUSA99999").unwrap();
        assert_eq!(orphan.linked_base, None);
    }

    #[test]
    fn a_sibling_image_is_attached_as_the_cover() {
        let p = vec![ProbedFile::new("Game[CUSA00001].pkg", GB,
            Some(info("EP0000-CUSA00001_00-X", "gd", "CUSA00001", "Game", "01.00", None)))];
        let covers = vec![("Game[CUSA00001].jpg".to_string(), 182 * 1024)];
        let r = group_ps4(&p, &covers);
        assert_eq!(r.groups[0].count_role(ConsoleRole::Cover), 1);
    }

    /// Minecraft on the drive: one base and five updates differing by firmware.
    #[test]
    fn firmware_gating_uses_the_base_packages_requirement() {
        let p = vec![
            ProbedFile::new("mc_v1.pkg", GB,
                Some(info("EP0001-CUSA00265_00-M", "gd", "CUSA00265", "Minecraft", "01.00", Some(0x0170_0000)))),
            ProbedFile::new("mc_v327_12.pkg", 2 * GB,
                Some(info("EP0001-CUSA00265_00-M", "gp", "CUSA00265", "Minecraft", "03.27", Some(0x1200_0000)))),
            ProbedFile::new("mc_v327_backport.pkg", 2 * GB,
                Some(info("EP0001-CUSA00265_00-M", "gp", "CUSA00265", "Minecraft", "03.27", Some(0x0505_0000)))),
        ];
        let g = &group_ps4(&p, &[]).groups[0];
        assert_eq!(g.min_system_ver, Some(0x0170_0000));
        assert!(g.playable_on(0x0900_0000), "the 1.70 base installs on 9.00");
        assert!(!g.playable_on(0x0100_0000), "nothing installs on 1.00");
    }

    /// Primary selection decides the item's identity, so it must not depend on the
    /// order the directory happened to be read in.
    #[test]
    fn primary_selection_is_order_independent() {
        let mk = || {
            vec![
                ProbedFile::new("update.pkg", 5 * GB,
                    Some(info("EP0-CUSA00001_00-X", "gp", "CUSA00001", "Game", "01.09", None))),
                ProbedFile::new("base.pkg", 40 * GB,
                    Some(info("EP0-CUSA00001_00-X", "gd", "CUSA00001", "Game", "01.00", None))),
                ProbedFile::new("dlc.pkg", GB,
                    Some(info("EP0-CUSA00001_00-Y", "ac", "CUSA00001", "Pack", "", None))),
            ]
        };
        let a = group_ps4(&mk(), &[]);
        let mut rev = mk();
        rev.reverse();
        let b = group_ps4(&rev, &[]);
        assert_eq!(a.groups[0].primary_file().name, "base.pkg");
        assert_eq!(b.groups[0].primary_file().name, "base.pkg");
    }

    #[test]
    fn an_empty_directory_produces_nothing() {
        let r = group_ps4(&[], &[]);
        assert!(r.groups.is_empty() && r.ungrouped.is_empty());
    }
}
