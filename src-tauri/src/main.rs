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
            eprintln!("usage: gamevault --console <path> [--write-covers]");
            std::process::exit(2);
        };
        // Extraction is opt-in so plain --console stays a pure read everywhere. Even with
        // the flag, writes go only to the local cover cache, never to the scanned drive.
        print_console(path, args.iter().any(|a| a == "--write-covers"));
        return;
    }

    // Exactly what the Game Files window will show, without opening it. The fastest way to
    // check a real drive, and the place to look when a game is missing from the window.
    if args.iter().any(|a| a == "--gamefiles") {
        let Some(path) = args.iter().find(|a| !a.starts_with("--")) else {
            eprintln!("usage: gamevault --gamefiles <path>");
            std::process::exit(2);
        };
        print_gamefiles(path);
        return;
    }

    // Store the metadata key without it passing through the command line, where it would
    // land in shell history. Reads `RAWG_API_KEY`, verifies it, then encrypts it with
    // DPAPI exactly as the Settings screen does.
    if args.iter().any(|a| a == "--save-key") {
        save_key_from_env();
        return;
    }

    // Check the metadata provider end to end without opening the window: match a title,
    // then fetch whatever artwork it resolves to.
    if args.iter().any(|a| a == "--meta") {
        let Some(title) = args.iter().find(|a| !a.starts_with("--")) else {
            eprintln!("usage: gamevault --meta \"<game title>\"");
            std::process::exit(2);
        };
        print_meta(title);
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

/// Verify the key in `RAWG_API_KEY` and store it encrypted.
///
/// The key arrives through the environment rather than as an argument so it stays out of
/// shell history and out of this process's visible command line.
fn save_key_from_env() {
    use gamevault_lib::meta::rawg;
    use gamevault_lib::platform::dpapi::{self, Credentials};

    let Ok(key) = std::env::var("RAWG_API_KEY") else {
        eprintln!("set RAWG_API_KEY first, then run: gamevault --save-key");
        std::process::exit(2);
    };
    let creds = Credentials { rawg_api_key: key.trim().to_string() };
    if !creds.is_complete() {
        eprintln!("RAWG_API_KEY is empty");
        std::process::exit(2);
    }

    print!("checking the key… ");
    if let Err(e) = rawg::test_credentials(&creds) {
        println!("rejected");
        eprintln!("  {e}");
        std::process::exit(1);
    }
    println!("accepted");

    match dpapi::save_credentials(&creds) {
        Ok(()) => println!("saved {} — encrypted for this Windows account only", creds.masked()),
        Err(e) => {
            eprintln!("could not save: {e}");
            std::process::exit(1);
        }
    }
}

/// Look one title up through the metadata provider and fetch its artwork.
///
/// Uses the key saved in Settings. `RAWG_API_KEY` overrides it, so the provider can be
/// exercised on a machine where nothing has been saved yet — the key then stays out of
/// the encrypted store and out of this program's arguments.
fn print_meta(title: &str) {
    use gamevault_lib::commands::meta::cover_dir;
    use gamevault_lib::meta::rawg;
    use gamevault_lib::platform::dpapi::{self, Credentials};

    let creds = match std::env::var("RAWG_API_KEY") {
        Ok(k) if !k.trim().is_empty() => Credentials { rawg_api_key: k.trim().to_string() },
        _ => dpapi::load_credentials(),
    };
    if !creds.is_complete() {
        eprintln!("no RAWG key: save one in Settings, or set RAWG_API_KEY for this run");
        std::process::exit(1);
    }

    print!("checking the key… ");
    match rawg::test_credentials(&creds) {
        Ok(()) => println!("accepted ({})", creds.masked()),
        Err(e) => {
            println!("rejected");
            eprintln!("  {e}");
            std::process::exit(1);
        }
    }

    let started = Instant::now();
    let found = match rawg::lookup(&creds, title, None) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("lookup failed: {e}");
            std::process::exit(1);
        }
    };

    let Some(meta) = found else {
        println!("\nno confident match for {title:?} — the cover stays a coloured placeholder");
        return;
    };

    println!("\nmatched in {:.2?}", started.elapsed());
    println!("  title    {}", meta.name);
    println!("  score    {:.1}", meta.match_score);
    println!("  year     {}", meta.year.map(|y| y.to_string()).unwrap_or_else(|| "-".into()));
    println!("  genres   {}", if meta.genres.is_empty() { "-".into() } else { meta.genres.join(", ") });
    println!("  rawg id  {}", meta.rawg_id.map(|i| i.to_string()).unwrap_or_else(|| "-".into()));
    println!(
        "  steam    {}",
        meta.steam_appid.map(|a| a.to_string()).unwrap_or_else(|| "not on Steam".into())
    );

    match rawg::fetch_cover(&meta, &cover_dir()) {
        Some(p) => {
            let bytes = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
            let shape = if meta.steam_appid.is_some() { "portrait box art from Steam" } else { "landscape art from RAWG" };
            println!("\n  cover    {} ({}, {})", p.display(), human(bytes), shape);
        }
        None => println!("\n  cover    none available"),
    }
}

