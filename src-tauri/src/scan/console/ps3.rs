//! Reading a PS3 game folder.
//!
//! The cheapest platform to add, because a PS3 disc rip states its own identity in a file
//! format this crate already parses: `PS3_GAME\PARAM.SFO` is the same `\0PSF` container as
//! a PS4 package's `param.sfo`, so `ps4::parse_sfo` reads it unchanged. The artwork is a
//! plain `PS3_GAME\ICON0.PNG`, so there is no binary extraction at all.
//!
//! Grouping needs no work either. A PS3 game is a folder, which is already the unit a
//! person browses and moves, and `group.rs` states the rule: group where the filesystem
//! gives no grouping, respect it where it does.

use super::group::Platform;
use super::ps4::{self, ParamSfo};
use std::path::{Path, PathBuf};

/// `PARAM.SFO` is a few hundred bytes; anything far past that is not one.
const MAX_SFO_LEN: u64 = 1024 * 1024;

/// What a PS3 folder turned out to be.
#[derive(Debug, Clone)]
pub struct Ps3Game {
    /// e.g. `BLES01925`.
    pub title_id: Option<String>,
    pub title: Option<String>,
    /// `DG` disc game · `HG` HDD game · `GD` patch.
    pub category: Option<String>,
    pub app_ver: Option<String>,
    /// Firmware as printed in the sfo, e.g. `03.5500`. Text, not the PS4's BCD packing.
    pub system_ver: Option<String>,
    /// `PS3_GAME\ICON0.PNG`, when it is there.
    pub icon: Option<PathBuf>,
}

/// Find the `PS3_GAME` directory of a game folder.
///
/// Accepts both shapes seen in the wild: a disc rip whose root holds `PS3_GAME`, and a
/// folder that *is* the `PS3_GAME` directory. Matching is case-insensitive because rips
/// come off FAT, exFAT and NTFS with every capitalisation.
pub fn ps3_game_dir(dir: &Path) -> Option<PathBuf> {
    if has_param_sfo(dir) {
        return Some(dir.to_path_buf());
    }
    let child = find_child_dir(dir, "ps3_game")?;
    has_param_sfo(&child).then_some(child)
}

fn has_param_sfo(dir: &Path) -> bool {
    find_child_file(dir, "param.sfo").is_some()
}

/// A child directory whose name matches, ignoring case.
fn find_child_dir(dir: &Path, want_lower: &str) -> Option<PathBuf> {
    for e in std::fs::read_dir(dir).ok()?.flatten() {
        if e.file_type().is_ok_and(|t| t.is_dir())
            && e.file_name().to_string_lossy().to_lowercase() == want_lower
        {
            return Some(e.path());
        }
    }
    None
}

/// A child file whose name matches, ignoring case.
fn find_child_file(dir: &Path, want_lower: &str) -> Option<PathBuf> {
    for e in std::fs::read_dir(dir).ok()?.flatten() {
        if e.file_type().is_ok_and(|t| t.is_file())
            && e.file_name().to_string_lossy().to_lowercase() == want_lower
        {
            return Some(e.path());
        }
    }
    None
}

/// Read the identity of a PS3 game from its `PS3_GAME` directory.
///
/// Never fails: a folder with an unreadable sfo is still a game folder, and the filename
/// will carry the title. Reading only bounded, named files means nothing here can be
/// steered into a huge allocation by a malformed rip.
pub fn read(game_dir: &Path) -> Ps3Game {
    let sfo = find_child_file(game_dir, "param.sfo")
        .filter(|p| std::fs::metadata(p).is_ok_and(|m| m.len() <= MAX_SFO_LEN))
        .and_then(|p| std::fs::read(p).ok())
        .map(|b| ps4::parse_sfo(&b))
        .unwrap_or_default();

    Ps3Game {
        // A PS3 sfo names the field TITLE_ID exactly as a PS4 one does.
        title_id: sfo.title_id.clone().filter(|s| !s.is_empty()),
        title: sfo.title.clone().filter(|s| !s.is_empty()),
        category: sfo.category.clone(),
        app_ver: sfo.app_ver.clone(),
        system_ver: system_ver_of(&sfo),
        icon: find_child_file(game_dir, "icon0.png"),
    }
}

