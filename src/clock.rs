//! UTC wall-clock formatting without a date crate.

/// Current UTC time as (year, month, day, hour, minute, second).
pub(crate) fn utc_now() -> (i64, i64, i64, i64, i64, i64) {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
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

/// `YYYYMMDD-HHMMSS` (sorts chronologically; safe in file names).
pub(crate) fn compact_stamp() -> String {
    let (y, m, d, h, mi, s) = utc_now();
    format!("{y:04}{m:02}{d:02}-{h:02}{mi:02}{s:02}")
}

/// `YYYY-MM-DDTHH:MM:SSZ`, as `date -u +%Y-%m-%dT%H:%M:%SZ` prints it.
pub(crate) fn iso_stamp() -> String {
    let (y, m, d, h, mi, s) = utc_now();
    format!("{y:04}-{m:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z")
}
