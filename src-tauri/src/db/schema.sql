-- GameVault catalogue schema.
--
-- Lives in %LOCALAPPDATA%\GameVault\catalog.db — never on an external drive, so the
-- catalogue survives any drive being unplugged, reformatted or lost.
--
-- The organising principle: a drive is identified by its volume GUID, never by its
-- letter. On the target machine a desktop shortcut points at F:\Ghost of Tsushimaa\...
-- while F: is currently a Zorin OS installer stick, and Steam's libraryfolders.vdf
-- references a long-gone E:\SteamLibrary. Letters are reassigned; GUIDs are not.

PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;
PRAGMA synchronous = NORMAL;

-- ---------------------------------------------------------------------------
-- Drives
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS drive (
    id              INTEGER PRIMARY KEY,
    -- \\?\Volume{GUID}\ — stable identity on this machine.
    volume_guid     TEXT    NOT NULL UNIQUE,
    -- Filesystem serial (e.g. 'BEE4EA3B'). Travels with the filesystem to other
    -- machines, so it is the fallback identity when a GUID is not recognised.
    volume_serial   TEXT    NOT NULL,
    label           TEXT    NOT NULL DEFAULT '',
    filesystem      TEXT    NOT NULL DEFAULT '',
    -- 'usb' | 'nvme' | 'sata' | ... Decides whether a drive is external. Deliberately
    -- not GetDriveType: the user's USB games drive reports DRIVE_FIXED.
    bus_type        TEXT    NOT NULL DEFAULT 'other',
    total_bytes     INTEGER NOT NULL DEFAULT 0,
    free_bytes      INTEGER NOT NULL DEFAULT 0,
    -- exFAT/FAT have no journal; the transfer engine flushes per file on these.
    is_unjournaled  INTEGER NOT NULL DEFAULT 0,
    -- 4 GiB-1 on FAT32, NULL when unlimited. The user's RAR volumes are 8.5 GB, so a
    -- copy to FAT32 must be refused in preflight rather than failing mid-transfer.
    max_file_bytes  INTEGER,
    -- Current mount point, e.g. 'E:\'. DERIVED on every poll, NULL when offline.
    -- Never used as identity.
    current_mount   TEXT,
    is_online       INTEGER NOT NULL DEFAULT 0,
    scan_roots_json TEXT    NOT NULL DEFAULT '[]',
    first_seen_utc  TEXT    NOT NULL,
    last_seen_utc   TEXT    NOT NULL,
    last_scan_utc   TEXT
);

CREATE INDEX IF NOT EXISTS ix_drive_serial ON drive(volume_serial);

-- ---------------------------------------------------------------------------
-- Items
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS item (
    id            INTEGER PRIMARY KEY,
    drive_id      INTEGER NOT NULL REFERENCES drive(id) ON DELETE CASCADE,
    -- Path relative to the drive root, e.g. 'Game\Assassins Creed Mirage'.
    rel_path      TEXT    NOT NULL,
    -- Populated ONLY when the path could not round-trip through UTF-8. Windows paths
    -- are UTF-16 and may contain unpaired surrogates, which no UTF-8 string can hold.
    rel_path_blob BLOB,

    -- 'archive_set' | 'installed_game' | 'iso' | 'media' | 'dev_project' | 'utility'
    kind          TEXT    NOT NULL,
    -- 'folder' when the directory is the item, 'file' for loose archives on a shelf.
    container     TEXT    NOT NULL,

    raw_name      TEXT    NOT NULL,          -- verbatim, never normalised
    clean_title   TEXT    NOT NULL,
    title_key     TEXT    NOT NULL,          -- latin match key
    -- Arabic-folded search key (أإآ->ا, ة->ه, ى->ي, tashkeel stripped) so a name is
    -- findable however it was typed. Windows does not normalise filenames, so the
    -- same Arabic name can exist on disk in both NFC and NFD.
    search_key    TEXT    NOT NULL,

    repacker      TEXT,
    release_group TEXT,
    version       TEXT,
    edition       TEXT,
    year          INTEGER,
    -- Bracketed content stripped but not recognised. Kept so nothing is silently lost.
    notes         TEXT,

    total_bytes   INTEGER NOT NULL DEFAULT 0,
    file_count    INTEGER NOT NULL DEFAULT 0,

    -- Archive-set facts.
    part_count         INTEGER,
    set_complete       INTEGER,
    missing_parts_json TEXT,
    incomplete_reason  TEXT,

    main_exe_rel  TEXT,

    -- 'game' | 'needs_review' | 'not_game'
    classification TEXT   NOT NULL,
    confidence    REAL    NOT NULL,
    -- [{rule, delta, note}] — rendered verbatim in the review UI so every verdict is
    -- explainable.
    reasons_json  TEXT    NOT NULL DEFAULT '[]',

    -- The user's override. NEVER written by the scanner; a rescan must not undo a
    -- correction the user made.
    user_verdict  TEXT,
    user_title    TEXT,

    -- sha1 of the sorted (name|size) list — a free duplicate *candidate* signal that
    -- needs no hashing. content_hash is the exact answer, and is opt-in because
    -- hashing 41 GB takes minutes.
    quick_sig     TEXT    NOT NULL DEFAULT '',
    content_hash  TEXT,

    steam_appid   INTEGER,

    first_seen_utc    TEXT NOT NULL,
    last_seen_utc     TEXT NOT NULL,
    -- Soft delete. The scanner never issues DELETE: a vanished item is marked, so an
    -- unplugged drive's contents stay browsable and a rescan cannot lose data.
    missing_since_utc TEXT,

    -- Console fields. Appended at the end because list_items reads positionally.
    platform       TEXT,     -- ps4 | xbox360 | xbox
    title_id       TEXT,     -- CUSA08948 | 4E4D07D1 — the exact grouping key
    content_id     TEXT,
    group_role     TEXT,     -- game | addon_linked | addon_orphan | update_orphan
    min_system_ver INTEGER,  -- lowest firmware that yields a playable base
    max_system_ver INTEGER,
    base_title_id  TEXT,     -- an orphan add-on's base game, on the same drive
    link_reason    TEXT,     -- why that link was made; shown to the user

    UNIQUE(drive_id, rel_path)
);

