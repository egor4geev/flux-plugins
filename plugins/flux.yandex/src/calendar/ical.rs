//! iCalendar (RFC 5545) as CalDAV gives it, read into the meetings of a time window: content
//! lines unfolded and unescaped; times in UTC, with a `TZID` (the calendar's own `VTIMEZONE`, the
//! user's zone, a table of zones without summer time), floating (the user's time) or a whole day;
//! recurrences — expanded by the server (instances with `RECURRENCE-ID`), or here from `RRULE`
//! (daily, weekly with days, monthly, yearly; `INTERVAL`, `COUNT`, `UNTIL`), with `EXDATE` and
//! moved or cancelled instances. Cancelled events and the ones the user declined are left out; a
//! meeting's link is its Telemost conference, else a link in its place, description or URL.

use std::collections::HashMap;

use crate::time;

/// What the reading needs to know of the user's clock.
pub trait Clock {
    /// How far the user's local time is ahead of UTC at `at`, in seconds.
    fn offset(&self, at: i64) -> i32;
    /// The user's time zone: "Europe/Moscow".
    fn zone(&self) -> String;
}

/// A meeting (an instance of a recurring one is a meeting of its own).
#[derive(Debug, Clone, PartialEq)]
pub struct Meeting {
    pub uid: String,
    /// Seconds since 1970, UTC.
    pub start: i64,
    pub end: i64,
    /// A whole day (or days): `start` is the local midnight.
    pub all_day: bool,
    pub title: String,
    pub location: Option<String>,
    /// Where to join: the Telemost conference, else a link of the meeting.
    pub link: Option<String>,
    /// The calendar's name.
    pub calendar: String,
}

impl Meeting {
    /// Names the meeting among the day's: its uid and start (an instance of a recurring one has
    /// the uid of the series).
    pub fn key(&self) -> String {
        format!("{}@{}", self.uid, self.start)
    }
}

/// The meetings of an iCalendar text that fall in `[from, to)`: `me` — the user's emails, to
/// leave out what they declined.
pub fn meetings(
    text: &str,
    calendar: &str,
    me: &[String],
    from: i64,
    to: i64,
    clock: &dyn Clock,
) -> Vec<Meeting> {
    let mut found = Vec::new();
    for root in parse(text) {
        let zones = Zones::of(&root);
        let mut series: Vec<(String, Vec<&Component>)> = Vec::new();
        for event in root.children.iter().filter(|child| child.name == "VEVENT") {
            let uid = event
                .property("UID")
                .map(|uid| uid.value.trim().to_string())
                .unwrap_or_default();
            match series.iter_mut().find(|(known, _)| *known == uid && !uid.is_empty()) {
                Some((_, events)) => events.push(event),
                None => series.push((uid, vec![event])),
            }
        }
        for (_, events) in series {
            read_series(&events, calendar, me, from, to, &zones, clock, &mut found);
        }
    }
    found.sort_by(|a, b| (a.start, !a.all_day, &a.title).cmp(&(b.start, !b.all_day, &b.title)));
    found
}

#[allow(clippy::too_many_arguments)]
fn read_series(
    events: &[&Component],
    calendar: &str,
    me: &[String],
    from: i64,
    to: i64,
    zones: &Zones,
    clock: &dyn Clock,
    found: &mut Vec<Meeting>,
) {
    let master = events
        .iter()
        .find(|event| event.property("RECURRENCE-ID").is_none())
        .copied();
    let overrides: Vec<&Component> = events
        .iter()
        .filter(|event| event.property("RECURRENCE-ID").is_some())
        .copied()
        .collect();
    let replaced: Vec<i64> = overrides
        .iter()
        .filter_map(|event| {
            let id = when_of(event.property("RECURRENCE-ID")?)?;
            Some(zones.utc(&id, clock))
        })
        .collect();
    if let Some(master) = master {
        let rule = master
            .property("RRULE")
            .and_then(|rule| parse_rule(&rule.value));
        match (rule, master.property("DTSTART").and_then(when_of)) {
            (Some(rule), Some(start)) => {
                let excluded: Vec<i64> = master
                    .all("EXDATE")
                    .flat_map(whens_of)
                    .map(|when| zones.utc(&when, clock))
                    .collect();
                let length = length_of(master, &start, zones, clock);
                for occurrence in expand(&rule, &start, from - length.max(86_400), to, zones, clock)
                {
                    let at = zones.utc(&occurrence, clock);
                    if excluded.contains(&at) || replaced.contains(&at) {
                        continue;
                    }
                    if let Some(meeting) =
                        meeting(master, &occurrence, calendar, me, zones, clock)
                    {
                        found.push(meeting);
                    }
                }
            }
            (_, Some(start)) => {
                if let Some(meeting) = meeting(master, &start, calendar, me, zones, clock) {
                    found.push(meeting);
                }
            }
            _ => {}
        }
    }
    for event in overrides {
        if let Some(start) = event.property("DTSTART").and_then(when_of)
            && let Some(meeting) = meeting(event, &start, calendar, me, zones, clock)
        {
            found.push(meeting);
        }
    }
    found.retain(|meeting| meeting.end.max(meeting.start + 1) > from && meeting.start < to);
}

/// A meeting of `event` starting at `start` (the event's own start, or an instance's): none
/// when it is cancelled or declined.
fn meeting(
    event: &Component,
    start: &When,
    calendar: &str,
    me: &[String],
    zones: &Zones,
    clock: &dyn Clock,
) -> Option<Meeting> {
    let status = event.property("STATUS").map(|status| status.value.trim().to_ascii_uppercase());
    if status.as_deref() == Some("CANCELLED") || declined(event, me) {
        return None;
    }
    let first = event.property("DTSTART").and_then(when_of)?;
    let length = length_of(event, &first, zones, clock);
    let all_day = matches!(start, When::Date(_));
    let at = zones.utc(start, clock);
    let end = match start {
        // A whole day ends at a later local midnight, whatever the summer time does meanwhile.
        When::Date(wall) => {
            let days = (length + 43_200).div_euclid(86_400).max(1);
            zones.utc(&When::Date(Wall::from_days(wall.days() + days)), clock)
        }
        _ => at + length,
    };
    Some(Meeting {
        uid: event
            .property("UID")
            .map(|uid| uid.value.trim().to_string())
            .unwrap_or_default(),
        start: at,
        end,
        all_day,
        title: event
            .property("SUMMARY")
            .map(Property::text)
            .map(|text| text.trim().to_string())
            .unwrap_or_default(),
        location: text_of(event, "LOCATION"),
        link: find_link(event),
        calendar: calendar.to_string(),
    })
}

