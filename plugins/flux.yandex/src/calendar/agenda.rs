//! What the day's meetings mean for the window, without calling Flux (tests run on the host):
//! the status bar's reading ("Daily in 12 min", "Daily now · 5 min left", "Review at 15:00"), the
//! agenda's days and lines, which meeting to join, which meetings to remind of. Times are seconds
//! since 1970 (UTC); `offset` — the user's offset from UTC at the moment in question.

use super::ical::Meeting;
use crate::text::ellipsis;
use crate::time;

/// A meeting about to start this soon is shown rather than one going on.
const SOON_OVER_NOW: i64 = 10 * 60;
/// Within this, the status bar counts the minutes; later — the time of day.
const COUNT_MINUTES: i64 = 60 * 60;
/// A title longer than this is cut in the status bar.
const TITLE_CHARS: usize = 28;

/// What the status bar says about the day.
#[derive(Debug, Clone, PartialEq)]
pub enum Reading<'a> {
    /// The next one starts in this many minutes (at least 1).
    Soon(&'a Meeting, i64),
    /// One is going on: this many minutes left (at least 1).
    Now(&'a Meeting, i64),
    /// The next one today starts later: at its time.
    Later(&'a Meeting),
    /// Nothing more today.
    Free,
}

/// The status bar's reading at `now`: a meeting starting within ten minutes beats one going on;
/// whole-day events don't count.
pub fn reading(meetings: &[Meeting], now: i64, offset: i32) -> Reading<'_> {
    let today = day_of(now, offset);
    let timed = || meetings.iter().filter(|meeting| !meeting.all_day);
    let next = timed()
        .filter(|meeting| meeting.start > now && day_of(meeting.start, offset) == today)
        .min_by_key(|meeting| meeting.start);
    let going = timed()
        .filter(|meeting| meeting.start <= now && now < meeting.end)
        .max_by_key(|meeting| meeting.start);
    match (next, going) {
        (Some(next), _) if next.start - now <= SOON_OVER_NOW => {
            Reading::Soon(next, minutes_until(now, next.start))
        }
        (_, Some(going)) => Reading::Now(going, minutes_until(now, going.end)),
        (Some(next), None) if next.start - now <= COUNT_MINUTES => {
            Reading::Soon(next, minutes_until(now, next.start))
        }
        (Some(next), None) => Reading::Later(next),
        (None, None) => Reading::Free,
    }
}

/// Whole minutes until `at`, rounded up, at least 1.
pub fn minutes_until(now: i64, at: i64) -> i64 {
    ((at - now + 59) / 60).max(1)
}

/// The local day (days since 1970) of a moment.
pub fn day_of(at: i64, offset: i32) -> i64 {
    (at + i64::from(offset)).div_euclid(86_400)
}

/// A title for the status bar.
pub fn short_title(meeting: &Meeting, untitled: &str) -> String {
    let title = meeting.title.trim();
    if title.is_empty() {
        untitled.to_string()
    } else {
        ellipsis(title, TITLE_CHARS)
    }
}

/// "11:00–11:15".
pub fn span(meeting: &Meeting, offset_at: impl Fn(i64) -> i32) -> String {
    format!(
        "{}–{}",
        time::clock(meeting.start, offset_at(meeting.start)),
        time::clock(meeting.end, offset_at(meeting.end))
    )
}

/// Which day of the agenda a meeting is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Day {
    Today,
    Tomorrow,
}

/// The agenda: the meetings of today and tomorrow (a whole-day event on the days it covers),
/// whole-day events first, then by start.
pub fn agenda(meetings: &[Meeting], now: i64, offset: i32) -> Vec<(Day, &Meeting)> {
    let today = day_of(now, offset);
    let mut lines = Vec::new();
    for (day, which) in [(today, Day::Today), (today + 1, Day::Tomorrow)] {
        let mut of_day: Vec<&Meeting> = meetings
            .iter()
            .filter(|meeting| {
                let first = day_of(meeting.start, offset);
                if meeting.all_day {
                    // The end is the next local midnight: the last day is the one before.
                    let last = day_of(meeting.end - 1, offset).max(first);
                    (first..=last).contains(&day)
                } else {
                    first == day
                }
            })
            .collect();
        of_day.sort_by_key(|meeting| (!meeting.all_day, meeting.start));
        lines.extend(of_day.into_iter().map(|meeting| (which, meeting)));
    }
    lines
}