CREATE INDEX IF NOT EXISTS ix_item_titlekey ON item(title_key);
CREATE INDEX IF NOT EXISTS ix_item_quicksig ON item(quick_sig);
CREATE INDEX IF NOT EXISTS ix_item_class    ON item(classification, confidence);
CREATE INDEX IF NOT EXISTS ix_item_drive    ON item(drive_id, missing_since_utc);
CREATE INDEX IF NOT EXISTS ix_item_hash     ON item(content_hash) WHERE content_hash IS NOT NULL;

-- Files belonging to an item. For an archive set these are the volumes, and
-- part_index is extraction order — the order a transfer must preserve.
CREATE TABLE IF NOT EXISTS item_file (
    id          INTEGER PRIMARY KEY,
    item_id     INTEGER NOT NULL REFERENCES item(id) ON DELETE CASCADE,
    rel_path    TEXT    NOT NULL,
    size_bytes  INTEGER NOT NULL,
    mtime_unix  INTEGER NOT NULL,
    part_index  INTEGER,
    -- 'archive_part' | 'exe' | 'data' | 'sidecar'
    role        TEXT    NOT NULL DEFAULT 'data',
    blake3      TEXT,
    -- Console per-file facts, read from inside the file itself.
    content_id     TEXT,
    app_ver        TEXT,
    system_ver     INTEGER,  -- required firmware, BCD (0x09000000 = 9.00)
    declared_bytes INTEGER,  -- the size the file says it is; a mismatch means truncated
    probe_status   TEXT,     -- ok | not_pkg | truncated | continuation | unreadable
    UNIQUE(item_id, rel_path)
);

CREATE INDEX IF NOT EXISTS ix_itemfile_item ON item_file(item_id, part_index);

-- ---------------------------------------------------------------------------
-- Metadata (strictly additive — nothing in scan, browse or copy depends on it)
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS metadata (
    item_id      INTEGER PRIMARY KEY REFERENCES item(id) ON DELETE CASCADE,
    -- 'steam_applist' | 'igdb' | 'steamgriddb' | 'manual' | 'none'
    source       TEXT    NOT NULL DEFAULT 'none',
    igdb_id      INTEGER,
    sgdb_id      INTEGER,
    steam_appid  INTEGER,
    matched_name TEXT,
    match_score  REAL,
    -- Set when the user corrects a match. Every enrichment pass filters on
    -- match_locked = 0, which is the structural guarantee that a correction sticks.
    match_locked INTEGER NOT NULL DEFAULT 0,
    summary      TEXT,
    release_year INTEGER,
    genres_json  TEXT,
    rating       REAL,
    -- Local cache paths under %LOCALAPPDATA%. Never on the external drive.
    cover_path   TEXT,
    hero_path    TEXT,
    logo_path    TEXT,
    fetched_utc  TEXT
);