/// How long an event lasts: `DTEND` − `DTSTART`, else `DURATION`, else a day for a whole-day
/// event and nothing for a moment.
fn length_of(event: &Component, start: &When, zones: &Zones, clock: &dyn Clock) -> i64 {
    if let Some(end) = event.property("DTEND").and_then(when_of) {
        return match (start, &end) {
            (When::Date(from), When::Date(to)) => (to.days() - from.days()).max(1) * 86_400,
            _ => (zones.utc(&end, clock) - zones.utc(start, clock)).max(0),
        };
    }
    if let Some(duration) = event
        .property("DURATION")
        .and_then(|duration| parse_duration(&duration.value))
    {
        return duration.max(0);
    }
    if matches!(start, When::Date(_)) { 86_400 } else { 0 }
}

/// The user declined it: an `ATTENDEE` with one of the user's emails and `PARTSTAT=DECLINED`.
fn declined(event: &Component, me: &[String]) -> bool {
    event.all("ATTENDEE").any(|attendee| {
        let email = attendee
            .value
            .trim()
            .trim_start_matches("mailto:")
            .trim_start_matches("MAILTO:");
        attendee
            .param("PARTSTAT")
            .is_some_and(|state| state.eq_ignore_ascii_case("DECLINED"))
            && me.iter().any(|mine| mine.eq_ignore_ascii_case(email))
    })
}

fn text_of(event: &Component, name: &str) -> Option<String> {
    event
        .property(name)
        .map(Property::text)
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
}

/// Where to join the meeting: its Telemost conference (`X-TELEMOST-CONFERENCE`), else a link in
/// its place, description or URL — a Telemost one first.
pub fn find_link(event: &Component) -> Option<String> {
    let conference = event
        .properties
        .iter()
        .filter(|property| property.name.contains("TELEMOST"))
        .flat_map(|property| urls_in(&property.text()))
        .next();
    if conference.is_some() {
        return conference;
    }
    let links: Vec<String> = ["LOCATION", "DESCRIPTION", "URL"]
        .iter()
        .filter_map(|name| event.property(name))
        .flat_map(|property| urls_in(&property.text()))
        .collect();
    links
        .iter()
        .find(|link| link.contains("telemost.yandex"))
        .or(links.first())
        .cloned()
}

/// The `http(s)` links in a text, without trailing punctuation.
pub fn urls_in(text: &str) -> Vec<String> {
    let mut links = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("http") {
        let tail = &rest[start..];
        if !(tail.starts_with("https://") || tail.starts_with("http://")) {
            rest = &tail[4..];
            continue;
        }
        let end = tail
            .find(|char: char| char.is_whitespace() || "<>\"'`)]}".contains(char))
            .unwrap_or(tail.len());
        let link = tail[..end].trim_end_matches(['.', ',', ';', ':', '!', '?']);
        if link.len() > "https://".len() {
            links.push(link.to_string());
        }
        rest = &tail[end..];
    }
    links
}

// --- Content lines ---

/// A content line: `NAME;PARAM=VALUE:value`.
#[derive(Debug, Clone, PartialEq)]
pub struct Property {
    /// Uppercase.
    pub name: String,
    /// Names uppercase, values unquoted.
    pub params: Vec<(String, String)>,
    /// As written (escaped for text values).
    pub value: String,
}

impl Property {
    pub fn param(&self, name: &str) -> Option<&str> {
        self.params
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    /// The value as text: `\n`, `\,`, `\;`, `\\` unescaped.
    pub fn text(&self) -> String {
        unescape(&self.value)
    }
}

/// A component: `VCALENDAR`, `VEVENT`, `VTIMEZONE`…
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Component {
    pub name: String,
    pub properties: Vec<Property>,
    pub children: Vec<Component>,
}

impl Component {
    pub fn property(&self, name: &str) -> Option<&Property> {
        self.properties.iter().find(|property| property.name == name)
    }

    pub fn all<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Property> {
        self.properties
            .iter()
            .filter(move |property| property.name == name)
    }
}

/// The top-level components of a text (its `VCALENDAR`s).
pub fn parse(text: &str) -> Vec<Component> {
    let mut stack: Vec<Component> = vec![Component::default()];
    for line in unfold(text) {
        let Some(property) = content_line(&line) else {
            continue;
        };
        match property.name.as_str() {
            "BEGIN" => stack.push(Component {
                name: property.value.trim().to_ascii_uppercase(),
                ..Component::default()
            }),
            "END" => {
                if stack.len() > 1 {
                    let done = stack.pop().unwrap_or_default();
                    if let Some(parent) = stack.last_mut() {
                        parent.children.push(done);
                    }
                }
            }
            _ => {
                if let Some(current) = stack.last_mut() {
                    current.properties.push(property);
                }
            }
        }
    }
    while stack.len() > 1 {
        let done = stack.pop().unwrap_or_default();
        if let Some(parent) = stack.last_mut() {
            parent.children.push(done);
        }
    }
    stack.pop().map(|root| root.children).unwrap_or_default()
}

/// Lines with their folds joined: a line that starts with a space or a tab continues the one
/// before.
fn unfold(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for line in text.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        match (line.strip_prefix([' ', '\t']), lines.last_mut()) {
            (Some(rest), Some(last)) => last.push_str(rest),
            _ if line.is_empty() => {}
            _ => lines.push(line.to_string()),
        }
    }
    lines
}

fn content_line(line: &str) -> Option<Property> {
    let mut quoted = false;
    let mut colon = None;
    for (index, char) in line.char_indices() {
        match char {
            '"' => quoted = !quoted,
            ':' if !quoted => {
                colon = Some(index);
                break;
            }
            _ => {}
        }
    }
    let colon = colon?;
    let (head, value) = (&line[..colon], &line[colon + 1..]);
    let mut parts = split_params(head).into_iter();
    let name = parts.next()?.trim().to_ascii_uppercase();
    if name.is_empty() {
        return None;
    }
    let params = parts
        .filter_map(|param| {
            let (key, value) = param.split_once('=')?;
            Some((
                key.trim().to_ascii_uppercase(),
                value.trim().trim_matches('"').to_string(),
            ))
        })
        .collect();
    Some(Property {
        name,
        params,
        value: value.to_string(),
    })
}

