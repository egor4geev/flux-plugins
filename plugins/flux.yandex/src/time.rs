//! Time: now as seconds since 1970 (UTC), the user's local
//! time through `system.utc-offset` (summer time included), Tracker's timestamps
//! (`2026-10-14T10:00:00.000+0300`) both ways, and how times read in the interface: "10:05",
//! "1:05" of a timer, "today, 10:05", "14 Oct" — in English or Russian.

use flux_plugin_api::bindings::flux::plugin::system;

/// Seconds since 1970-01-01 UTC.
pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or(0)
}

/// How far the user's local time is ahead of UTC at `at`, in seconds (Flux asks the system).
pub fn offset_at(at: i64) -> i32 {
    system::utc_offset(at)
}

/// A moment as a calendar and a clock show it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Civil {
    pub year: i64,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
}

/// Days since 1970-01-01 of a date of the proleptic Gregorian calendar (H. Hinnant's algorithm).
pub fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month = i64::from(month);
    let shifted = if month > 2 { month - 3 } else { month + 9 };
    let day_of_year = (153 * shifted + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// The date of a day since 1970-01-01.
pub fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let mp = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// The moment `at` in a time zone `offset` seconds ahead of UTC.
pub fn civil(at: i64, offset: i32) -> Civil {
    let local = at + i64::from(offset);
    let days = local.div_euclid(86_400);
    let seconds = local.rem_euclid(86_400) as u32;
    let (year, month, day) = civil_from_days(days);
    Civil {
        year,
        month,
        day,
        hour: seconds / 3600,
        minute: seconds / 60 % 60,
        second: seconds % 60,
    }
}

/// A timestamp as Tracker takes it: `2026-10-14T10:00:00.000+0300`.
pub fn iso_with_offset(at: i64, offset: i32) -> String {
    let time = civil(at, offset);
    let sign = if offset < 0 { '-' } else { '+' };
    let offset = offset.unsigned_abs();
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.000{sign}{:02}{:02}",
        time.year,
        time.month,
        time.day,
        time.hour,
        time.minute,
        time.second,
        offset / 3600,
        offset / 60 % 60
    )
}

/// Seconds since 1970 of an ISO 8601 timestamp: `2026-10-14T10:00:00.000+0000`, `…Z`,
/// `…+03:00`, without fractions or seconds; none when it isn't one.
pub fn parse_iso(text: &str) -> Option<i64> {
    let text = text.trim();
    let (date, rest) = text.split_once('T')?;
    let mut parts = date.split('-');
    let year: i64 = parts.next()?.parse().ok()?;
    let month: u32 = parts.next()?.parse().ok()?;
    let day: u32 = parts.next()?.parse().ok()?;
    if parts.next().is_some() || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let zone_at = rest.find(['Z', 'z', '+', '-']).unwrap_or(rest.len());
    let (clock, zone) = rest.split_at(zone_at);
    let clock = clock.split('.').next()?;
    let mut fields = clock.split(':');
    let hour: i64 = fields.next()?.parse().ok()?;
    let minute: i64 = fields.next()?.parse().ok()?;
    let second: i64 = match fields.next() {
        Some(second) => second.parse().ok()?,
        None => 0,
    };
    let offset = match zone {
        "" | "Z" | "z" => 0,
        zone => {
            let sign = if zone.starts_with('-') { -1 } else { 1 };
            let digits: String = zone[1..].chars().filter(char::is_ascii_digit).collect();
            if digits.len() != 4 {
                return None;
            }
            let hours: i64 = digits[..2].parse().ok()?;
            let minutes: i64 = digits[2..].parse().ok()?;
            sign * (hours * 3600 + minutes * 60)
        }
    };
    Some(days_from_civil(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second - offset)
}

/// A date `2026-10-20` (a deadline); none when it isn't one.
pub fn parse_date(text: &str) -> Option<(i64, u32, u32)> {
    let mut parts = text.trim().get(..10)?.split('-');
    let year = parts.next()?.parse().ok()?;
    let month = parts.next()?.parse().ok()?;
    let day = parts.next()?.parse().ok()?;
    ((1..=12).contains(&month) && (1..=31).contains(&day)).then_some((year, month, day))
}

/// A timer's reading: hours and minutes, "1:05".
pub fn hours_minutes(seconds: i64) -> String {
    let minutes = seconds.max(0) / 60;
    format!("{}:{:02}", minutes / 60, minutes % 60)
}

fn month_name(month: u32, language: &str) -> &'static str {
    const EN: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    // The genitive, as a date reads: "20 окт", "3 мая".
    const RU: [&str; 12] = [
        "янв", "фев", "мар", "апр", "мая", "июн", "июл", "авг", "сен", "окт", "ноя", "дек",
    ];
    let names = if language == "ru" { &RU } else { &EN };
    names[(month.clamp(1, 12) - 1) as usize]
}

