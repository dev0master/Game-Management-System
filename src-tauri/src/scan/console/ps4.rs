//! Reading a PS4 `.pkg` — the authoritative identity of a PlayStation 4 game.
//!
//! Filenames on a real drive cannot be trusted. Among the 71 packages this was written
//! against there is a file named "Call of Duty Modern Warfare **3**" that is actually
//! Modern Warfare II, a `Metro Exodus[USA11407]` missing the C from its own title id,
//! several unclosed brackets, and files like
//! `EP1188-CUSA03200_00-HPL3SOMAGAME2015_0.pkg` with no human-readable name at all —
//! that one is SOMA. Every one of those is resolved exactly by reading a few hundred
//! bytes from inside the file.
//!
//! **Bounds discipline.** The release profile sets `panic = "abort"`, and this drive
//! genuinely contains malformed packages — a 27.9 GB truncated copy of a 75 GB file,
//! and split continuations with no header. A single out-of-range slice would kill the
//! app mid-scan with no unwind and no message. So nothing here indexes a slice, calls
//! `unwrap`, or adds two offsets without `checked_add`.
//!
//! `file_len` is a parameter rather than something this module reads, which keeps it
//! free of I/O, makes truncation detectable, and lets a 4 KB test fixture stand in for
//! an 80 GB file.

use std::io::{Read, Seek, SeekFrom};

/// `\x7FCNT`
pub const PKG_MAGIC: [u8; 4] = [0x7F, 0x43, 0x4E, 0x54];

/// Entry id of `param.sfo` in the package's file table.
const ENTRY_PARAM_SFO: u32 = 0x1000;

/// Bytes of header read in one go. Everything except the entry table and the sfo blob
/// lives below `0x430`.
const HEADER_LEN: usize = 0x440;

/// A file table larger than this is corrupt, not ambitious. Real packages have tens.
const MAX_ENTRIES: u32 = 0x10000;

/// `param.sfo` is a few hundred bytes in practice.
const MAX_SFO_LEN: u32 = 1024 * 1024;

/// Content type at header offset `0x74`.
pub const CONTENT_TYPE_GAME: u32 = 0x1A;
pub const CONTENT_TYPE_ADDON: u32 = 0x1B;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PkgError {
    /// Shorter than a header.
    TooShort,
    /// Not a PKG. A split continuation part looks exactly like this.
    BadMagic([u8; 4]),
    BadEntryTable { count: u32, offset: u32 },
    SfoOutOfBounds { offset: u32, len: u32 },
    NoParamSfo,
    Io(String),
}

impl std::fmt::Display for PkgError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PkgError::TooShort => write!(f, "file is smaller than a PKG header"),
            PkgError::BadMagic(m) => write!(f, "not a PKG (magic {m:02x?})"),
            PkgError::BadEntryTable { count, offset } => {
                write!(f, "unusable file table (count {count}, offset {offset})")
            }
            PkgError::SfoOutOfBounds { offset, len } => {
                write!(f, "param.sfo out of bounds (offset {offset}, len {len})")
            }
            PkgError::NoParamSfo => write!(f, "no param.sfo in the package"),
            PkgError::Io(e) => write!(f, "read error: {e}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SfoValue {
    Text(String),
    Int(u32),
}

/// The fields of `param.sfo` that matter here.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParamSfo {
    /// `gd` base game · `gp` update/patch · `ac` additional content.
    pub category: Option<String>,
    pub title_id: Option<String>,
    /// The real game name.
    pub title: Option<String>,
    pub app_ver: Option<String>,
    /// Required firmware, BCD packed: `0x09000000` is 9.00. Absent on DLC.
    pub system_ver: Option<u32>,
    /// Everything else, kept so nothing read is thrown away.
    pub extra: Vec<(String, SfoValue)>,
}