/// `NAME;A=1;B="x;y"` → `NAME`, `A=1`, `B="x;y"`.
fn split_params(head: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut quoted = false;
    let mut start = 0;
    for (index, char) in head.char_indices() {
        match char {
            '"' => quoted = !quoted,
            ';' if !quoted => {
                parts.push(&head[start..index]);
                start = index + 1;
            }
            _ => {}
        }
    }
    parts.push(&head[start..]);
    parts
}

/// A text value unescaped: `\n` / `\N` — a new line, `\,` `\;` `\\` — the character.
pub fn unescape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(char) = chars.next() {
        if char != '\\' {
            out.push(char);
            continue;
        }
        match chars.next() {
            Some('n' | 'N') => out.push('\n'),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

// --- Times ---

/// A wall-clock moment: a date and a time, in no zone yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Wall {
    pub year: i64,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
}

impl Wall {
    /// Days since 1970-01-01 of its date.
    pub fn days(&self) -> i64 {
        time::days_from_civil(self.year, self.month, self.day)
    }

    /// Seconds since 1970 as if it were UTC.
    pub fn seconds(&self) -> i64 {
        self.days() * 86_400
            + i64::from(self.hour) * 3600
            + i64::from(self.minute) * 60
            + i64::from(self.second)
    }

    /// The midnight of a day since 1970.
    pub fn from_days(days: i64) -> Wall {
        let (year, month, day) = time::civil_from_days(days);
        Wall {
            year,
            month,
            day,
            hour: 0,
            minute: 0,
            second: 0,
        }
    }

    /// The same time of day on another day.
    fn on_day(&self, days: i64) -> Wall {
        Wall {
            hour: self.hour,
            minute: self.minute,
            second: self.second,
            ..Wall::from_days(days)
        }
    }
}

/// A moment as iCalendar writes it.
#[derive(Debug, Clone, PartialEq)]
pub enum When {
    /// A whole day (`VALUE=DATE`).
    Date(Wall),
    /// UTC (`…Z`), as seconds since 1970.
    Utc(i64),
    /// A wall-clock time in a zone (`TZID`), or floating — the user's (none).
    Local(Wall, Option<String>),
}

/// The moment of a property (`DTSTART`, `DTEND`, `RECURRENCE-ID`).
fn when_of(property: &Property) -> Option<When> {
    whens_of(property).into_iter().next()
}

/// The moments of a property with a list (`EXDATE:…,…`).
fn whens_of(property: &Property) -> Vec<When> {
    let date_only = property
        .param("VALUE")
        .is_some_and(|value| value.eq_ignore_ascii_case("DATE"));
    let zone = property
        .param("TZID")
        .map(|zone| zone.trim_start_matches('/').to_string());
    property
        .value
        .split(',')
        .filter_map(|value| {
            let value = value.trim();
            let (wall, utc) = parse_wall(value)?;
            Some(if date_only || value.len() == 8 {
                When::Date(Wall::from_days(wall.days()))
            } else if utc {
                When::Utc(wall.seconds())
            } else {
                When::Local(wall, zone.clone())
            })
        })
        .collect()
}

/// `20261014`, `20261014T110000`, `20261014T080000Z` → the wall time and whether it is UTC.
fn parse_wall(value: &str) -> Option<(Wall, bool)> {
    let digits = |range: std::ops::Range<usize>| -> Option<i64> {
        let part = value.get(range)?;
        part.chars()
            .all(|char| char.is_ascii_digit())
            .then(|| part.parse().ok())?
    };
    let year = digits(0..4)?;
    let month = digits(4..6)? as u32;
    let day = digits(6..8)? as u32;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let (hour, minute, second, utc) = if value.len() >= 15 && value.as_bytes()[8] == b'T' {
        (
            digits(9..11)? as u32,
            digits(11..13)? as u32,
            digits(13..15)? as u32,
            value[15..].eq_ignore_ascii_case("Z"),
        )
    } else {
        (0, 0, 0, false)
    };
    Some((
        Wall {
            year,
            month,
            day,
            hour,
            minute,
            second,
        },
        utc,
    ))
}

/// An iCalendar duration in seconds: `PT15M`, `P1D`, `P1DT2H`, `P1W`, `-PT5M`.
pub fn parse_duration(value: &str) -> Option<i64> {
    let value = value.trim();
    let (sign, value) = match value.strip_prefix('-') {
        Some(rest) => (-1, rest),
        None => (1, value.strip_prefix('+').unwrap_or(value)),
    };
    let value = value.strip_prefix('P')?;
    let mut total = 0i64;
    let mut number = String::new();
    let mut in_time = false;
    for char in value.chars() {
        match char {
            'T' => in_time = true,
            '0'..='9' => number.push(char),
            unit => {
                let amount: i64 = number.parse().ok()?;
                number.clear();
                total += amount
                    * match (unit, in_time) {
                        ('W', _) => 7 * 86_400,
                        ('D', _) => 86_400,
                        ('H', true) => 3600,
                        ('M', true) => 60,
                        ('S', true) => 1,
                        _ => return None,
                    };
            }
        }
    }
    number.is_empty().then_some(sign * total)
}

/// The time zones a calendar defines (`VTIMEZONE`), by `TZID`.
#[derive(Debug, Default)]
pub struct Zones(HashMap<String, Zone>);

#[derive(Debug, Default)]
struct Zone {
    observances: Vec<Observance>,
}

/// A part of a zone's rules (`STANDARD`, `DAYLIGHT`): from its onset on, the offset is `to`.
#[derive(Debug)]
struct Observance {
    /// The first onset, in the wall time before it (`offset-from`).
    start: Wall,
    from: i32,
    to: i32,
    /// The yearly rule of the onsets (`FREQ=YEARLY;BYMONTH=3;BYDAY=-1SU`).
    rule: Option<Rule>,
}

impl Zones {
    pub fn of(calendar: &Component) -> Zones {
        let mut zones = HashMap::new();
        for zone in calendar.children.iter().filter(|child| child.name == "VTIMEZONE") {
            let Some(id) = zone.property("TZID").map(|id| id.value.trim().to_string()) else {
                continue;
            };
            let observances = zone
                .children
                .iter()
                .filter(|part| part.name == "STANDARD" || part.name == "DAYLIGHT")
                .filter_map(|part| {
                    let (start, _) = parse_wall(part.property("DTSTART")?.value.trim())?;
                    Some(Observance {
                        start,
                        from: parse_offset(&part.property("TZOFFSETFROM")?.value)?,
                        to: parse_offset(&part.property("TZOFFSETTO")?.value)?,
                        rule: part.property("RRULE").and_then(|rule| parse_rule(&rule.value)),
                    })
                })
                .collect();
            zones.insert(id, Zone { observances });
        }
        Zones(zones)
    }

    /// A moment in seconds since 1970, UTC.
    pub fn utc(&self, when: &When, clock: &dyn Clock) -> i64 {
        match when {
            When::Utc(at) => *at,
            When::Date(wall) => user_time(wall.seconds(), clock),
            When::Local(wall, None) => user_time(wall.seconds(), clock),
            When::Local(wall, Some(zone)) => {
                let local = wall.seconds();
                if let Some(offset) = self.0.get(zone).and_then(|known| known.offset(*wall)) {
                    return local - i64::from(offset);
                }
                if *zone == clock.zone() {
                    return user_time(local, clock);
                }
                match fixed_offset(zone) {
                    Some(offset) => local - i64::from(offset),
                    None => user_time(local, clock),
                }
            }
        }
    }
}

impl Zone {
    /// The offset at a wall time: the one of the latest onset not after it.
    fn offset(&self, wall: Wall) -> Option<i32> {
        let mut latest: Option<(Wall, i32)> = None;
        for observance in &self.observances {
            for onset in observance.onsets_near(wall) {
                if onset <= wall && latest.is_none_or(|(at, _)| onset > at) {
                    latest = Some((onset, observance.to));
                }
            }
        }
        latest
            .map(|(_, offset)| offset)
            .or_else(|| self.observances.first().map(|first| first.from))
    }
}

impl Observance {
    /// Its onsets in the year of `wall` and the year before (the latest one not after `wall` is
    /// among them), and its first one.
    fn onsets_near(&self, wall: Wall) -> Vec<Wall> {
        let mut onsets = vec![self.start];
        let Some(rule) = &self.rule else {
            return onsets;
        };
        for year in [wall.year - 1, wall.year] {
            if year <= self.start.year {
                continue;
            }
            let month = rule.by_month.first().copied().unwrap_or(self.start.month);
            let day = match rule.by_day.first() {
                Some(&(nth, weekday)) => nth_weekday(year, month, nth.unwrap_or(1), weekday),
                None => valid_day(year, month, self.start.day),
            };
            if let Some(day) = day {
                onsets.push(self.start.on_day(time::days_from_civil(year, month, day)));
            }
        }
        onsets
    }
}

/// `+0300`, `-0530`, `+030000` → seconds.
fn parse_offset(value: &str) -> Option<i32> {
    let value = value.trim();
    let sign = match value.chars().next()? {
        '-' => -1,
        '+' => 1,
        _ => return None,
    };
    let digits = &value[1..];
    if digits.len() < 4 || !digits.chars().all(|char| char.is_ascii_digit()) {
        return None;
    }
    let hours: i32 = digits[..2].parse().ok()?;
    let minutes: i32 = digits[2..4].parse().ok()?;
    let seconds: i32 = digits.get(4..6).and_then(|part| part.parse().ok()).unwrap_or(0);
    Some(sign * (hours * 3600 + minutes * 60 + seconds))
}

/// The UTC moment of a wall time in the user's zone (`local` — as if UTC).
fn user_time(local: i64, clock: &dyn Clock) -> i64 {
    let guess = local - i64::from(clock.offset(local));
    local - i64::from(clock.offset(guess))
}

/// Zones without summer time, for a `TZID` the calendar doesn't define.
fn fixed_offset(zone: &str) -> Option<i32> {
    let hours: f32 = match zone {
        "UTC" | "Etc/UTC" | "GMT" | "Etc/GMT" | "Z" => 0.,
        "Europe/Kaliningrad" => 2.,
        "Europe/Moscow" | "Europe/Minsk" | "Europe/Simferopol" | "Europe/Volgograd"
        | "Europe/Kirov" | "Europe/Istanbul" | "Asia/Riyadh" | "Russian Standard Time" => 3.,
        "Europe/Samara" | "Europe/Ulyanovsk" | "Europe/Astrakhan" | "Europe/Saratov"
        | "Asia/Dubai" | "Asia/Baku" | "Asia/Tbilisi" | "Asia/Yerevan" => 4.,
        "Asia/Yekaterinburg" | "Asia/Tashkent" | "Asia/Almaty" | "Asia/Karachi" => 5.,
        "Asia/Kolkata" | "Asia/Calcutta" => 5.5,
        "Asia/Omsk" | "Asia/Bishkek" | "Asia/Dhaka" => 6.,
        "Asia/Novosibirsk" | "Asia/Barnaul" | "Asia/Tomsk" | "Asia/Novokuznetsk"
        | "Asia/Krasnoyarsk" | "Asia/Bangkok" | "Asia/Jakarta" => 7.,
        "Asia/Irkutsk" | "Asia/Shanghai" | "Asia/Singapore" | "Asia/Hong_Kong"
        | "Asia/Taipei" => 8.,
        "Asia/Yakutsk" | "Asia/Chita" | "Asia/Tokyo" | "Asia/Seoul" => 9.,
        "Asia/Vladivostok" => 10.,
        "Asia/Magadan" | "Asia/Sakhalin" | "Asia/Srednekolymsk" => 11.,
        "Asia/Kamchatka" | "Asia/Anadyr" => 12.,
        _ => return None,
    };
    Some((hours * 3600.) as i32)
}

// --- Recurrences ---

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Freq {
    Daily,
    Weekly,
    Monthly,
    Yearly,
}

/// An `RRULE` (the parts a calendar's meetings use).
#[derive(Debug, Clone, PartialEq)]
pub struct Rule {
    pub freq: Freq,
    pub interval: i64,
    pub count: Option<u32>,
    pub until: Option<When>,
    /// (the n-th in the month or the year; none — every), the weekday: Monday — 0.
    pub by_day: Vec<(Option<i32>, u32)>,
    pub by_month_day: Vec<i32>,
    pub by_month: Vec<u32>,
    /// The week's first day: Monday — 0.
    pub week_start: u32,
}

/// An `RRULE`'s value; none for a rule it doesn't know (`HOURLY`…): the event stays single.
pub fn parse_rule(value: &str) -> Option<Rule> {
    let mut rule = Rule {
        freq: Freq::Daily,
        interval: 1,
        count: None,
        until: None,
        by_day: Vec::new(),
        by_month_day: Vec::new(),
        by_month: Vec::new(),
        week_start: 0,
    };
    let mut freq = None;
    for part in value.trim().split(';') {
        let Some((key, value)) = part.split_once('=') else {
            continue;
        };
        let value = value.trim();
        match key.trim().to_ascii_uppercase().as_str() {
            "FREQ" => {
                freq = Some(match value.to_ascii_uppercase().as_str() {
                    "DAILY" => Freq::Daily,
                    "WEEKLY" => Freq::Weekly,
                    "MONTHLY" => Freq::Monthly,
                    "YEARLY" => Freq::Yearly,
                    _ => return None,
                })
            }
            "INTERVAL" => rule.interval = value.parse::<i64>().ok()?.max(1),
            "COUNT" => rule.count = value.parse().ok(),
            "UNTIL" => {
                rule.until = parse_wall(value).map(|(wall, utc)| {
                    if value.len() == 8 {
                        When::Date(wall)
                    } else if utc {
                        When::Utc(wall.seconds())
                    } else {
                        When::Local(wall, None)
                    }
                })
            }
            "BYDAY" => {
                rule.by_day = value
                    .split(',')
                    .filter_map(|day| {
                        let day = day.trim();
                        let split = day.len().checked_sub(2)?;
                        let weekday = weekday_of(&day[split..])?;
                        let nth = match &day[..split] {
                            "" => None,
                            nth => Some(nth.trim_start_matches('+').parse().ok()?),
                        };
                        Some((nth, weekday))
                    })
                    .collect()
            }
            "BYMONTHDAY" => {
                rule.by_month_day = value
                    .split(',')
                    .filter_map(|day| day.trim().parse().ok())
                    .collect()
            }
            "BYMONTH" => {
                rule.by_month = value
                    .split(',')
                    .filter_map(|month| month.trim().parse().ok())
                    .filter(|month| (1..=12).contains(month))
                    .collect()
            }
            "WKST" => rule.week_start = weekday_of(value).unwrap_or(0),
            _ => {}
        }
    }
    rule.freq = freq?;
    Some(rule)
}

fn weekday_of(name: &str) -> Option<u32> {
    ["MO", "TU", "WE", "TH", "FR", "SA", "SU"]
        .iter()
        .position(|day| day.eq_ignore_ascii_case(name))
        .map(|day| day as u32)
}

/// Monday — 0, Sunday — 6, of a day since 1970.
pub fn weekday(days: i64) -> u32 {
    (days + 3).rem_euclid(7) as u32
}

fn days_in_month(year: i64, month: u32) -> u32 {
    let (next_year, next_month) = if month == 12 { (year + 1, 1) } else { (year, month + 1) };
    (time::days_from_civil(next_year, next_month, 1) - time::days_from_civil(year, month, 1)) as u32
}

fn valid_day(year: i64, month: u32, day: u32) -> Option<u32> {
    (day >= 1 && day <= days_in_month(year, month)).then_some(day)
}

/// The `nth` `weekday` of a month (−1 — the last); none when there isn't one.
fn nth_weekday(year: i64, month: u32, nth: i32, weekday_wanted: u32) -> Option<u32> {
    let first = time::days_from_civil(year, month, 1);
    let length = i64::from(days_in_month(year, month));
    let days: Vec<i64> = (0..length)
        .filter(|day| weekday(first + day) == weekday_wanted)
        .collect();
    let index = if nth > 0 {
        nth as usize - 1
    } else {
        days.len().checked_sub(nth.unsigned_abs() as usize)?
    };
    days.get(index).map(|day| *day as u32 + 1)
}

/// The days of a month a monthly (or yearly, for that month) rule gives, in order.
fn month_days(rule: &Rule, year: i64, month: u32, default_day: u32) -> Vec<u32> {
    let length = days_in_month(year, month) as i32;
    let mut days: Vec<u32> = if !rule.by_month_day.is_empty() {
        rule.by_month_day
            .iter()
            .filter_map(|&day| {
                let day = if day < 0 { length + 1 + day } else { day };
                (1..=length).contains(&day).then_some(day as u32)
            })
            .collect()
    } else if !rule.by_day.is_empty() {
        let first = time::days_from_civil(year, month, 1);
        rule.by_day
            .iter()
            .flat_map(|&(nth, wanted)| match nth {
                Some(nth) => nth_weekday(year, month, nth, wanted).into_iter().collect::<Vec<_>>(),
                None => (0..length as i64)
                    .filter(|day| weekday(first + day) == wanted)
                    .map(|day| day as u32 + 1)
                    .collect(),
            })
            .collect()
    } else {
        valid_day(year, month, default_day).into_iter().collect()
    };
    days.sort_unstable();
    days.dedup();
    days
}

/// The starts of a recurring event that begin before `to` (and end after `from`, roughly — the
/// caller checks), in the event's own kind of time: `COUNT` counts from the first one.
fn expand(
    rule: &Rule,
    start: &When,
    from: i64,
    to: i64,
    zones: &Zones,
    clock: &dyn Clock,
) -> Vec<When> {
    // The rule runs on the wall time of the event's zone (summer time keeps the hour).
    let (base, make): (Wall, Box<dyn Fn(Wall) -> When>) = match start {
        When::Date(wall) => (*wall, Box::new(When::Date)),
        When::Utc(at) => {
            let civil = time::civil(*at, 0);
            (
                Wall {
                    year: civil.year,
                    month: civil.month,
                    day: civil.day,
                    hour: civil.hour,
                    minute: civil.minute,
                    second: civil.second,
                },
                Box::new(|wall: Wall| When::Utc(wall.seconds())),
            )
        }
        When::Local(wall, zone) => {
            let zone = zone.clone();
            (*wall, Box::new(move |wall| When::Local(wall, zone.clone())))
        }
    };
    let until = rule.until.as_ref().map(|until| match until {
        // A date as the last day: its end.
        When::Date(wall) => zones.utc(&When::Date(Wall::from_days(wall.days() + 1)), clock) - 1,
        until => zones.utc(until, clock),
    });
    let first_day = base.days();
    let mut out = Vec::new();
    let mut count = 0u32;
    // Gives a candidate day: false — stop.
    let mut emit = |day: i64| -> bool {
        if day < first_day {
            return true;
        }
        let when = make(base.on_day(day));
        let at = zones.utc(&when, clock);
        if until.is_some_and(|until| at > until) || at >= to {
            return false;
        }
        count += 1;
        if rule.count.is_some_and(|limit| count > limit) {
            return false;
        }
        if at >= from {
            out.push(when);
        }
        true
    };
    let fits_filters = |day: i64| -> bool {
        let (_, month, date) = time::civil_from_days(day);
        (rule.by_month.is_empty() || rule.by_month.contains(&month))
            && (rule.by_month_day.is_empty()
                || rule.by_month_day.contains(&(date as i32)))
            && (rule.by_day.is_empty()
                || rule.by_day.iter().any(|&(_, wanted)| wanted == weekday(day)))
    };
    const LIMIT: i64 = 20_000;
    match rule.freq {
        Freq::Daily => {
            for step in 0..LIMIT {
                let day = first_day + step * rule.interval;
                if fits_filters(day) && !emit(day) {
                    break;
                }
            }
        }
        Freq::Weekly => {
            let week = first_day - i64::from((weekday(first_day) + 7 - rule.week_start) % 7);
            let mut weekdays: Vec<u32> = if rule.by_day.is_empty() {
                vec![weekday(first_day)]
            } else {
                rule.by_day.iter().map(|&(_, day)| day).collect()
            };
            // In the week's order, from its first day.
            weekdays.sort_by_key(|day| (day + 7 - rule.week_start) % 7);
            weekdays.dedup();
            'weeks: for step in 0..LIMIT / 7 {
                let monday = week + step * 7 * rule.interval;
                for &day in &weekdays {
                    let day = monday + i64::from((day + 7 - rule.week_start) % 7);
                    if !rule.by_month.is_empty() && !fits_filters(day) {
                        continue;
                    }
                    if !emit(day) {
                        break 'weeks;
                    }
                }
            }
        }
        Freq::Monthly | Freq::Yearly => {
            let months = if rule.freq == Freq::Monthly { 1 } else { 12 };
            'months: for step in 0..LIMIT / 28 {
                let index = base.year * 12 + i64::from(base.month - 1)
                    + step * months * rule.interval;
                let year = index.div_euclid(12);
                let first_month = index.rem_euclid(12) as u32 + 1;
                let month_list: Vec<u32> =
                    if rule.freq == Freq::Yearly && !rule.by_month.is_empty() {
                        rule.by_month.clone()
                    } else {
                        vec![first_month]
                    };
                for month in month_list {
                    if rule.freq == Freq::Monthly
                        && !rule.by_month.is_empty()
                        && !rule.by_month.contains(&month)
                    {
                        continue;
                    }
                    for day in month_days(rule, year, month, base.day) {
                        if !emit(time::days_from_civil(year, month, day)) {
                            break 'months;
                        }
                    }
                }
                if year > time::civil(to, 0).year + 1 {
                    break;
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Moscow: +3 all year.
    struct Moscow;

    impl Clock for Moscow {
        fn offset(&self, _: i64) -> i32 {
            3 * 3600
        }

        fn zone(&self) -> String {
            "Europe/Moscow".into()
        }
    }

    /// Berlin-like: +1, summer +2 (last Sunday of March … last Sunday of October, at 01:00 UTC).
    struct Berlin;

    impl Clock for Berlin {
        fn offset(&self, at: i64) -> i32 {
            let year = time::civil(at, 0).year;
            let spring = time::days_from_civil(year, 3, nth_weekday(year, 3, -1, 6).unwrap())
                * 86_400
                + 3600;
            let autumn = time::days_from_civil(year, 10, nth_weekday(year, 10, -1, 6).unwrap())
                * 86_400
                + 3600;
            if (spring..autumn).contains(&at) { 7200 } else { 3600 }
        }

        fn zone(&self) -> String {
            "Europe/Berlin".into()
        }
    }

    fn utc(year: i64, month: u32, day: u32, hour: u32, minute: u32) -> i64 {
        time::days_from_civil(year, month, day) * 86_400
            + i64::from(hour) * 3600
            + i64::from(minute) * 60
    }

    fn calendar(events: &str) -> String {
        format!(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Yandex LLC//Yandex Calendar//EN\r\n\
             BEGIN:VTIMEZONE\r\nTZID:Europe/Moscow\r\nTZURL:http://tzurl.org/zoneinfo/Europe/Moscow\r\n\
             BEGIN:STANDARD\r\nTZOFFSETFROM:+0300\r\nTZOFFSETTO:+0300\r\nTZNAME:MSK\r\n\
             DTSTART:20141026T020000\r\nEND:STANDARD\r\nEND:VTIMEZONE\r\n\
             BEGIN:VTIMEZONE\r\nTZID:Europe/Berlin\r\n\
             BEGIN:DAYLIGHT\r\nTZOFFSETFROM:+0100\r\nTZOFFSETTO:+0200\r\nDTSTART:19810329T020000\r\n\
             RRULE:FREQ=YEARLY;BYMONTH=3;BYDAY=-1SU\r\nEND:DAYLIGHT\r\n\
             BEGIN:STANDARD\r\nTZOFFSETFROM:+0200\r\nTZOFFSETTO:+0100\r\nDTSTART:19961027T030000\r\n\
             RRULE:FREQ=YEARLY;BYMONTH=10;BYDAY=-1SU\r\nEND:STANDARD\r\nEND:VTIMEZONE\r\n\
             {events}END:VCALENDAR\r\n"
        )
    }

    fn read(events: &str, from: i64, to: i64) -> Vec<Meeting> {
        meetings(
            &calendar(events),
            "Мои события",
            &["egor@example.ru".into()],
            from,
            to,
            &Moscow,
        )
    }

    #[test]
    fn unfolds_and_unescapes() {
        let text = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nSUMMARY:Дейли\\, кофе\\; и\\n\r\n  \
                    план\r\nDESCRIPTION;LANGUAGE=ru:Ссылка: https://telemost.yandex.ru/j/1234\r\n\
                    ATTENDEE;CN=\"Иванов; Пётр\";PARTSTAT=ACCEPTED:mailto:p@example.ru\r\n\
                    END:VEVENT\r\nEND:VCALENDAR\r\n";
        let roots = parse(text);
        let event = &roots[0].children[0];
        assert_eq!(event.property("SUMMARY").unwrap().text(), "Дейли, кофе; и\n план");
        let attendee = event.property("ATTENDEE").unwrap();
        assert_eq!(attendee.param("CN"), Some("Иванов; Пётр"));
        assert_eq!(attendee.value, "mailto:p@example.ru");
        assert_eq!(
            find_link(event).as_deref(),
            Some("https://telemost.yandex.ru/j/1234")
        );
    }

    #[test]
    fn reads_times_of_every_kind() {
        let day = utc(2026, 10, 14, 0, 0) - 3 * 3600;
        let events = "BEGIN:VEVENT\r\nUID:zoned\r\nSUMMARY:Дейли\r\n\
                      DTSTART;TZID=Europe/Moscow:20261014T110000\r\n\
                      DTEND;TZID=Europe/Moscow:20261014T111500\r\nEND:VEVENT\r\n\
                      BEGIN:VEVENT\r\nUID:utc\r\nSUMMARY:Созвон\r\nDTSTART:20261014T120000Z\r\n\
                      DURATION:PT30M\r\nEND:VEVENT\r\n\
                      BEGIN:VEVENT\r\nUID:floating\r\nSUMMARY:Обед\r\nDTSTART:20261014T130000\r\n\
                      DTEND:20261014T140000\r\nEND:VEVENT\r\n\
                      BEGIN:VEVENT\r\nUID:allday\r\nSUMMARY:ДР\r\nDTSTART;VALUE=DATE:20261014\r\n\
                      DTEND;VALUE=DATE:20261015\r\nEND:VEVENT\r\n";
        let found = read(events, day, day + 86_400);
        let by = |uid: &str| found.iter().find(|meeting| meeting.uid == uid).unwrap();
        assert_eq!(by("zoned").start, utc(2026, 10, 14, 8, 0));
        assert_eq!(by("zoned").end, utc(2026, 10, 14, 8, 15));
        assert_eq!(by("utc").start, utc(2026, 10, 14, 12, 0));
        assert_eq!(by("utc").end, utc(2026, 10, 14, 12, 30));
        assert_eq!(by("floating").start, utc(2026, 10, 14, 10, 0));
        assert!(by("allday").all_day);
        assert_eq!(by("allday").start, day);
        assert_eq!(by("allday").end, day + 86_400);
        // All-day first, then by start.
        assert_eq!(found[0].uid, "allday");
    }

    #[test]
    fn a_zone_with_summer_time() {
        let roots = parse(&calendar(""));
        let zones = Zones::of(&roots[0]);
        let summer = When::Local(
            Wall {
                year: 2026,
                month: 7,
                day: 1,
                hour: 10,
                minute: 0,
                second: 0,
            },
            Some("Europe/Berlin".into()),
        );
        assert_eq!(zones.utc(&summer, &Moscow), utc(2026, 7, 1, 8, 0));
        let winter = When::Local(
            Wall {
                year: 2026,
                month: 12,
                day: 1,
                hour: 10,
                minute: 0,
                second: 0,
            },
            Some("Europe/Berlin".into()),
        );
        assert_eq!(zones.utc(&winter, &Moscow), utc(2026, 12, 1, 9, 0));
        // A zone the calendar doesn't define: the table, else the user's.
        let novosibirsk = When::Local(
            Wall {
                year: 2026,
                month: 12,
                day: 1,
                hour: 10,
                minute: 0,
                second: 0,
            },
            Some("Asia/Novosibirsk".into()),
        );
        assert_eq!(zones.utc(&novosibirsk, &Moscow), utc(2026, 12, 1, 3, 0));
        let unknown = When::Local(
            Wall {
                year: 2026,
                month: 12,
                day: 1,
                hour: 10,
                minute: 0,
                second: 0,
            },
            Some("Mars/Olympus".into()),
        );
        assert_eq!(zones.utc(&unknown, &Berlin), utc(2026, 12, 1, 9, 0));
    }

    #[test]
    fn expands_a_daily_meeting_with_exceptions() {
        // Daily at 11:00 Moscow since a week before; tomorrow's is excluded, today's moved to
        // 12:00, the day after tomorrow's cancelled.
        let events = "BEGIN:VEVENT\r\nUID:daily\r\nSUMMARY:Дейли\r\n\
                      DTSTART;TZID=Europe/Moscow:20261007T110000\r\n\
                      DTEND;TZID=Europe/Moscow:20261007T111500\r\nRRULE:FREQ=DAILY\r\n\
                      EXDATE;TZID=Europe/Moscow:20261015T110000\r\n\
                      X-TELEMOST-CONFERENCE:https://telemost.yandex.ru/j/42\r\nEND:VEVENT\r\n\
                      BEGIN:VEVENT\r\nUID:daily\r\nSUMMARY:Дейли (перенос)\r\n\
                      RECURRENCE-ID;TZID=Europe/Moscow:20261014T110000\r\n\
                      DTSTART;TZID=Europe/Moscow:20261014T120000\r\n\
                      DTEND;TZID=Europe/Moscow:20261014T121500\r\nEND:VEVENT\r\n\
                      BEGIN:VEVENT\r\nUID:daily\r\nSUMMARY:Дейли\r\nSTATUS:CANCELLED\r\n\
                      RECURRENCE-ID;TZID=Europe/Moscow:20261016T110000\r\n\
                      DTSTART;TZID=Europe/Moscow:20261016T110000\r\n\
                      DTEND;TZID=Europe/Moscow:20261016T111500\r\nEND:VEVENT\r\n";
        let from = utc(2026, 10, 13, 21, 0);
        let found = read(events, from, from + 4 * 86_400);
        let starts: Vec<(i64, &str)> = found
            .iter()
            .map(|meeting| (meeting.start, meeting.title.as_str()))
            .collect();
        assert_eq!(
            starts,
            vec![
                (utc(2026, 10, 14, 9, 0), "Дейли (перенос)"),
                (utc(2026, 10, 17, 8, 0), "Дейли"),
            ]
        );
        assert_eq!(
            found[1].link.as_deref(),
            Some("https://telemost.yandex.ru/j/42")
        );
        assert_eq!(found[1].end - found[1].start, 15 * 60);
    }

    #[test]
    fn expands_weekly_monthly_and_limits() {
        let from = utc(2026, 10, 12, 0, 0);
        let to = from + 21 * 86_400;
        // Mondays and Thursdays, every other week, from Monday the 12th; three in all.
        let weekly = "BEGIN:VEVENT\r\nUID:w\r\nSUMMARY:Ревью\r\nDTSTART:20261012T070000Z\r\n\
                      DTEND:20261012T080000Z\r\nRRULE:FREQ=WEEKLY;INTERVAL=2;BYDAY=MO,TH;COUNT=3\r\n\
                      END:VEVENT\r\n";
        let found = read(weekly, from, to);
        let starts: Vec<i64> = found.iter().map(|meeting| meeting.start).collect();
        assert_eq!(
            starts,
            vec![
                utc(2026, 10, 12, 7, 0),
                utc(2026, 10, 15, 7, 0),
                utc(2026, 10, 26, 7, 0)
            ]
        );
        // Weekdays until the 16th (inclusive, a date).
        let weekdays = "BEGIN:VEVENT\r\nUID:d\r\nSUMMARY:Стендап\r\n\
                        DTSTART;TZID=Europe/Moscow:20261009T100000\r\nDURATION:PT15M\r\n\
                        RRULE:FREQ=DAILY;BYDAY=MO,TU,WE,TH,FR;UNTIL=20261016\r\nEND:VEVENT\r\n";
        let found = read(weekdays, from, to);
        assert_eq!(found.len(), 5);
        assert_eq!(found[0].start, utc(2026, 10, 12, 7, 0));
        assert_eq!(found[4].start, utc(2026, 10, 16, 7, 0));
        // The second Tuesday of every month.
        let monthly = "BEGIN:VEVENT\r\nUID:m\r\nSUMMARY:Планирование\r\n\
                       DTSTART:20260908T090000Z\r\nDTEND:20260908T100000Z\r\n\
                       RRULE:FREQ=MONTHLY;BYDAY=2TU\r\nEND:VEVENT\r\n";
        let found = read(monthly, utc(2026, 10, 1, 0, 0), utc(2026, 12, 1, 0, 0));
        let starts: Vec<i64> = found.iter().map(|meeting| meeting.start).collect();
        assert_eq!(starts, vec![utc(2026, 10, 13, 9, 0), utc(2026, 11, 10, 9, 0)]);
    }

    #[test]
    fn leaves_out_the_declined_and_the_cancelled() {
        let events = "BEGIN:VEVENT\r\nUID:a\r\nSUMMARY:Отказался\r\nDTSTART:20261014T090000Z\r\n\
                      DTEND:20261014T100000Z\r\n\
                      ATTENDEE;PARTSTAT=DECLINED;CN=Егор:mailto:EGOR@example.ru\r\nEND:VEVENT\r\n\
                      BEGIN:VEVENT\r\nUID:b\r\nSUMMARY:Отменена\r\nSTATUS:CANCELLED\r\n\
                      DTSTART:20261014T110000Z\r\nDTEND:20261014T120000Z\r\nEND:VEVENT\r\n\
                      BEGIN:VEVENT\r\nUID:c\r\nSUMMARY:Иду\r\nDTSTART:20261014T130000Z\r\n\
                      DTEND:20261014T140000Z\r\nLOCATION:Переговорка «Нева» https://example.com/room.\r\n\
                      ATTENDEE;PARTSTAT=DECLINED:mailto:someone@example.ru\r\nEND:VEVENT\r\n";
        let from = utc(2026, 10, 14, 0, 0);
        let found = read(events, from, from + 86_400);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].title, "Иду");
        assert_eq!(found[0].link.as_deref(), Some("https://example.com/room"));
    }

    #[test]
    fn server_expanded_instances() {
        // `expand` on the server: instances with RECURRENCE-ID, in UTC, no master.
        let events = "BEGIN:VEVENT\r\nUID:s\r\nSUMMARY:Дейли\r\nRECURRENCE-ID:20261014T080000Z\r\n\
                      DTSTART:20261014T080000Z\r\nDTEND:20261014T081500Z\r\nEND:VEVENT\r\n\
                      BEGIN:VEVENT\r\nUID:s\r\nSUMMARY:Дейли\r\nRECURRENCE-ID:20261015T080000Z\r\n\
                      DTSTART:20261015T080000Z\r\nDTEND:20261015T081500Z\r\nEND:VEVENT\r\n";
        let from = utc(2026, 10, 14, 0, 0);
        let found = read(events, from, from + 2 * 86_400);
        assert_eq!(found.len(), 2);
        assert_ne!(found[0].key(), found[1].key());
    }

    #[test]
    fn durations_and_links() {
        assert_eq!(parse_duration("PT15M"), Some(900));
        assert_eq!(parse_duration("P1DT2H"), Some(93_600));
        assert_eq!(parse_duration("P1W"), Some(604_800));
        assert_eq!(parse_duration("-PT5M"), Some(-300));
        assert_eq!(parse_duration("P1X"), None);
        assert_eq!(
            urls_in("Зум (https://zoom.us/j/1), и ещё http://a.b/c."),
            vec!["https://zoom.us/j/1".to_string(), "http://a.b/c".to_string()]
        );
        assert!(urls_in("no links, httpx").is_empty());
    }
}
