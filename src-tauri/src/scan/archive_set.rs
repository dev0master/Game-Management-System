//! Grouping multi-part archive volumes into a single logical item.
//!
//! On a cold-storage drive a 41 GB game is five `.part*.rar` files. Treating them as
//! five items would be wrong in every direction: the library would show five entries,
//! sizes would be a fifth of reality, and a copy job could move four volumes and leave
//! the fifth behind — producing a set that looks present but cannot be extracted.
//!
//! Grouping is per-directory. Volumes of one set always live side by side, and
//! restricting to a single directory keeps two unrelated games that happen to share a
//! stem from merging.

use regex::Regex;
use std::collections::BTreeMap;
use std::sync::LazyLock;

/// How a multi-part set spells its volume numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PartScheme {
    /// `name.part1.rar`, `name.part2.rar`, ...
    RarPart,
    /// `name.rar` + `name.r00`, `name.r01`, ... (pre-3.0 RAR).
    RarOld,
    /// `name.7z.001`, `name.7z.002`, ... — also `.zip.001`, `.iso.001`.
    SplitNumeric,
    /// `name.z01`, `name.z02`, ... + `name.zip` **last** (PKZIP spanned archive:
    /// the `.zip` holds the central directory and terminates the set).
    ZipSpanned,
    /// A single self-contained archive.
    Single,
}

/// One volume of a set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchivePart {
    pub file_name: String,
    pub size_bytes: u64,
    /// 1-based position in extraction order.
    pub index: u32,
}

/// A logical archive: one or more volumes that must always travel together.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveSet {
    /// Name with the volume suffix and extension removed.
    pub stem: String,
    pub scheme: PartScheme,
    /// Volumes in extraction order.
    pub parts: Vec<ArchivePart>,
    pub total_bytes: u64,
    /// False when a volume is missing or the sizes contradict a complete set.
    pub complete: bool,
    /// Indices absent from an otherwise contiguous run.
    pub missing_parts: Vec<u32>,
    /// Human-readable reason `complete` is false.
    pub incomplete_reason: Option<String>,
}

impl ArchiveSet {
    pub fn is_multipart(&self) -> bool {
        self.parts.len() > 1
    }

    /// File names in extraction order — the unit a transfer job must move atomically.
    pub fn file_names(&self) -> Vec<&str> {
        self.parts.iter().map(|p| p.file_name.as_str()).collect()
    }
}

static RE_RAR_PART: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^(?P<stem>.+?)\.part(?P<n>\d+)\.rar$").unwrap());
static RE_RAR_OLD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^(?P<stem>.+?)\.r(?P<n>\d{2,3})$").unwrap());
static RE_SPLIT_NUMERIC: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(?P<stem>.+?)\.(?P<ext>7z|zip|rar|iso|bin|tar|gz)\.(?P<n>\d{3,})$").unwrap()
});
static RE_ZIP_SPANNED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^(?P<stem>.+?)\.z(?P<n>\d{2,})$").unwrap());
static RE_SINGLE_ARCHIVE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^(?P<stem>.+?)\.(rar|zip|7z|iso|bin|tar|gz|mdf|nrg)$").unwrap());

/// Input row: a file name and its size, from one directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEntry {
    pub name: String,
    pub size_bytes: u64,
    /// Modification time, Unix seconds. Free from the walk's existing `metadata()`
    /// call, and the second half of the probe cache key.
    pub mtime_unix: i64,
}

impl FileEntry {
    /// Existing callers and tests do not care about mtime; it defaults to 0.
    pub fn new(name: impl Into<String>, size_bytes: u64) -> Self {
        Self { name: name.into(), size_bytes, mtime_unix: 0 }
    }

    pub fn with_mtime(name: impl Into<String>, size_bytes: u64, mtime_unix: i64) -> Self {
        Self { name: name.into(), size_bytes, mtime_unix }
    }
}