/// `PS3_SYSTEM_VER` is text here, unlike the PS4's packed integer, so it is carried
/// through as written rather than reformatted.
fn system_ver_of(sfo: &ParamSfo) -> Option<String> {
    sfo.extra.iter().find_map(|(k, v)| {
        if k != "PS3_SYSTEM_VER" {
            return None;
        }
        match v {
            ps4::SfoValue::Text(t) => Some(t.trim().to_string()).filter(|s| !s.is_empty()),
            ps4::SfoValue::Int(_) => None,
        }
    })
}

/// Read the four magic bytes at the start of a package.
///
/// Four bytes, so this is cheap enough to ask about a file that already failed a PS4 parse.
pub fn peek_pkg_magic(path: &Path) -> Option<[u8; 4]> {
    use std::io::Read;
    let mut f = std::fs::File::open(path).ok()?;
    let mut magic = [0u8; 4];
    f.read_exact(&mut magic).ok()?;
    Some(magic)
}

/// The platform a `.pkg` belongs to, from the magic its header carries.
///
/// PS3 and PS4 packages share the extension and differ in four bytes: `\x7FPKG` against
/// `\x7FCNT`. Telling them apart here is what keeps a PS3 package out of the damaged list,
/// where it would otherwise land after failing the PS4 parse.
pub fn platform_for_pkg_magic(magic: [u8; 4]) -> Option<Platform> {
    match &magic {
        b"\x7FCNT" => Some(Platform::Ps4),
        b"\x7FPKG" => Some(Platform::Ps3),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::console::ps4::{SfoValue, PNG_MAGIC};
    use std::fs;

    fn tmp(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "gv_ps3_{tag}_{}_{}",
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

    /// Build a `\0PSF` blob, the same container a PS4 package carries.
    fn sfo(fields: &[(&str, SfoValue)]) -> Vec<u8> {
        let n = fields.len();
        let index_len = 0x14 + n * 16;
        let mut keys = Vec::new();
        let mut key_offs = Vec::new();
        for (k, _) in fields {
            key_offs.push(keys.len() as u16);
            keys.extend_from_slice(k.as_bytes());
            keys.push(0);
        }
        while keys.len() % 4 != 0 {
            keys.push(0);
        }
        let mut vals = Vec::new();
        let mut meta = Vec::new();
        for (_, v) in fields {
            let off = vals.len() as u32;
            match v {
                SfoValue::Text(t) => {
                    let mut b = t.as_bytes().to_vec();
                    b.push(0);
                    meta.push((0x0204u16, b.len() as u32, off));
                    vals.extend_from_slice(&b);
                }
                SfoValue::Int(i) => {
                    meta.push((0x0404u16, 4, off));
                    vals.extend_from_slice(&i.to_le_bytes());
                }
            }
        }
        let key_table = index_len as u32;
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

    fn txt(s: &str) -> SfoValue {
        SfoValue::Text(s.into())
    }

    /// A disc rip: the folder holds `PS3_GAME`, which holds the sfo and the icon.
    #[test]
    fn a_disc_rip_folder_yields_its_title_and_its_icon() {
        let root = tmp("rip");
        let game = root.join("PS3_GAME");
        fs::create_dir_all(&game).unwrap();
        fs::write(
            game.join("PARAM.SFO"),
            sfo(&[
                ("CATEGORY", txt("DG")),
                ("TITLE_ID", txt("BLES01925")),
                ("TITLE", txt("The Last of Us")),
                ("APP_VER", txt("01.11")),
                ("PS3_SYSTEM_VER", txt("04.2500")),
            ]),
        )
        .unwrap();
        fs::write(game.join("ICON0.PNG"), PNG_MAGIC.to_vec()).unwrap();

        let dir = ps3_game_dir(&root).expect("recognised as a PS3 game");
        assert_eq!(dir, game);

        let g = read(&dir);
        assert_eq!(g.title.as_deref(), Some("The Last of Us"));
        assert_eq!(g.title_id.as_deref(), Some("BLES01925"));
        assert_eq!(g.category.as_deref(), Some("DG"));
        assert_eq!(g.app_ver.as_deref(), Some("01.11"));
        // Text, not the PS4's BCD packing.
        assert_eq!(g.system_ver.as_deref(), Some("04.2500"));
        assert!(g.icon.is_some(), "the icon is a plain file beside the sfo");

        let _ = fs::remove_dir_all(&root);
    }

    /// Rips come off every filesystem, so capitalisation cannot be relied on.
    #[test]
    fn the_folder_and_file_names_are_matched_case_insensitively() {
        let root = tmp("case");
        let game = root.join("ps3_game");
        fs::create_dir_all(&game).unwrap();
        fs::write(game.join("param.sfo"), sfo(&[("TITLE", txt("Whatever"))])).unwrap();

        let dir = ps3_game_dir(&root).expect("lowercase names are still a PS3 game");
        assert_eq!(read(&dir).title.as_deref(), Some("Whatever"));

        let _ = fs::remove_dir_all(&root);
    }

    /// Pointing straight at the `PS3_GAME` directory must work too.
    #[test]
    fn the_game_directory_itself_is_accepted() {
        let root = tmp("direct");
        fs::write(root.join("PARAM.SFO"), sfo(&[("TITLE", txt("Direct"))])).unwrap();
        assert_eq!(ps3_game_dir(&root).as_deref(), Some(root.as_path()));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn an_ordinary_folder_is_not_a_ps3_game() {
        let root = tmp("plain");
        fs::create_dir_all(root.join("Screenshots")).unwrap();
        fs::write(root.join("readme.txt"), b"hello").unwrap();
        assert!(ps3_game_dir(&root).is_none());
        let _ = fs::remove_dir_all(&root);
    }

    /// An unreadable sfo must not make the folder disappear — the filename still names it.
    #[test]
    fn a_corrupt_sfo_still_leaves_a_usable_game_folder() {
        let root = tmp("corrupt");
        let game = root.join("PS3_GAME");
        fs::create_dir_all(&game).unwrap();
        fs::write(game.join("PARAM.SFO"), b"not a psf at all").unwrap();

        let dir = ps3_game_dir(&root).expect("still a PS3 game folder");
        let g = read(&dir);
        assert_eq!(g.title, None);
        assert_eq!(g.title_id, None);

        let _ = fs::remove_dir_all(&root);
    }

    /// An absurdly large PARAM.SFO is refused rather than read into memory.
    #[test]
    fn an_oversized_sfo_is_refused() {
        let root = tmp("huge");
        let game = root.join("PS3_GAME");
        fs::create_dir_all(&game).unwrap();
        fs::write(game.join("PARAM.SFO"), vec![0u8; (MAX_SFO_LEN + 1) as usize]).unwrap();

        assert!(ps3_game_dir(&root).is_some(), "the folder is still recognised");
        assert_eq!(read(&ps3_game_dir(&root).unwrap()).title, None);

        let _ = fs::remove_dir_all(&root);
    }

    /// PS3 and PS4 packages share an extension and differ by four bytes.
    #[test]
    fn a_package_is_attributed_by_its_magic() {
        assert_eq!(platform_for_pkg_magic(*b"\x7FCNT"), Some(Platform::Ps4));
        assert_eq!(platform_for_pkg_magic(*b"\x7FPKG"), Some(Platform::Ps3));
        assert_eq!(platform_for_pkg_magic(*b"RIFF"), None);
        // A split continuation part is random data, and must stay unattributed.
        assert_eq!(platform_for_pkg_magic([0x11, 0x5f, 0x2b, 0x3f]), None);
    }
}
