//! The copy loop, and the gate that stands between a move and deleting anything.
//!
//! Three properties matter more than speed here:
//!
//! 1. **A crash never leaves a file that looks complete.** Every file is written to a
//!    `.gcpart` stub in the destination directory and renamed into place only once it
//!    is fully written and flushed. An interrupted transfer therefore leaves an
//!    obviously-partial artefact, never a truncated file with the right name.
//! 2. **Data reaches the platter before a file is called copied.** The target drive is
//!    exFAT, which has no journal: an unflushed unplug leaves a directory entry that
//!    looks correct but points at clusters that were never written. `FlushFileBuffers`
//!    after each file is what prevents that.
//! 3. **A move deletes nothing until every file has been verified.** The gate is
//!    evaluated over the whole set, so a failure on the last volume of a five-volume
//!    archive leaves all five sources in place.

use super::plan::{long_path, PlannedFile, Preflight, TransferKind, TransferRequest, VerifyMode, FAT_TIME_TOLERANCE_SECS};
use serde::Serialize;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

/// 8 MiB. Large enough to keep a USB link saturated, small enough that cancelling is
/// responsive and progress stays smooth.
const BUFFER_SIZE: usize = 8 * 1024 * 1024;

/// Progress is coalesced to this interval. Emitting per chunk floods the IPC bridge
/// and makes the window stutter — the work becomes drawing, not copying.
pub const PROGRESS_INTERVAL: Duration = Duration::from_millis(250);

/// Suffix for a file still being written.
const PART_SUFFIX: &str = ".gvpart";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FileState {
    Pending,
    Copying,
    Copied,
    Verified,
    SourceDeleted,
    Skipped,
    Failed,
}

