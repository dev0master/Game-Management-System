//! Turning raw filesystem names into clean titles and stable match keys.
//!
//! Names on cold-storage drives are not titles. They carry repacker tags, release
//! groups, version numbers, language lists and archive part suffixes. All of that has
//! to come off before a title can be matched against a metadata source, but the
//! original is never destroyed: `raw_name` is kept verbatim on the item, and bracketed
//! content we strip without recognising is preserved in `notes`.

use regex::Regex;
use std::sync::LazyLock;
use unicode_normalization::UnicodeNormalization;

/// Repacker and scene-group tags, matched case-insensitively as whole words.
pub const RELEASE_TAGS: &[&str] = &[
    // Repackers.
    "FitGirl", "DODI", "xatab", "ElAmigos", "KaOsKrew", "Masquerade", "R.G. Mechanics",
    "RG Mechanics", "Corepack", "qoob", "Seyter", "GLaDOS",
    // Scene / p2p groups.
    "CODEX", "PLAZA", "SKIDROW", "RUNE", "EMPRESS", "TENOKE", "RELOADED", "HOODLUM",
    "DARKSiDERS", "TiNYiSO", "PROPHET", "Razor1911", "FAIRLIGHT", "GOLDBERG", "SiMPLEX",
    "ALI213", "3DM", "CPY", "DOGE", "FLT",
];

/// Tags that identify a *repacker* rather than a scene group. Lowercase.
const REPACKER_TAGS: &[&str] = &[
    "fitgirl", "dodi", "xatab", "elamigos", "kaoskrew", "masquerade", "corepack",
    "qoob", "seyter", "glados", "rg mechanics", "r.g. mechanics",
];

/// Words that are never part of a title.
const NOISE_WORDS: &[&str] = &[
    "Repack", "PROPER", "Crack", "Cracked", "Portable", "Preinstalled", "Pre-installed",
    "PreActivated", "Preactivated", "Activated", "Lossless", "Rip", "P2P",
];

/// Edition markers worth recording. Longest-first so "Game of the Year" is tried
/// before a shorter overlapping entry could match.
const EDITIONS: &[&str] = &[
    "Game of the Year Edition", "Game of the Year", "Anniversary Edition",
    "Legendary Edition", "Definitive Edition", "Complete Edition", "Ultimate Edition",
    "Enhanced Edition", "Deluxe Edition", "Gold Edition", "Directors Cut",
    "Director's Cut", "Remastered", "Redux", "GOTY",
];

static RE_PART_SUFFIX: LazyLock<Regex> = LazyLock::new(|| {
    // .part12 | .7z.001 | .zip.002 | .r07 | .z03 — trailing volume markers.
    Regex::new(r"(?i)\.(?:part\d+|(?:7z|zip|rar|iso|bin)\.\d{3,}|r\d{2,3}|z\d{2,3})$").unwrap()
});

static RE_ARCHIVE_EXT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\.(rar|zip|7z|tar|gz|bz2|xz|iso|bin|mdf|mds|nrg|cue|exe)$").unwrap()
});

static RE_BRACKETED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[\[\(\{]([^\[\]\(\)\{\}]*)[\]\)\}]").unwrap());

static RE_VERSION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(?:v\.?\s?\d+(?:\.\d+){0,3}[a-z]?|build[\s._-]?\d+|update[\s._-]?\d+|rev[\s._-]?\d+)\b",
    )
    .unwrap()
});

/// A plausible *release* year. Deliberately not used to strip text from titles —
/// see `clean` — because "Metro 2033" and "Cyberpunk 2077" are titles, not years.
static RE_YEAR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b(19[7-9]\d|20[0-3]\d)\b").unwrap());

static RE_MULTI_LANG: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(?:multi\s?\d+|ru\s?/\s?en|en\s?/\s?ru|incl\.?\s?dlcs?|all\s?dlcs?|\+\s?\d+\s?dlcs?)\b",
    )
    .unwrap()
});

