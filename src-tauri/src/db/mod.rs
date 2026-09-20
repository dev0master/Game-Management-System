//! Catalogue persistence.
//!
//! Two rules shape every write in this module:
//!
//! * **The scanner never deletes.** An item that has vanished gets `missing_since_utc`
//!   set. That is what lets an unplugged drive stay fully browsable, and it means a
//!   mis-scan can never destroy catalogue data.
//! * **A user's verdict outranks the scanner's.** `user_verdict` and `user_title` are
//!   written only by explicit user action and are never touched by a rescan.

use crate::platform::volume::VolumeInfo;
use crate::scan::classify::Verdict;
use crate::scan::walker::ScannedItem;
use rusqlite::{params, Connection, OptionalExtension};
use std::path::{Path, PathBuf};

pub mod time;

pub type Result<T> = std::result::Result<T, rusqlite::Error>;

/// A drive row, including drives that are not currently attached.
#[derive(Debug, Clone, serde::Serialize)]
pub struct DriveRow {
    pub id: i64,
    pub volume_guid: String,
    pub volume_serial: String,
    pub label: String,
    pub filesystem: String,
    pub bus_type: String,
    pub total_bytes: i64,
    pub free_bytes: i64,
    pub is_unjournaled: bool,
    pub max_file_bytes: Option<i64>,
    pub current_mount: Option<String>,
    pub is_online: bool,
    pub last_seen_utc: String,
    pub last_scan_utc: Option<String>,
    pub item_count: i64,
    pub game_count: i64,
    pub catalogued_bytes: i64,
}

/// An item row as the library shows it.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ItemRow {
    pub id: i64,
    pub drive_id: i64,
    pub drive_label: String,
    pub drive_online: bool,
    pub rel_path: String,
    pub kind: String,
    pub container: String,
    pub raw_name: String,
    pub clean_title: String,
    pub display_title: String,
    pub repacker: Option<String>,
    pub release_group: Option<String>,
    pub year: Option<i64>,
    pub edition: Option<String>,
    pub notes: Option<String>,
    pub total_bytes: i64,
    pub file_count: i64,
    pub part_count: Option<i64>,
    pub set_complete: Option<bool>,
    pub incomplete_reason: Option<String>,
    /// Effective verdict: the user's if they set one, else the scanner's.
    pub verdict: String,
    pub scanner_verdict: String,
    pub user_verdict: Option<String>,
    pub confidence: f64,
    pub reasons_json: String,
    pub missing_since_utc: Option<String>,
    /// Local cover file, when metadata has been fetched. Served to the UI over the
    /// asset protocol, never inlined into IPC JSON.
    pub cover_path: Option<String>,
    pub matched_name: Option<String>,
    pub match_locked: bool,
}

