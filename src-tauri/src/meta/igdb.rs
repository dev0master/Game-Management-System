//! IGDB metadata, and covers by way of Steam.
//!
//! This network reaches IGDB's API and Twitch's OAuth endpoint, but not
//! `images.igdb.com`, SteamGridDB, or `store.steampowered.com`. Steam's own image CDN
//! *is* reachable. So the working path is: ask IGDB for the game, take the Steam app id
//! it reports under `external_games`, and fetch the cover from Steam's CDN.
//!
//! Everything here is optional. With no credentials the app is exactly as useful as
//! before, just without artwork — nothing in scanning, browsing or copying waits on it.

use crate::platform::dpapi::Credentials;
use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use std::time::{Duration, Instant};

const TOKEN_URL: &str = "https://id.twitch.tv/oauth2/token";
const IGDB_URL: &str = "https://api.igdb.com/v4";

/// IGDB permits 4 requests per second. A little under keeps us clear of 429s even when
/// several replies land in the same tick.
const MIN_REQUEST_GAP: Duration = Duration::from_millis(260);

/// Steam's cover art. Verified reachable from this network, unlike IGDB's own CDN.
fn steam_cover_url(appid: i64) -> String {
    format!("https://cdn.cloudflare.steamstatic.com/steam/apps/{appid}/library_600x900.jpg")
}

fn steam_hero_url(appid: i64) -> String {
    format!("https://cdn.cloudflare.steamstatic.com/steam/apps/{appid}/library_hero.jpg")
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GameMeta {
    pub igdb_id: Option<i64>,
    pub steam_appid: Option<i64>,
    pub name: String,
    pub year: Option<i32>,
    pub summary: Option<String>,
    pub genres: Vec<String>,
    pub rating: Option<f64>,
    /// Remote cover URL, when one could be resolved.
    pub cover_url: Option<String>,
    pub hero_url: Option<String>,
    /// 0-100 confidence that this is the right game.
    pub match_score: f64,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    expires_in: u64,
}

#[derive(Deserialize)]
struct IgdbGame {
    id: i64,
    name: String,
    #[serde(default)]
    first_release_date: Option<i64>,
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    genres: Option<Vec<IgdbNamed>>,
    #[serde(default)]
    total_rating: Option<f64>,
    #[serde(default)]
    external_games: Option<Vec<IgdbExternal>>,
}

#[derive(Deserialize)]
struct IgdbNamed {
    name: String,
}

#[derive(Deserialize)]
struct IgdbExternal {
    /// IGDB's external service id. 1 is Steam.
    #[serde(default)]
    category: Option<i32>,
    #[serde(default)]
    uid: Option<String>,
}

/// A cached OAuth token. Twitch client-credentials tokens last about 60 days, so this
/// avoids a token request per lookup.
struct CachedToken {
    value: String,
    expires_at: Instant,
}

static TOKEN: Mutex<Option<CachedToken>> = Mutex::new(None);
static LAST_REQUEST: Mutex<Option<Instant>> = Mutex::new(None);

/// Block until at least [`MIN_REQUEST_GAP`] has passed since the previous call.
///
/// A simple serialising gate rather than a token bucket: lookups here are a batch of
/// sequential requests, so pacing them is all that is needed to stay inside the limit.
fn throttle() {
    let mut last = LAST_REQUEST.lock().unwrap();
    if let Some(prev) = *last {
        let elapsed = prev.elapsed();
        if elapsed < MIN_REQUEST_GAP {
            std::thread::sleep(MIN_REQUEST_GAP - elapsed);
        }
    }
    *last = Some(Instant::now());
}

fn client() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(25))
        .user_agent("GameVault/0.1")
        .build()
        .map_err(|e| e.to_string())
}