static RE_BY_REPACKER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\s+by\s+([A-Za-z0-9._'-]{2,24})\s*$").unwrap());

static RE_WS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").unwrap());
static RE_NON_ALNUM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[^a-z0-9]+").unwrap());
static RE_ONLY_DIGITS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\d{4}$").unwrap());

/// Everything recovered from a raw name.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CleanedTitle {
    pub clean_title: String,
    /// Aggressively normalised key for grouping, dedupe and fuzzy matching.
    pub title_key: String,
    pub repacker: Option<String>,
    pub release_group: Option<String>,
    pub version: Option<String>,
    pub edition: Option<String>,
    pub year: Option<i32>,
    /// Bracketed content stripped but not recognised. Kept so nothing is silently
    /// lost; it surfaces in the review UI.
    pub notes: Option<String>,
}

/// Fold Arabic-Indic (U+0660..) and Extended Arabic-Indic (U+06F0..) digits to ASCII.
/// Both ranges are contiguous and ordered, so the offset is the digit value.
fn fold_arabic_digits(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '\u{0660}'..='\u{0669}' => char::from(b'0' + (c as u32 - 0x0660) as u8),
            '\u{06F0}'..='\u{06F9}' => char::from(b'0' + (c as u32 - 0x06F0) as u8),
            other => other,
        })
        .collect()
}

/// True when a match at `at` does not cut a word in half on either side.
fn is_word_boundary_match(hay: &str, at: usize, needle: &str) -> bool {
    let before_ok = at == 0
        || !hay[..at]
            .chars()
            .next_back()
            .is_some_and(char::is_alphanumeric);
    let after = at + needle.len();
    let after_ok = after >= hay.len()
        || !hay[after..].chars().next().is_some_and(char::is_alphanumeric);
    before_ok && after_ok
}