/// Filter for a library query.
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct ItemFilter {
    pub drive_id: Option<i64>,
    /// `game` | `needs_review` | `not_game`. Empty means all.
    pub verdict: Option<String>,
    pub search: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

pub struct Db {
    conn: Connection,
}

impl Db {
    /// Open (creating if needed) the catalogue at `path` and bring the schema up to date.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let conn = Connection::open(path)?;
        conn.execute_batch(include_str!("schema.sql"))?;
        migrate(&conn)?;
        Ok(Self { conn })
    }

    /// An in-memory catalogue, for tests.
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(include_str!("schema.sql"))?;
        migrate(&conn)?;
        Ok(Self { conn })
    }

    /// Default location: `%LOCALAPPDATA%\GameVault\catalog.db`.
    ///
    /// Deliberately never on an external drive — the catalogue has to outlive any
    /// drive being unplugged, reformatted or lost.
    pub fn default_path() -> PathBuf {
        let base = std::env::var("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|_| std::env::temp_dir());
        base.join("GameVault").join("catalog.db")
    }

    /// Record a volume that is present now, returning its row id.
    ///
    /// Matched first on volume GUID, then on serial. The serial fallback is what
    /// recognises a drive that was re-mounted with a different GUID — for instance
    /// after being connected through a different enclosure.
    pub fn upsert_drive(&mut self, v: &VolumeInfo) -> Result<i64> {
        let now = time::now_utc();
        let bus = serde_json::to_string(&v.bus_type)
            .unwrap_or_default()
            .trim_matches('"')
            .to_string();

        let existing: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM drive WHERE volume_guid = ?1",
                params![v.volume_guid],
                |r| r.get(0),
            )
            .optional()?
            .or(self
                .conn
                .query_row(
                    "SELECT id FROM drive WHERE volume_serial = ?1 AND volume_serial <> ''",
                    params![v.volume_serial],
                    |r| r.get(0),
                )
                .optional()?);

        match existing {
            Some(id) => {
                self.conn.execute(
                    "UPDATE drive SET volume_guid=?1, volume_serial=?2, label=?3, filesystem=?4,
                        bus_type=?5, total_bytes=?6, free_bytes=?7, is_unjournaled=?8,
                        max_file_bytes=?9, current_mount=?10, is_online=1, last_seen_utc=?11
                     WHERE id=?12",
                    params![
                        v.volume_guid, v.volume_serial, v.label, v.filesystem, bus,
                        v.total_bytes as i64, v.free_bytes as i64, v.is_unjournaled as i64,
                        v.max_file_bytes.map(|m| m as i64), v.mount_point, now, id
                    ],
                )?;
                Ok(id)
            }
            None => {
                self.conn.execute(
                    "INSERT INTO drive (volume_guid, volume_serial, label, filesystem, bus_type,
                        total_bytes, free_bytes, is_unjournaled, max_file_bytes, current_mount,
                        is_online, first_seen_utc, last_seen_utc)
                     VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,1,?11,?11)",
                    params![
                        v.volume_guid, v.volume_serial, v.label, v.filesystem, bus,
                        v.total_bytes as i64, v.free_bytes as i64, v.is_unjournaled as i64,
                        v.max_file_bytes.map(|m| m as i64), v.mount_point, now
                    ],
                )?;
                Ok(self.conn.last_insert_rowid())
            }
        }
    }

    /// Mark every drive not in `present_guids` as offline.
    ///
    /// Only the online flag and mount point change; the drive's catalogued contents
    /// stay exactly as they were, which is what makes offline browsing work.
    pub fn mark_offline_except(&mut self, present_guids: &[String]) -> Result<usize> {
        let list = present_guids
            .iter()
            .map(|g| format!("'{}'", g.replace('\'', "''")))
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "UPDATE drive SET is_online = 0, current_mount = NULL
             WHERE volume_guid NOT IN ({}) AND is_online = 1",
            if list.is_empty() { "''".into() } else { list }
        );
        self.conn.execute(&sql, [])
    }

    /// Persist the results of a scan of one drive.
    ///
    /// Items seen in the scan are inserted or refreshed; items previously known on
    /// this drive but absent now are marked missing rather than deleted. Returns
    /// `(seen, new, missing)`.
    pub fn record_scan(&mut self, drive_id: i64, items: &[ScannedItem]) -> Result<(usize, usize, usize)> {
        let now = time::now_utc();
        let tx = self.conn.transaction()?;
        let mut new_count = 0usize;

        // Which paths this scan saw, tracked explicitly rather than by comparing
        // timestamps. Two scans within the same second share a timestamp, so a
        // time-based comparison silently fails to mark anything missing.
        tx.execute_batch(
            "CREATE TEMP TABLE IF NOT EXISTS seen_path (rel_path TEXT PRIMARY KEY);
             DELETE FROM seen_path;",
        )?;

        for it in items {
            tx.execute(
                "INSERT OR IGNORE INTO seen_path (rel_path) VALUES (?1)",
                params![it.rel_path],
            )?;
            let verdict = verdict_str(it.verdict);
            let reasons = serde_json::to_string(&it.classification.reasons).unwrap_or_else(|_| "[]".into());
            let missing_parts = it
                .archive_set
                .as_ref()
                .map(|s| serde_json::to_string(&s.missing_parts).unwrap_or_default());
            let search_key = search_key(&it.title.clean_title, &it.raw_name);

            let existing: Option<i64> = tx
                .query_row(
                    "SELECT id FROM item WHERE drive_id = ?1 AND rel_path = ?2",
                    params![drive_id, it.rel_path],
                    |r| r.get(0),
                )
                .optional()?;

            if existing.is_none() {
                new_count += 1;
            }

            // Note what is absent from this statement: user_verdict and user_title are
            // never written here, so a rescan cannot undo a correction the user made.
            tx.execute(
                "INSERT INTO item (drive_id, rel_path, kind, container, raw_name, clean_title,
                    title_key, search_key, repacker, release_group, version, edition, year, notes,
                    total_bytes, file_count, part_count, set_complete, missing_parts_json,
                    incomplete_reason, classification, confidence, reasons_json, quick_sig,
                    first_seen_utc, last_seen_utc, missing_since_utc)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,
                         ?21,?22,?23,?24,?25,?25,NULL)
                 ON CONFLICT(drive_id, rel_path) DO UPDATE SET
                    kind=excluded.kind, container=excluded.container,
                    raw_name=excluded.raw_name, clean_title=excluded.clean_title,
                    title_key=excluded.title_key, search_key=excluded.search_key,
                    repacker=excluded.repacker, release_group=excluded.release_group,
                    version=excluded.version, edition=excluded.edition, year=excluded.year,
                    notes=excluded.notes, total_bytes=excluded.total_bytes,
                    file_count=excluded.file_count, part_count=excluded.part_count,
                    set_complete=excluded.set_complete,
                    missing_parts_json=excluded.missing_parts_json,
                    incomplete_reason=excluded.incomplete_reason,
                    classification=excluded.classification, confidence=excluded.confidence,
                    reasons_json=excluded.reasons_json, quick_sig=excluded.quick_sig,
                    last_seen_utc=excluded.last_seen_utc,
                    missing_since_utc=NULL",
                params![
                    drive_id, it.rel_path, kind_str(it.kind),
                    if it.is_folder { "folder" } else { "file" },
                    it.raw_name, it.title.clean_title, it.title.title_key, search_key,
                    it.title.repacker, it.title.release_group, it.title.version,
                    it.title.edition, it.title.year, it.title.notes,
                    it.total_bytes as i64, it.file_count as i64,
                    it.archive_set.as_ref().map(|s| s.parts.len() as i64),
                    it.archive_set.as_ref().map(|s| s.complete as i64),
                    missing_parts,
                    it.archive_set.as_ref().and_then(|s| s.incomplete_reason.clone()),
                    verdict, it.confidence as f64, reasons, quick_sig(it), now
                ],
            )?;

            let item_id: i64 = tx.query_row(
                "SELECT id FROM item WHERE drive_id = ?1 AND rel_path = ?2",
                params![drive_id, it.rel_path],
                |r| r.get(0),
            )?;

            // Rewritten from scratch each scan so a renamed or removed member cannot
            // linger. Driven by `file_specs`, which every item carries — previously only
            // archive volumes were saved and any other multi-file item lost its contents.
            tx.execute("DELETE FROM item_file WHERE item_id = ?1", params![item_id])?;
            for f in &it.file_specs {
                tx.execute(
                    "INSERT OR REPLACE INTO item_file
                       (item_id, rel_path, size_bytes, mtime_unix, part_index, role,
                        content_id, app_ver, system_ver, declared_bytes, probe_status)
                     VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
                    params![
                        item_id, f.rel_path, f.size_bytes as i64, f.mtime_unix,
                        f.part_index, f.role, f.content_id, f.app_ver, f.system_ver,
                        f.declared_bytes, f.probe_status
                    ],
                )?;
            }
        }

        // Soft-delete: anything on this drive this scan did not see. The row stays,
        // so an unplugged drive keeps its contents and a bad scan loses nothing.
        let missing = tx.execute(
            "UPDATE item SET missing_since_utc = ?1
             WHERE drive_id = ?2 AND missing_since_utc IS NULL
               AND rel_path NOT IN (SELECT rel_path FROM seen_path)",
            params![now, drive_id],
        )?;

        tx.execute(
            "UPDATE drive SET last_scan_utc = ?1 WHERE id = ?2",
            params![now, drive_id],
        )?;
        tx.commit()?;
        Ok((items.len(), new_count, missing))
    }

    /// Every known drive, attached or not, newest activity first.
    pub fn list_drives(&self) -> Result<Vec<DriveRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT d.id, d.volume_guid, d.volume_serial, d.label, d.filesystem, d.bus_type,
                    d.total_bytes, d.free_bytes, d.is_unjournaled, d.max_file_bytes,
                    d.current_mount, d.is_online, d.last_seen_utc, d.last_scan_utc,
                    (SELECT COUNT(*) FROM item i
                      WHERE i.drive_id = d.id AND i.missing_since_utc IS NULL),
                    (SELECT COUNT(*) FROM item i
                      WHERE i.drive_id = d.id AND i.missing_since_utc IS NULL
                        AND COALESCE(i.user_verdict, i.classification) = 'game'),
                    (SELECT COALESCE(SUM(i.total_bytes), 0) FROM item i
                      WHERE i.drive_id = d.id AND i.missing_since_utc IS NULL
                        AND COALESCE(i.user_verdict, i.classification) = 'game')
             FROM drive d
             ORDER BY d.is_online DESC, d.label",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(DriveRow {
                id: r.get(0)?,
                volume_guid: r.get(1)?,
                volume_serial: r.get(2)?,
                label: r.get(3)?,
                filesystem: r.get(4)?,
                bus_type: r.get(5)?,
                total_bytes: r.get(6)?,
                free_bytes: r.get(7)?,
                is_unjournaled: r.get::<_, i64>(8)? != 0,
                max_file_bytes: r.get(9)?,
                current_mount: r.get(10)?,
                is_online: r.get::<_, i64>(11)? != 0,
                last_seen_utc: r.get(12)?,
                last_scan_utc: r.get(13)?,
                item_count: r.get(14)?,
                game_count: r.get(15)?,
                catalogued_bytes: r.get(16)?,
            })
        })?;
        rows.collect()
    }

    /// Query the library. Works identically whether or not the drives are attached.
    pub fn list_items(&self, f: &ItemFilter) -> Result<Vec<ItemRow>> {
        let mut sql = String::from(
            "SELECT i.id, i.drive_id,
                    COALESCE(NULLIF(d.label, ''), NULLIF(d.current_mount, ''), d.volume_serial),
                    d.is_online, i.rel_path, i.kind, i.container,
                    i.raw_name, i.clean_title, COALESCE(i.user_title, i.clean_title),
                    i.repacker, i.release_group, i.year, i.edition, i.notes,
                    i.total_bytes, i.file_count, i.part_count, i.set_complete,
                    i.incomplete_reason,
                    COALESCE(i.user_verdict, i.classification), i.classification, i.user_verdict,
                    i.confidence, i.reasons_json, i.missing_since_utc,
                    m.cover_path, m.matched_name, COALESCE(m.match_locked, 0)
             FROM item i
             JOIN drive d ON d.id = i.drive_id
             LEFT JOIN metadata m ON m.item_id = i.id
             WHERE i.missing_since_utc IS NULL",
        );
        let mut args: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

        if let Some(id) = f.drive_id {
            sql.push_str(" AND i.drive_id = ?");
            args.push(Box::new(id));
        }
        if let Some(v) = f.verdict.as_ref().filter(|v| !v.is_empty()) {
            sql.push_str(" AND COALESCE(i.user_verdict, i.classification) = ?");
            args.push(Box::new(v.clone()));
        }
        if let Some(q) = f.search.as_ref().filter(|q| !q.trim().is_empty()) {
            // LIKE over the folded search key, so an Arabic query matches however the
            // name was typed. FTS5 is reserved for larger catalogues.
            sql.push_str(" AND (i.search_key LIKE ? OR i.raw_name LIKE ?)");
            let pat = format!("%{}%", fold_for_search(q));
            args.push(Box::new(pat.clone()));
            args.push(Box::new(format!("%{q}%")));
        }
        sql.push_str(" ORDER BY i.total_bytes DESC LIMIT ? OFFSET ?");
        args.push(Box::new(f.limit.unwrap_or(200)));
        args.push(Box::new(f.offset.unwrap_or(0)));

        let mut stmt = self.conn.prepare(&sql)?;
        let refs: Vec<&dyn rusqlite::ToSql> = args.iter().map(|b| b.as_ref()).collect();
        let rows = stmt.query_map(refs.as_slice(), |r| {
            Ok(ItemRow {
                id: r.get(0)?,
                drive_id: r.get(1)?,
                drive_label: r.get(2)?,
                drive_online: r.get::<_, i64>(3)? != 0,
                rel_path: r.get(4)?,
                kind: r.get(5)?,
                container: r.get(6)?,
                raw_name: r.get(7)?,
                clean_title: r.get(8)?,
                display_title: r.get(9)?,
                repacker: r.get(10)?,
                release_group: r.get(11)?,
                year: r.get(12)?,
                edition: r.get(13)?,
                notes: r.get(14)?,
                total_bytes: r.get(15)?,
                file_count: r.get(16)?,
                part_count: r.get(17)?,
                set_complete: r.get::<_, Option<i64>>(18)?.map(|v| v != 0),
                incomplete_reason: r.get(19)?,
                verdict: r.get(20)?,
                scanner_verdict: r.get(21)?,
                user_verdict: r.get(22)?,
                confidence: r.get(23)?,
                reasons_json: r.get(24)?,
                missing_since_utc: r.get(25)?,
                cover_path: r.get(26)?,
                matched_name: r.get(27)?,
                match_locked: r.get::<_, i64>(28)? != 0,
            })
        })?;
        rows.collect()
    }

    /// Record the user's decision about an item. This is the only writer of
    /// `user_verdict`, and no rescan will overwrite it.
    pub fn set_user_verdict(&mut self, item_id: i64, verdict: Option<&str>) -> Result<()> {
        self.conn.execute(
            "UPDATE item SET user_verdict = ?1 WHERE id = ?2",
            params![verdict, item_id],
        )?;
        Ok(())
    }

    pub fn set_user_title(&mut self, item_id: i64, title: Option<&str>) -> Result<()> {
        self.conn.execute(
            "UPDATE item SET user_title = ?1 WHERE id = ?2",
            params![title, item_id],
        )?;
        Ok(())
    }

    /// Items appearing on more than one drive, grouped by title key.
    pub fn duplicates(&self) -> Result<Vec<(String, i64, i64)>> {
        let mut stmt = self.conn.prepare(
            "SELECT title_key, COUNT(*) AS n, SUM(total_bytes) AS bytes
             FROM item
             WHERE missing_since_utc IS NULL AND title_key <> ''
               AND COALESCE(user_verdict, classification) = 'game'
             GROUP BY title_key HAVING n > 1
             ORDER BY bytes DESC",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        rows.collect()
    }

    /// Bytes per classification for one drive, for the storage breakdown.
    pub fn storage_breakdown(&self, drive_id: i64) -> Result<Vec<(String, i64, i64)>> {
        let mut stmt = self.conn.prepare(
            "SELECT kind, COUNT(*), COALESCE(SUM(total_bytes), 0)
             FROM item WHERE drive_id = ?1 AND missing_since_utc IS NULL
             GROUP BY kind ORDER BY SUM(total_bytes) DESC",
        )?;
        let rows = stmt.query_map(params![drive_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        rows.collect()
    }
}

/// One item plus the files that make it up, and enough drive context to rebuild
/// absolute paths.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ItemDetail {
    pub item: ItemRow,
    /// File names relative to the item, in extraction order for an archive set.
    pub files: Vec<ItemFileRow>,
    /// The drive's mount point right now, or `None` when it is not attached. A
    /// transfer is only possible while this is `Some`.
    pub drive_mount: Option<String>,
    pub drive_guid: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ItemFileRow {
    pub rel_path: String,
    pub size_bytes: i64,
    pub part_index: Option<i64>,
    pub role: String,
}

impl Db {
    /// Fetch one item with its files.
    pub fn get_item(&self, item_id: i64) -> Result<Option<ItemDetail>> {
        let mut rows = self.list_items(&ItemFilter { limit: Some(10_000), ..Default::default() })?;
        let Some(pos) = rows.iter().position(|r| r.id == item_id) else {
            return Ok(None);
        };
        let item = rows.swap_remove(pos);

        let (drive_mount, drive_guid): (Option<String>, String) = self.conn.query_row(
            "SELECT current_mount, volume_guid FROM drive WHERE id = ?1",
            params![item.drive_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;

        let mut stmt = self.conn.prepare(
            "SELECT rel_path, size_bytes, part_index, role FROM item_file
             WHERE item_id = ?1 ORDER BY COALESCE(part_index, 0), rel_path",
        )?;
        let files = stmt
            .query_map(params![item_id], |r| {
                Ok(ItemFileRow {
                    rel_path: r.get(0)?,
                    size_bytes: r.get(1)?,
                    part_index: r.get(2)?,
                    role: r.get(3)?,
                })
            })?
            .collect::<Result<Vec<_>>>()?;

        Ok(Some(ItemDetail { item, files, drive_mount, drive_guid }))
    }

    /// Re-point an item at a new drive and path after it has been moved.
    ///
    /// Called only once a move has fully verified, so the catalogue follows the files
    /// instead of reporting the game as missing on the next scan.
    pub fn relocate_item(&mut self, item_id: i64, new_drive_id: i64, new_rel_path: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE item SET drive_id = ?1, rel_path = ?2, missing_since_utc = NULL WHERE id = ?3",
            params![new_drive_id, new_rel_path, item_id],
        )?;
        Ok(())
    }

    /// Duplicate an item's row onto another drive after a copy, so both locations are
    /// catalogued without waiting for a rescan.
    pub fn note_copy(&mut self, item_id: i64, new_drive_id: i64, new_rel_path: &str) -> Result<()> {
        let now = time::now_utc();
        self.conn.execute(
            "INSERT OR IGNORE INTO item (drive_id, rel_path, kind, container, raw_name,
                clean_title, title_key, search_key, repacker, release_group, version, edition,
                year, notes, total_bytes, file_count, part_count, set_complete,
                classification, confidence, reasons_json, quick_sig, first_seen_utc, last_seen_utc)
             SELECT ?1, ?2, kind, container, raw_name, clean_title, title_key, search_key,
                    repacker, release_group, version, edition, year, notes, total_bytes,
                    file_count, part_count, set_complete, classification, confidence,
                    reasons_json, quick_sig, ?3, ?3
             FROM item WHERE id = ?4",
            params![new_drive_id, new_rel_path, now, item_id],
        )?;
        Ok(())
    }

    /// Row id for a volume GUID, if the drive is known.
    pub fn drive_id_for_guid(&self, guid: &str) -> Result<Option<i64>> {
        self.conn
            .query_row("SELECT id FROM drive WHERE volume_guid = ?1", params![guid], |r| r.get(0))
            .optional()
    }
}

impl Db {
    /// Items that still need metadata: games with no match yet, and never one the user
    /// has corrected. Filtering on `match_locked` here is the structural guarantee
    /// that a manual correction is permanent.
    pub fn items_needing_metadata(&self, limit: i64) -> Result<Vec<(i64, String, Option<i64>)>> {
        let mut stmt = self.conn.prepare(
            "SELECT i.id, COALESCE(i.user_title, i.clean_title), i.year
             FROM item i
             LEFT JOIN metadata m ON m.item_id = i.id
             WHERE i.missing_since_utc IS NULL
               AND COALESCE(i.user_verdict, i.classification) = 'game'
               AND COALESCE(m.match_locked, 0) = 0
               AND (m.item_id IS NULL OR m.source = 'none' OR m.cover_path IS NULL)
             ORDER BY i.total_bytes DESC
             LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        rows.collect()
    }

    /// Record a metadata match.
    #[allow(clippy::too_many_arguments)]
    pub fn save_metadata(
        &mut self,
        item_id: i64,
        source: &str,
        igdb_id: Option<i64>,
        steam_appid: Option<i64>,
        matched_name: &str,
        match_score: f64,
        year: Option<i32>,
        summary: Option<&str>,
        genres: &[String],
        rating: Option<f64>,
        cover_path: Option<&str>,
        locked: bool,
    ) -> Result<()> {
        let now = time::now_utc();
        self.conn.execute(
            "INSERT INTO metadata (item_id, source, igdb_id, steam_appid, matched_name,
                match_score, match_locked, summary, release_year, genres_json, rating,
                cover_path, fetched_utc)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)
             ON CONFLICT(item_id) DO UPDATE SET
                source=excluded.source, igdb_id=excluded.igdb_id,
                steam_appid=excluded.steam_appid, matched_name=excluded.matched_name,
                match_score=excluded.match_score, match_locked=excluded.match_locked,
                summary=excluded.summary, release_year=excluded.release_year,
                genres_json=excluded.genres_json, rating=excluded.rating,
                cover_path=COALESCE(excluded.cover_path, metadata.cover_path),
                fetched_utc=excluded.fetched_utc",
            params![
                item_id, source, igdb_id, steam_appid, matched_name, match_score,
                locked as i64, summary, year,
                serde_json::to_string(genres).unwrap_or_else(|_| "[]".into()),
                rating, cover_path, now
            ],
        )?;
        // Keep the appid on the item too, so duplicate detection can use it later.
        if let Some(appid) = steam_appid {
            self.conn.execute(
                "UPDATE item SET steam_appid = ?1 WHERE id = ?2",
                params![appid, item_id],
            )?;
        }
        Ok(())
    }

    /// Pin an item's metadata match so enrichment never overwrites it.
    pub fn lock_metadata(&mut self, item_id: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE metadata SET match_locked = 1 WHERE item_id = ?1",
            params![item_id],
        )?;
        Ok(())
    }

    /// Count of games still lacking artwork, for the settings screen.
    pub fn metadata_pending_count(&self) -> Result<i64> {
        self.conn.query_row(
            "SELECT COUNT(*) FROM item i
             LEFT JOIN metadata m ON m.item_id = i.id
             WHERE i.missing_since_utc IS NULL
               AND COALESCE(i.user_verdict, i.classification) = 'game'
               AND COALESCE(m.match_locked, 0) = 0
               AND (m.item_id IS NULL OR m.cover_path IS NULL)",
            [],
            |r| r.get(0),
        )
    }
}

