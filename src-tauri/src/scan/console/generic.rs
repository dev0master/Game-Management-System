//! Recognising a console game file without reading inside it.
//!
//! The library holds files for every PlayStation and Xbox generation, and reading identity
//! out of each format is a long job — a PS2 `.iso` needs an ISO9660 walk to find
//! `SYSTEM.CNF`, a `.chd` needs a decompressor. None of that is needed for the file to be
//! *seen*, so this layer recognises by extension and folder, takes the title from the
//! filename, and says plainly which platform it is guessing.
//!
//! The rule it follows: **show it, and be honest about how much is known.** A file listed
//! under a guessed platform with a filename-derived title is useful. The same file left out
//! because its header has not been parsed yet looks like the program is broken.
//!
//! Deeper identity for each format replaces a guess here without changing anything else.

use super::group::Platform;

/// Extensions that are certainly one platform, because no other console uses them.
const CERTAIN: &[(&str, Platform)] = &[
    ("xex", Platform::Xbox360),
    ("xbe", Platform::XboxOriginal),
    ("vpk", Platform::PsVita),
    ("cso", Platform::Psp),
    ("pbp", Platform::Psp),
];

/// Disc images that several consoles share. The platform has to come from elsewhere.
const AMBIGUOUS: &[&str] =
    &["iso", "bin", "cue", "img", "chd", "mdf", "mds", "nrg", "ccd", "gdi"];

/// Extensions belonging to consoles outside PlayStation and Xbox.
///
/// Listed so they are shown rather than silently skipped — the request was for the PS and
/// Xbox libraries "and the rest of the console files if there are any".
const OTHER_CONSOLE: &[&str] = &[
    "nsp", "xci", "wbfs", "rvz", "gcm", "gcz", "wia", "nkit", "wad", "wux", "wud", "3ds",
    "cia", "nds", "gba", "gb", "gbc", "n64", "z64", "v64", "nes", "sfc", "smc", "md",
    "gen", "gg", "sms", "pce", "cdi",
];

/// How a file was recognised.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Confidence {
    /// The extension belongs to exactly one console.
    Certain,
    /// A shared disc-image extension, attributed from the folder it sits in.
    FromFolder,
    /// Recognised as a console file, but nothing says which console.
    Unattributed,
}

/// A console file recognised from the outside.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recognised {
    pub platform: Platform,
    pub confidence: Confidence,
    /// True for a container that holds a whole disc, so a `.cue` can claim its tracks.
    pub is_disc_image: bool,
}

/// The extension of `name`, lowercased. Empty when there is none.
fn ext_of(name: &str) -> String {
    name.rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default()
}

/// Recognise one file. `folder_hint` is the platform its folder names, if any.
///
/// Returns `None` for a file that is not a console game — a text file, a screenshot, an
/// installer. Being unsure is reported as `Unattributed`, never as a wrong platform.
pub fn recognise(name: &str, folder_hint: Option<Platform>) -> Option<Recognised> {
    let ext = ext_of(name);
    if ext.is_empty() {
        return None;
    }

    if let Some((_, p)) = CERTAIN.iter().find(|(e, _)| *e == ext) {
        return Some(Recognised {
            platform: *p,
            confidence: Confidence::Certain,
            is_disc_image: matches!(ext.as_str(), "cso" | "pbp"),
        });
    }

    if AMBIGUOUS.contains(&ext.as_str()) {
        return Some(match folder_hint {
            Some(p) => Recognised {
                platform: p,
                confidence: Confidence::FromFolder,
                is_disc_image: true,
            },
            None => Recognised {
                platform: Platform::Unknown,
                confidence: Confidence::Unattributed,
                is_disc_image: true,
            },
        });
    }

    if OTHER_CONSOLE.contains(&ext.as_str()) {
        return Some(Recognised {
            platform: Platform::Unknown,
            confidence: Confidence::Unattributed,
            is_disc_image: true,
        });
    }

    None
}

/// The platform named by the nearest enclosing folder, searched outward.
///
/// `E:\Games\PS2\Shadow of the Colossus\disc.iso` attributes to PS2 from `PS2`, not from
/// `Games`. The nearest match wins because that is how people actually nest these.
pub fn folder_hint(rel_dir: &str) -> Option<Platform> {
    rel_dir
        .split(['\\', '/'])
        .filter(|s| !s.is_empty())
        .rev()
        .find_map(Platform::from_folder_name)
}

/// Does `member` belong to the disc set headed by `stem`?
///
/// A `.cue` names its tracks, but reading it is not needed to group them: a track file
/// always begins with the sheet's own stem. `Game.cue` therefore claims `Game.bin`,
/// `Game (Track 01).bin` and `Game01.bin`.
///
/// A prefix match alone is not enough, which is the trap here: `Final Fantasy VII.cue`
/// would swallow `Final Fantasy VIII.bin` and the eighth game would vanish from the
/// library. So whatever follows the stem must not continue the title — empty, or starting
/// with punctuation, a space or a track number, never with another letter.
pub fn belongs_to_set(stem_lower: &str, member: &str) -> bool {
    let m = member.to_lowercase();
    if !matches!(
        ext_of(&m).as_str(),
        "bin" | "img" | "iso" | "wav" | "mp3" | "ogg" | "sub" | "ccd"
    ) {
        return false;
    }
    let Some(rest) = stem_of(&m).strip_prefix(stem_lower) else {
        return false;
    };
    rest.chars().next().is_none_or(|c| !c.is_alphabetic())
}

