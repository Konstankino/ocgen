//! UTC wall-clock formatting without a date crate.

/// Seconds since the Unix epoch, now.
pub(crate) fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// `secs` since the epoch as UTC (year, month, day, hour, minute, second).
pub(crate) fn civil(secs: i64) -> (i64, i64, i64, i64, i64, i64) {
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Howard Hinnant's civil-from-days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d, rem / 3600, (rem % 3600) / 60, rem % 60)
}

/// Days since the epoch of a UTC date (Hinnant's days-from-civil, the inverse
/// of [`civil`]). No range check: see [`date_secs`].
pub(crate) fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Seconds since the epoch of a UTC date and time, or `None` for one that
/// doesn't exist (month 13, February 30, hour 24).
pub(crate) fn date_secs(y: i64, m: i64, d: i64, h: i64, mi: i64, s: i64) -> Option<i64> {
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) || h > 23 || mi > 59 || s > 60 {
        return None;
    }
    let days = days_from_civil(y, m, d);
    // A day past the month's end rolls over into the next month.
    let (cy, cm, cd, ..) = civil(days * 86_400);
    ((cy, cm, cd) == (y, m, d)).then_some(days * 86_400 + h * 3600 + mi * 60 + s.min(59))
}

/// Current UTC time as (year, month, day, hour, minute, second).
pub(crate) fn utc_now() -> (i64, i64, i64, i64, i64, i64) {
    civil(now_secs())
}

/// `YYYYMMDD-HHMMSS` (sorts chronologically; safe in file names).
pub(crate) fn compact_stamp() -> String {
    let (y, m, d, h, mi, s) = utc_now();
    format!("{y:04}{m:02}{d:02}-{h:02}{mi:02}{s:02}")
}

/// `secs` since the epoch as `YYYY-MM-DDTHH:MM:SSZ`.
pub(crate) fn iso(secs: i64) -> String {
    let (y, m, d, h, mi, s) = civil(secs);
    format!("{y:04}-{m:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z")
}

/// `YYYY-MM-DDTHH:MM:SSZ`, as `date -u +%Y-%m-%dT%H:%M:%SZ` prints it.
pub(crate) fn iso_stamp() -> String {
    iso(now_secs())
}