/// The schema version this build expects.
const SCHEMA_VERSION: i64 = 1;

/// Bring an existing catalogue up to [`SCHEMA_VERSION`].
///
/// `schema.sql` uses `CREATE TABLE IF NOT EXISTS`, which silently does nothing to a
/// table that already exists — so it can never add a column. Without this, upgrading
/// the app would leave every existing `catalog.db` missing the new columns and every
/// query against them would fail.
fn migrate(conn: &Connection) -> Result<()> {
    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if version >= SCHEMA_VERSION {
        return Ok(());
    }

    // Adding a column that is already present is not an error worth failing on: a
    // freshly created database already has them from schema.sql.
    let add = |table: &str, column: &str, ty: &str| {
        let _ = conn.execute(&format!("ALTER TABLE {table} ADD COLUMN {column} {ty}"), []);
    };

    for (col, ty) in [
        ("platform", "TEXT"), ("title_id", "TEXT"), ("content_id", "TEXT"),
        ("group_role", "TEXT"), ("min_system_ver", "INTEGER"),
        ("max_system_ver", "INTEGER"), ("base_title_id", "TEXT"), ("link_reason", "TEXT"),
    ] {
        add("item", col, ty);
    }
    for (col, ty) in [
        ("content_id", "TEXT"), ("app_ver", "TEXT"), ("system_ver", "INTEGER"),
        ("declared_bytes", "INTEGER"), ("probe_status", "TEXT"),
    ] {
        add("item_file", col, ty);
    }

    conn.execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION}"))?;
    Ok(())
}

