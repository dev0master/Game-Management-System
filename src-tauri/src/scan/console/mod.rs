//! Console game formats: reading identity from inside the container.

pub mod group;
pub mod ps4;

use std::path::Path;

/// Read every package in one directory.
///
/// Strictly read-only: each file is opened, a few hundred bytes are read from its
/// header, and it is closed. Nothing on the scanned drive is written, moved or removed.
pub fn probe_ps4_dir(dir: &Path) -> std::io::Result<(Vec<group::ProbedFile>, Vec<(String, u64)>)> {
    let mut probed = Vec::new();
    let mut covers = Vec::new();

    for entry in std::fs::read_dir(dir)? {
        let Ok(entry) = entry else { continue };
        let Ok(ft) = entry.file_type() else { continue };
        if !ft.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let lower = name.to_lowercase();
        let (size, mtime) = match entry.metadata() {
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

        if lower.ends_with(".jpg") || lower.ends_with(".jpeg") || lower.ends_with(".png") {
            covers.push((name, size));
            continue;
        }
        if !lower.ends_with(".pkg") {
            continue;
        }

        let (status, info) = match std::fs::File::open(entry.path()) {
            Ok(mut f) => match ps4::parse(&mut f, size) {
                Ok(i) => (group::ProbeStatus::Ok, Some(i)),
                // A parse failure is data, not an error: it is how a continuation part
                // and a truncated copy are told apart from a healthy package.
                Err(_) => (group::ProbeStatus::NotPkg, None),
            },
            Err(_) => (group::ProbeStatus::Unreadable, None),
        };

        probed.push(group::ProbedFile { name, size_bytes: size, mtime_unix: mtime, status, info });
    }

    probed.sort_by(|a, b| a.name.cmp(&b.name));
    covers.sort();
    Ok((probed, covers))
}