/// Fetch, or reuse, an OAuth token.
fn token(creds: &Credentials) -> Result<String, String> {
    {
        let cached = TOKEN.lock().unwrap();
        if let Some(t) = cached.as_ref() {
            if t.expires_at > Instant::now() {
                return Ok(t.value.clone());
            }
        }
    }

    throttle();
    let resp = client()?
        .post(TOKEN_URL)
        .query(&[
            ("client_id", creds.twitch_client_id.as_str()),
            ("client_secret", creds.twitch_client_secret.as_str()),
            ("grant_type", "client_credentials"),
        ])
        .send()
        .map_err(|e| format!("could not reach Twitch: {e}"))?;

    if !resp.status().is_success() {
        let code = resp.status();
        return Err(match code.as_u16() {
            400 | 401 | 403 => "Twitch rejected the credentials — check the Client ID and Secret".to_string(),
            _ => format!("Twitch returned {code}"),
        });
    }

    let body: TokenResponse = resp.json().map_err(|e| format!("bad token response: {e}"))?;
    let mut cached = TOKEN.lock().unwrap();
    *cached = Some(CachedToken {
        value: body.access_token.clone(),
        // Refresh a minute early rather than racing the expiry.
        expires_at: Instant::now() + Duration::from_secs(body.expires_in.saturating_sub(60)),
    });
    Ok(body.access_token)
}

/// Verify credentials without performing a lookup. Used by the settings screen.
pub fn test_credentials(creds: &Credentials) -> Result<(), String> {
    if !creds.is_complete() {
        return Err("both the Client ID and the Client Secret are required".into());
    }
    // Clear any cached token so a changed secret is actually exercised.
    *TOKEN.lock().unwrap() = None;
    token(creds).map(|_| ())
}

/// Words of a title, lowercased, sorted and de-duplicated.
///
/// Sorting is what makes the comparison order-insensitive: release names reorder and
/// pad words freely, so "Witcher 3 Wild Hunt, The" and "The Witcher 3: Wild Hunt"
/// should score the same. A plain edit distance over the raw strings would not.
fn token_key(s: &str) -> String {
    let mut tokens: Vec<&str> = s
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .collect();
    tokens.sort_unstable();
    tokens.dedup();
    tokens.join(" ").to_lowercase()
}

/// Similarity between a catalogue title and a candidate name, 0-100.
fn score(query_title: &str, candidate: &str) -> f64 {
    // The normalised keys already fold articles, punctuation and roman numerals, so an
    // exact key match is a certain hit.
    if crate::scan::title::make_key(query_title) == crate::scan::title::make_key(candidate) {
        return 100.0;
    }
    let a = token_key(query_title);
    let b = token_key(candidate);
    // rapidfuzz returns a 0..1 similarity; the thresholds here are expressed on a
    // 0..100 scale to match how the score is shown to the user.
    rapidfuzz::fuzz::ratio(a.chars(), b.chars()) * 100.0
}

/// Look up one title. Returns the best match, or `None` when nothing scores usefully.
pub fn lookup(creds: &Credentials, clean_title: &str, year_hint: Option<i32>) -> Result<Option<GameMeta>, String> {
    let tok = token(creds)?;
    let query_key = crate::scan::title::make_key(clean_title);
    if query_key.is_empty() {
        return Ok(None);
    }

    // IGDB's query language. `external_games` is requested because it carries the Steam
    // app id, which is the only route to cover art on this network.
    let escaped = clean_title.replace('"', "");
    let body = format!(
        r#"search "{escaped}"; fields id,name,first_release_date,summary,total_rating,genres.name,external_games.category,external_games.uid; limit 12;"#
    );

    throttle();
    let resp = client()?
        .post(format!("{IGDB_URL}/games"))
        .header("Client-ID", &creds.twitch_client_id)
        .header("Authorization", format!("Bearer {tok}"))
        .body(body)
        .send()
        .map_err(|e| format!("could not reach IGDB: {e}"))?;

    if resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return Err("IGDB rate limit reached — try again in a moment".into());
    }
    if !resp.status().is_success() {
        return Err(format!("IGDB returned {}", resp.status()));
    }

    let games: Vec<IgdbGame> = resp.json().map_err(|e| format!("bad IGDB response: {e}"))?;

    let mut best: Option<(f64, &IgdbGame)> = None;
    for g in &games {
        let mut s = score(clean_title, &g.name);
        // A matching year is a meaningful tiebreak between same-named releases.
        if let (Some(hint), Some(ts)) = (year_hint, g.first_release_date) {
            if year_of(ts) == Some(hint) {
                s = (s + 6.0).min(100.0);
            }
        }
        if best.as_ref().map(|(bs, _)| s > *bs).unwrap_or(true) {
            best = Some((s, g));
        }
    }

    let Some((match_score, g)) = best else {
        return Ok(None);
    };
    // Below this the candidate is more likely wrong than right, and a wrong cover is
    // worse than none: it makes the library actively misleading.
    if match_score < 70.0 {
        return Ok(None);
    }

    let steam_appid = g
        .external_games
        .as_ref()
        .and_then(|list| {
            list.iter()
                .find(|e| e.category == Some(1)) // 1 = Steam
                .and_then(|e| e.uid.as_ref())
                .and_then(|uid| uid.parse::<i64>().ok())
        });

    Ok(Some(GameMeta {
        igdb_id: Some(g.id),
        steam_appid,
        name: g.name.clone(),
        year: g.first_release_date.and_then(year_of),
        summary: g.summary.clone(),
        genres: g
            .genres
            .as_ref()
            .map(|v| v.iter().map(|x| x.name.clone()).collect())
            .unwrap_or_default(),
        rating: g.total_rating,
        cover_url: steam_appid.map(steam_cover_url),
        hero_url: steam_appid.map(steam_hero_url),
        match_score,
    }))
}