/// Result of grouping one directory.
#[derive(Debug, Default)]
pub struct Grouped {
    pub sets: Vec<ArchiveSet>,
    /// Files that belong to no archive set, by index into the input slice.
    pub leftovers: Vec<usize>,
}

/// A volume detected during the first pass, before sets are assembled.
struct Detected {
    stem: String,
    scheme: PartScheme,
    order: u32,
    input_idx: usize,
}

/// Group the files of a **single directory** into archive sets.
///
/// Files are matched against the volume schemes in priority order; the first match
/// wins. Anything unmatched is returned in `leftovers` so the caller can still see it.
pub fn group_directory(files: &[FileEntry]) -> Grouped {
    let mut detected: Vec<Detected> = Vec::new();
    let mut unmatched: Vec<usize> = Vec::new();

    // Names present in this directory, for the sibling lookups that the `.rNN` and
    // `.zNN` schemes need to confirm their terminating volume.
    let lower_names: Vec<String> = files.iter().map(|f| f.name.to_lowercase()).collect();
    let has = |n: &str| lower_names.iter().any(|x| x == n);

    for (idx, f) in files.iter().enumerate() {
        let name = &f.name;

        if let Some(c) = RE_RAR_PART.captures(name) {
            detected.push(Detected {
                stem: c["stem"].to_string(),
                scheme: PartScheme::RarPart,
                order: c["n"].parse().unwrap_or(0),
                input_idx: idx,
            });
            continue;
        }

        if let Some(c) = RE_SPLIT_NUMERIC.captures(name) {
            // The stem keeps its inner extension (`Game.7z`) so that `Game.7z.001` and
            // a hypothetical `Game.zip.001` in the same folder stay separate sets.
            detected.push(Detected {
                stem: format!("{}.{}", &c["stem"], &c["ext"]),
                scheme: PartScheme::SplitNumeric,
                order: c["n"].parse().unwrap_or(0),
                input_idx: idx,
            });
            continue;
        }

        if let Some(c) = RE_RAR_OLD.captures(name) {
            let stem = c["stem"].to_string();
            // `.rNN` volumes are only a set when the leading `.rar` is present;
            // otherwise the name is probably just an odd extension.
            if has(&format!("{}.rar", stem.to_lowercase())) {
                let n: u32 = c["n"].parse().unwrap_or(0);
                detected.push(Detected {
                    stem,
                    scheme: PartScheme::RarOld,
                    order: n + 1, // `.rar` occupies order 0.
                    input_idx: idx,
                });
                continue;
            }
        }

        if let Some(c) = RE_ZIP_SPANNED.captures(name) {
            let stem = c["stem"].to_string();
            if has(&format!("{}.zip", stem.to_lowercase())) {
                detected.push(Detected {
                    stem,
                    scheme: PartScheme::ZipSpanned,
                    order: c["n"].parse().unwrap_or(0),
                    input_idx: idx,
                });
                continue;
            }
        }

        unmatched.push(idx);
    }

    // Second pass: attach the terminating volume of the sibling-dependent schemes.
    // `.rar` leads its set (order 0); `.zip` closes its set (order u32::MAX).
    let mut still_unmatched: Vec<usize> = Vec::new();
    for idx in unmatched {
        let name = &files[idx].name;
        let mut claimed = false;
        if let Some(c) = RE_SINGLE_ARCHIVE.captures(name) {
            let stem = c["stem"].to_string();
            let stem_lower = stem.to_lowercase();
            let ext = name.rsplit('.').next().unwrap_or("").to_lowercase();
            if ext == "rar"
                && detected
                    .iter()
                    .any(|d| d.scheme == PartScheme::RarOld && d.stem.to_lowercase() == stem_lower)
            {
                detected.push(Detected { stem, scheme: PartScheme::RarOld, order: 0, input_idx: idx });
                claimed = true;
            } else if ext == "zip"
                && detected.iter().any(|d| {
                    d.scheme == PartScheme::ZipSpanned && d.stem.to_lowercase() == stem_lower
                })
            {
                detected.push(Detected {
                    stem,
                    scheme: PartScheme::ZipSpanned,
                    order: u32::MAX,
                    input_idx: idx,
                });
                claimed = true;
            }
        }
        if !claimed {
            still_unmatched.push(idx);
        }
    }

    // Assemble. Key on (lowercased stem, scheme) so case differences between volumes
    // — common on exFAT, which is case-insensitive — do not split a set.
    let mut buckets: BTreeMap<(String, String), Vec<Detected>> = BTreeMap::new();
    for d in detected {
        let key = (d.stem.to_lowercase(), format!("{:?}", d.scheme));
        buckets.entry(key).or_default().push(d);
    }

    let mut sets: Vec<ArchiveSet> = buckets
        .into_values()
        .map(|group| build_set(group, files))
        .collect();

    // Remaining single archives become one-part sets; everything else is a leftover.
    let mut leftovers: Vec<usize> = Vec::new();
    for idx in still_unmatched {
        let f = &files[idx];
        match RE_SINGLE_ARCHIVE.captures(&f.name) {
            Some(c) => sets.push(ArchiveSet {
                stem: c["stem"].to_string(),
                scheme: PartScheme::Single,
                parts: vec![ArchivePart {
                    file_name: f.name.clone(),
                    size_bytes: f.size_bytes,
                    index: 1,
                }],
                total_bytes: f.size_bytes,
                complete: true,
                missing_parts: Vec::new(),
                incomplete_reason: None,
            }),
            None => leftovers.push(idx),
        }
    }

    sets.sort_by(|a, b| a.stem.to_lowercase().cmp(&b.stem.to_lowercase()));
    Grouped { sets, leftovers }
}

