//! RAWG metadata, and box art by way of Steam where it exists.
//!
//! RAWG replaced IGDB here for one practical reason: IGDB authenticates through Twitch,
//! which this user cannot reach, while RAWG needs nothing but a free API key on the query
//! string. RAWG also covers console titles that Steam has never heard of, which matters
//! for a library that is mostly PlayStation and Xbox.
//!
//! **Artwork.** RAWG's own `background_image` is a 16:9 key art, and the full-size file is
//! several megabytes — one was 6 MB in testing. Two consequences shape this module:
//!
//! * images are always fetched through RAWG's `/media/resize/<width>/-/` path, which
//!   returned the same picture at 117 KB;
//! * when RAWG reports the game on Steam, the cover is taken from Steam's CDN instead,
//!   because `library_600x900.jpg` is real portrait box art and the card grid is portrait.
//!   RAWG's landscape art is the fallback, not the first choice.
//!
//! Everything here is optional. With no key the app is exactly as useful, just without
//! artwork — nothing in scanning, browsing or copying waits on it.

use crate::platform::dpapi::Credentials;
use serde::Deserialize;
use std::sync::Mutex;
use std::time::{Duration, Instant};

const RAWG_URL: &str = "https://api.rawg.io/api";

/// RAWG's free tier allows 20,000 requests a month, with no published per-second limit.
/// A small gap keeps a library-wide run from looking like a flood.
const MIN_REQUEST_GAP: Duration = Duration::from_millis(220);

/// RAWG's store id for Steam, from `/api/stores`.
const STORE_STEAM: i64 = 1;

/// Candidates to score per title. RAWG orders by relevance, so the answer is near the top;
/// more than this only adds noise from sequels and bundles.
const CANDIDATES: usize = 8;

/// Below this the match is a guess, and a wrong cover is worse than none.
///
/// Set at 74 rather than 70 from measured scores. A bare franchise name against one of its
/// installments lands just over 70 — "Call of Duty" vs "Call of Duty: Black Ops" scores
/// 70.6, "Final Fantasy" vs "Final Fantasy VII Remake" 70.3 — and taking those attaches
/// whichever entry is most popular, which is a coin flip. Everything legitimate sits far
/// above: an exact title scores 100 and a numbered short form 88, so the band from 70 to 74
/// holds nothing worth having.
const MIN_SCORE: f64 = 74.0;

/// Steam's portrait box art. Reachable on this network, and the right shape for the grid.
fn steam_cover_url(appid: i64) -> String {
    format!("https://cdn.cloudflare.steamstatic.com/steam/apps/{appid}/library_600x900.jpg")
}

/// RAWG serves a resized copy from the same path with a size segment inserted.
///
/// `https://media.rawg.io/media/games/...jpg`
///   → `https://media.rawg.io/media/resize/640/-/games/...jpg`
///
/// Worth doing rather than skipping: the full-size original was 6 MB against 117 KB here,
/// and this runs once per game across a whole library.
fn resized(url: &str) -> String {
    match url.split_once("/media/") {
        Some((host, rest)) if !rest.starts_with("resize/") => {
            format!("{host}/media/resize/640/-/{rest}")
        }
        _ => url.to_string(),
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct GameMeta {
    pub rawg_id: Option<i64>,
    pub steam_appid: Option<i64>,
    pub name: String,
    pub year: Option<i32>,
    pub summary: Option<String>,
    pub genres: Vec<String>,
    pub rating: Option<f64>,
    pub match_score: f64,
    /// RAWG's own artwork, already rewritten to the resized path.
    pub image_url: Option<String>,
    /// True when RAWG lists the game on Steam, so portrait box art may exist.
    pub on_steam: bool,
}

/* ------------------------------------------------------------------ wire types */

#[derive(Debug, Deserialize)]
struct SearchResponse {
    #[serde(default)]
    results: Vec<SearchResult>,
}

#[derive(Debug, Deserialize)]
struct SearchResult {
    id: i64,
    name: String,
    #[serde(default)]
    released: Option<String>,
    #[serde(default)]
    background_image: Option<String>,
    #[serde(default)]
    rating: Option<f64>,
    /// How many RAWG users have this in a collection. The library's popularity signal,
    /// and the only thing that separates a famous game from a same-named unknown.
    #[serde(default)]
    added: Option<i64>,
    #[serde(default)]
    genres: Vec<NamedRef>,
    #[serde(default)]
    stores: Option<Vec<StoreRef>>,
}

#[derive(Debug, Deserialize)]
struct NamedRef {
    name: String,
}

#[derive(Debug, Deserialize)]
struct StoreRef {
    #[serde(default)]
    store: Option<StoreInner>,
}

#[derive(Debug, Deserialize)]
struct StoreInner {
    #[serde(default)]
    slug: String,
}

#[derive(Debug, Deserialize)]
struct StoreLinks {
    #[serde(default)]
    results: Vec<StoreLink>,
}

#[derive(Debug, Deserialize)]
struct StoreLink {
    #[serde(default)]
    store_id: i64,
    #[serde(default)]
    url: String,
}

/* ---------------------------------------------------------------------- client */

static LAST_REQUEST: Mutex<Option<Instant>> = Mutex::new(None);

/// Serialise outgoing requests so a library-wide run stays under the rate limit.
fn throttle() {
    let mut last = LAST_REQUEST.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(prev) = *last {
        let since = prev.elapsed();
        if since < MIN_REQUEST_GAP {
            std::thread::sleep(MIN_REQUEST_GAP - since);
        }
    }
    *last = Some(Instant::now());
}

fn client() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(20))
        .user_agent("GameVault/0.1")
        .build()
        .map_err(|e| e.to_string())
}