fn verdict_str(v: Verdict) -> &'static str {
    match v {
        Verdict::Game => "game",
        Verdict::NeedsReview => "needs_review",
        Verdict::NotGame => "not_game",
    }
}

fn kind_str(k: crate::scan::classify::ItemKind) -> &'static str {
    use crate::scan::classify::ItemKind as K;
    match k {
        K::ArchiveSet => "archive_set",
        K::InstalledGame => "installed_game",
        K::Iso => "iso",
        K::Media => "media",
        K::DevProject => "dev_project",
        K::Utility => "utility",
        K::Unknown => "unknown",
    }
}

/// Cheap duplicate *candidate* signal: the item's file names and sizes. Free to
/// compute during a scan, unlike a content hash over 41 GB.
fn quick_sig(it: &ScannedItem) -> String {
    let mut parts: Vec<String> = match &it.archive_set {
        Some(set) => set
            .parts
            .iter()
            .map(|p| format!("{}|{}", p.file_name.to_lowercase(), p.size_bytes))
            .collect(),
        None => vec![format!("{}|{}", it.raw_name.to_lowercase(), it.total_bytes)],
    };
    parts.sort();
    format!("{:016x}", fnv1a(parts.join("\n").as_bytes()))
}

fn fnv1a(data: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in data {
        h ^= *b as u64;
        h = h.wrapping_mul(0x1000_0000_01b3);
    }
    h
}