/// The filename without its extension.
pub fn stem_of(name: &str) -> &str {
    name.rsplit_once('.').map(|(s, _)| s).unwrap_or(name)
}

/// The console boot executable in a folder, if it has one.
///
/// An Xbox game is a folder holding `default.xbe`, and a 360 one a folder holding
/// `default.xex`. The executable's name is the console's convention, not the game's, so the
/// folder is both the title and the unit to measure — a card reading "default" would be
/// useless. This is the same rule the PS3 path follows: respect the grouping the filesystem
/// already provides.
pub fn boot_executable(files: &[super::DirFile]) -> Option<(String, Platform)> {
    files.iter().find_map(|f| {
        let lower = f.name.to_lowercase();
        match lower.as_str() {
            "default.xbe" => Some((f.name.clone(), Platform::XboxOriginal)),
            "default.xex" => Some((f.name.clone(), Platform::Xbox360)),
            _ => None,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_extension_unique_to_one_console_is_certain() {
        for (name, want) in [
            ("default.xex", Platform::Xbox360),
            ("default.xbe", Platform::XboxOriginal),
            ("Game.VPK", Platform::PsVita),
            ("Game.cso", Platform::Psp),
            ("Game.PBP", Platform::Psp),
        ] {
            let r = recognise(name, None).unwrap_or_else(|| panic!("{name}"));
            assert_eq!(r.platform, want, "{name}");
            assert_eq!(r.confidence, Confidence::Certain, "{name}");
        }
    }

    /// A shared disc extension takes the platform from its folder, and says it guessed.
    #[test]
    fn a_disc_image_is_attributed_from_its_folder() {
        let hint = folder_hint(r"Games\PS2\Shadow of the Colossus");
        assert_eq!(hint, Some(Platform::Ps2));

        let r = recognise("disc.iso", hint).unwrap();
        assert_eq!(r.platform, Platform::Ps2);
        assert_eq!(r.confidence, Confidence::FromFolder);
        assert!(r.is_disc_image);
    }

    /// With nothing to go on, the platform is unknown rather than wrong.
    #[test]
    fn an_unattributable_disc_image_is_never_guessed_at() {
        let r = recognise("disc.iso", None).unwrap();
        assert_eq!(r.platform, Platform::Unknown);
        assert_eq!(r.confidence, Confidence::Unattributed);
    }

    /// The nearest enclosing folder wins, and unrelated folder names are ignored.
    #[test]
    fn the_nearest_platform_folder_wins() {
        assert_eq!(folder_hint(r"PS3\Games\PSP\Titles"), Some(Platform::Psp));
        assert_eq!(folder_hint(r"Games\Xbox 360\Halo"), Some(Platform::Xbox360));
        assert_eq!(folder_hint(r"Games\xbox360"), Some(Platform::Xbox360));
        assert_eq!(folder_hint(r"Backups\Stuff"), None);
        assert_eq!(folder_hint(""), None);
        // Forward slashes are accepted as readily as backslashes.
        assert_eq!(folder_hint("games/ps1/misc"), Some(Platform::Ps1));
    }

    #[test]
    fn xbox_series_folder_names_are_recognised() {
        for n in ["Xbox One", "xboxone", "Xbox Series X", "Series S"] {
            assert_eq!(Platform::from_folder_name(n), Some(Platform::XboxSeries), "{n}");
        }
    }

    /// Files that are not games must not appear in a game list.
    #[test]
    fn ordinary_files_are_not_recognised_as_games() {
        for n in ["notes.txt", "cover.jpg", "setup.exe", "archive.rar", "movie.mkv", "noext"] {
            assert!(recognise(n, Some(Platform::Ps2)).is_none(), "{n}");
        }
    }

    /// Consoles outside the request are still shown, unattributed.
    #[test]
    fn other_consoles_are_shown_rather_than_skipped() {
        let r = recognise("Game.nsp", None).unwrap();
        assert_eq!(r.platform, Platform::Unknown);
        assert_eq!(r.confidence, Confidence::Unattributed);
        assert!(recognise("Game.wbfs", None).is_some());
    }

    /// A cue sheet claims its own tracks and nothing else, without being parsed.
    #[test]
    fn a_cue_sheet_claims_only_its_own_tracks() {
        let stem = "final fantasy vii";
        assert!(belongs_to_set(stem, "Final Fantasy VII.bin"));
        assert!(belongs_to_set(stem, "Final Fantasy VII (Track 01).bin"));
        assert!(belongs_to_set(stem, "final fantasy vii02.img"));
        assert!(!belongs_to_set(stem, "Final Fantasy VIII.bin"), "a longer title is a different game");
        assert!(!belongs_to_set(stem, "Final Fantasy VII.cue"), "the sheet is not its own track");
        assert!(!belongs_to_set(stem, "Final Fantasy VII.txt"));
        // The prefix trap, both directions.
        assert!(!belongs_to_set("game", "Gameplay Recording.bin"));
        assert!(belongs_to_set("game", "Game - Track 2.bin"));
        assert!(belongs_to_set("game", "game_2.img"));
        assert!(!belongs_to_set("game 2", "Game.bin"), "a shorter name is a different game");
    }

    #[test]
    fn stem_and_extension_are_split_at_the_last_dot() {
        assert_eq!(stem_of("A Game v1.2.iso"), "A Game v1.2");
        assert_eq!(ext_of("A Game v1.2.iso"), "iso");
        assert_eq!(stem_of("noext"), "noext");
        assert_eq!(ext_of("noext"), "");
    }
}