fn year_of(unix_secs: i64) -> Option<i32> {
    let s = crate::db::time::format_unix(unix_secs);
    s.get(..4).and_then(|y| y.parse().ok())
}

/// Download an image to the local cover cache and return its path.
///
/// Covers live under `%LOCALAPPDATA%`, never on an external drive: they are derived
/// data that must not consume space on the drives the user is trying to manage.
pub fn download_image(url: &str, cache_dir: &std::path::Path, key: &str) -> Result<std::path::PathBuf, String> {
    std::fs::create_dir_all(cache_dir).map_err(|e| e.to_string())?;
    let dest = cache_dir.join(format!("{key}.jpg"));
    if dest.exists() {
        return Ok(dest);
    }

    let resp = client()?
        .get(url)
        .send()
        .map_err(|e| format!("could not fetch image: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("image request returned {}", resp.status()));
    }
    let bytes = resp.bytes().map_err(|e| e.to_string())?;
    // Steam answers a missing cover with a tiny placeholder rather than a 404, so a
    // suspiciously small body means "no artwork", not "here is your artwork".
    if bytes.len() < 2048 {
        return Err("no artwork available for this title".into());
    }
    std::fs::write(&dest, &bytes).map_err(|e| e.to_string())?;
    Ok(dest)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pins the score scale. An earlier version compared a 0..1 similarity against
    /// 0..100 thresholds, which rejected every candidate.
    #[test]
    fn the_score_is_on_a_zero_to_hundred_scale() {
        assert!((score("Mad Max", "Mad Max") - 100.0).abs() < 0.001);
        assert!(score("Mad Max", "Mad Max 2") > 50.0);
    }

    #[test]
    fn an_exact_title_scores_perfectly() {
        assert_eq!(score("Sekiro Shadows Die Twice", "Sekiro: Shadows Die Twice"), 100.0);
    }

    /// The real folder name on the user's drive has a doubled letter. The matcher has
    /// to bridge that, since the scanner deliberately preserves the typo.
    #[test]
    fn a_typo_still_matches_the_right_game() {
        let s = score("Ghost of Tsushimaa", "Ghost of Tsushima");
        assert!(s >= 70.0, "typo scored only {s}");
    }

    #[test]
    fn word_order_and_punctuation_do_not_break_matching() {
        assert!(score("The Witcher 3 Wild Hunt", "The Witcher 3: Wild Hunt") >= 95.0);
    }

    /// A wrong cover is worse than no cover, so unrelated titles must score low enough
    /// to be rejected by the 70-point floor.
    #[test]
    fn unrelated_titles_score_below_the_acceptance_floor() {
        for other in ["Cyberpunk 2077", "Stardew Valley", "Factorio"] {
            assert!(score("Mad Max", other) < 70.0, "{other} scored too high");
        }
    }

    #[test]
    fn cover_urls_point_at_the_reachable_steam_cdn() {
        let url = steam_cover_url(292030);
        assert!(url.contains("cdn.cloudflare.steamstatic.com"), "{url}");
        assert!(url.ends_with("library_600x900.jpg"));
    }

    #[test]
    fn release_timestamps_convert_to_years() {
        // 2015-05-18, The Witcher 3's release.
        assert_eq!(year_of(1_431_907_200), Some(2015));
    }

    #[test]
    fn incomplete_credentials_are_rejected_before_any_request() {
        let err = test_credentials(&Credentials::default()).unwrap_err();
        assert!(err.contains("required"), "{err}");
    }
}