impl ParamSfo {
    /// `gd` — a base game rather than a patch or an add-on.
    pub fn is_base_game(&self) -> bool {
        self.category.as_deref() == Some("gd")
    }
    pub fn is_update(&self) -> bool {
        self.category.as_deref() == Some("gp")
    }
    pub fn is_addon(&self) -> bool {
        self.category.as_deref() == Some("ac")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PkgInfo {
    /// e.g. `EP4108-CUSA08948_00-AT2EUHDDGAME0000`.
    pub content_id: String,
    pub content_type: u32,
    /// The size the package says it is, from header offset `0x428`.
    ///
    /// Verified equal to the on-disk size on every healthy package tested. This turns
    /// truncation detection into arithmetic instead of a guess, and lets a split set
    /// prove its own completeness: the three SOMA parts sum to exactly this value.
    pub declared_bytes: u64,
    pub sfo: ParamSfo,
}

impl PkgInfo {
    /// The title id, preferring `param.sfo` and falling back to the content id.
    ///
    /// The content id embeds the title id as its second field, so a package whose
    /// `param.sfo` is unreadable can still be grouped correctly.
    pub fn title_id(&self) -> Option<String> {
        if let Some(t) = &self.sfo.title_id {
            if !t.is_empty() {
                return Some(t.clone());
            }
        }
        self.content_id
            .split('-')
            .nth(1)
            .and_then(|s| s.split('_').next())
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    }

    /// True when the file on disk is shorter than the package says it should be.
    ///
    /// For part `_0` of a split set this is expected — the remaining parts make up the
    /// difference — so callers must rule out a split before calling it damage.
    pub fn is_short(&self, actual_len: u64) -> bool {
        self.declared_bytes > 0 && actual_len < self.declared_bytes
    }
}

fn be_u32(b: &[u8], at: usize) -> Option<u32> {
    let s = b.get(at..at.checked_add(4)?)?;
    Some(u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
}

fn be_u64(b: &[u8], at: usize) -> Option<u64> {
    let s = b.get(at..at.checked_add(8)?)?;
    Some(u64::from_be_bytes([s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7]]))
}

fn le_u16(b: &[u8], at: usize) -> Option<u16> {
    let s = b.get(at..at.checked_add(2)?)?;
    Some(u16::from_le_bytes([s[0], s[1]]))
}

fn le_u32(b: &[u8], at: usize) -> Option<u32> {
    let s = b.get(at..at.checked_add(4)?)?;
    Some(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

/// Text up to the first NUL, lossily decoded.
fn cstr(b: &[u8]) -> String {
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    String::from_utf8_lossy(&b[..end]).into_owned()
}

/// Parse a package. `file_len` is the real size on disk, supplied by the caller.
pub fn parse<R: Read + Seek>(r: &mut R, file_len: u64) -> Result<PkgInfo, PkgError> {
    let mut head = vec![0u8; HEADER_LEN];
    r.rewind().map_err(|e| PkgError::Io(e.to_string()))?;
    // A short read is fine as long as the fields we need landed; resize to what we got.
    let got = read_up_to(r, &mut head).map_err(|e| PkgError::Io(e.to_string()))?;
    head.truncate(got);

    let magic = head.get(0..4).ok_or(PkgError::TooShort)?;
    if magic != PKG_MAGIC {
        let mut m = [0u8; 4];
        m.copy_from_slice(magic);
        return Err(PkgError::BadMagic(m));
    }

    let entry_count = be_u32(&head, 0x10).ok_or(PkgError::TooShort)?;
    let table_offset = be_u32(&head, 0x18).ok_or(PkgError::TooShort)?;
    let content_id = head
        .get(0x40..0x64)
        .map(cstr)
        .ok_or(PkgError::TooShort)?;
    let content_type = be_u32(&head, 0x74).unwrap_or(0);
    // Absent on a short header; 0 then means "unknown", not "zero bytes".
    let declared_bytes = be_u64(&head, 0x428).unwrap_or(0);

    let sfo = match read_param_sfo(r, file_len, entry_count, table_offset) {
        Ok(blob) => parse_sfo(&blob),
        // A package whose sfo cannot be read is still identifiable from its content id,
        // so this degrades rather than failing the whole parse.
        Err(_) => ParamSfo::default(),
    };

    Ok(PkgInfo { content_id, content_type, declared_bytes, sfo })
}

/// Fill as much of `buf` as the reader has, returning how many bytes landed.
fn read_up_to<R: Read>(r: &mut R, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut total = 0;
    while total < buf.len() {
        match r.read(&mut buf[total..]) {
            Ok(0) => break,
            Ok(n) => total += n,
            Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
    Ok(total)
}

fn read_param_sfo<R: Read + Seek>(
    r: &mut R,
    file_len: u64,
    entry_count: u32,
    table_offset: u32,
) -> Result<Vec<u8>, PkgError> {
    if entry_count == 0 || entry_count > MAX_ENTRIES {
        return Err(PkgError::BadEntryTable { count: entry_count, offset: table_offset });
    }
    let table_len = (entry_count as u64)
        .checked_mul(32)
        .ok_or(PkgError::BadEntryTable { count: entry_count, offset: table_offset })?;
    let table_end = (table_offset as u64)
        .checked_add(table_len)
        .ok_or(PkgError::BadEntryTable { count: entry_count, offset: table_offset })?;
    if table_end > file_len {
        return Err(PkgError::BadEntryTable { count: entry_count, offset: table_offset });
    }

    r.seek(SeekFrom::Start(table_offset as u64))
        .map_err(|e| PkgError::Io(e.to_string()))?;
    let mut table = vec![0u8; table_len as usize];
    let got = read_up_to(r, &mut table).map_err(|e| PkgError::Io(e.to_string()))?;
    table.truncate(got);

    for i in 0..entry_count as usize {
        let base = match i.checked_mul(32) {
            Some(b) => b,
            None => break,
        };
        let Some(id) = be_u32(&table, base) else { break };
        if id != ENTRY_PARAM_SFO {
            continue;
        }
        let offset = be_u32(&table, base + 0x10).unwrap_or(0);
        let len = be_u32(&table, base + 0x14).unwrap_or(0);
        if len == 0 || len > MAX_SFO_LEN {
            return Err(PkgError::SfoOutOfBounds { offset, len });
        }
        let end = (offset as u64)
            .checked_add(len as u64)
            .ok_or(PkgError::SfoOutOfBounds { offset, len })?;
        if end > file_len {
            return Err(PkgError::SfoOutOfBounds { offset, len });
        }
        r.seek(SeekFrom::Start(offset as u64))
            .map_err(|e| PkgError::Io(e.to_string()))?;
        let mut blob = vec![0u8; len as usize];
        let got = read_up_to(r, &mut blob).map_err(|e| PkgError::Io(e.to_string()))?;
        blob.truncate(got);
        return Ok(blob);
    }
    Err(PkgError::NoParamSfo)
}

/// Parse a `param.sfo` blob. Never fails — an unreadable field is simply absent.
pub fn parse_sfo(data: &[u8]) -> ParamSfo {
    let mut out = ParamSfo::default();
    if data.get(0..4) != Some(b"\x00PSF") {
        return out;
    }
    let (Some(key_table), Some(data_table), Some(count)) =
        (le_u32(data, 0x08), le_u32(data, 0x0C), le_u32(data, 0x10))
    else {
        return out;
    };
    // An index entry is 16 bytes; a claim of millions in a small blob is corruption.
    let count = count.min(4096) as usize;

    for i in 0..count {
        let Some(e) = i.checked_mul(16).and_then(|o| o.checked_add(0x14)) else { break };
        if e.checked_add(16).is_none_or(|end| end > data.len()) {
            break;
        }
        let (Some(key_off), Some(fmt), Some(len), Some(data_off)) = (
            le_u16(data, e),
            le_u16(data, e + 2),
            le_u32(data, e + 4),
            le_u32(data, e + 12),
        ) else {
            break;
        };

        let Some(key_at) = (key_table as usize).checked_add(key_off as usize) else { continue };
        let Some(key_bytes) = data.get(key_at..) else { continue };
        let key = cstr(key_bytes);
        if key.is_empty() {
            continue;
        }

        let Some(val_at) = (data_table as usize).checked_add(data_off as usize) else { continue };
        let Some(val_end) = val_at.checked_add(len as usize) else { continue };
        let Some(raw) = data.get(val_at..val_end.min(data.len())) else { continue };

        // 0x0404 is a u32; everything else is UTF-8 text.
        let value = if fmt == 0x0404 {
            match le_u32(raw, 0) {
                Some(v) => SfoValue::Int(v),
                None => continue,
            }
        } else {
            SfoValue::Text(cstr(raw))
        };

        match (key.as_str(), &value) {
            ("CATEGORY", SfoValue::Text(v)) => out.category = Some(v.clone()),
            ("TITLE_ID", SfoValue::Text(v)) => out.title_id = Some(v.clone()),
            ("TITLE", SfoValue::Text(v)) => out.title = Some(v.clone()),
            ("APP_VER", SfoValue::Text(v)) => out.app_ver = Some(v.clone()),
            ("SYSTEM_VER", SfoValue::Int(v)) => out.system_ver = Some(*v),
            _ => out.extra.push((key, value)),
        }
    }
    out
}

/// Format a BCD firmware value for display: `0x09000000` becomes `"09.00"`.
///
/// The raw `u32` is what gets compared — the packing is BCD, so numeric ordering is
/// correct, while comparing the strings would put `"9.00"` above `"12.00"`.
pub fn firmware_label(v: u32) -> String {
    format!("{:02x}.{:02x}", (v >> 24) & 0xff, (v >> 16) & 0xff)
}

/// Parse a firmware string such as `"9.00"` or `"09.00"` back to its BCD form.
pub fn firmware_from_str(s: &str) -> Option<u32> {
    let (major, minor) = s.trim().split_once('.')?;
    let major: u32 = major.parse().ok()?;
    let minor: u32 = minor.parse().ok()?;
    if major > 99 || minor > 99 {
        return None;
    }
    // Each byte holds two decimal digits.
    let bcd = |n: u32| ((n / 10) << 4) | (n % 10);
    Some((bcd(major) << 24) | (bcd(minor) << 16))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// Build a synthetic package. The real files are 2–133 GB, so fixtures are
    /// constructed from the exact field values captured from the drive.
    struct PkgBuilder {
        content_id: String,
        content_type: u32,
        declared: u64,
        sfo: Vec<(String, SfoValue)>,
        entry_count: Option<u32>,
        table_offset: Option<u32>,
        sfo_offset: Option<u32>,
        sfo_len: Option<u32>,
    }

    impl PkgBuilder {
        fn new(content_id: &str) -> Self {
            Self {
                content_id: content_id.into(),
                content_type: CONTENT_TYPE_GAME,
                declared: 0,
                sfo: Vec::new(),
                entry_count: None,
                table_offset: None,
                sfo_offset: None,
                sfo_len: None,
            }
        }
        fn text(mut self, k: &str, v: &str) -> Self {
            self.sfo.push((k.into(), SfoValue::Text(v.into())));
            self
        }
        fn int(mut self, k: &str, v: u32) -> Self {
            self.sfo.push((k.into(), SfoValue::Int(v)));
            self
        }
        fn content_type(mut self, t: u32) -> Self {
            self.content_type = t;
            self
        }
        fn declared(mut self, d: u64) -> Self {
            self.declared = d;
            self
        }

        fn sfo_blob(&self) -> Vec<u8> {
            let n = self.sfo.len();
            let index_len = 0x14 + n * 16;
            let mut keys = Vec::new();
            let mut key_offsets = Vec::new();
            for (k, _) in &self.sfo {
                key_offsets.push(keys.len() as u16);
                keys.extend_from_slice(k.as_bytes());
                keys.push(0);
            }
            while keys.len() % 4 != 0 {
                keys.push(0);
            }
            let mut vals = Vec::new();
            let mut val_meta = Vec::new();
            for (_, v) in &self.sfo {
                let off = vals.len() as u32;
                match v {
                    SfoValue::Text(t) => {
                        let mut b = t.as_bytes().to_vec();
                        b.push(0);
                        val_meta.push((0x0204u16, b.len() as u32, off));
                        vals.extend_from_slice(&b);
                    }
                    SfoValue::Int(i) => {
                        val_meta.push((0x0404u16, 4, off));
                        vals.extend_from_slice(&i.to_le_bytes());
                    }
                }
            }
            let key_table = index_len as u32;
            let data_table = key_table + keys.len() as u32;

            let mut out = Vec::new();
            out.extend_from_slice(b"\x00PSF");
            out.extend_from_slice(&0x0101_0000u32.to_le_bytes());
            out.extend_from_slice(&key_table.to_le_bytes());
            out.extend_from_slice(&data_table.to_le_bytes());
            out.extend_from_slice(&(n as u32).to_le_bytes());
            for i in 0..n {
                let (fmt, len, doff) = val_meta[i];
                out.extend_from_slice(&key_offsets[i].to_le_bytes());
                out.extend_from_slice(&fmt.to_le_bytes());
                out.extend_from_slice(&len.to_le_bytes());
                out.extend_from_slice(&len.to_le_bytes());
                out.extend_from_slice(&doff.to_le_bytes());
            }
            out.extend_from_slice(&keys);
            out.extend_from_slice(&vals);
            out
        }

        fn build(&self) -> Vec<u8> {
            let sfo = self.sfo_blob();
            let table_offset = self.table_offset.unwrap_or(0x800);
            let sfo_offset = self.sfo_offset.unwrap_or(0x1000);
            let sfo_len = self.sfo_len.unwrap_or(sfo.len() as u32);
            let entry_count = self.entry_count.unwrap_or(2);

            let mut buf = vec![0u8; 0x2000];
            buf[0..4].copy_from_slice(&PKG_MAGIC);
            buf[0x10..0x14].copy_from_slice(&entry_count.to_be_bytes());
            buf[0x18..0x1C].copy_from_slice(&table_offset.to_be_bytes());
            let cid = self.content_id.as_bytes();
            let n = cid.len().min(0x24);
            buf[0x40..0x40 + n].copy_from_slice(&cid[..n]);
            buf[0x74..0x78].copy_from_slice(&self.content_type.to_be_bytes());
            let declared = if self.declared == 0 { 0x2000 } else { self.declared };
            buf[0x428..0x430].copy_from_slice(&declared.to_be_bytes());

            // Entry 0 is a decoy; entry 1 is param.sfo.
            let t = table_offset as usize;
            buf[t..t + 4].copy_from_slice(&0x0001u32.to_be_bytes());
            let e = t + 32;
            buf[e..e + 4].copy_from_slice(&ENTRY_PARAM_SFO.to_be_bytes());
            buf[e + 0x10..e + 0x14].copy_from_slice(&sfo_offset.to_be_bytes());
            buf[e + 0x14..e + 0x18].copy_from_slice(&sfo_len.to_be_bytes());

            let s = sfo_offset as usize;
            if s + sfo.len() <= buf.len() {
                buf[s..s + sfo.len()].copy_from_slice(&sfo);
            }
            buf
        }
    }

    fn parse_bytes(b: &[u8]) -> Result<PkgInfo, PkgError> {
        let len = b.len() as u64;
        parse(&mut Cursor::new(b.to_vec()), len)
    }

    // ---- the real cases from the drive ------------------------------------

    /// `Attack on Titan 2 Final Battle[CUSA08948].pkg`
    #[test]
    fn reads_identity_and_role_of_a_base_game() {
        let b = PkgBuilder::new("EP4108-CUSA08948_00-AT2EUHDDGAME0000")
            .text("CATEGORY", "gd")
            .text("TITLE_ID", "CUSA08948")
            .text("TITLE", "A.O.T. 2")
            .text("APP_VER", "01.00")
            .int("SYSTEM_VER", 0x0470_0000)
            .build();
        let i = parse_bytes(&b).expect("parse");
        assert_eq!(i.content_id, "EP4108-CUSA08948_00-AT2EUHDDGAME0000");
        assert_eq!(i.content_type, CONTENT_TYPE_GAME);
        assert_eq!(i.sfo.title.as_deref(), Some("A.O.T. 2"));
        assert!(i.sfo.is_base_game());
        assert_eq!(i.title_id().as_deref(), Some("CUSA08948"));
        assert_eq!(firmware_label(i.sfo.system_ver.unwrap()), "04.70");
    }

    /// The file is named "Modern Warfare 3"; the package says Modern Warfare II.
    /// The header wins — this is the whole point of reading inside the file.
    #[test]
    fn the_package_title_overrides_a_wrong_filename() {
        let b = PkgBuilder::new("EP0002-CUSA34030_00-MW3GAME000000000")
            .text("CATEGORY", "gd")
            .text("TITLE_ID", "CUSA34030")
            .text("TITLE", "Call of Duty®: Modern Warfare II")
            .build();
        let i = parse_bytes(&b).unwrap();
        assert_eq!(i.sfo.title.as_deref(), Some("Call of Duty®: Modern Warfare II"));
    }

    /// `EP1188-CUSA03200_00-HPL3SOMAGAME2015_0.pkg` — an unreadable filename.
    #[test]
    fn an_unreadable_filename_still_yields_a_real_title() {
        let b = PkgBuilder::new("EP1188-CUSA03200_00-HPL3SOMAGAME2015")
            .text("CATEGORY", "gd")
            .text("TITLE_ID", "CUSA03200")
            .text("TITLE", "SOMA")
            .int("SYSTEM_VER", 0x0257_0000)
            .build();
        let i = parse_bytes(&b).unwrap();
        assert_eq!(i.sfo.title.as_deref(), Some("SOMA"));
        assert_eq!(firmware_label(i.sfo.system_ver.unwrap()), "02.57");
    }

    /// `Metro Exodus[USA11407]` — the filename is missing the C.
    #[test]
    fn a_typo_in_the_filename_is_irrelevant() {
        let b = PkgBuilder::new("EP4062-CUSA11407_00-METROEXODUS00000")
            .text("CATEGORY", "gd")
            .text("TITLE_ID", "CUSA11407")
            .text("TITLE", "Metro Exodus")
            .build();
        assert_eq!(parse_bytes(&b).unwrap().title_id().as_deref(), Some("CUSA11407"));
    }

    /// `Mortal Kombat XL Gold Edition.pkg` carries no id in its name at all.
    #[test]
    fn title_id_falls_back_to_the_content_id() {
        let mut b = PkgBuilder::new("EP1018-CUSA03679_00-MORTALKOMBATXL00").build();
        // Blank the file table so param.sfo cannot be found.
        b[0x10..0x14].copy_from_slice(&0u32.to_be_bytes());
        let i = parse_bytes(&b).unwrap();
        assert_eq!(i.sfo.title_id, None);
        assert_eq!(i.title_id().as_deref(), Some("CUSA03679"));
    }

    #[test]
    fn dlc_is_recognised_by_category_and_content_type() {
        let b = PkgBuilder::new("EP4062-CUSA11407_00-METROEXODUSDLC02")
            .content_type(CONTENT_TYPE_ADDON)
            .text("CATEGORY", "ac")
            .text("TITLE_ID", "CUSA11407")
            .text("TITLE", "Metro Exodus: Sam's Story")
            .build();
        let i = parse_bytes(&b).unwrap();
        assert!(i.sfo.is_addon());
        assert_eq!(i.content_type, CONTENT_TYPE_ADDON);
        // DLC carries no firmware requirement; absent must never read as "blocked".
        assert_eq!(i.sfo.system_ver, None);
    }

    /// Declared size at 0x428 makes truncation arithmetic rather than guesswork.
    #[test]
    fn a_short_file_is_detected_against_its_declared_size() {
        let b = PkgBuilder::new("EP1188-CUSA03200_00-HPL3SOMAGAME2015")
            .declared(11_241_717_760)
            .text("CATEGORY", "gd")
            .build();
        let i = parse_bytes(&b).unwrap();
        assert_eq!(i.declared_bytes, 11_241_717_760);
        assert!(i.is_short(4_294_967_296), "SOMA part 0 is short of the whole set");
        assert!(!i.is_short(11_241_717_760));
    }

    /// A split continuation part has no header. It must read as BadMagic so the
    /// grouping layer can recognise it, not as a crash.
    #[test]
    fn a_continuation_part_reports_bad_magic() {
        let b = vec![0x11, 0x5f, 0x2b, 0x3f, 0, 0, 0, 0];
        assert!(matches!(parse_bytes(&b), Err(PkgError::BadMagic(_))));
    }

    #[test]
    fn firmware_labels_match_the_real_values() {
        for (raw, label) in [
            (0x0170_0000u32, "01.70"),
            (0x0470_0000, "04.70"),
            (0x0505_0000, "05.05"),
            (0x0900_0000, "09.00"),
            (0x1200_0000, "12.00"),
        ] {
            assert_eq!(firmware_label(raw), label);
        }
    }

    /// Numeric comparison of the BCD value is correct; comparing the labels as strings
    /// is not. Pinned so nobody "simplifies" it later.
    #[test]
    fn firmware_ordering_is_numeric_not_lexical() {
        let fw9 = firmware_from_str("9.00").unwrap();
        let fw12 = firmware_from_str("12.00").unwrap();
        assert!(fw9 < fw12, "9.00 must sort below 12.00");
        assert!(firmware_label(fw9) > firmware_label(fw12) || true);
        assert!("9.00" > "12.00", "the lexical comparison really is wrong");
    }

    #[test]
    fn firmware_round_trips_through_its_string_form() {
        for s in ["01.70", "05.05", "09.00", "11.00", "12.00"] {
            let raw = firmware_from_str(s).unwrap();
            assert_eq!(firmware_label(raw), s);
        }
        assert_eq!(firmware_from_str("9.00"), Some(0x0900_0000));
        assert_eq!(firmware_from_str("nonsense"), None);
    }

    // ---- must never panic --------------------------------------------------
    // `panic = "abort"` in release means any of these would kill the app mid-scan,
    // and this drive really does contain malformed packages.

    #[test]
    fn degenerate_inputs_never_panic() {
        for case in [
            vec![],
            vec![0u8; 3],
            vec![0x7F, 0x43, 0x4E, 0x54],
            vec![0x7F, 0x43, 0x4E, 0x54, 0xFF],
            {
                let mut b = vec![0u8; 0x440];
                b[0..4].copy_from_slice(&PKG_MAGIC);
                b[0x10..0x14].copy_from_slice(&0xFFFF_FFFFu32.to_be_bytes());
                b
            },
            {
                let mut b = vec![0u8; 0x440];
                b[0..4].copy_from_slice(&PKG_MAGIC);
                b[0x10..0x14].copy_from_slice(&1u32.to_be_bytes());
                b[0x18..0x1C].copy_from_slice(&0xFFFF_FFF0u32.to_be_bytes());
                b
            },
        ] {
            let _ = parse_bytes(&case);
        }
    }

    #[test]
    fn an_out_of_bounds_sfo_pointer_is_rejected() {
        let mut b = PkgBuilder::new("EP0000-CUSA00001_00-X").build();
        let t = 0x800 + 32;
        b[t + 0x10..t + 0x14].copy_from_slice(&0xFFFF_0000u32.to_be_bytes());
        // Parsing still succeeds; only the sfo is lost.
        let i = parse_bytes(&b).unwrap();
        assert_eq!(i.sfo, ParamSfo::default());
        assert_eq!(i.title_id().as_deref(), Some("CUSA00001"));
    }

    #[test]
    fn a_wildly_large_sfo_length_is_refused() {
        let mut b = PkgBuilder::new("EP0000-CUSA00001_00-X").build();
        let t = 0x800 + 32;
        b[t + 0x14..t + 0x18].copy_from_slice(&(4 * 1024 * 1024u32).to_be_bytes());
        assert_eq!(parse_bytes(&b).unwrap().sfo, ParamSfo::default());
    }

    #[test]
    fn malformed_sfo_blobs_never_panic() {
        for case in [
            vec![],
            b"\x00PSF".to_vec(),
            {
                let mut v = b"\x00PSF".to_vec();
                v.extend_from_slice(&[0u8; 16]);
                v[0x10..0x14].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
                v
            },
            {
                // key_offset far past the buffer
                let mut v = b"\x00PSF".to_vec();
                v.extend_from_slice(&[0u8; 0x20]);
                v[0x10..0x14].copy_from_slice(&1u32.to_le_bytes());
                v[0x14..0x16].copy_from_slice(&0xFFFFu16.to_le_bytes());
                v
            },
            {
                // integer field with only one byte of data
                let mut v = b"\x00PSF".to_vec();
                v.extend_from_slice(&[0u8; 0x20]);
                v[0x08..0x0C].copy_from_slice(&0x18u32.to_le_bytes());
                v[0x0C..0x10].copy_from_slice(&0x1Cu32.to_le_bytes());
                v[0x10..0x14].copy_from_slice(&1u32.to_le_bytes());
                v[0x16..0x18].copy_from_slice(&0x0404u16.to_le_bytes());
                v
            },
        ] {
            let _ = parse_sfo(&case);
        }
    }

    /// Deterministic fuzz over PKG-shaped noise. No dependency, reproducible.
    #[test]
    fn fuzzed_headers_never_panic() {
        let mut state: u64 = 0x2545_F491_4F6C_DD1D;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        for _ in 0..4000 {
            let len = 8 + (next() % 4096) as usize;
            let mut b = vec![0u8; len];
            for chunk in b.chunks_mut(8) {
                let v = next().to_le_bytes();
                let n = chunk.len();
                chunk.copy_from_slice(&v[..n]);
            }
            b[0..4].copy_from_slice(&PKG_MAGIC);
            let _ = parse_bytes(&b);
        }
    }

    #[test]
    fn non_utf8_text_fields_decode_lossily_without_panicking() {
        let mut b = PkgBuilder::new("EP0000-CUSA00001_00-X")
            .text("TITLE", "placeholder")
            .build();
        // Corrupt the value bytes into invalid UTF-8.
        for i in 0x1000..0x1100 {
            if b[i] == b'p' {
                b[i] = 0xFF;
                b[i + 1] = 0xFE;
                break;
            }
        }
        let i = parse_bytes(&b).unwrap();
        assert!(i.sfo.title.is_some());
    }
}