CREATE TABLE IF NOT EXISTS tag (
    id    INTEGER PRIMARY KEY,
    name  TEXT NOT NULL UNIQUE,
    color TEXT
);

CREATE TABLE IF NOT EXISTS item_tag (
    item_id INTEGER NOT NULL REFERENCES item(id) ON DELETE CASCADE,
    tag_id  INTEGER NOT NULL REFERENCES tag(id)  ON DELETE CASCADE,
    PRIMARY KEY (item_id, tag_id)
);

-- ---------------------------------------------------------------------------
-- Transfer jobs
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS job (
    id           INTEGER PRIMARY KEY,
    kind         TEXT    NOT NULL,          -- 'copy' | 'move'
    item_id      INTEGER REFERENCES item(id) ON DELETE SET NULL,
    src_drive_id INTEGER NOT NULL REFERENCES drive(id),
    dst_drive_id INTEGER NOT NULL REFERENCES drive(id),
    src_rel_path TEXT    NOT NULL,
    dst_rel_path TEXT    NOT NULL,
    total_bytes  INTEGER NOT NULL DEFAULT 0,
    copied_bytes INTEGER NOT NULL DEFAULT 0,
    -- queued | preflight | copying | paused | verifying | deleting_source
    --        | done | failed | cancelled
    state        TEXT    NOT NULL DEFAULT 'queued',
    verify_mode  TEXT    NOT NULL DEFAULT 'size',      -- size | size_mtime | hash
    strategy     TEXT    NOT NULL DEFAULT 'rust_stream',
    collision    TEXT    NOT NULL DEFAULT 'rename',    -- never silently overwrite
    delete_mode  TEXT,                                 -- recycle | hard | none
    error        TEXT,
    created_utc  TEXT    NOT NULL,
    started_utc  TEXT,
    finished_utc TEXT
);

CREATE INDEX IF NOT EXISTS ix_job_state ON job(state, created_utc);

-- Per-file progress. Persisted after every transition so an unplug or crash resumes
-- exactly, and so the "all verified" gate below can be evaluated from durable state.
CREATE TABLE IF NOT EXISTS job_file (
    id         INTEGER PRIMARY KEY,
    job_id     INTEGER NOT NULL REFERENCES job(id) ON DELETE CASCADE,
    src_rel    TEXT    NOT NULL,
    dst_rel    TEXT    NOT NULL,
    size_bytes INTEGER NOT NULL,
    bytes_done INTEGER NOT NULL DEFAULT 0,
    -- pending | copying | copied | verified | source_deleted | failed | skipped
    state      TEXT    NOT NULL DEFAULT 'pending',
    src_hash   TEXT,
    dst_hash   TEXT,
    attempts   INTEGER NOT NULL DEFAULT 0,
    error      TEXT
);

CREATE INDEX IF NOT EXISTS ix_jobfile_job ON job_file(job_id, state);

-- A move may delete its sources only when every file has reached 'verified'. Reading
-- this view is the gate; it is a view rather than inline SQL so the rule exists in
-- exactly one place.
CREATE VIEW IF NOT EXISTS job_delete_gate AS
SELECT j.id                                                   AS job_id,
       COUNT(f.id)                                            AS total_files,
       SUM(CASE WHEN f.state = 'verified' THEN 1 ELSE 0 END)  AS verified_files,
       SUM(CASE WHEN f.state <> 'verified' THEN 1 ELSE 0 END) AS unverified_files
FROM job j
JOIN job_file f ON f.job_id = j.id
GROUP BY j.id;

CREATE TABLE IF NOT EXISTS scan_run (
    id            INTEGER PRIMARY KEY,
    drive_id      INTEGER NOT NULL REFERENCES drive(id) ON DELETE CASCADE,
    started_utc   TEXT    NOT NULL,
    finished_utc  TEXT,
    dirs_seen     INTEGER NOT NULL DEFAULT 0,
    files_seen    INTEGER NOT NULL DEFAULT 0,
    bytes_seen    INTEGER NOT NULL DEFAULT 0,
    items_found   INTEGER NOT NULL DEFAULT 0,
    items_new     INTEGER NOT NULL DEFAULT 0,
    items_missing INTEGER NOT NULL DEFAULT 0,
    error         TEXT
);