/// Remove every whole-word, case-insensitive occurrence of `needle`.
fn strip_word(text: &str, needle: &str) -> String {
    let lower = text.to_lowercase();
    let needle_lower = needle.to_lowercase();
    // Operating on the lowercase copy is only safe when case folding preserves byte
    // offsets; for the ASCII tags handled here it does.
    if lower.len() != text.len() {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0usize;
    while let Some(rel) = lower[cursor..].find(&needle_lower) {
        let at = cursor + rel;
        if is_word_boundary_match(&lower, at, &needle_lower) {
            out.push_str(&text[cursor..at]);
            out.push(' ');
        } else {
            out.push_str(&text[cursor..at + needle_lower.len()]);
        }
        cursor = at + needle_lower.len();
    }
    out.push_str(&text[cursor..]);
    out
}

fn find_word(text: &str, needle: &str) -> bool {
    let lower = text.to_lowercase();
    if lower.len() != text.len() {
        return false;
    }
    let needle_lower = needle.to_lowercase();
    lower
        .match_indices(&needle_lower)
        .any(|(at, _)| is_word_boundary_match(&lower, at, &needle_lower))
}

/// Strip the archive extension and any multi-part volume suffix.
///
/// Looped because `foo.7z.001` carries both, and in `foo.part1.rar` the extension
/// sits outside the part marker.
pub fn strip_archive_suffixes(name: &str) -> String {
    let mut s = name.to_string();
    for _ in 0..3 {
        let before = s.clone();
        s = RE_ARCHIVE_EXT.replace(&s, "").into_owned();
        s = RE_PART_SUFFIX.replace(&s, "").into_owned();
        if s == before {
            break;
        }
    }
    s
}

/// Run the full cleaning pipeline over a raw file or folder name.
pub fn clean(raw: &str, is_file: bool) -> CleanedTitle {
    let mut out = CleanedTitle::default();

    // 1. Archive extension + part suffix.
    let mut s = if is_file {
        strip_archive_suffixes(raw)
    } else {
        raw.to_string()
    };

    // 2. NFC + Arabic-Indic digit folding. Windows does not normalise filenames, so
    //    the same Arabic name can exist on disk in both NFC and NFD.
    s = fold_arabic_digits(&s.nfc().collect::<String>());

    // 3. Separators to spaces, protecting dots inside version numbers.
    s = to_spaces(&s);

    // 4. Bracketed groups. Recognised tags are captured; a bare 4-digit year is taken
    //    as the release year; anything else is kept in `notes` rather than discarded.
    let mut notes: Vec<String> = Vec::new();
    let mut captured_group: Option<String> = None;
    let mut bracket_year: Option<i32> = None;
    s = RE_BRACKETED
        .replace_all(&s, |caps: &regex::Captures| {
            let inner = caps.get(1).map_or("", |m| m.as_str()).trim().to_string();
            if inner.is_empty() {
                return String::from(" ");
            }
            let lower = inner.to_lowercase();
            if let Some(tag) = RELEASE_TAGS
                .iter()
                .find(|t| lower.contains(&t.to_lowercase()))
            {
                captured_group = Some((*tag).to_string());
            } else if RE_ONLY_DIGITS.is_match(&inner) && RE_YEAR.is_match(&inner) {
                bracket_year = inner.parse().ok();
            } else if !is_pure_noise(&lower) {
                notes.push(inner);
            }
            String::from(" ")
        })
        .into_owned();

    // 5. Trailing "by <name>".
    if let Some(caps) = RE_BY_REPACKER.captures(&s.clone()) {
        out.repacker = caps.get(1).map(|m| m.as_str().to_string());
        s = RE_BY_REPACKER.replace(&s, "").into_owned();
    }

    // 6. Bare release-group tags anywhere in the name (`-CODEX`, `FitGirl`, ...).
    for tag in RELEASE_TAGS {
        if find_word(&s, tag) {
            captured_group.get_or_insert_with(|| (*tag).to_string());
            s = strip_word(&s, tag);
        }
    }
    if let Some(g) = captured_group {
        if REPACKER_TAGS.contains(&g.to_lowercase().as_str()) {
            out.repacker.get_or_insert(g);
        } else {
            out.release_group = Some(g);
        }
    }

    // 7. Version and language/DLC markers are removed. The year is *recorded only* —
    //    stripping it would turn "Metro 2033" into "Metro" and "Cyberpunk 2077" into
    //    "Cyberpunk", because in those titles the number is the name.
    if let Some(m) = RE_VERSION.find(&s) {
        out.version = Some(m.as_str().trim().to_string());
        s = RE_VERSION.replace_all(&s, " ").into_owned();
    }
    s = RE_MULTI_LANG.replace_all(&s, " ").into_owned();
    out.year = bracket_year.or_else(|| RE_YEAR.find(&s).and_then(|m| m.as_str().parse().ok()));

    // 8. Editions are recorded and removed, so a base game and its GOTY release share
    //    a `title_key` and can be reported as duplicates.
    for ed in EDITIONS {
        if find_word(&s, ed) {
            out.edition = Some((*ed).to_string());
            s = strip_word(&s, ed);
            break;
        }
    }
    for w in NOISE_WORDS {
        if find_word(&s, w) {
            s = strip_word(&s, w);
        }
    }

    // 9. Collapse whitespace, then trim the separator debris left by the removals.
    //    This is what turns "Mad Max  - " into "Mad Max".
    s = RE_WS.replace_all(&s, " ").trim().to_string();
    s = s
        .trim_matches(|c: char| matches!(c, '-' | '_' | ':' | '.' | ',') || c.is_whitespace())
        .to_string();
    s = RE_WS.replace_all(&s, " ").trim().to_string();

    // Never return an empty title — fall back to the raw name so the item stays
    // identifiable in the UI.
    if s.is_empty() {
        s = raw.trim().to_string();
    }

    out.clean_title = s;
    out.title_key = make_key(&out.clean_title);
    if !notes.is_empty() {
        out.notes = Some(notes.join("; "));
    }
    out
}

/// True for bracket content carrying no information worth keeping.
fn is_pure_noise(lower: &str) -> bool {
    RE_VERSION.is_match(lower)
        || RE_MULTI_LANG.is_match(lower)
        || NOISE_WORDS.iter().any(|w| lower == w.to_lowercase())
        || lower
            .chars()
            .all(|c| c.is_ascii_digit() || c == '.' || c == ' ')
}

/// Convert separators to spaces without breaking version numbers.
///
/// A dot flanked by digits on both sides (`1.2`) belongs to a version and is kept;
/// every other dot, underscore or plus becomes a space.
fn to_spaces(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    for (i, &c) in chars.iter().enumerate() {
        let replace = match c {
            '_' | '+' => true,
            '.' => {
                let prev_digit = i > 0 && chars[i - 1].is_ascii_digit();
                let next_digit = chars.get(i + 1).is_some_and(char::is_ascii_digit);
                !(prev_digit && next_digit)
            }
            _ => false,
        };
        out.push(if replace { ' ' } else { c });
    }
    out
}

/// Build the match key: lowercase, alphanumerics only, no leading article, trailing
/// roman numeral folded to digits.
pub fn make_key(title: &str) -> String {
    let lowered = title.to_lowercase();
    // The article is dropped as a whole word, before punctuation is flattened away.
    // Stripping "the" from the flattened string would turn "They Did" into "ydid".
    let without_article = lowered
        .strip_prefix("the ")
        .map(str::trim_start)
        .unwrap_or(lowered.as_str());
    let folded = fold_trailing_roman(without_article);
    RE_NON_ALNUM.replace_all(&folded, "").into_owned()
}

/// Fold a standalone *trailing* roman numeral to its arabic value.
///
/// Only the final token, and only values up to 20. Both limits matter: roman digits
/// spell ordinary words, and "Mix" is a perfectly valid numeral for 1009. Capping at
/// a plausible sequel number keeps "Final Fantasy VII" working while leaving "Super
/// Mix" alone.
fn fold_trailing_roman(s: &str) -> String {
    let Some((head, last)) = s.rsplit_once(char::is_whitespace) else {
        return s.to_string();
    };
    match roman_to_u32(last).filter(|n| *n <= 20) {
        Some(n) => format!("{head} {n}"),
        None => s.to_string(),
    }
}

/// Parse a lowercase roman numeral, rejecting non-canonical spellings.
fn roman_to_u32(token: &str) -> Option<u32> {
    if token.is_empty() || token.len() > 4 {
        return None;
    }
    let mut total = 0u32;
    let mut prev = 0u32;
    for c in token.chars().rev() {
        let v = match c {
            'i' => 1,
            'v' => 5,
            'x' => 10,
            'l' => 50,
            'c' => 100,
            'd' => 500,
            'm' => 1000,
            _ => return None,
        };
        if v < prev {
            total = total.checked_sub(v)?;
        } else {
            total += v;
            prev = v;
        }
    }
    (total > 0).then_some(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Every case below is a real name observed on the target drives, or a direct
    // variation of one. These are the Phase 1 acceptance criteria.

    #[test]
    fn plain_folder_name_is_unchanged() {
        let c = clean("Assassins Creed Mirage", false);
        assert_eq!(c.clean_title, "Assassins Creed Mirage");
        assert_eq!(c.title_key, "assassinscreedmirage");
        assert_eq!(c.repacker, None);
    }

    #[test]
    fn trailing_by_repacker_is_captured_and_removed() {
        let c = clean("Sekiro Shadows Die Twice by xatab.rar", true);
        assert_eq!(c.clean_title, "Sekiro Shadows Die Twice");
        assert_eq!(c.repacker.as_deref(), Some("xatab"));
    }

    /// The real file has two spaces before the dash. Once the bracket is removed the
    /// remainder is "Mad Max  - ", so both the doubled space and the dangling dash
    /// have to go.
    #[test]
    fn bracket_tag_double_space_and_dangling_dash() {
        let c = clean("Mad Max  - [DODI Repack].zip", true);
        assert_eq!(c.clean_title, "Mad Max");
        assert_eq!(c.title_key, "madmax");
        assert_eq!(c.repacker.as_deref(), Some("DODI"));
    }

    #[test]
    fn multipart_rar_volume_suffixes_all_reduce_to_one_title() {
        for name in [
            "Assassins Creed Mirage.part1.rar",
            "Assassins Creed Mirage.part5.rar",
        ] {
            assert_eq!(clean(name, true).clean_title, "Assassins Creed Mirage");
        }
    }

    #[test]
    fn old_style_and_sevenzip_volume_suffixes_are_stripped() {
        assert_eq!(clean("Some Game.7z.001", true).clean_title, "Some Game");
        assert_eq!(clean("Some Game.r00", true).clean_title, "Some Game");
        assert_eq!(clean("Some Game.z01", true).clean_title, "Some Game");
    }

    #[test]
    fn typo_in_folder_name_is_preserved_verbatim() {
        // The desktop shortcut points at "Ghost of Tsushimaa" (doubled trailing a).
        // Cleaning must not "fix" it — that is the fuzzy matcher's job.
        let typo = clean("Ghost of Tsushimaa", false);
        assert_eq!(typo.clean_title, "Ghost of Tsushimaa");
        assert_eq!(typo.title_key, "ghostoftsushimaa");
        assert!(typo.title_key.starts_with(&clean("Ghost of Tsushima", false).title_key));
    }

    #[test]
    fn scene_group_is_captured_and_number_in_title_survives() {
        let c = clean("Cyberpunk.2077.v2.1-CODEX.rar", true);
        // 2077 is part of the name, not a release year.
        assert_eq!(c.clean_title, "Cyberpunk 2077");
        assert_eq!(c.release_group.as_deref(), Some("CODEX"));
        assert_eq!(c.version.as_deref(), Some("v2.1"));
        assert_eq!(c.year, None);
    }

    /// Regression guard: an earlier draft stripped any 4-digit year, which silently
    /// turned "Metro 2033" into "Metro".
    #[test]
    fn year_inside_a_title_is_recorded_but_not_removed() {
        let c = clean("Metro 2033 Redux", false);
        assert_eq!(c.clean_title, "Metro 2033");
        assert_eq!(c.edition.as_deref(), Some("Redux"));
        assert_eq!(c.year, Some(2033));
    }

    #[test]
    fn parenthesised_year_is_treated_as_release_year_and_removed() {
        let c = clean("Some Game (2019)", false);
        assert_eq!(c.clean_title, "Some Game");
        assert_eq!(c.year, Some(2019));
    }

    #[test]
    fn dots_become_spaces_but_version_numbers_survive() {
        let c = clean("The.Witcher.3.Wild.Hunt.v1.32", false);
        assert_eq!(c.clean_title, "The Witcher 3 Wild Hunt");
        assert_eq!(c.version.as_deref(), Some("v1.32"));
        // The leading article is dropped from the key only.
        assert_eq!(c.title_key, "witcher3wildhunt");
    }

    #[test]
    fn unknown_bracket_content_is_preserved_in_notes() {
        let c = clean("Some Game [Publisher Note].rar", true);
        assert_eq!(c.clean_title, "Some Game");
        assert_eq!(c.notes.as_deref(), Some("Publisher Note"));
    }

    #[test]
    fn arabic_indic_digits_fold_to_ascii() {
        assert_eq!(clean("Some Game (٢٠٢٣)", false).year, Some(2023));
    }

    #[test]
    fn arabic_name_survives_cleaning_uncorrupted() {
        let c = clean("سفرة إيران", false);
        assert_eq!(c.clean_title, "سفرة إيران");
        // A non-latin title yields an empty latin key; callers fall back to
        // `search_key`, which keeps the folded Arabic form.
        assert!(c.title_key.is_empty());
    }

    #[test]
    fn title_is_never_empty_even_when_everything_is_stripped() {
        assert!(!clean("[FitGirl Repack].rar", true).clean_title.is_empty());
    }

    #[test]
    fn trailing_roman_numeral_folds_in_the_key_only() {
        let c = clean("Final Fantasy VII", false);
        assert_eq!(c.clean_title, "Final Fantasy VII");
        assert_eq!(c.title_key, "finalfantasy7");
    }

    /// "Mix" is a canonical roman numeral for 1009 and "Did" for 999. Capping the fold
    /// at 20 is what keeps ordinary words intact.
    #[test]
    fn roman_folding_does_not_mangle_ordinary_words() {
        assert_eq!(clean("Super Mix", false).title_key, "supermix");
        assert_eq!(clean("They Did", false).title_key, "theydid");
        assert_eq!(clean("Mad Max", false).title_key, "madmax");
    }

    #[test]
    fn os_image_name_is_cleaned_without_panicking() {
        // Not a game — the classifier rejects it — but cleaning must stay total.
        let c = clean(
            "Windows 7 Professional SP1 Multilingual Preactivated January 2026 [FileCR].iso",
            true,
        );
        assert!(!c.clean_title.is_empty());
        assert_eq!(c.notes.as_deref(), Some("FileCR"));
    }
}
