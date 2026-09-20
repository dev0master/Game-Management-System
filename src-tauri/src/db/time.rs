//! UTC timestamps, without pulling in a date library.
//!
//! Stored as ISO-8601 text so the catalogue stays readable in any SQLite browser,
//! which matters for a file the user owns and may want to inspect directly.

use std::time::{SystemTime, UNIX_EPOCH};

/// Current time as `YYYY-MM-DDTHH:MM:SSZ`.
pub fn now_utc() -> String {
    format_unix(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0),
    )
}

/// Format Unix seconds as ISO-8601 UTC.
pub fn format_unix(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let tod = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    let (h, mi, s) = (tod / 3600, (tod % 3600) / 60, tod % 60);
    format!("{y:04}-{m:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z")
}

/// Days since the Unix epoch to a civil date.
///
/// Howard Hinnant's `civil_from_days`, which is exact for the proleptic Gregorian
/// calendar over the whole range we care about.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_formats_correctly() {
        assert_eq!(format_unix(0), "1970-01-01T00:00:00Z");
    }

    #[test]
    fn known_instants_round_trip() {
        assert_eq!(format_unix(1_700_000_000), "2023-11-14T22:13:20Z");
        assert_eq!(format_unix(1_000_000_000), "2001-09-09T01:46:40Z");
    }

    /// Leap-year handling is the usual source of off-by-one date bugs.
    #[test]
    fn leap_day_is_handled() {
        // 2024-02-29T12:00:00Z
        assert_eq!(format_unix(1_709_208_000), "2024-02-29T12:00:00Z");
    }

    #[test]
    fn now_is_sortable_and_well_formed() {
        let s = now_utc();
        assert_eq!(s.len(), 20, "{s}");
        assert!(s.ends_with('Z'));
        // Lexical order equals chronological order, which the catalogue relies on.
        assert!(now_utc() >= s);
    }
}
