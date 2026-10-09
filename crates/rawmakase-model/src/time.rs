//! Calendar dates from Unix time, without a date library.

/// `[year, month, day, hour, minute, second]` in UTC of `seconds` since 1970.
pub fn utc(seconds: i64) -> [i64; 6] {
    let days = seconds.div_euclid(86_400);
    let rest = seconds.rem_euclid(86_400);
    // Civil-from-days (Howard Hinnant).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    [year, month, day, rest / 3600, rest % 3600 / 60, rest % 60]
}

/// Seconds since 1970 now.
fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// Now in UTC as XMP writes it, e.g. "2026-09-27T06:12:22Z".
pub fn now_xmp() -> String {
    let [y, mo, d, h, mi, s] = utc(now());
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z")
}

/// `seconds` since 1970 in UTC as catalogs store times, e.g.
/// "2026-09-27 06:12:22" (SQL's `CURRENT_TIMESTAMP` form).
pub fn utc_text(seconds: i64) -> String {
    let [y, mo, d, h, mi, s] = utc(seconds);
    // Years before 1 as SQL writes them: "-0001".
    let sign = if y < 0 { "-" } else { "" };
    let y = y.abs();
    format!("{sign}{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02}")
}

/// Now in UTC as catalogs store times (see [`utc_text`]).
pub fn now_text() -> String {
    utc_text(now())
}

#[cfg(test)]
mod tests {
    #[test]
    fn converts_unix_seconds_to_utc() {
        assert_eq!(super::utc(1_469_686_444), [2016, 7, 28, 6, 14, 4]);
        assert_eq!(super::utc(0), [1970, 1, 1, 0, 0, 0]);
        let now = super::now_xmp();
        assert!(now.len() == 20 && now.ends_with('Z'));
        assert_eq!(super::utc_text(1_469_686_444), "2016-07-28 06:14:04");
        assert_eq!(super::now_text().len(), 19);
    }
}