/// The meeting to join from the palette: one going on that has a link, else the next one with a
/// link starting within an hour.
pub fn to_join(meetings: &[Meeting], now: i64) -> Option<&Meeting> {
    let with_link = || {
        meetings
            .iter()
            .filter(|meeting| !meeting.all_day && meeting.link.is_some())
    };
    with_link()
        .filter(|meeting| meeting.start - SOON_OVER_NOW <= now && now < meeting.end)
        .min_by_key(|meeting| meeting.start)
        .or_else(|| {
            with_link()
                .filter(|meeting| meeting.start > now && meeting.start - now <= COUNT_MINUTES)
                .min_by_key(|meeting| meeting.start)
        })
}

/// The meetings to remind of at `now`: starting within `minutes`, not whole-day, not reminded
/// yet (`reminded` — their keys).
pub fn due<'a>(
    meetings: &'a [Meeting],
    now: i64,
    minutes: i64,
    reminded: &[String],
) -> Vec<&'a Meeting> {
    if minutes <= 0 {
        return Vec::new();
    }
    meetings
        .iter()
        .filter(|meeting| {
            !meeting.all_day
                && meeting.start > now
                && meeting.start - now <= minutes * 60
                && !reminded.contains(&meeting.key())
        })
        .collect()
}

/// Whether a link joins Telemost.
pub fn is_telemost(link: &str) -> bool {
    link.contains("telemost.yandex") || link.contains("telemost.360.yandex")
}

/// Yandex Calendar's page of a day: `https://calendar.yandex.ru/day?show_date=2026-10-14`.
pub fn calendar_page(at: i64, offset: i32) -> String {
    let day = time::civil(at, offset);
    format!(
        "https://calendar.yandex.ru/day?show_date={:04}-{:02}-{:02}",
        day.year, day.month, day.day
    )
}

/// A place for a line of the agenda: without links (they are the badge), one line, short.
pub fn place(meeting: &Meeting) -> Option<String> {
    let location = meeting.location.as_deref()?;
    let mut text = location.to_string();
    for link in super::ical::urls_in(location) {
        text = text.replace(&link, "");
    }
    let text = text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim_matches(|char: char| char == ',' || char == ';' || char.is_whitespace())
        .to_string();
    (!text.is_empty()).then(|| ellipsis(&text, 40))
}

#[cfg(test)]
mod tests {
    use super::*;

    const MSK: i32 = 3 * 3600;

    /// 2026-10-14 at `hour:minute` Moscow time.
    fn at(hour: i64, minute: i64) -> i64 {
        time::days_from_civil(2026, 10, 14) * 86_400 + hour * 3600 + minute * 60 - i64::from(MSK)
    }

    fn meeting(title: &str, start: i64, minutes: i64) -> Meeting {
        Meeting {
            uid: title.into(),
            start,
            end: start + minutes * 60,
            all_day: false,
            title: title.into(),
            location: None,
            link: Some("https://telemost.yandex.ru/j/1".into()),
            calendar: "Мои события".into(),
        }
    }

    fn all_day(title: &str, days: i64) -> Meeting {
        Meeting {
            all_day: true,
            link: None,
            ..meeting(title, at(0, 0), days * 24 * 60)
        }
    }

    #[test]
    fn reads_the_day() {
        let day = vec![
            meeting("Дейли", at(11, 0), 15),
            meeting("Ревью", at(15, 0), 60),
            all_day("ДР", 1),
        ];
        let soon = reading(&day, at(10, 48), MSK);
        assert!(matches!(soon, Reading::Soon(meeting, 12) if meeting.title == "Дейли"));
        assert!(matches!(reading(&day, at(10, 59) + 30, MSK), Reading::Soon(_, 1)));
        let now = reading(&day, at(11, 10), MSK);
        assert!(matches!(now, Reading::Now(meeting, 5) if meeting.title == "Дейли"));
        let later = reading(&day, at(12, 0), MSK);
        assert!(matches!(later, Reading::Later(meeting) if meeting.title == "Ревью"));
        assert!(matches!(reading(&day, at(14, 5), MSK), Reading::Soon(_, 55)));
        assert_eq!(reading(&day, at(16, 0), MSK), Reading::Free);
        // Tomorrow's meeting isn't today's.
        let tomorrow = vec![meeting("Завтра", at(24 + 9, 0), 30)];
        assert_eq!(reading(&tomorrow, at(18, 0), MSK), Reading::Free);
    }

