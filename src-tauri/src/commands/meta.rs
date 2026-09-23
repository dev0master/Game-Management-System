//! Metadata commands.
//!
//! Enrichment is a decorator, never a dependency. It runs on its own thread, writes to
//! its own table, and every screen works identically when it has never run — which is
//! the normal state until the user supplies credentials.

use super::{AppState, CmdError, CmdResult};
use crate::db::MetaRecord;
use crate::meta::rawg;
use crate::platform::dpapi::{self, Credentials};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tauri::{AppHandle, Emitter, Manager, State};

/// Where downloaded covers live. Under `%LOCALAPPDATA%`, never on an external drive:
/// artwork is derived data and must not eat space on the drives being managed.
pub fn cover_dir() -> PathBuf {
    let base = std::env::var("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir());
    base.join("GameVault").join("covers")
}

/// Credential status for the settings screen.
///
/// The secret is never returned — only whether one is stored. Sending it back to the
/// UI would put it in webview memory and in any IPC log for no benefit.
#[derive(Debug, Serialize)]
pub struct CredentialStatus {
    pub has_credentials: bool,
    /// The stored key, masked. Never the key itself: returning it would put the secret
    /// back into webview memory and into every IPC log for no benefit.
    pub key_hint: String,
    pub pending_count: i64,
    pub cover_dir: String,
}

#[tauri::command]
pub async fn credential_status(state: State<'_, AppState>) -> CmdResult<CredentialStatus> {
    let creds = dpapi::load_credentials();
    let pending = {
        let db = state.db.lock().map_err(|e| CmdError::from(e.to_string()))?;
        db.metadata_pending_count()?
    };
    Ok(CredentialStatus {
        has_credentials: creds.is_complete(),
        key_hint: creds.masked(),
        pending_count: pending,
        cover_dir: cover_dir().to_string_lossy().into_owned(),
    })
}

#[derive(Debug, Deserialize)]
pub struct CredentialArgs {
    pub api_key: String,
}

/// Save credentials and verify them in one step, so the user learns immediately
/// whether they work rather than at the next enrichment run.
#[tauri::command]
pub async fn save_credentials(args: CredentialArgs) -> CmdResult<()> {
    let creds = Credentials { rawg_api_key: args.api_key.trim().to_string() };
    rawg::test_credentials(&creds).map_err(CmdError::from)?;
    dpapi::save_credentials(&creds).map_err(CmdError::from)?;
    Ok(())
}

#[tauri::command]
pub async fn clear_credentials() -> CmdResult<()> {
    dpapi::clear_credentials().map_err(CmdError::from)
}

#[derive(Debug, Serialize, Clone)]
pub struct EnrichProgress {
    pub done: usize,
    pub total: usize,
    pub current: String,
    pub matched: usize,
    pub covers: usize,
    pub finished: bool,
    pub error: Option<String>,
}

/// Fetch metadata and covers for everything that still lacks them.
///
/// Returns as soon as the run starts; progress arrives as `meta://progress` events.
#[tauri::command]
pub async fn enrich_library(
    limit: Option<i64>,
    app: AppHandle,
    state: State<'_, AppState>,
) -> CmdResult<usize> {
    let creds = dpapi::load_credentials();
    if !creds.is_complete() {
        return Err(CmdError::from(
            "no API key saved — add a free RAWG key in Settings",
        ));
    }

    let pending = {
        let db = state.db.lock().map_err(|e| CmdError::from(e.to_string()))?;
        db.items_needing_metadata(limit.unwrap_or(200))?
    };
    let total = pending.len();
    if total == 0 {
        return Ok(0);
    }

    std::thread::spawn(move || {
        let dir = cover_dir();
        let mut matched = 0usize;
        let mut covers = 0usize;

        for (done, (item_id, title, year)) in pending.into_iter().enumerate() {
            let _ = app.emit(
                "meta://progress",
                EnrichProgress {
                    done,
                    total,
                    current: title.clone(),
                    matched,
                    covers,
                    finished: false,
                    error: None,
                },
            );

            // RAWG is rate limited, so a failure on one title must not abort the run;
            // the next attempt simply picks it up again.
            let found = match rawg::lookup(&creds, &title, year.map(|y| y as i32)) {
                Ok(v) => v,
                Err(e) => {
                    let _ = app.emit(
                        "meta://progress",
                        EnrichProgress {
                            done,
                            total,
                            current: title.clone(),
                            matched,
                            covers,
                            finished: false,
                            error: Some(e),
                        },
                    );
                    continue;
                }
            };

            let Some(meta) = found else { continue };
            matched += 1;

            // Steam's portrait box art when RAWG says the game is there, RAWG's own
            // landscape art otherwise. No artwork is not an error worth stopping for.
            let cover_path = rawg::fetch_cover(&meta, &dir).map(|p| {
                covers += 1;
                p.to_string_lossy().into_owned()
            });

            if let Some(state) = app.try_state::<AppState>() {
                if let Ok(mut db) = state.db.lock() {
                    let _ = db.save_metadata(&MetaRecord {
                        item_id,
                        source: "rawg",
                        rawg_id: meta.rawg_id,
                        igdb_id: None,
                        steam_appid: meta.steam_appid,
                        matched_name: &meta.name,
                        match_score: meta.match_score,
                        year: meta.year,
                        summary: meta.summary.as_deref(),
                        genres: &meta.genres,
                        rating: meta.rating,
                        cover_path: cover_path.as_deref(),
                        locked: false,
                    });
                }
            }
        }

        let _ = app.emit(
            "meta://progress",
            EnrichProgress {
                done: total,
                total,
                current: String::new(),
                matched,
                covers,
                finished: true,
                error: None,
            },
        );
    });

    Ok(total)
}

/// Accept a metadata match explicitly, locking it against future enrichment runs.
#[tauri::command]
pub async fn lock_metadata(item_id: i64, state: State<'_, AppState>) -> CmdResult<()> {
    let mut db = state.db.lock().map_err(|e| CmdError::from(e.to_string()))?;
    db.lock_metadata(item_id)?;
    Ok(())
}