/* ----------------------------------------------------------------- matching */

/// Words of a title, lowercased, sorted and de-duplicated.
///
/// Sorting is what makes the comparison order-insensitive: release names reorder and pad
/// words freely, so "Witcher 3 Wild Hunt, The" and "The Witcher 3: Wild Hunt" score the
/// same. A plain edit distance over the raw strings would not.
fn token_key(s: &str) -> String {
    let mut tokens: Vec<&str> = s
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .collect();
    tokens.sort_unstable();
    tokens.dedup();
    tokens.join(" ").to_lowercase()
}

/// Words that appear in titles without identifying them.
const FILLER: &[&str] = &["the", "of", "a", "an", "and", "de", "la", "le", "el", "no"];

/// Awarded when the query is a numbered installment spelled short.
///
/// Comfortably over `MIN_SCORE` so the match is taken, comfortably under an exact match so
/// it never outranks one.
const CONTAINED_SCORE: f64 = 88.0;

/// Tokens that carry identity: lowercased, filler words dropped.
fn distinctive_tokens(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| t.to_lowercase())
        .filter(|t| !FILLER.contains(&t.as_str()))
        .collect()
}

/// Is the query a shortened form of the candidate, specific enough to be sure?
///
/// A folder is often named `Uncharted 4` for `Uncharted 4: A Thief's End`, and a plain
/// ratio scores that far too low because the candidate carries four extra words.
///
/// The guard is the number. `Uncharted 4` names one installment, so the subtitle it omits
/// cannot change which game it is. `Call of Duty` names a franchise of dozens, and
/// accepting it would attach whichever entry happened to be most popular — precisely the
/// wrong cover this module exists to avoid. So containment counts only when the query
/// carries a numeric token, and never for a bare series name.
fn is_shortened_form(query: &str, candidate: &str) -> bool {
    let q = distinctive_tokens(query);
    if q.len() < 2 || !q.iter().any(|t| t.chars().all(|c| c.is_ascii_digit())) {
        return false;
    }
    let c = distinctive_tokens(candidate);
    q.iter().all(|t| c.contains(t))
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
    let ratio = rapidfuzz::fuzz::ratio(a.chars(), b.chars()) * 100.0;

    if is_shortened_form(query_title, candidate) {
        return ratio.max(CONTAINED_SCORE);
    }
    ratio
}

fn year_of(released: Option<&str>) -> Option<i32> {
    released?.get(0..4)?.parse().ok()
}

/// Two candidates this close in name similarity are treated as equally good, and the
/// choice between them is made on popularity instead.
const TIE_WINDOW: f64 = 1.5;

/// RAWG disambiguates reboots by appending the year: `God of War (2018)`.
///
/// Scoring against the raw name would rank that below a same-named unknown, which is
/// exactly what happened: searching "God of War" matched a 2023 entry with two users
/// instead of Sony's game with fourteen thousand.
fn strip_disambiguator(name: &str) -> &str {
    let trimmed = name.trim_end();
    let Some(open) = trimmed.strip_suffix(')').and_then(|s| s.rfind('(')) else {
        return name;
    };
    let inside = &trimmed[open + 1..trimmed.len() - 1];
    if inside.len() == 4 && inside.chars().all(|c| c.is_ascii_digit()) {
        trimmed[..open].trim_end()
    } else {
        name
    }
}

