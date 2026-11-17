//! The calendar smart collections' relative date rules are relative to:
//! today's local date, the local date of a time stored in UTC, and the
//! first day of the week of the user's region (macOS and Windows; elsewhere
//! Sunday, as Lightroom measured on an en-US system, since reading the
//! environment's locale would need the process-wide `setlocale`).
use crate::collection_rules::{Clock, Date};

/// Today and the week's first day, as rules see them now.
pub(crate) fn clock() -> Clock {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX));
    Clock {
        today: local_date(now).unwrap_or_else(|| utc_date(now)),
        week_start: first_weekday().unwrap_or(0),
    }
}

/// The UTC date of `seconds` since 1970.
fn utc_date(seconds: i64) -> Date {
    let [y, m, d, ..] = crate::time::utc(seconds);
    Date::from_ymd(y, m, d)
}

/// The local date of `seconds` since 1970 in UTC, with the offset (daylight
/// saving time included) the system's time zone had then.
#[cfg(unix)]
pub(crate) fn local_date(seconds: i64) -> Option<Date> {
    let time = libc::time_t::try_from(seconds).ok()?;
    // SAFETY: `tm` is plain data that `localtime_r` fills in; both pointers
    // are valid for the call, and the reentrant form keeps no shared state.
    let tm = unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&time, &mut tm).is_null() {
            return None;
        }
        tm
    };
    Some(Date::from_ymd(
        i64::from(tm.tm_year) + 1900,
        i64::from(tm.tm_mon) + 1,
        i64::from(tm.tm_mday),
    ))
}

#[cfg(windows)]
pub(crate) fn local_date(seconds: i64) -> Option<Date> {
    use windows_sys::Win32::Foundation::SYSTEMTIME;
    use windows_sys::Win32::System::Time::SystemTimeToTzSpecificLocalTime;
    let [y, m, d, h, mi, s] = crate::time::utc(seconds);
    let field = |v: i64| u16::try_from(v).ok();
    let utc = SYSTEMTIME {
        wYear: field(y)?,
        wMonth: field(m)?,
        wDayOfWeek: 0,
        wDay: field(d)?,
        wHour: field(h)?,
        wMinute: field(mi)?,
        wSecond: field(s)?,
        wMilliseconds: 0,
    };
    let mut local = utc;
    // SAFETY: a null time zone is the current one; both pointers are valid
    // SYSTEMTIMEs for the call.
    let ok = unsafe { SystemTimeToTzSpecificLocalTime(std::ptr::null(), &utc, &mut local) };
    (ok != 0).then(|| {
        Date::from_ymd(
            i64::from(local.wYear),
            i64::from(local.wMonth),
            i64::from(local.wDay),
        )
    })
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn local_date(_seconds: i64) -> Option<Date> {
    None
}

/// The first day of the week, 0 for Sunday … 6 for Saturday.
#[cfg(target_os = "macos")]
fn first_weekday() -> Option<i64> {
    // NSCalendar counts 1 for Sunday … 7 for Saturday.
    let first = objc2_foundation::NSCalendar::currentCalendar().firstWeekday();
    i64::try_from(first)
        .ok()?
        .checked_sub(1)
        .filter(|d| (0..7).contains(d))
}

#[cfg(windows)]
fn first_weekday() -> Option<i64> {
    use windows_sys::Win32::Globalization::{GetLocaleInfoEx, LOCALE_IFIRSTDAYOFWEEK};
    let mut buffer = [0u16; 4];
    // SAFETY: a null name is the user's default locale; the buffer's length
    // is given in characters, as the call counts them.
    let written = unsafe {
        GetLocaleInfoEx(
            std::ptr::null(),
            LOCALE_IFIRSTDAYOFWEEK,
            buffer.as_mut_ptr(),
            buffer.len() as i32,
        )
    };
    let length = usize::try_from(written).ok()?.checked_sub(1)?;
    let text = String::from_utf16(buffer.get(..length)?).ok()?;
    // Windows counts 0 for Monday … 6 for Sunday.
    let monday_based: i64 = text.trim().parse().ok()?;
    (0..7)
        .contains(&monday_based)
        .then_some((monday_based + 1) % 7)
}

#[cfg(not(any(target_os = "macos", windows)))]
fn first_weekday() -> Option<i64> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_clock_is_a_real_date_within_a_day_of_utc() {
        let clock = clock();
        assert!((0..7).contains(&clock.week_start));
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        assert!((clock.today.0 - utc_date(now).0).abs() <= 1);
        // Noon UTC is the same or a neighbouring day anywhere.
        let noon = 1_763_380_800; // 2025-11-17T12:00:00Z
        let local = local_date(noon).unwrap_or_else(|| utc_date(noon));
        assert!((local.0 - Date::from_ymd(2025, 11, 17).0).abs() <= 1);
    }
}