/// Build the searchable key: latin lowercase plus Arabic folded to a canonical form,
/// so a name is findable however it was typed.
pub fn search_key(clean_title: &str, raw_name: &str) -> String {
    format!("{} {}", fold_for_search(clean_title), fold_for_search(raw_name))
        .trim()
        .to_string()
}

/// Fold a string for searching: lowercase, Arabic letter variants unified, tashkeel
/// removed. Windows does not normalise filenames, so the same Arabic name can exist on
/// disk in several forms.
pub fn fold_for_search(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .filter_map(|c| match c {
            // Tashkeel (harakat) and tatweel carry no search value.
            '\u{064B}'..='\u{0652}' | '\u{0640}' => None,
            // Bidi control marks. Real filenames on the drive begin with U+200F, so
            // leaving them in makes two otherwise identical names fold differently.
            '\u{200E}' | '\u{200F}' | '\u{061C}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2066}'..='\u{2069}' => None,
            // Alef variants.
            '\u{0623}' | '\u{0625}' | '\u{0622}' | '\u{0671}' => Some('\u{0627}'),
            // Ta marbuta -> ha; alef maqsura -> ya.
            '\u{0629}' => Some('\u{0647}'),
            '\u{0649}' => Some('\u{064A}'),
            other => Some(other),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::classify::{Classification, ItemKind};
    use crate::scan::title;

    fn vol(guid: &str, serial: &str, label: &str) -> VolumeInfo {
        use crate::platform::volume::BusType;
        VolumeInfo {
            volume_guid: guid.into(),
            volume_serial: serial.into(),
            mount_point: Some("E:\\".into()),
            label: label.into(),
            filesystem: "exFAT".into(),
            bus_type: BusType::Usb,
            total_bytes: 500_000_000_000,
            free_bytes: 300_000_000_000,
            is_unjournaled: true,
            max_file_bytes: None,
        }
    }

    fn item(rel: &str, name: &str, bytes: u64, verdict: Verdict) -> ScannedItem {
        ScannedItem {
            rel_path: rel.into(),
            is_folder: true,
            raw_name: name.into(),
            title: title::clean(name, false),
            kind: ItemKind::ArchiveSet,
            verdict,
            confidence: 0.9,
            classification: Classification {
                verdict,
                kind: ItemKind::ArchiveSet,
                confidence: 0.9,
                reasons: vec![],
            },
            total_bytes: bytes,
            file_count: 1,
            files: vec![],
            file_specs: vec![],
            archive_set: None,
        }
    }

    #[test]
    fn a_drive_is_matched_by_guid_not_by_letter() {
        let mut db = Db::open_in_memory().unwrap();
        let mut v = vol("\\\\?\\Volume{aaa}\\", "BEE4EA3B", "خارجي");
        let id1 = db.upsert_drive(&v).unwrap();
        // Same volume, remounted under a different letter — must be the same row.
        v.mount_point = Some("G:\\".into());
        let id2 = db.upsert_drive(&v).unwrap();
        assert_eq!(id1, id2, "a drive letter change must not create a new drive");
        assert_eq!(db.list_drives().unwrap().len(), 1);
    }

    /// The core promise: unplug the drive and the catalogue still answers.
    #[test]
    fn contents_remain_browsable_after_a_drive_goes_offline() {
        let mut db = Db::open_in_memory().unwrap();
        let v = vol("\\\\?\\Volume{aaa}\\", "BEE4EA3B", "خارجي");
        let id = db.upsert_drive(&v).unwrap();
        db.record_scan(
            id,
            &[item("Game\\Assassins Creed Mirage", "Assassins Creed Mirage", 44_000_000_000, Verdict::Game)],
        )
        .unwrap();

        db.mark_offline_except(&[]).unwrap();

        let drives = db.list_drives().unwrap();
        assert!(!drives[0].is_online);
        assert!(drives[0].current_mount.is_none(), "mount must be cleared when offline");
        assert_eq!(drives[0].game_count, 1, "contents survive going offline");

        let items = db.list_items(&ItemFilter::default()).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].display_title, "Assassins Creed Mirage");
        assert!(!items[0].drive_online);
    }

    /// A rescan must never undo a user's correction.
    #[test]
    fn rescan_preserves_the_user_verdict() {
        let mut db = Db::open_in_memory().unwrap();
        let id = db.upsert_drive(&vol("\\\\?\\Volume{a}\\", "S1", "d")).unwrap();
        let scanned = [item("X", "Mystery Thing", 3_000_000_000, Verdict::NeedsReview)];
        db.record_scan(id, &scanned).unwrap();

        let item_id = db.list_items(&ItemFilter::default()).unwrap()[0].id;
        db.set_user_verdict(item_id, Some("game")).unwrap();
        db.set_user_title(item_id, Some("My Actual Game")).unwrap();

        // Scanner still says needs_review; the user says game.
        db.record_scan(id, &scanned).unwrap();

        let rows = db.list_items(&ItemFilter::default()).unwrap();
        assert_eq!(rows[0].verdict, "game", "user verdict must win");
        assert_eq!(rows[0].scanner_verdict, "needs_review", "scanner opinion is kept too");
        assert_eq!(rows[0].display_title, "My Actual Game");
    }

    /// A vanished item is marked, never deleted, so a bad scan cannot destroy data.
    #[test]
    fn a_vanished_item_is_soft_deleted_and_can_come_back() {
        let mut db = Db::open_in_memory().unwrap();
        let id = db.upsert_drive(&vol("\\\\?\\Volume{a}\\", "S1", "d")).unwrap();
        let a = item("A", "Game A", 3_000_000_000, Verdict::Game);
        let b = item("B", "Game B", 3_000_000_000, Verdict::Game);
        db.record_scan(id, &[a.clone(), b.clone()]).unwrap();
        assert_eq!(db.list_items(&ItemFilter::default()).unwrap().len(), 2);

        // B disappears.
        let (_, _, missing) = db.record_scan(id, &[a.clone()]).unwrap();
        assert_eq!(missing, 1);
        assert_eq!(db.list_items(&ItemFilter::default()).unwrap().len(), 1);
        let still_there: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM item", [], |r| r.get(0))
            .unwrap();
        assert_eq!(still_there, 2, "the row is kept, only hidden");

        // B comes back — the same row is revived.
        db.record_scan(id, &[a, b]).unwrap();
        assert_eq!(db.list_items(&ItemFilter::default()).unwrap().len(), 2);
    }

    #[test]
    fn arabic_search_matches_regardless_of_alef_form() {
        let mut db = Db::open_in_memory().unwrap();
        let id = db.upsert_drive(&vol("\\\\?\\Volume{a}\\", "S1", "d")).unwrap();
        db.record_scan(id, &[item("m", "سفرة إيران", 2_000_000_000, Verdict::Game)]).unwrap();

        // Query written with a bare alef rather than alef-with-hamza.
        let f = ItemFilter { search: Some("ايران".into()), ..Default::default() };
        assert_eq!(db.list_items(&f).unwrap().len(), 1, "alef variants must fold together");
    }

    #[test]
    fn filters_narrow_by_drive_and_verdict() {
        let mut db = Db::open_in_memory().unwrap();
        let id = db.upsert_drive(&vol("\\\\?\\Volume{a}\\", "S1", "d")).unwrap();
        db.record_scan(
            id,
            &[
                item("A", "Real Game", 9_000_000_000, Verdict::Game),
                item("B", "Some ISO", 4_000_000_000, Verdict::NotGame),
            ],
        )
        .unwrap();

        let games = ItemFilter { verdict: Some("game".into()), ..Default::default() };
        assert_eq!(db.list_items(&games).unwrap().len(), 1);
        let all = db.list_items(&ItemFilter::default()).unwrap();
        assert_eq!(all.len(), 2, "rejected items stay visible and promotable");
    }

    #[test]
    fn duplicates_are_found_across_drives() {
        let mut db = Db::open_in_memory().unwrap();
        let d1 = db.upsert_drive(&vol("\\\\?\\Volume{a}\\", "S1", "one")).unwrap();
        let d2 = db.upsert_drive(&vol("\\\\?\\Volume{b}\\", "S2", "two")).unwrap();
        db.record_scan(d1, &[item("A", "Mad Max", 4_000_000_000, Verdict::Game)]).unwrap();
        db.record_scan(d2, &[item("B", "Mad Max", 4_000_000_000, Verdict::Game)]).unwrap();

        let dupes = db.duplicates().unwrap();
        assert_eq!(dupes.len(), 1);
        assert_eq!(dupes[0].0, "madmax");
        assert_eq!(dupes[0].1, 2);
    }
}