/// Turn one bucket of volumes into a validated set.
fn build_set(mut group: Vec<Detected>, files: &[FileEntry]) -> ArchiveSet {
    group.sort_by_key(|d| d.order);
    let scheme = group[0].scheme;
    let stem = group[0].stem.clone();

    let parts: Vec<ArchivePart> = group
        .iter()
        .enumerate()
        .map(|(i, d)| ArchivePart {
            file_name: files[d.input_idx].name.clone(),
            size_bytes: files[d.input_idx].size_bytes,
            index: (i + 1) as u32,
        })
        .collect();

    let total_bytes = parts.iter().map(|p| p.size_bytes).sum();

    // Validation 1: the declared volume numbers form a contiguous run. Checked on the
    // scheme's own numbering, not the re-indexed position, so a gap is visible.
    let declared: Vec<u32> = group
        .iter()
        .filter(|d| d.order != u32::MAX)
        .map(|d| d.order)
        .collect();
    let mut missing_parts = Vec::new();
    if let (Some(&lo), Some(&hi)) = (declared.iter().min(), declared.iter().max()) {
        for n in lo..=hi {
            if !declared.contains(&n) {
                missing_parts.push(n);
            }
        }
    }

    // Validation 2: every volume but the last is the same size. Splitters emit
    // fixed-size volumes with only the tail short, so a short volume mid-set means a
    // truncated download or a file that does not really belong to this set.
    //
    // The comparison is approximate on purpose. Real volumes vary by a few dozen bytes
    // because each carries its own header — the live Assassins Creed Mirage set has a
    // 66-byte spread across its 8.5 GB volumes. Exact equality reported that healthy
    // set as damaged. Truncation is a megabyte-scale event, so the tolerance separates
    // the two cleanly.
    const SIZE_TOLERANCE: u64 = 1024 * 1024;
    let differs = |a: u64, b: u64| a.abs_diff(b) > SIZE_TOLERANCE;

    let mut size_anomaly = None;
    if parts.len() > 2 {
        let first = parts[0].size_bytes;
        if let Some(bad) = parts[..parts.len() - 1]
            .iter()
            .find(|p| differs(p.size_bytes, first))
        {
            size_anomaly = Some(format!(
                "volume {} is {} bytes but the set's volume size is {first}",
                bad.index, bad.size_bytes
            ));
        }
    }
    // A final volume larger than the rest is impossible for a fixed-size split.
    if parts.len() > 1 {
        let last = &parts[parts.len() - 1];
        if last.size_bytes > parts[0].size_bytes + SIZE_TOLERANCE {
            size_anomaly.get_or_insert_with(|| {
                format!(
                    "final volume {} is larger ({}) than the volume size ({})",
                    last.index, last.size_bytes, parts[0].size_bytes
                )
            });
        }
    }

    let incomplete_reason = if !missing_parts.is_empty() {
        Some(format!(
            "incomplete set — missing part{} {}",
            if missing_parts.len() > 1 { "s" } else { "" },
            missing_parts
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        ))
    } else {
        size_anomaly
    };

    ArchiveSet {
        stem,
        scheme,
        parts,
        total_bytes,
        complete: incomplete_reason.is_none(),
        missing_parts,
        incomplete_reason,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MB: u64 = 1024 * 1024;

    /// The real directory: E:\Game\Assassins Creed Mirage.
    fn ac_mirage() -> Vec<FileEntry> {
        vec![
            FileEntry::new("Assassins Creed Mirage.part2.rar", 8704 * MB),
            FileEntry::new("Assassins Creed Mirage.part3.rar", 8704 * MB),
            FileEntry::new("Assassins Creed Mirage.part4.rar", 8704 * MB),
            FileEntry::new("Assassins Creed Mirage.part5.rar", 7330 * MB),
            FileEntry::new("Assassins Creed Mirage.part1.rar", 8704 * MB),
        ]
    }

    #[test]
    fn five_rar_volumes_become_one_set_in_extraction_order() {
        let g = group_directory(&ac_mirage());
        assert_eq!(g.sets.len(), 1, "expected exactly one set, got {:?}", g.sets);
        let s = &g.sets[0];
        assert_eq!(s.scheme, PartScheme::RarPart);
        assert_eq!(s.parts.len(), 5);
        assert!(s.complete, "reason: {:?}", s.incomplete_reason);
        assert_eq!(s.total_bytes, (8704 * 4 + 7330) * MB);
        // Listed on disk out of order; must come back in extraction order.
        assert_eq!(s.file_names()[0], "Assassins Creed Mirage.part1.rar");
        assert_eq!(s.file_names()[4], "Assassins Creed Mirage.part5.rar");
    }

    #[test]
    fn a_missing_volume_is_reported_not_silently_undersized() {
        let files: Vec<FileEntry> = ac_mirage()
            .into_iter()
            .filter(|f| !f.name.contains("part3"))
            .collect();
        let g = group_directory(&files);
        let s = &g.sets[0];
        assert!(!s.complete);
        assert_eq!(s.missing_parts, vec![3]);
        assert!(s.incomplete_reason.as_ref().unwrap().contains("part 3"));
    }

    /// Regression: the live Assassins Creed Mirage volumes differ by 66 bytes because
    /// each carries its own header. An exact-equality check reported that healthy
    /// 41 GB set as damaged.
    #[test]
    fn tiny_header_size_variance_is_not_reported_as_damage() {
        let files = vec![
            FileEntry::new("ACM.part1.rar", 9_126_805_438),
            FileEntry::new("ACM.part2.rar", 9_126_805_504),
            FileEntry::new("ACM.part3.rar", 9_126_805_438),
            FileEntry::new("ACM.part4.rar", 9_126_805_470),
            FileEntry::new("ACM.part5.rar", 7_685_996_544),
        ];
        let g = group_directory(&files);
        let s = &g.sets[0];
        assert!(s.complete, "healthy set flagged: {:?}", s.incomplete_reason);
    }

    #[test]
    fn a_short_middle_volume_is_flagged_as_a_size_anomaly() {
        let mut files = ac_mirage();
        files[1].size_bytes = 512 * MB; // part3 truncated
        let g = group_directory(&files);
        let s = &g.sets[0];
        assert!(!s.complete);
        assert!(s.missing_parts.is_empty());
        assert!(s.incomplete_reason.as_ref().unwrap().contains("volume size"));
    }

    #[test]
    fn two_unrelated_archives_in_one_folder_stay_separate() {
        // The real E:\Game directory.
        let files = vec![
            FileEntry::new("Sekiro Shadows Die Twice by xatab.rar", 12_270 * MB),
            FileEntry::new("Mad Max  - [DODI Repack].zip", 3_890 * MB),
        ];
        let g = group_directory(&files);
        assert_eq!(g.sets.len(), 2);
        assert!(g.sets.iter().all(|s| s.scheme == PartScheme::Single && s.complete));
    }

    #[test]
    fn old_style_rar_volumes_put_the_rar_file_first() {
        let files = vec![
            FileEntry::new("Old Game.r01", 100 * MB),
            FileEntry::new("Old Game.rar", 100 * MB),
            FileEntry::new("Old Game.r00", 100 * MB),
        ];
        let g = group_directory(&files);
        assert_eq!(g.sets.len(), 1);
        let s = &g.sets[0];
        assert_eq!(s.scheme, PartScheme::RarOld);
        assert_eq!(s.file_names(), vec!["Old Game.rar", "Old Game.r00", "Old Game.r01"]);
        assert!(s.complete);
    }

    /// In a PKZIP spanned archive the `.zip` holds the central directory and must be
    /// last, not first as lexical ordering would put it.
    #[test]
    fn zip_spanned_set_places_the_zip_last() {
        let files = vec![
            FileEntry::new("Game.zip", 40 * MB),
            FileEntry::new("Game.z01", 100 * MB),
            FileEntry::new("Game.z02", 100 * MB),
        ];
        let g = group_directory(&files);
        assert_eq!(g.sets.len(), 1);
        let s = &g.sets[0];
        assert_eq!(s.scheme, PartScheme::ZipSpanned);
        assert_eq!(*s.file_names().last().unwrap(), "Game.zip");
        assert_eq!(s.parts.len(), 3);
    }

    #[test]
    fn sevenzip_numeric_split_is_grouped() {
        let files = vec![
            FileEntry::new("Game.7z.001", 100 * MB),
            FileEntry::new("Game.7z.002", 100 * MB),
            FileEntry::new("Game.7z.003", 20 * MB),
        ];
        let g = group_directory(&files);
        assert_eq!(g.sets.len(), 1);
        assert_eq!(g.sets[0].scheme, PartScheme::SplitNumeric);
        assert!(g.sets[0].complete);
    }

    /// A lone `.r00` with no `.rar` is not a set — the sibling check prevents a false
    /// positive on files that merely end in an r-number.
    #[test]
    fn rnn_without_a_leading_rar_is_not_treated_as_a_set() {
        let files = vec![FileEntry::new("Notes.r01", MB)];
        let g = group_directory(&files);
        assert!(g.sets.is_empty());
        assert_eq!(g.leftovers, vec![0]);
    }

    #[test]
    fn non_archive_files_are_returned_as_leftovers() {
        let files = vec![
            FileEntry::new("readme.txt", 1024),
            FileEntry::new("Game.rar", 100 * MB),
        ];
        let g = group_directory(&files);
        assert_eq!(g.sets.len(), 1);
        assert_eq!(g.leftovers, vec![0]);
    }

    #[test]
    fn volume_case_differences_do_not_split_a_set() {
        // exFAT is case-insensitive, so mixed case across volumes is common.
        let files = vec![
            FileEntry::new("Game.Part1.rar", 100 * MB),
            FileEntry::new("game.part2.rar", 50 * MB),
        ];
        let g = group_directory(&files);
        assert_eq!(g.sets.len(), 1);
        assert_eq!(g.sets[0].parts.len(), 2);
    }

    #[test]
    fn empty_directory_yields_nothing() {
        let g = group_directory(&[]);
        assert!(g.sets.is_empty() && g.leftovers.is_empty());
    }
}
