//! Metadata commands.
//!
//! Enrichment is a decorator, never a dependency. It runs on its own thread, writes to
//! its own table, and every screen works identically when it has never run — which is
//! the normal state until the user supplies credentials.

use super::{AppState, CmdError, CmdResult};
use crate::meta::igdb;
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
    pub client_id: String,
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
        client_id: creds.twitch_client_id,
        pending_count: pending,
        cover_dir: cover_dir().to_string_lossy().into_owned(),
    })
}

#[derive(Debug, Deserialize)]
pub struct CredentialArgs {
    pub client_id: String,
    pub client_secret: String,
}

/// Save credentials and verify them in one step, so the user learns immediately
/// whether they work rather than at the next enrichment run.
#[tauri::command]
pub async fn save_credentials(args: CredentialArgs) -> CmdResult<()> {
    let creds = Credentials {
        twitch_client_id: args.client_id.trim().to_string(),
        twitch_client_secret: args.client_secret.trim().to_string(),
    };
    igdb::test_credentials(&creds).map_err(CmdError::from)?;
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
            "no credentials saved — add a Twitch Client ID and Secret in Settings",
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

            // IGDB is rate limited, so a failure on one title must not abort the run;
            // the next attempt simply picks it up again.
            let found = match igdb::lookup(&creds, &title, year.map(|y| y as i32)) {
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

            // The cover comes from Steam's CDN, reached via the app id IGDB supplies.
            let cover_path = meta.cover_url.as_ref().and_then(|url| {
                let key = meta.steam_appid.map(|a| a.to_string()).unwrap_or_else(|| format!("igdb{}", meta.igdb_id.unwrap_or(0)));
                match igdb::download_image(url, &dir, &key) {
                    Ok(p) => {
                        covers += 1;
                        Some(p.to_string_lossy().into_owned())
                    }
                    Err(_) => None, // No artwork is not an error worth stopping for.
                }
            });

            if let Some(state) = app.try_state::<AppState>() {
                if let Ok(mut db) = state.db.lock() {
                    let _ = db.save_metadata(
                        item_id,
                        "igdb",
                        meta.igdb_id,
                        meta.steam_appid,
                        &meta.name,
                        meta.match_score,
                        meta.year,
                        meta.summary.as_deref(),
                        &meta.genres,
                        meta.rating,
                        cover_path.as_deref(),
                        false,
                    );
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
