//! Scan harness.
//!
//! Runs the real scanner over a real path and prints what it found, so classifier
//! behaviour can be checked against actual drives rather than only fixtures.
//!
//!     cargo run -- E:\
//!     cargo run -- E:\ --all      # include rejected items with their reasons

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use gamevault_lib::scan::classify::{ClassifyConfig, ItemKind, Verdict};
use gamevault_lib::scan::walker;
use std::path::PathBuf;
use std::time::Instant;

/// Reattach to the terminal that launched us.
///
/// The release build is a `windows` subsystem binary so launching the app never flashes
/// a console. That also means `println!` goes nowhere when the same exe is run with CLI
/// arguments, so the diagnostic modes attach to the parent console first.
#[cfg(not(debug_assertions))]
fn attach_console() {
    use windows_sys::Win32::System::Console::{AttachConsole, ATTACH_PARENT_PROCESS};
    unsafe { AttachConsole(ATTACH_PARENT_PROCESS) };
}

#[cfg(debug_assertions)]
fn attach_console() {}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if !args.is_empty() {
        attach_console();
    }

    // No arguments launches the app. The scan harness stays available behind flags so
    // classifier behaviour can be checked against a real drive without the UI.
    if args.is_empty() {
        gamevault_lib::run();
        return;
    }

    let show_all = args.iter().any(|a| a == "--all");

    if args.iter().any(|a| a == "--console") {
        let Some(path) = args.iter().find(|a| !a.starts_with("--")) else {
            eprintln!("usage: gamevault --console <path>");
            std::process::exit(2);
        };
        print_console(path);
        return;
    }

    if args.iter().any(|a| a == "--drives") {
        print_drives();
        return;
    }

    // Scan and write to the catalogue without opening the window — useful for
    // indexing a drive from a script, and for checking what the app will display.
    if args.iter().any(|a| a == "--index") {
        let Some(path) = args.iter().find(|a| !a.starts_with("--")) else {
            eprintln!("usage: gamevault --index <path>");
            std::process::exit(2);
        };
        index_path(path);
        return;
    }

    let Some(root) = args.iter().find(|a| !a.starts_with("--")) else {
        eprintln!("usage: gamevault             launch the app");
        eprintln!("       gamevault <path>      scan a path and print findings");
        eprintln!("       gamevault --drives    list volumes with their identity");
        std::process::exit(2);
    };
    let root = PathBuf::from(root);
    if !root.exists() {
        eprintln!("path not found: {}", root.display());
        std::process::exit(1);
    }

    let started = Instant::now();
    let res = walker::scan(&root, &ClassifyConfig::default());
    let elapsed = started.elapsed();

    println!("\nScanned {} in {:.2?}", root.display(), elapsed);
    println!(
        "  {} dirs, {} files, {} on disk\n",
        res.stats.dirs_seen,
        res.stats.files_seen,
        human(res.stats.bytes_seen)
    );

    let mut games: Vec<_> = res.items.iter().filter(|i| i.verdict == Verdict::Game).collect();
    let review: Vec<_> = res.items.iter().filter(|i| i.verdict == Verdict::NeedsReview).collect();
    let rejected: Vec<_> = res.items.iter().filter(|i| i.verdict == Verdict::NotGame).collect();
    games.sort_by_key(|i| std::cmp::Reverse(i.total_bytes));

    println!("=== GAMES ({}) ===", games.len());
    for i in &games {
        let parts = if i.files.len() > 1 {
            format!(", {} volumes", i.files.len())
        } else {
            String::new()
        };
        let tags = [
            i.title.repacker.as_ref().map(|r| format!("repacker: {r}")),
            i.title.release_group.as_ref().map(|g| format!("group: {g}")),
            i.title.year.map(|y| format!("year: {y}")),
            i.title.edition.as_ref().map(|e| format!("edition: {e}")),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(", ");

        println!(
            "  {:<38} {:>10}{}  [{:?}, {:.2}]",
            truncate(&i.title.clean_title, 38),
            human(i.total_bytes),
            parts,
            i.kind,
            i.confidence
        );
        println!("      path: {}", i.rel_path);
        if !tags.is_empty() {
            println!("      {tags}");
        }
        if let Some(set) = &i.archive_set {
            if let Some(reason) = &set.incomplete_reason {
                println!("      WARNING: {reason}");
            }
        }
    }

    if !review.is_empty() {
        println!("\n=== NEEDS REVIEW ({}) ===", review.len());
        for i in &review {
            println!(
                "  {:<38} {:>10}  [{:.2}]  {}",
                truncate(&i.title.clean_title, 38),
                human(i.total_bytes),
                i.confidence,
                i.rel_path
            );
            for r in &i.classification.reasons {
                println!("      {:+.2}  {}", r.delta, r.note);
            }
        }
    }

    // Rejected items are counted always and listed on request. They are never dropped:
    // each one keeps a row so it can be reviewed and promoted.
    let mut by_kind: Vec<(ItemKind, usize, u64)> = Vec::new();
    for i in &rejected {
        match by_kind.iter_mut().find(|(k, _, _)| *k == i.kind) {
            Some(e) => {
                e.1 += 1;
                e.2 += i.total_bytes;
            }
            None => by_kind.push((i.kind, 1, i.total_bytes)),
        }
    }
    by_kind.sort_by_key(|(_, _, b)| std::cmp::Reverse(*b));

    println!("\n=== FILTERED OUT ({}) ===", rejected.len());
    for (kind, n, bytes) in &by_kind {
        println!("  {:<16} {:>3} items  {:>10}", format!("{kind:?}"), n, human(*bytes));
    }
    if show_all {
        for i in &rejected {
            println!("  - {:<50} {:>10}  {}", truncate(&i.rel_path, 50), human(i.total_bytes),
                i.classification.reasons.first().map(|r| r.note.as_str()).unwrap_or(""));
        }
    }
    println!();
}