/// Pick the best candidate, or none when nothing scores usefully.
///
/// Split out from the network call so the ranking can be tested against captured
/// responses without reaching RAWG at all.
fn best_match(title: &str, year_hint: Option<i32>, results: &[SearchResult]) -> Option<GameMeta> {
    let mut best: Option<(f64, i64, &SearchResult)> = None;

    for r in results.iter().take(CANDIDATES) {
        let mut s = score(title, strip_disambiguator(&r.name));
        // A matching release year breaks ties between a game and its remaster without
        // ever being enough to rescue a bad name match on its own.
        if let (Some(want), Some(got)) = (year_hint, year_of(r.released.as_deref())) {
            if want == got {
                s += 6.0;
            }
        }
        let popularity = r.added.unwrap_or(0);

        // Name similarity decides; popularity only separates candidates the name cannot.
        // Without this, any exact-named entry wins however obscure it is.
        let better = match &best {
            None => true,
            Some((bs, bp, _)) => {
                if (s - *bs).abs() <= TIE_WINDOW {
                    popularity > *bp
                } else {
                    s > *bs
                }
            }
        };
        if better {
            best = Some((s, popularity, r));
        }
    }

    let (s, _, r) = best?;
    if s < MIN_SCORE {
        return None;
    }

    let on_steam = r
        .stores
        .as_deref()
        .unwrap_or_default()
        .iter()
        .filter_map(|x| x.store.as_ref())
        .any(|x| x.slug == "steam");

    Some(GameMeta {
        rawg_id: Some(r.id),
        steam_appid: None,
        name: r.name.clone(),
        year: year_of(r.released.as_deref()),
        summary: None,
        genres: r.genres.iter().map(|g| g.name.clone()).collect(),
        rating: r.rating,
        match_score: s.min(100.0),
        image_url: r.background_image.as_deref().map(resized),
        on_steam,
    })
}

/// Pull the Steam app id out of a store URL such as
/// `http://store.steampowered.com/app/412020/`.
fn steam_appid_from_url(url: &str) -> Option<i64> {
    let rest = url.split("/app/").nth(1)?;
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

/* ------------------------------------------------------------------- requests */

fn api_key(creds: &Credentials) -> Result<&str, String> {
    let key = creds.rawg_api_key.trim();
    if key.is_empty() {
        return Err("no RAWG API key saved".into());
    }
    Ok(key)
}

/// Verify a key without performing a lookup. Used by the settings screen.
pub fn test_credentials(creds: &Credentials) -> Result<(), String> {
    let key = api_key(creds)?;
    throttle();
    let resp = client()?
        .get(format!("{RAWG_URL}/games"))
        .query(&[("key", key), ("page_size", "1")])
        .send()
        .map_err(|e| format!("could not reach RAWG: {e}"))?;

    match resp.status().as_u16() {
        200 => Ok(()),
        401 | 403 => Err("RAWG rejected this API key".into()),
        429 => Err("RAWG rate limit reached — try again shortly".into()),
        other => Err(format!("RAWG returned {other}")),
    }
}

/// Look up one title. Returns the best match, or `None` when nothing scores usefully.
pub fn lookup(
    creds: &Credentials,
    clean_title: &str,
    year_hint: Option<i32>,
) -> Result<Option<GameMeta>, String> {
    let key = api_key(creds)?;
    if crate::scan::title::make_key(clean_title).is_empty() {
        return Ok(None);
    }

    throttle();
    let resp = client()?
        .get(format!("{RAWG_URL}/games"))
        .query(&[
            ("key", key),
            ("search", clean_title),
            ("page_size", &CANDIDATES.to_string()),
            ("search_precise", "true"),
        ])
        .send()
        .map_err(|e| format!("could not reach RAWG: {e}"))?;

    if resp.status() == 429 {
        return Err("RAWG rate limit reached".into());
    }
    if !resp.status().is_success() {
        return Err(format!("RAWG returned {}", resp.status()));
    }
    let body: SearchResponse = resp.json().map_err(|e| format!("unreadable reply: {e}"))?;

    let Some(mut meta) = best_match(clean_title, year_hint, &body.results) else {
        return Ok(None);
    };

    // Only ask for store links when RAWG already said the game is on Steam; that keeps
    // the extra request off every console-only title.
    if meta.on_steam {
        if let Some(id) = meta.rawg_id {
            meta.steam_appid = steam_links(key, id).unwrap_or(None);
        }
    }
    Ok(Some(meta))
}

/// The Steam app id for a RAWG game, when it has one.
///
/// A failure here is not a failure of the lookup: it only means the cover falls back to
/// RAWG's own artwork.
fn steam_links(key: &str, rawg_id: i64) -> Result<Option<i64>, String> {
    throttle();
    let resp = client()?
        .get(format!("{RAWG_URL}/games/{rawg_id}/stores"))
        .query(&[("key", key)])
        .send()
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Ok(None);
    }
    let links: StoreLinks = resp.json().map_err(|e| e.to_string())?;
    Ok(links
        .results
        .iter()
        .find(|l| l.store_id == STORE_STEAM)
        .and_then(|l| steam_appid_from_url(&l.url)))
}