/// "20 Oct" ("20 окт"); with the year when it isn't `this_year`.
pub fn day_month(year: i64, month: u32, day: u32, this_year: i64, language: &str) -> String {
    let name = month_name(month, language);
    if year == this_year {
        format!("{day} {name}")
    } else {
        format!("{day} {name} {year}")
    }
}

/// When something happened, for a comment: "today, 10:05", "yesterday, 18:40", "14 Oct, 10:05".
pub fn moment(at: i64, now: i64, offset: i32, language: &str) -> String {
    let time = civil(at, offset);
    let today = civil(now, offset);
    let day = days_from_civil(time.year, time.month, time.day);
    let this_day = days_from_civil(today.year, today.month, today.day);
    let clock = format!("{}:{:02}", time.hour, time.minute);
    let (today_word, yesterday_word) = if language == "ru" {
        ("сегодня", "вчера")
    } else {
        ("today", "yesterday")
    };
    match this_day - day {
        0 => format!("{today_word}, {clock}"),
        1 => format!("{yesterday_word}, {clock}"),
        _ => format!(
            "{}, {clock}",
            day_month(time.year, time.month, time.day, today.year, language)
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calendar_days() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2000, 3, 1), 11_017);
        assert_eq!(civil_from_days(11_017), (2000, 3, 1));
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
        for days in [-800_000, -1, 0, 59, 365, 10_957, 20_740, 2_000_000] {
            let (year, month, day) = civil_from_days(days);
            assert_eq!(days_from_civil(year, month, day), days);
        }
    }

    #[test]
    fn tracker_timestamps() {
        // 2026-10-14 07:00:00 UTC.
        let at = days_from_civil(2026, 10, 14) * 86_400 + 7 * 3600;
        assert_eq!(iso_with_offset(at, 3 * 3600), "2026-10-14T10:00:00.000+0300");
        assert_eq!(iso_with_offset(at, -(5 * 3600 + 1800)), "2026-10-14T01:30:00.000-0530");
        assert_eq!(parse_iso("2026-10-14T10:00:00.000+0300"), Some(at));
        assert_eq!(parse_iso("2026-10-14T07:00:00.000+0000"), Some(at));
        assert_eq!(parse_iso("2026-10-14T07:00:00Z"), Some(at));
        assert_eq!(parse_iso("2026-10-14T10:00+03:00"), Some(at));
        assert_eq!(parse_iso("2026-10-14"), None);
        assert_eq!(parse_iso("2026-13-14T10:00:00Z"), None);
        assert_eq!(parse_date("2026-10-20"), Some((2026, 10, 20)));
        assert_eq!(parse_date("20.10.2026"), None);
    }

    #[test]
    fn readings() {
        let at = days_from_civil(2026, 10, 14) * 86_400 + 7 * 3600 + 5 * 60;
        let msk = 3 * 3600;
        assert_eq!(hours_minutes(3900), "1:05");
        assert_eq!(hours_minutes(59), "0:00");
        assert_eq!(moment(at, at + 3600, msk, "en"), "today, 10:05");
        assert_eq!(moment(at, at + 86_400, msk, "ru"), "вчера, 10:05");
        assert_eq!(moment(at, at + 5 * 86_400, msk, "en"), "14 Oct, 10:05");
        assert_eq!(day_month(2025, 5, 3, 2026, "ru"), "3 мая 2025");
    }
}