#[derive(Debug, Clone, Serialize)]
pub struct Progress {
    pub current_file: String,
    pub files_done: usize,
    pub files_total: usize,
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub bytes_per_sec: u64,
    pub eta_secs: u64,
    pub phase: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct FileOutcome {
    pub src: String,
    pub dst: String,
    pub size_bytes: u64,
    pub state: FileState,
    pub src_hash: Option<String>,
    pub dst_hash: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TransferOutcome {
    pub kind: TransferKind,
    pub files: Vec<FileOutcome>,
    pub bytes_copied: u64,
    pub elapsed_secs: f64,
    /// True when every non-skipped file reached `Verified`.
    pub all_verified: bool,
    /// Sources actually removed. Empty for a copy, and for any move that failed the gate.
    pub sources_deleted: Vec<String>,
    /// Set when the move gate refused to delete, explaining why.
    pub delete_blocked_reason: Option<String>,
    pub cancelled: bool,
}

/// Shared flags so the UI can pause or cancel a running transfer.
#[derive(Debug, Default)]
pub struct Control {
    pub cancel: AtomicBool,
    pub pause: AtomicBool,
    pub bytes_done: AtomicU64,
}

impl Control {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
    pub fn set_paused(&self, paused: bool) {
        self.pause.store(paused, Ordering::Relaxed);
    }
    fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
    fn is_paused(&self) -> bool {
        self.pause.load(Ordering::Relaxed)
    }
}

/// Force this file's data out of the OS cache and onto the device.
///
/// Without it, a file can be renamed into place and reported as copied while its
/// contents are still buffered. On a journalled filesystem that is survivable; on
/// exFAT an unplug at that moment yields a correct-looking directory entry pointing at
/// garbage. This is the single most important durability call in the engine.
fn flush_to_disk(f: &File) -> std::io::Result<()> {
    use windows_sys::Win32::Storage::FileSystem::FlushFileBuffers;
    // Ask the runtime to flush its own buffers first, then the OS cache.
    let handle = f.as_raw_handle() as windows_sys::Win32::Foundation::HANDLE;
    let ok = unsafe { FlushFileBuffers(handle) };
    if ok == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

fn mtime_secs(p: &Path) -> Option<u64> {
    fs::metadata(long_path(p))
        .ok()?
        .modified()
        .ok()?
        .duration_since(SystemTime::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs())
}

/// Stream a file to its destination, hashing as we go when asked.
///
/// Returns the source hash when hashing was requested. Writes to a `.gvpart` stub and
/// renames only after a successful flush.
fn copy_one(
    src: &Path,
    dst: &Path,
    want_hash: bool,
    ctl: &Control,
    mut on_bytes: impl FnMut(u64),
) -> std::io::Result<Option<String>> {
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(long_path(parent))?;
    }

    let staging = dst.with_extension(format!(
        "{}{}",
        dst.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default(),
        PART_SUFFIX
    ));

    let mut reader = File::open(long_path(src))?;
    let mut writer = File::create(long_path(&staging))?;
    let mut hasher = want_hash.then(blake3::Hasher::new);
    let mut buf = vec![0u8; BUFFER_SIZE];

    loop {
        if ctl.is_cancelled() {
            drop(writer);
            // The stub is left behind deliberately: it marks an interrupted transfer
            // and can be resumed or cleaned up, unlike a silently truncated file.
            return Err(std::io::Error::new(std::io::ErrorKind::Interrupted, "cancelled"));
        }
        while ctl.is_paused() && !ctl.is_cancelled() {
            std::thread::sleep(Duration::from_millis(120));
        }

        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        writer.write_all(&buf[..n])?;
        if let Some(h) = hasher.as_mut() {
            h.update(&buf[..n]);
        }
        on_bytes(n as u64);
    }

    writer.flush()?;
    flush_to_disk(&writer)?;
    drop(writer);

    // Rename into place. On the same volume this is atomic, so the final name appears
    // only once the bytes are durable.
    if dst.exists() {
        fs::remove_file(long_path(dst))?;
    }
    fs::rename(long_path(&staging), long_path(dst))?;

    Ok(hasher.map(|h| h.finalize().to_hex().to_string()))
}

fn hash_file(p: &Path) -> std::io::Result<String> {
    let mut f = File::open(long_path(p))?;
    let mut hasher = blake3::Hasher::new();
    let mut buf = vec![0u8; BUFFER_SIZE];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().to_hex().to_string())
}

/// Verify one copied file according to the requested mode.
fn verify_one(
    file: &PlannedFile,
    mode: VerifyMode,
    src_hash: Option<&str>,
) -> Result<Option<String>, String> {
    let meta = fs::metadata(long_path(&file.dst))
        .map_err(|e| format!("destination unreadable: {e}"))?;

    if meta.len() != file.size_bytes {
        return Err(format!(
            "size mismatch: expected {}, wrote {}",
            file.size_bytes,
            meta.len()
        ));
    }

    match mode {
        VerifyMode::Size => Ok(None),
        VerifyMode::SizeMtime => {
            // Compared with a tolerance because exFAT and FAT store modification times
            // to 2-second granularity; an exact comparison against an NTFS source marks
            // every file as different.
            if let (Some(a), Some(b)) = (mtime_secs(&file.src), mtime_secs(&file.dst)) {
                if a.abs_diff(b) > FAT_TIME_TOLERANCE_SECS {
                    return Err(format!("modification time differs by {}s", a.abs_diff(b)));
                }
            }
            Ok(None)
        }
        VerifyMode::Hash => {
            let dst_hash = hash_file(&file.dst).map_err(|e| format!("could not hash: {e}"))?;
            // The source hash was computed during the copy, so only the destination is
            // re-read here.
            let expected = match src_hash {
                Some(h) => h.to_string(),
                None => hash_file(&file.src).map_err(|e| format!("could not hash source: {e}"))?,
            };
            if dst_hash != expected {
                return Err("content hash mismatch".into());
            }
            Ok(Some(dst_hash))
        }
    }
}

/// Run a transfer that has already passed preflight.
///
/// `on_progress` is called at most every [`PROGRESS_INTERVAL`].
pub fn execute(
    req: &TransferRequest,
    plan: &Preflight,
    ctl: &Control,
    mut on_progress: impl FnMut(Progress),
) -> TransferOutcome {
    let started = Instant::now();
    let want_hash = req.verify == VerifyMode::Hash;

    let active: Vec<&PlannedFile> = plan.files.iter().filter(|f| !f.skipped).collect();
    let bytes_total: u64 = active.iter().map(|f| f.size_bytes).sum();

    let mut outcomes: Vec<FileOutcome> = plan
        .files
        .iter()
        .map(|f| FileOutcome {
            src: f.src.to_string_lossy().into_owned(),
            dst: f.dst.to_string_lossy().into_owned(),
            size_bytes: f.size_bytes,
            state: if f.skipped { FileState::Skipped } else { FileState::Pending },
            src_hash: None,
            dst_hash: None,
            error: None,
        })
        .collect();

    // A same-volume move is a rename: no bytes move, so there is nothing to verify and
    // nothing to delete afterwards.
    if plan.same_volume && req.kind == TransferKind::Move && !req.safe_mode {
        return rename_in_place(req, plan, outcomes, ctl, started);
    }

    let mut bytes_done = 0u64;
    let mut last_tick = Instant::now();
    let mut cancelled = false;

    for (idx, file) in plan.files.iter().enumerate() {
        if outcomes[idx].state == FileState::Skipped {
            continue;
        }
        if ctl.is_cancelled() {
            cancelled = true;
            break;
        }

        let name = file
            .src
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        outcomes[idx].state = FileState::Copying;

        let mut file_bytes = 0u64;
        let result = copy_one(&file.src, &file.dst, want_hash, ctl, |n| {
            file_bytes += n;
            let done = bytes_done + file_bytes;
            ctl.bytes_done.store(done, Ordering::Relaxed);
            if last_tick.elapsed() >= PROGRESS_INTERVAL {
                last_tick = Instant::now();
                let secs = started.elapsed().as_secs_f64().max(0.001);
                let rate = (done as f64 / secs) as u64;
                on_progress(Progress {
                    current_file: name.clone(),
                    files_done: idx,
                    files_total: plan.files.len(),
                    bytes_done: done,
                    bytes_total,
                    bytes_per_sec: rate,
                    eta_secs: if rate > 0 { bytes_total.saturating_sub(done) / rate } else { 0 },
                    phase: "copying",
                });
            }
        });

        match result {
            Ok(src_hash) => {
                bytes_done += file.size_bytes;
                outcomes[idx].state = FileState::Copied;
                outcomes[idx].src_hash = src_hash.clone();

                match verify_one(file, req.verify, src_hash.as_deref()) {
                    Ok(dst_hash) => {
                        outcomes[idx].dst_hash = dst_hash;
                        outcomes[idx].state = FileState::Verified;
                    }
                    Err(e) => {
                        outcomes[idx].state = FileState::Failed;
                        outcomes[idx].error = Some(e);
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {
                cancelled = true;
                outcomes[idx].state = FileState::Pending;
                break;
            }
            Err(e) => {
                outcomes[idx].state = FileState::Failed;
                outcomes[idx].error = Some(e.to_string());
            }
        }
    }

    // The gate. Evaluated across the whole set, so a failure on the final volume of a
    // five-volume archive protects all five sources.
    let unverified: Vec<&FileOutcome> = outcomes
        .iter()
        .filter(|o| o.state != FileState::Verified && o.state != FileState::Skipped)
        .collect();
    let all_verified = unverified.is_empty();

    let mut sources_deleted = Vec::new();
    let mut delete_blocked_reason = None;

    if req.kind == TransferKind::Move {
        if req.safe_mode {
            delete_blocked_reason = Some("Safe mode is on — the source was kept".into());
        } else if cancelled {
            delete_blocked_reason = Some("transfer was cancelled — nothing was deleted".into());
        } else if !all_verified {
            delete_blocked_reason = Some(format!(
                "{} file(s) did not verify — nothing was deleted",
                unverified.len()
            ));
        } else {
            for (idx, file) in plan.files.iter().enumerate() {
                if outcomes[idx].state != FileState::Verified {
                    continue;
                }
                match fs::remove_file(long_path(&file.src)) {
                    Ok(()) => {
                        outcomes[idx].state = FileState::SourceDeleted;
                        sources_deleted.push(file.src.to_string_lossy().into_owned());
                    }
                    Err(e) => {
                        outcomes[idx].error = Some(format!("copied, but source not removed: {e}"));
                    }
                }
            }
            // Remove the now-empty source folder, but only if it really is empty.
            if let Some(parent) = plan.files.first().and_then(|f| f.src.parent()) {
                if req.remove_empty_source_dir {
                    if let Ok(mut entries) = fs::read_dir(long_path(parent)) {
                        if entries.next().is_none() {
                            let _ = fs::remove_dir(long_path(parent));
                        }
                    }
                }
            }
        }
    }

    on_progress(Progress {
        current_file: String::new(),
        files_done: plan.files.len(),
        files_total: plan.files.len(),
        bytes_done,
        bytes_total,
        bytes_per_sec: 0,
        eta_secs: 0,
        phase: if cancelled { "cancelled" } else { "done" },
    });

    TransferOutcome {
        kind: req.kind,
        files: outcomes,
        bytes_copied: bytes_done,
        elapsed_secs: started.elapsed().as_secs_f64(),
        all_verified,
        sources_deleted,
        delete_blocked_reason,
        cancelled,
    }
}

/// Same-volume move: a metadata rename, instant regardless of size.
///
/// All-or-nothing. A multi-part archive is a single unit, so if any volume cannot be
/// renamed the ones already moved are put back. Without the rollback, a failure on
/// volume 3 of 5 would leave the set split across two folders — present in neither
/// place as a usable archive, and far harder to recover from than a refused move.
fn rename_in_place(
    req: &TransferRequest,
    plan: &Preflight,
    mut outcomes: Vec<FileOutcome>,
    ctl: &Control,
    started: Instant,
) -> TransferOutcome {
    let finish = |outcomes: Vec<FileOutcome>,
                  deleted: Vec<String>,
                  all_ok: bool,
                  reason: Option<String>,
                  cancelled: bool| TransferOutcome {
        kind: req.kind,
        files: outcomes,
        bytes_copied: 0,
        elapsed_secs: started.elapsed().as_secs_f64(),
        all_verified: all_ok,
        sources_deleted: deleted,
        delete_blocked_reason: reason,
        cancelled,
    };

    if ctl.is_cancelled() {
        return finish(
            outcomes,
            Vec::new(),
            false,
            Some("transfer was cancelled — nothing was moved".into()),
            true,
        );
    }

    // (index, moved-from, moved-to) for everything renamed so far.
    let mut done: Vec<(usize, PathBuf, PathBuf)> = Vec::new();
    let mut failure: Option<(usize, String)> = None;

    for (idx, file) in plan.files.iter().enumerate() {
        if outcomes[idx].state == FileState::Skipped {
            continue;
        }
        if ctl.is_cancelled() {
            failure = Some((idx, "cancelled".into()));
            break;
        }
        if let Some(parent) = file.dst.parent() {
            let _ = fs::create_dir_all(long_path(parent));
        }
        match fs::rename(long_path(&file.src), long_path(&file.dst)) {
            Ok(()) => done.push((idx, file.src.clone(), file.dst.clone())),
            Err(e) => {
                failure = Some((idx, e.to_string()));
                break;
            }
        }
    }

    if let Some((failed_idx, err)) = failure {
        // Undo, newest first, so the set ends up exactly where it started.
        let mut rollback_failures = Vec::new();
        for (idx, from, to) in done.iter().rev() {
            match fs::rename(long_path(to), long_path(from)) {
                Ok(()) => outcomes[*idx].state = FileState::Pending,
                Err(e) => {
                    outcomes[*idx].state = FileState::Failed;
                    outcomes[*idx].error = Some(format!("could not roll back: {e}"));
                    rollback_failures.push(from.to_string_lossy().into_owned());
                }
            }
        }
        let cancelled = err == "cancelled";
        outcomes[failed_idx].state = FileState::Failed;
        outcomes[failed_idx].error = Some(err.clone());

        let reason = if rollback_failures.is_empty() {
            if cancelled {
                "transfer was cancelled — every file was put back".to_string()
            } else {
                format!("a file could not be moved ({err}) — every file was put back")
            }
        } else {
            format!(
                "a file could not be moved ({err}); {} file(s) could NOT be restored: {}",
                rollback_failures.len(),
                rollback_failures.join(", ")
            )
        };
        return finish(outcomes, Vec::new(), false, Some(reason), cancelled);
    }

    let deleted: Vec<String> = done
        .iter()
        .map(|(_, from, _)| from.to_string_lossy().into_owned())
        .collect();
    for (idx, _, _) in &done {
        outcomes[*idx].state = FileState::SourceDeleted;
    }

    // Clean up the emptied source folder, but only when it really is empty.
    if req.remove_empty_source_dir {
        if let Some(parent) = plan.files.first().and_then(|f| f.src.parent()) {
            if let Ok(mut entries) = fs::read_dir(long_path(parent)) {
                if entries.next().is_none() {
                    let _ = fs::remove_dir(long_path(parent));
                }
            }
        }
    }

    finish(outcomes, deleted, true, None, false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transfer::plan::{preflight, CollisionPolicy};

    fn tmp(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "gv_eng_{tag}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .subsec_nanos()
        ));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    struct Fixture {
        base: PathBuf,
        src: PathBuf,
        dst: PathBuf,
        files: Vec<PathBuf>,
    }

    /// Five volumes with distinct contents, mirroring a real multi-part archive.
    fn multipart(tag: &str) -> Fixture {
        let base = tmp(tag);
        let src = base.join("src");
        let dst = base.join("dst");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&dst).unwrap();
        let mut files = Vec::new();
        for n in 1..=5u8 {
            let f = src.join(format!("Game.part{n}.rar"));
            fs::write(&f, vec![n; 64 * 1024]).unwrap();
            files.push(f);
        }
        Fixture { base, src, dst, files }
    }

    fn request(fx: &Fixture, kind: TransferKind, verify: VerifyMode, safe: bool) -> TransferRequest {
        TransferRequest {
            kind,
            sources: fx.files.clone(),
            source_root: Some(fx.src.clone()),
            dst_dir: fx.dst.clone(),
            folder_name: None,
            remove_empty_source_dir: false,
            verify,
            collision: CollisionPolicy::Rename,
            safe_mode: safe,
        }
    }

    #[test]
    fn a_copy_reproduces_every_byte_and_leaves_the_source_alone() {
        let fx = multipart("copy");
        let req = request(&fx, TransferKind::Copy, VerifyMode::Hash, true);
        let plan = preflight(&req);
        assert!(plan.ok, "{:?}", plan.problems);

        let out = execute(&req, &plan, &Control::default(), |_| {});
        assert!(out.all_verified, "{:#?}", out.files);
        assert!(out.sources_deleted.is_empty(), "a copy must not delete");

        for n in 1..=5u8 {
            let a = fs::read(fx.src.join(format!("Game.part{n}.rar"))).unwrap();
            let b = fs::read(fx.dst.join(format!("Game.part{n}.rar"))).unwrap();
            assert_eq!(a, b, "volume {n} differs");
        }
        let _ = fs::remove_dir_all(&fx.base);
    }

    /// No `.gvpart` stub may survive a successful transfer, or the destination would
    /// accumulate debris that looks like interrupted work.
    #[test]
    fn staging_files_do_not_survive_a_successful_copy() {
        let fx = multipart("staging");
        let req = request(&fx, TransferKind::Copy, VerifyMode::Size, true);
        let plan = preflight(&req);
        execute(&req, &plan, &Control::default(), |_| {});

        let leftovers: Vec<_> = fs::read_dir(&fx.dst)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains(PART_SUFFIX))
            .collect();
        assert!(leftovers.is_empty(), "left staging files: {leftovers:?}");
        let _ = fs::remove_dir_all(&fx.base);
    }

    /// The central safety property: a move deletes nothing unless every file verified.
    #[test]
    fn a_move_deletes_sources_only_after_all_files_verify() {
        let fx = multipart("move_ok");
        let req = request(&fx, TransferKind::Move, VerifyMode::Hash, false);
        let plan = preflight(&req);
        let out = execute(&req, &plan, &Control::default(), |_| {});

        assert!(out.all_verified);
        assert_eq!(out.sources_deleted.len(), 5);
        for n in 1..=5u8 {
            assert!(!fx.src.join(format!("Game.part{n}.rar")).exists(), "source {n} should be gone");
            assert!(fx.dst.join(format!("Game.part{n}.rar")).exists(), "dest {n} should exist");
        }
        let _ = fs::remove_dir_all(&fx.base);
    }

    /// If verification fails for even one volume, all five sources must survive.
    /// Simulated by making a destination file read-only so the copy cannot complete.
    #[test]
    fn a_failure_on_one_volume_protects_every_source() {
        let fx = multipart("move_fail");

        // Pre-create a read-only blocker at the third destination path.
        let blocker = fx.dst.join("Game.part3.rar");
        fs::write(&blocker, b"blocked").unwrap();
        let mut perms = fs::metadata(&blocker).unwrap().permissions();
        perms.set_readonly(true);
        fs::set_permissions(&blocker, perms).unwrap();

        let mut req = request(&fx, TransferKind::Move, VerifyMode::Size, false);
        // Overwrite forces the engine to collide with the read-only file.
        req.collision = CollisionPolicy::Overwrite;
        let plan = preflight(&req);
        let out = execute(&req, &plan, &Control::default(), |_| {});

        assert!(!out.all_verified, "the blocked volume should not verify");
        assert!(out.sources_deleted.is_empty(), "no source may be deleted");
        assert!(out.delete_blocked_reason.is_some());
        for n in 1..=5u8 {
            assert!(
                fx.src.join(format!("Game.part{n}.rar")).exists(),
                "source volume {n} was deleted despite a failed set"
            );
        }

        let mut perms = fs::metadata(&blocker).unwrap().permissions();
        perms.set_readonly(false);
        let _ = fs::set_permissions(&blocker, perms);
        let _ = fs::remove_dir_all(&fx.base);
    }

    /// Safe mode downgrades a move to a copy; this is the default and must hold.
    #[test]
    fn safe_mode_prevents_deletion_even_on_a_verified_move() {
        let fx = multipart("safe");
        let req = request(&fx, TransferKind::Move, VerifyMode::Hash, true);
        let plan = preflight(&req);
        let out = execute(&req, &plan, &Control::default(), |_| {});

        assert!(out.all_verified);
        assert!(out.sources_deleted.is_empty());
        assert!(out.delete_blocked_reason.unwrap().contains("Safe mode"));
        for n in 1..=5u8 {
            assert!(fx.src.join(format!("Game.part{n}.rar")).exists());
        }
        let _ = fs::remove_dir_all(&fx.base);
    }

    #[test]
    fn cancelling_stops_the_transfer_and_deletes_nothing() {
        let fx = multipart("cancel");
        let req = request(&fx, TransferKind::Move, VerifyMode::Size, false);
        let plan = preflight(&req);

        let ctl = Control::default();
        ctl.cancel(); // Cancel before the first byte moves.
        let out = execute(&req, &plan, &ctl, |_| {});

        assert!(out.cancelled);
        assert!(out.sources_deleted.is_empty());
        for n in 1..=5u8 {
            assert!(fx.src.join(format!("Game.part{n}.rar")).exists());
        }
        let _ = fs::remove_dir_all(&fx.base);
    }

    /// A corrupted destination must be caught by hash verification, not reported as
    /// success. Verified by hashing a file whose content does not match its source.
    #[test]
    fn hash_verification_detects_a_content_mismatch() {
        let base = tmp("hashfail");
        let src = base.join("a.bin");
        let dst = base.join("b.bin");
        fs::write(&src, vec![1u8; 4096]).unwrap();
        fs::write(&dst, vec![2u8; 4096]).unwrap(); // same size, different content

        let file = PlannedFile { src: src.clone(), dst: dst.clone(), rel: "b.bin".into(), size_bytes: 4096, skipped: false };
        let err = verify_one(&file, VerifyMode::Hash, None).unwrap_err();
        assert!(err.contains("hash mismatch"), "{err}");

        // Size-only verification cannot see this, which is precisely why hash mode exists.
        assert!(verify_one(&file, VerifyMode::Size, None).is_ok());
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn size_verification_catches_a_truncated_destination() {
        let base = tmp("trunc");
        let src = base.join("a.bin");
        let dst = base.join("b.bin");
        fs::write(&src, vec![1u8; 4096]).unwrap();
        fs::write(&dst, vec![1u8; 100]).unwrap();

        let file = PlannedFile { src, dst, rel: "b.bin".into(), size_bytes: 4096, skipped: false };
        let err = verify_one(&file, VerifyMode::Size, None).unwrap_err();
        assert!(err.contains("size mismatch"), "{err}");
        let _ = fs::remove_dir_all(&base);
    }

    /// A collision under the default policy must not destroy the existing file.
    #[test]
    fn an_existing_destination_file_is_never_silently_replaced() {
        let fx = multipart("collide");
        fs::write(fx.dst.join("Game.part1.rar"), b"precious").unwrap();

        let req = request(&fx, TransferKind::Copy, VerifyMode::Size, true);
        let plan = preflight(&req);
        let out = execute(&req, &plan, &Control::default(), |_| {});

        assert!(out.all_verified, "{:#?}", out.files);
        assert_eq!(
            fs::read(fx.dst.join("Game.part1.rar")).unwrap(),
            b"precious",
            "the pre-existing file must survive"
        );
        assert!(fx.dst.join("Game (2).part1.rar").exists() || fx.dst.join("Game.part1 (2).rar").exists(),
            "the new file should be written under a different name: {:?}",
            fs::read_dir(&fx.dst).unwrap().filter_map(|e| e.ok()).map(|e| e.file_name()).collect::<Vec<_>>());
        let _ = fs::remove_dir_all(&fx.base);
    }

    #[test]
    fn progress_is_reported_with_totals_that_match_the_plan() {
        let fx = multipart("progress");
        let req = request(&fx, TransferKind::Copy, VerifyMode::Size, true);
        let plan = preflight(&req);

        let mut last: Option<Progress> = None;
        let out = execute(&req, &plan, &Control::default(), |p| last = Some(p));

        let final_progress = last.expect("a final progress event is always emitted");
        assert_eq!(final_progress.phase, "done");
        assert_eq!(final_progress.files_total, 5);
        assert_eq!(out.bytes_copied, 5 * 64 * 1024);
        let _ = fs::remove_dir_all(&fx.base);
    }
}