/* --------------------------------------------------------------------- covers */

/// Fetch the best artwork available for a match, into the local cover cache.
///
/// Steam's portrait box art first, RAWG's landscape art second. Returns the path written,
/// or `None` when the title simply has no usable artwork — which is an ordinary outcome,
/// not an error.
pub fn fetch_cover(meta: &GameMeta, cache_dir: &std::path::Path) -> Option<std::path::PathBuf> {
    if let Some(appid) = meta.steam_appid {
        let key = format!("steam{appid}");
        if let Ok(p) = download_image(&steam_cover_url(appid), cache_dir, &key) {
            return Some(p);
        }
    }
    let url = meta.image_url.as_ref()?;
    let key = format!("rawg{}", meta.rawg_id.unwrap_or(0));
    download_image(url, cache_dir, &key).ok()
}

/// Download an image to the local cover cache and return its path.
///
/// Covers live under `%LOCALAPPDATA%`, never on an external drive: they are derived data
/// that must not consume space on the drives the user is trying to manage.
pub fn download_image(
    url: &str,
    cache_dir: &std::path::Path,
    key: &str,
) -> Result<std::path::PathBuf, String> {
    std::fs::create_dir_all(cache_dir).map_err(|e| e.to_string())?;
    let dest = cache_dir.join(format!("{key}.jpg"));
    if dest.exists() {
        return Ok(dest);
    }

    throttle();
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

    /// Trimmed from a real reply to `GET /api/games?search=Metro Exodus`.
    const METRO: &str = r#"{
      "count": 1446,
      "results": [
        {
          "id": 28201, "name": "Metro Exodus", "released": "2019-02-13",
          "background_image": "https://media.rawg.io/media/games/152/152e788b.jpg",
          "rating": 4.36,
          "genres": [{"name": "Action"}, {"name": "Shooter"}],
          "stores": [{"store": {"slug": "steam"}}, {"store": {"slug": "playstation-store"}}]
        },
        {
          "id": 613493, "name": "Metro Exodus: Enhanced Edition", "released": "2021-05-06",
          "background_image": "https://media.rawg.io/media/games/389/389a26a0.jpg",
          "rating": 4.1, "genres": [{"name": "Action"}],
          "stores": [{"store": {"slug": "steam"}}]
        },
        {
          "id": 378575, "name": "Metro Exodus: Sam's Story", "released": "2020-02-11",
          "background_image": "https://media.rawg.io/media/games/c3f/c3f1dd09.jpeg",
          "rating": 3.5, "genres": [], "stores": []
        }
      ]
    }"#;

    fn results(json: &str) -> Vec<SearchResult> {
        serde_json::from_str::<SearchResponse>(json).expect("parses").results
    }

    #[test]
    fn the_exact_title_wins_over_its_editions_and_dlc() {
        let m = best_match("Metro Exodus", None, &results(METRO)).expect("a match");
        assert_eq!(m.name, "Metro Exodus");
        assert_eq!(m.rawg_id, Some(28201));
        assert_eq!(m.year, Some(2019));
        assert_eq!(m.match_score, 100.0, "an exact key match is certain");
        assert_eq!(m.genres, vec!["Action", "Shooter"]);
        assert!(m.on_steam, "RAWG listed a Steam store entry");
    }

    /// The DLC shares most of its words with the base game; asking for it must not return
    /// the base game instead.
    #[test]
    fn a_dlc_title_matches_the_dlc() {
        let m = best_match("Metro Exodus Sam's Story", None, &results(METRO)).expect("a match");
        assert_eq!(m.rawg_id, Some(378575));
        assert!(!m.on_steam, "that entry lists no stores");
    }

    /// The year is a tie-breaker, never a rescue.
    #[test]
    fn the_year_hint_separates_a_remaster_from_the_original() {
        let original = best_match("Metro Exodus", Some(2019), &results(METRO)).unwrap();
        assert_eq!(original.rawg_id, Some(28201));
        let enhanced = best_match("Metro Exodus Enhanced Edition", Some(2021), &results(METRO)).unwrap();
        assert_eq!(enhanced.rawg_id, Some(613493));
    }

    /// Trimmed from a real reply to `GET /api/games?search=God of War`, ordered as RAWG
    /// returned it. The obscure 2023 entry really does come back before Sony's game.
    const GOW: &str = r#"{
      "results": [
        {"id": 58175, "name": "God of War I", "released": "2005-03-22", "rating": 4.36, "added": 1995,
         "background_image": "https://media.rawg.io/media/games/a/a1.jpg"},
        {"id": 58134, "name": "God of War (2018)", "released": "2018-04-20", "rating": 4.54, "added": 14682,
         "background_image": "https://media.rawg.io/media/games/b/b2.jpg",
         "stores": [{"store": {"slug": "steam"}}]},
        {"id": 930477, "name": "God Of War", "released": "2023-02-17", "rating": 0.0, "added": 2,
         "background_image": "https://media.rawg.io/media/games/c/c3.jpg"},
        {"id": 58617, "name": "God of War: Ascension", "released": "2013-03-12", "rating": 3.85, "added": 1751,
         "background_image": "https://media.rawg.io/media/games/d/d4.jpg"},
        {"id": 58812, "name": "God of War II", "released": "2007-03-13", "rating": 4.39, "added": 2822,
         "background_image": "https://media.rawg.io/media/games/e/e5.jpg"}
      ]
    }"#;

    /// A same-named unknown must not beat a famous game.
    ///
    /// This is a real failure, not a hypothetical: "God of War" first matched a 2023 entry
    /// with two users and no rating, because its name matched character for character while
    /// Sony's was listed as "God of War (2018)".
    #[test]
    fn a_popular_game_beats_an_obscure_one_with_the_same_name() {
        let m = best_match("God of War", None, &results(GOW)).expect("a match");
        assert_eq!(m.rawg_id, Some(58134), "picked {:?} instead of God of War (2018)", m.name);
        assert_eq!(m.year, Some(2018));
        assert!(m.on_steam);
    }

    /// Popularity is only a tie-breaker: a better name match still wins outright.
    #[test]
    fn popularity_never_overrides_a_clearly_better_name_match() {
        let m = best_match("God of War Ascension", None, &results(GOW)).expect("a match");
        assert_eq!(m.rawg_id, Some(58617), "picked {:?}", m.name);
        // Ascension has far fewer users than God of War (2018), and still wins on name.
        let m2 = best_match("God of War II", None, &results(GOW)).expect("a match");
        assert_eq!(m2.rawg_id, Some(58812), "picked {:?}", m2.name);
    }

    #[test]
    fn a_trailing_year_in_parentheses_is_not_part_of_the_title() {
        assert_eq!(strip_disambiguator("God of War (2018)"), "God of War");
        assert_eq!(strip_disambiguator("DOOM (2016)"), "DOOM");
        // Not a year, so it is part of the name and stays.
        assert_eq!(strip_disambiguator("Rock Band (Track Pack)"), "Rock Band (Track Pack)");
        assert_eq!(strip_disambiguator("Portal 2"), "Portal 2");
        assert_eq!(strip_disambiguator("(2018)"), "");
    }

    /// A wrong cover is worse than none, so a weak match is refused outright.
    #[test]
    fn an_unrelated_title_matches_nothing() {
        assert!(best_match("Pro Evolution Soccer", None, &results(METRO)).is_none());
        assert!(best_match("", None, &results(METRO)).is_none());
    }

    #[test]
    fn an_empty_reply_is_not_an_error() {
        assert!(best_match("Anything", None, &results(r#"{"results":[]}"#)).is_none());
        // RAWG omits fields freely; none of that may fail the parse.
        let sparse = results(r#"{"results":[{"id":1,"name":"Doom"}]}"#);
        let m = best_match("Doom", None, &sparse).unwrap();
        assert_eq!(m.year, None);
        assert_eq!(m.image_url, None);
        assert!(!m.on_steam);
    }

    /// The whole point of the resize path: the original was 6 MB, the resized copy 117 KB.
    #[test]
    fn image_urls_are_rewritten_to_the_resized_path() {
        assert_eq!(
            resized("https://media.rawg.io/media/games/152/152e788b.jpg"),
            "https://media.rawg.io/media/resize/640/-/games/152/152e788b.jpg"
        );
        // Already resized, or an unexpected shape: left exactly as it is.
        let already = "https://media.rawg.io/media/resize/640/-/games/152/x.jpg";
        assert_eq!(resized(already), already);
        assert_eq!(resized("https://example.com/x.jpg"), "https://example.com/x.jpg");
    }

    #[test]
    fn the_steam_appid_is_read_out_of_the_store_url() {
        assert_eq!(
            steam_appid_from_url("http://store.steampowered.com/app/412020/"),
            Some(412020)
        );
        assert_eq!(
            steam_appid_from_url("https://store.steampowered.com/app/292030?snr=1"),
            Some(292030)
        );
        assert_eq!(steam_appid_from_url("https://store.playstation.com/x"), None);
        assert_eq!(steam_appid_from_url("http://store.steampowered.com/app/"), None);
    }

    #[test]
    fn only_the_steam_store_link_is_used() {
        let links: StoreLinks = serde_json::from_str(
            r#"{"results":[
                 {"store_id":3,"url":"https://store.playstation.com/en-us/product/UP2047-CUSA11408_00-METROEXODUS00000"},
                 {"store_id":1,"url":"http://store.steampowered.com/app/412020/"},
                 {"store_id":5,"url":"http://www.gog.com/game/metro_exodus"}
               ]}"#,
        )
        .unwrap();
        let appid = links
            .results
            .iter()
            .find(|l| l.store_id == STORE_STEAM)
            .and_then(|l| steam_appid_from_url(&l.url));
        assert_eq!(appid, Some(412020));
    }

    #[test]
    fn a_missing_key_is_refused_before_any_request() {
        let empty = Credentials::default();
        assert!(lookup(&empty, "Doom", None).is_err());
        assert!(test_credentials(&empty).is_err());
    }

    /// Title matching folds articles and punctuation, so these must not be near-misses.
    #[test]
    fn scoring_is_insensitive_to_word_order_and_punctuation() {
        assert_eq!(score("The Witcher 3: Wild Hunt", "Witcher 3 Wild Hunt, The"), 100.0);
        assert!(score("Metro Exodus", "Metro Exodus") >= 100.0);
        assert!(score("Metro Exodus", "Pro Evolution Soccer") < MIN_SCORE);
    }

    /// A folder named `Uncharted 4` is the game whose full title adds a subtitle. A plain
    /// ratio scores that below the threshold and loses the cover.
    #[test]
    fn a_numbered_installment_matches_its_full_title() {
        assert!(
            score("Uncharted 4", "Uncharted 4: A Thief's End") >= MIN_SCORE,
            "scored {}",
            score("Uncharted 4", "Uncharted 4: A Thief's End")
        );
        assert!(score("Halo 3", "Halo 3: ODST") >= MIN_SCORE);
        assert!(score("Metro Exodus 2", "Metro Exodus 2: Something") >= MIN_SCORE);
    }

    /// The guard on that rule: a bare franchise name names dozens of games, and picking
    /// one of them would attach a wrong cover.
    #[test]
    fn a_bare_series_name_is_still_refused() {
        assert!(score("Call of Duty", "Call of Duty: Black Ops") < MIN_SCORE);
        assert!(score("Uncharted", "Uncharted 4: A Thief's End") < MIN_SCORE);
        assert!(score("Final Fantasy", "Final Fantasy VII Remake") < MIN_SCORE);
        // A number alone is not a title either.
        assert!(!is_shortened_form("4", "Uncharted 4: A Thief's End"));
    }

    /// The short form must not outrank the exact title when both are on offer.
    #[test]
    fn an_exact_title_still_beats_its_shortened_form() {
        let r = results(
            r#"{"results":[
                 {"id":1,"name":"Halo 3: ODST","added":5000},
                 {"id":2,"name":"Halo 3","added":40}
               ]}"#,
        );
        let m = best_match("Halo 3", None, &r).expect("a match");
        assert_eq!(m.rawg_id, Some(2), "picked {:?}, but the exact title exists", m.name);
    }

    #[test]
    fn filler_words_do_not_count_towards_being_specific() {
        assert_eq!(distinctive_tokens("The Last of Us"), vec!["last", "us"]);
        assert_eq!(distinctive_tokens("God of War 3"), vec!["god", "war", "3"]);
    }
}