/// Read a folder tree exactly as the Game Files window does, and print what it found.
///
/// Read-only against the folder. Covers found inside game files are extracted into the
/// local cache, which is what the window does too — nothing is written to the drive.
fn print_gamefiles(path: &str) {
    use gamevault_lib::commands::meta::cover_dir;
    use gamevault_lib::scan::console::index;
    use std::collections::HashMap;

    let root = PathBuf::from(path);
    if !root.is_dir() {
        eprintln!("folder not found: {}", root.display());
        std::process::exit(1);
    }

    // No probe cache from the CLI: this is meant to show the cost of a first read.
    let read = index::read_tree(&root, &HashMap::new(), &cover_dir(), &mut |_| {});

    println!("\nRead {} in {} ms", read.root, read.stats.elapsed_ms);
    println!(
        "  {} folders · {} packages opened · {} from cache",
        read.stats.dirs_read, read.stats.probed, read.stats.from_cache
    );
    println!(
        "  {} covers from inside game files · {} from images beside them",
        read.stats.covers_embedded, read.stats.covers_sibling
    );
    println!("\n==> {} games\n", read.games.len());

    // How much is really known, per platform.
    let mut by_platform: std::collections::BTreeMap<&str, (usize, u64)> = Default::default();
    for g in &read.games {
        let e = by_platform.entry(&g.platform).or_insert((0, 0));
        e.0 += 1;
        e.1 += g.total_bytes;
    }
    println!("{:<14}{:>7}{:>12}", "PLATFORM", "GAMES", "SIZE");
    println!("{}", "-".repeat(33));
    for (p, (n, bytes)) in &by_platform {
        println!("{:<14}{:>7}{:>12}", p, n, human(*bytes));
    }

    println!(
        "\n{:<38}{:<10}{:<11}{:>9}  {}",
        "GAME", "PLATFORM", "TITLE ID", "SIZE", "COVER"
    );
    println!("{}", "-".repeat(92));
    for g in &read.games {
        let cover = match g.cover_source.as_deref() {
            Some("pkg_icon0") => "from the file",
            Some("pkg_sibling") => "image beside it",
            _ => "-",
        };
        println!(
            "{:<38}{:<10}{:<11}{:>9}  {}{}",
            truncate(&g.display_title, 36),
            g.platform,
            g.title_id.as_deref().unwrap_or("-"),
            human(g.total_bytes),
            cover,
            if g.platform_guessed { "  (platform guessed from folder)" } else { "" },
        );
    }

    if !read.damaged.is_empty() {
        let wasted: u64 = read.damaged.iter().map(|f| f.size_bytes).sum();
        println!("\n=== DAMAGED ({}) — {} ===", read.damaged.len(), human(wasted));
        for f in &read.damaged {
            println!("  {:>10}  {:<44} {}", human(f.size_bytes), truncate(&f.name, 42), f.detail);
        }
    }
    println!();
}

/// Read a directory of console packages and print the games they form.
///
/// Read-only against the scanned drive: it opens each package, reads its header, and
/// closes it. Nothing there is written, moved or removed.
///
/// With `write_covers`, artwork found inside the packages is extracted into the local
/// cover cache under `%LOCALAPPDATA%\GameVault\covers` — still nothing on the drive.
fn print_console(path: &str, write_covers: bool) {
    use gamevault_lib::commands::meta::cover_dir;
    use gamevault_lib::scan::console::{
        cache_group_cover, group, inspect_artwork, probe_ps4_dir, ps4, ArtworkStatus,
    };

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

    // Tallies for the one question this harness exists to answer.
    let mut n_embedded = 0usize;
    let mut n_encrypted = 0usize;
    let mut n_not_image = 0usize;
    let mut n_no_entry = 0usize;
    let mut n_sibling = 0usize;
    let covers_to = cover_dir();

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

        // The artwork line: what the Game Files window will be able to show.
        let status = inspect_artwork(&dir, g);
        match status {
            ArtworkStatus::Embedded(n) => {
                n_embedded += 1;
                print!("      icon0: {:>9}", human(n as u64));
            }
            ArtworkStatus::Encrypted => {
                n_encrypted += 1;
                print!("      icon0: encrypted");
            }
            ArtworkStatus::NotAnImage => {
                n_not_image += 1;
                print!("      icon0: not a PNG");
            }
            ArtworkStatus::NoEntry => {
                n_no_entry += 1;
                let has_sibling = g.count_role(group::ConsoleRole::Cover) > 0;
                if has_sibling {
                    n_sibling += 1;
                }
                print!(
                    "      icon0: none{}",
                    if has_sibling { " (sibling image available)" } else { "" }
                );
            }
        }
        if write_covers {
            match cache_group_cover(&dir, g, &covers_to) {
                Some((p, src)) => {
                    let name = p.file_name().unwrap_or_default().to_string_lossy().into_owned();
                    println!("  →  {name}  [{}]", src.as_str());
                }
                None => println!("  →  no cover written"),
            }
        } else {
            println!();
        }
    }

    println!(
        "\n=== ARTWORK: {} games · {} embedded · {} sibling images · {} none ===",
        groups.len(),
        n_embedded,
        n_sibling,
        n_no_entry - n_sibling + n_encrypted + n_not_image
    );
    if n_encrypted > 0 || n_not_image > 0 {
        println!(
            "  {n_encrypted} with encrypted metadata, {n_not_image} whose bytes are not a PNG"
        );
    }
    if write_covers {
        println!("  written to {}", covers_to.display());
    } else {
        println!("  (nothing written — pass --write-covers to extract them)");
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