fn human(b: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = b as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 { format!("{b} B") } else { format!("{v:.1} {}", UNITS[i]) }
}

/// Truncate by characters, not bytes, so Arabic names are never cut mid-codepoint.
fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(n - 1).collect::<String>())
    }
}

/// Print every volume with the identity the catalogue will key on.
fn print_drives() {
    println!("\n{:<10} {:<9} {:<8} {:<7} {:>9} {:>9}  {}",
        "MOUNT", "SERIAL", "FS", "BUS", "SIZE", "FREE", "LABEL");
    println!("{}", "-".repeat(78));
    for v in gamevault_lib::platform::volume::enumerate() {
        println!(
            "{:<10} {:<9} {:<8} {:<7} {:>9} {:>9}  {}",
            v.mount_point.clone().unwrap_or_else(|| "(none)".into()),
            v.volume_serial,
            v.filesystem,
            format!("{:?}", v.bus_type),
            human(v.total_bytes),
            human(v.free_bytes),
            v.display_name()
        );
        println!("           guid: {}", v.volume_guid);
        let mut flags = Vec::new();
        if v.bus_type.is_removable_bus() {
            flags.push("removable bus".to_string());
        }
        if v.is_unjournaled {
            flags.push("no journal — flush every file before marking it copied".into());
        }
        if let Some(max) = v.max_file_bytes {
            flags.push(format!("max file {}", human(max)));
        }
        if !flags.is_empty() {
            println!("           {}", flags.join(" · "));
        }
    }
    println!();
}