CREATE TABLE IF NOT EXISTS app_setting (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

-- Full-text search. remove_diacritics 2 folds Arabic tashkeel as well as latin
-- accents; the trigram index backs substring search, which unicode61 cannot do for
-- scripts that do not tokenise on spaces the way latin does.
CREATE VIRTUAL TABLE IF NOT EXISTS item_fts USING fts5(
    search_key, clean_title, raw_name, repacker,
    content = 'item', content_rowid = 'id',
    tokenize = "unicode61 remove_diacritics 2"
);

CREATE VIRTUAL TABLE IF NOT EXISTS item_tri USING fts5(
    search_key,
    content = 'item', content_rowid = 'id',
    tokenize = "trigram"
);

CREATE TRIGGER IF NOT EXISTS item_ai AFTER INSERT ON item BEGIN
    INSERT INTO item_fts(rowid, search_key, clean_title, raw_name, repacker)
    VALUES (new.id, new.search_key, new.clean_title, new.raw_name, new.repacker);
    INSERT INTO item_tri(rowid, search_key) VALUES (new.id, new.search_key);
END;

CREATE TRIGGER IF NOT EXISTS item_ad AFTER DELETE ON item BEGIN
    INSERT INTO item_fts(item_fts, rowid, search_key, clean_title, raw_name, repacker)
    VALUES ('delete', old.id, old.search_key, old.clean_title, old.raw_name, old.repacker);
    INSERT INTO item_tri(item_tri, rowid, search_key) VALUES ('delete', old.id, old.search_key);
END;

CREATE TRIGGER IF NOT EXISTS item_au AFTER UPDATE ON item BEGIN
    INSERT INTO item_fts(item_fts, rowid, search_key, clean_title, raw_name, repacker)
    VALUES ('delete', old.id, old.search_key, old.clean_title, old.raw_name, old.repacker);
    INSERT INTO item_fts(rowid, search_key, clean_title, raw_name, repacker)
    VALUES (new.id, new.search_key, new.clean_title, new.raw_name, new.repacker);
    INSERT INTO item_tri(item_tri, rowid, search_key) VALUES ('delete', old.id, old.search_key);
    INSERT INTO item_tri(rowid, search_key) VALUES (new.id, new.search_key);
END;

-- ---------------------------------------------------------------------------
-- Console support
-- ---------------------------------------------------------------------------

-- Cached results of reading inside game containers. Keyed on (path, size, mtime) so a
-- rescan re-reads nothing that has not changed: probing is ~106 ms per PKG over USB,
-- which is 7.5 s for one drive the first time and instant afterwards.
CREATE TABLE IF NOT EXISTS console_probe (
    drive_id     INTEGER NOT NULL REFERENCES drive(id) ON DELETE CASCADE,
    rel_path     TEXT    NOT NULL,
    size_bytes   INTEGER NOT NULL,
    mtime_unix   INTEGER NOT NULL,
    platform     TEXT    NOT NULL,
    status       TEXT    NOT NULL,
    title_id     TEXT,
    content_id   TEXT,
    category     TEXT,
    title        TEXT,
    app_ver      TEXT,
    system_ver   INTEGER,
    declared_bytes INTEGER,
    content_type INTEGER,
    cover_cache  TEXT,
    probed_utc   TEXT    NOT NULL,
    PRIMARY KEY (drive_id, rel_path)
);

-- Problems worth acting on: truncated files, duplicates, orphaned updates.
--
-- `dismissed` follows the same rule as item.user_verdict: the scanner NEVER writes it,
-- so a dismissal is permanent. A finding the scan no longer sees gets resolved_utc set
-- rather than being deleted, so the history survives.
CREATE TABLE IF NOT EXISTS health_finding (
    id                INTEGER PRIMARY KEY,
    drive_id          INTEGER NOT NULL REFERENCES drive(id) ON DELETE CASCADE,
    item_id           INTEGER REFERENCES item(id) ON DELETE SET NULL,
    code              TEXT    NOT NULL,
    severity          TEXT    NOT NULL,   -- error | warn | info
    rel_path          TEXT    NOT NULL,
    peer_rel_path     TEXT,
    reclaimable_bytes INTEGER NOT NULL DEFAULT 0,
    detail            TEXT    NOT NULL,
    dismissed         INTEGER NOT NULL DEFAULT 0,
    first_seen_utc    TEXT    NOT NULL,
    last_seen_utc     TEXT    NOT NULL,
    resolved_utc      TEXT,
    UNIQUE(drive_id, code, rel_path)
);

CREATE INDEX IF NOT EXISTS ix_item_titleid  ON item(platform, title_id);
CREATE INDEX IF NOT EXISTS ix_finding_drive ON health_finding(drive_id, dismissed, resolved_utc);
