//! ISO 8601 timestamps of transcript entries, in milliseconds since the
//! Unix epoch.

/// Parses `YYYY-MM-DD`, optionally followed by `T` or a space and
/// `HH:MM[:SS[.fraction]]`, and an optional `Z` or `±HH:MM` offset. A
/// timestamp without an offset is read as UTC.
pub(crate) fn parse_iso_millis(text: &str) -> Option<i64> {
    let bytes = text.as_bytes();
    let year = digits(bytes, 0, 4)?;
    expect(bytes, 4, b'-')?;
    let month = digits(bytes, 5, 2)?;
    expect(bytes, 7, b'-')?;
    let day = digits(bytes, 8, 2)?;
    if !(1..=12).contains(&month) || day < 1 || day > days_in_month(year, month) {
        return None;
    }
    let mut millis = days_from_civil(year, month, day) * 86_400_000;
    if bytes.len() == 10 {
        return Some(millis);
    }
    if !matches!(bytes.get(10), Some(b'T' | b't' | b' ')) {
        return None;
    }
    let hour = digits(bytes, 11, 2)?;
    expect(bytes, 13, b':')?;
    let minute = digits(bytes, 14, 2)?;
    let mut position = 16;
    let mut second = 0;
    let mut fraction_millis = 0;
    if bytes.get(position) == Some(&b':') {
        second = digits(bytes, position + 1, 2)?;
        position += 3;
        if matches!(bytes.get(position), Some(b'.' | b',')) {
            position += 1;
            let start = position;
            while bytes.get(position).is_some_and(u8::is_ascii_digit) {
                position += 1;
            }
            if position == start {
                return None;
            }
            let fraction = &text[start..position.min(start + 3)];
            fraction_millis =
                fraction.parse::<i64>().ok()? * 10_i64.pow(3 - u32::try_from(fraction.len()).ok()?);
        }
    }
    if hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    millis += ((hour * 60 + minute) * 60 + second) * 1000 + fraction_millis;
    let offset_minutes = match &bytes[position..] {
        [] | [b'Z' | b'z'] => 0,
        [sign @ (b'+' | b'-'), ..] => {
            let hours = digits(bytes, position + 1, 2)?;
            let minutes = match bytes.len() - position {
                3 => 0,
                6 if bytes[position + 3] == b':' => digits(bytes, position + 4, 2)?,
                5 => digits(bytes, position + 3, 2)?,
                _ => return None,
            };
            let offset = hours * 60 + minutes;
            if *sign == b'+' { offset } else { -offset }
        }
        _ => return None,
    };
    Some(millis - offset_minutes * 60_000)
}

fn digits(bytes: &[u8], start: usize, count: usize) -> Option<i64> {
    let slice = bytes.get(start..start + count)?;
    if !slice.iter().all(u8::is_ascii_digit) {
        return None;
    }
    Some(
        slice
            .iter()
            .fold(0, |value, digit| value * 10 + i64::from(digit - b'0')),
    )
}

fn expect(bytes: &[u8], position: usize, byte: u8) -> Option<()> {
    (bytes.get(position) == Some(&byte)).then_some(())
}

fn is_leap(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        2 if is_leap(year) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Days from 1970-01-01 to a date of the proleptic Gregorian calendar.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month_index = (month + 9) % 12;
    let day_of_year = (153 * month_index + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// The current time in milliseconds since the Unix epoch.
pub(crate) fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
        })
}

/// A UTC time in milliseconds since the Unix epoch, formatted like the
/// JavaScript `Date.prototype.toISOString`.
pub(crate) fn format_iso_millis(millis: i64) -> String {
    let days = millis.div_euclid(86_400_000);
    let in_day = millis.rem_euclid(86_400_000);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        in_day / 3_600_000,
        in_day / 60_000 % 60,
        in_day / 1000 % 60,
        in_day % 1000
    )
}

fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let days = days + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// The current time, formatted like [`format_iso_millis`].
pub(crate) fn now_iso() -> String {
    format_iso_millis(now_millis())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_formats_the_cli_writes() {
        assert_eq!(
            parse_iso_millis("2026-01-15T10:30:00Z"),
            Some(1_768_473_000_000)
        );
        assert_eq!(
            parse_iso_millis("2026-01-15T10:30:00+00:00"),
            Some(1_768_473_000_000)
        );
        assert_eq!(
            parse_iso_millis("2026-01-15T10:30:00.123Z"),
            Some(1_768_473_000_123)
        );
        assert_eq!(
            parse_iso_millis("2026-01-15T12:30:00+02:00"),
            Some(1_768_473_000_000)
        );
        assert_eq!(parse_iso_millis("1970-01-01"), Some(0));
        assert_eq!(
            parse_iso_millis("2024-02-29T00:00:00.5Z"),
            Some(1_709_164_800_500)
        );
    }

    #[test]
    fn rejects_malformed_timestamps() {
        for text in [
            "not-a-date",
            "2026-13-01",
            "2026-02-30",
            "2026-01-15T25:00:00Z",
            "2026-01-15T10:30:00Q",
            "",
        ] {
            assert_eq!(parse_iso_millis(text), None, "{text}");
        }
    }

    #[test]
    fn formats_like_javascript() {
        assert_eq!(
            format_iso_millis(1_768_473_000_123),
            "2026-01-15T10:30:00.123Z"
        );
        assert_eq!(format_iso_millis(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(
            parse_iso_millis(&format_iso_millis(1_709_164_800_500)),
            Some(1_709_164_800_500)
        );
    }
}