    #[test]
    fn the_next_one_beats_the_one_going_on() {
        let day = vec![meeting("Долгая", at(10, 0), 120), meeting("Дейли", at(11, 0), 15)];
        assert!(matches!(reading(&day, at(10, 30), MSK), Reading::Now(meeting, 90) if meeting.title == "Долгая"));
        assert!(matches!(reading(&day, at(10, 52), MSK), Reading::Soon(meeting, 8) if meeting.title == "Дейли"));
        assert!(matches!(reading(&day, at(11, 5), MSK), Reading::Now(meeting, 10) if meeting.title == "Дейли"));
    }

    #[test]
    fn the_agenda_of_two_days() {
        let meetings = vec![
            meeting("Ревью", at(15, 0), 60),
            meeting("Дейли", at(11, 0), 15),
            meeting("Планирование", at(24 + 10, 0), 60),
            meeting("Послезавтра", at(48 + 10, 0), 60),
            all_day("Отпуск", 2),
        ];
        let lines: Vec<(Day, &str)> = agenda(&meetings, at(9, 0), MSK)
            .into_iter()
            .map(|(day, meeting)| (day, meeting.title.as_str()))
            .collect();
        assert_eq!(
            lines,
            vec![
                (Day::Today, "Отпуск"),
                (Day::Today, "Дейли"),
                (Day::Today, "Ревью"),
                (Day::Tomorrow, "Отпуск"),
                (Day::Tomorrow, "Планирование"),
            ]
        );
        assert_eq!(span(&meetings[1], |_| MSK), "11:00–11:15");
    }

    #[test]
    fn joining_and_reminding() {
        let mut no_link = meeting("Без ссылки", at(10, 0), 30);
        no_link.link = None;
        let meetings = vec![no_link, meeting("Дейли", at(11, 0), 15), meeting("Ревью", at(15, 0), 60)];
        // The next one with a link is more than an hour away; the one at 10:00 has no link.
        assert_eq!(to_join(&meetings, at(9, 50)), None);
        assert_eq!(to_join(&meetings, at(10, 10)).unwrap().title, "Дейли");
        assert_eq!(to_join(&meetings, at(10, 55)).unwrap().title, "Дейли");
        assert_eq!(to_join(&meetings, at(11, 10)).unwrap().title, "Дейли");
        assert_eq!(to_join(&meetings, at(14, 30)).unwrap().title, "Ревью");
        assert_eq!(to_join(&meetings, at(12, 0)), None);
        let due_now: Vec<&str> = due(&meetings, at(10, 56), 5, &[])
            .iter()
            .map(|meeting| meeting.title.as_str())
            .collect();
        assert_eq!(due_now, vec!["Дейли"]);
        assert!(due(&meetings, at(10, 56), 5, &[meetings[1].key()]).is_empty());
        assert!(due(&meetings, at(10, 56), 0, &[]).is_empty());
        assert!(due(&meetings, at(11, 1), 5, &[]).is_empty());
    }

    #[test]
    fn places_and_pages() {
        let mut room = meeting("Ревью", at(15, 0), 60);
        room.location = Some("Переговорка «Нева», https://telemost.yandex.ru/j/2 ".into());
        assert_eq!(place(&room).as_deref(), Some("Переговорка «Нева»"));
        room.location = Some("https://telemost.yandex.ru/j/2".into());
        assert_eq!(place(&room), None);
        assert_eq!(
            calendar_page(at(23, 30), MSK),
            "https://calendar.yandex.ru/day?show_date=2026-10-14"
        );
        assert!(is_telemost("https://telemost.yandex.ru/j/2"));
        assert!(!is_telemost("https://zoom.us/j/2"));
        assert_eq!(minutes_until(0, 1), 1);
        assert_eq!(minutes_until(0, 60), 1);
        assert_eq!(minutes_until(0, 61), 2);
    }
}