/// Scan a path and fold the results into the catalogue, then report what changed.
fn index_path(path: &str) {
    use gamevault_lib::db::Db;
    use gamevault_lib::platform::volume;

    let root = PathBuf::from(path);
    let Some(guid) = volume::guid_for_path(&root) else {
        eprintln!("no volume found for {path}");
        std::process::exit(1);
    };
    let Some(info) = volume::describe(&guid) else {
        eprintln!("could not read volume {guid}");
        std::process::exit(1);
    };

    let db_path = Db::default_path();
    let mut db = match Db::open(&db_path) {
        Ok(db) => db,
        Err(e) => {
            eprintln!("could not open catalogue at {}: {e}", db_path.display());
            std::process::exit(1);
        }
    };

    let started = Instant::now();
    let mut res = walker::scan(&root, &ClassifyConfig::default());
    res.rebase(&root, &PathBuf::from(info.mount_point.clone().unwrap_or_default()));
    let drive_id = db.upsert_drive(&info).expect("upsert drive");
    let (found, new, missing) = db.record_scan(drive_id, &res.items).expect("record scan");

    println!(
        "\nIndexed {} ({}) in {:.2?}",
        info.display_name(),
        path,
        started.elapsed()
    );
    println!("  catalogue: {}", db_path.display());
    println!("  {found} items ({new} new, {missing} now missing)");
    let games = res
        .items
        .iter()
        .filter(|i| i.verdict == Verdict::Game)
        .count();
    println!("  {games} classified as games\n");
}

/// Read a directory of console packages and print the games they form.
///
/// Read-only: it opens each package, reads its header, and closes it. Nothing on the
/// scanned drive is modified.
fn print_console(path: &str) {
    use gamevault_lib::scan::console::{group, probe_ps4_dir, ps4};

    let dir = PathBuf::from(path);
    let started = Instant::now();
    let (probed, covers) = match probe_ps4_dir(&dir) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("cannot read {}: {e}", dir.display());
            std::process::exit(1);
        }
    };
    let elapsed = started.elapsed();
    let parsed = probed.iter().filter(|p| p.info.is_some()).count();
    let result = group::group_ps4(&probed, &covers);

    println!("\nProbed {} in {:.2?}", dir.display(), elapsed);
    println!(
        "  {} packages ({} parsed, {} without a header), {} cover images",
        probed.len(),
        parsed,
        probed.len() - parsed,
        covers.len()
    );
    println!("\n==> {} packages = {} games\n", probed.len(), result.groups.len());

    let mut groups: Vec<&group::ConsoleGroup> = result.groups.iter().collect();
    groups.sort_by_key(|g| std::cmp::Reverse(g.total_bytes));

    println!("{:<34}{:<11}{:<7}{:<24}{:>9}", "GAME", "TITLE ID", "FW", "CONTENTS", "SIZE");
    println!("{}", "-".repeat(86));
    for g in &groups {
        let mut parts = Vec::new();
        for (role, label) in [
            (group::ConsoleRole::Game, "game"),
            (group::ConsoleRole::Update, "update"),
            (group::ConsoleRole::Dlc, "DLC"),
            (group::ConsoleRole::Part, "part"),
        ] {
            let n = g.count_role(role);
            if n > 0 {
                parts.push(format!("{n} {label}{}", if n > 1 { "s" } else { "" }));
            }
        }
        let fw = g.min_system_ver.map(ps4::firmware_label).unwrap_or_else(|| "-".into());
        println!(
            "{:<34}{:<11}{:<7}{:<24}{:>9}",
            truncate(&g.display_title, 32),
            g.title_id,
            fw,
            parts.join(", "),
            human(g.total_bytes)
        );
        if let (Some(base), Some(why)) = (&g.linked_base, &g.link_reason) {
            println!("      ↳ add-on for {base} — {why}");
        }
    }

    if !result.ungrouped.is_empty() {
        let wasted: u64 = result.ungrouped.iter().map(|p| p.size_bytes).sum();
        println!("\n=== UNREADABLE / DAMAGED ({}) — {} ===", result.ungrouped.len(), human(wasted));
        for p in &result.ungrouped {
            println!("  {:>10}  {}", human(p.size_bytes), truncate(&p.name, 64));
        }
    }
    println!();
}
