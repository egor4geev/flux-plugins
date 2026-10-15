//! CalDAV with Yandex Calendar (`https://caldav.yandex.ru`): the requests' bodies, and reading
//! the answers (WebDAV `multistatus`). The way there: `PROPFIND /` for the user's principal →
//! `PROPFIND <principal>` for the calendar home → `PROPFIND <home>` (depth 1) for the calendars →
//! `REPORT <calendar>` (`calendar-query` over a time range, asking the server to expand
//! recurrences) for each calendar's events, as iCalendar.

use super::xml::{self, Element};
use crate::time;

/// Yandex's CalDAV server.
pub const BASE: &str = "https://caldav.yandex.ru";

/// `PROPFIND /` (depth 0): who the credentials are.
pub const PRINCIPAL_BODY: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<D:propfind xmlns:D="DAV:"><D:prop><D:current-user-principal/></D:prop></D:propfind>"#;

/// `PROPFIND <principal>` (depth 0): where the user's calendars are.
pub const HOME_BODY: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<D:propfind xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav"><D:prop><C:calendar-home-set/></D:prop></D:propfind>"#;

/// `PROPFIND <home>` (depth 1): the calendars, their names and colors.
pub const CALENDARS_BODY: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<D:propfind xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav" xmlns:A="http://apple.com/ns/ical/"><D:prop><D:displayname/><D:resourcetype/><C:supported-calendar-component-set/><A:calendar-color/></D:prop></D:propfind>"#;

/// `REPORT <calendar>` (depth 1): the events between `from` and `to` (seconds since 1970, UTC),
/// recurrences expanded by the server when it can.
pub fn events_body(from: i64, to: i64) -> String {
    let (from, to) = (stamp(from), stamp(to));
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<C:calendar-query xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav"><D:prop><D:getetag/><C:calendar-data><C:expand start="{from}" end="{to}"/></C:calendar-data></D:prop><C:filter><C:comp-filter name="VCALENDAR"><C:comp-filter name="VEVENT"><C:time-range start="{from}" end="{to}"/></C:comp-filter></C:comp-filter></C:filter></C:calendar-query>"#
    )
}

/// A UTC moment as CalDAV writes it: `20261014T070000Z`.
pub fn stamp(at: i64) -> String {
    let time = time::civil(at, 0);
    format!(
        "{:04}{:02}{:02}T{:02}{:02}{:02}Z",
        time.year, time.month, time.day, time.hour, time.minute, time.second
    )
}

/// A calendar of the user's.
#[derive(Debug, Clone, PartialEq)]
pub struct CalendarInfo {
    /// Its address: a path on the server.
    pub href: String,
    pub name: String,
}

/// The user's principal (`current-user-principal`), a path on the server.
pub fn principal(answer: &str) -> Result<Option<String>, String> {
    let root = xml::parse(answer)?;
    Ok(ok_props(&root)
        .into_iter()
        .find_map(|prop| prop.find("current-user-principal")?.find("href"))
        .map(|href| href.trimmed().to_string())
        .filter(|href| !href.is_empty()))
}

/// The calendar home (`calendar-home-set`), a path on the server.
pub fn home(answer: &str) -> Result<Option<String>, String> {
    let root = xml::parse(answer)?;
    Ok(ok_props(&root)
        .into_iter()
        .find_map(|prop| prop.find("calendar-home-set")?.find("href"))
        .map(|href| href.trimmed().to_string())
        .filter(|href| !href.is_empty()))
}

/// The calendars of a home: collections that are calendars and hold events (a calendar of tasks
/// only is left out); without a name — the last part of the path.
pub fn calendars(answer: &str) -> Result<Vec<CalendarInfo>, String> {
    let root = xml::parse(answer)?;
    let mut found = Vec::new();
    for response in root.children_named("response") {
        let Some(href) = response.child("href").map(|href| href.trimmed().to_string()) else {
            continue;
        };
        let props = response_props(response);
        let is_calendar = props.iter().any(|prop| {
            prop.child("resourcetype")
                .is_some_and(|kind| kind.has("calendar"))
        });
        if !is_calendar {
            continue;
        }
        let components: Vec<&str> = props
            .iter()
            .filter_map(|prop| prop.child("supported-calendar-component-set"))
            .flat_map(|set| set.children_named("comp"))
            .filter_map(|comp| comp.attribute("name"))
            .collect();
        if !components.is_empty()
            && !components
                .iter()
                .any(|name| name.eq_ignore_ascii_case("VEVENT"))
        {
            continue;
        }
        let name = props
            .iter()
            .find_map(|prop| prop.child("displayname"))
            .map(|name| name.trimmed().to_string())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| last_segment(&href));
        found.push(CalendarInfo { href, name });
    }
    Ok(found)
}

/// The iCalendar texts of a `REPORT`'s answer, one per event resource.
pub fn calendar_data(answer: &str) -> Result<Vec<String>, String> {
    let root = xml::parse(answer)?;
    Ok(ok_props(&root)
        .into_iter()
        .filter_map(|prop| prop.find("calendar-data"))
        .map(|data| data.text.trim().to_string())
        .filter(|text| !text.is_empty())
        .collect())
}

/// An address on the server: a path made absolute; a full URL kept.
pub fn absolute(href: &str) -> String {
    if href.starts_with("http://") || href.starts_with("https://") {
        href.to_string()
    } else {
        format!("{BASE}/{}", href.trim_start_matches('/'))
    }
}

/// The email a principal or a home names: `/principals/users/egor%40example.ru/` →
/// `egor@example.ru`.
pub fn email_of(href: &str) -> Option<String> {
    let segment = percent_decode(&last_segment(href));
    segment.contains('@').then_some(segment)
}

/// The `prop`s of every `propstat` whose status is 200 (or has none), of every response.
fn ok_props(root: &Element) -> Vec<&Element> {
    let responses: Vec<&Element> = if root.name == "response" {
        vec![root]
    } else {
        root.children_named("response").collect()
    };
    responses.into_iter().flat_map(response_props).collect()
}

fn response_props(response: &Element) -> Vec<&Element> {
    response
        .children_named("propstat")
        .filter(|propstat| {
            propstat
                .child("status")
                .is_none_or(|status| status.trimmed().contains(" 200"))
        })
        .filter_map(|propstat| propstat.child("prop"))
        .collect()
}

fn last_segment(href: &str) -> String {
    href.trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_string()
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && let Some(byte) = text
                .get(index + 1..index + 3)
                .and_then(|hex| u8::from_str_radix(hex, 16).ok())
        {
            out.push(byte);
            index += 3;
            continue;
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Shaped as Yandex's CalDAV answers.
    const PRINCIPAL: &str = r#"<?xml version='1.0' encoding='UTF-8'?>
<multistatus xmlns='DAV:'><response><href>/</href><propstat><status>HTTP/1.1 200 OK</status><prop><current-user-principal><href>/principals/users/egor%40example.ru/</href></current-user-principal></prop></propstat></response></multistatus>"#;
    const HOME: &str = r#"<?xml version='1.0' encoding='UTF-8'?>
<multistatus xmlns='DAV:'><response><href>/principals/users/egor%40example.ru/</href><propstat><status>HTTP/1.1 200 OK</status><prop><C:calendar-home-set xmlns:C='urn:ietf:params:xml:ns:caldav'><href>/calendars/egor%40example.ru/</href></C:calendar-home-set></prop></propstat></response></multistatus>"#;
    const CALENDARS: &str = r#"<?xml version='1.0' encoding='UTF-8'?>
<multistatus xmlns='DAV:' xmlns:C='urn:ietf:params:xml:ns:caldav'>
<response><href>/calendars/egor%40example.ru/</href><propstat><status>HTTP/1.1 200 OK</status><prop><resourcetype><collection/></resourcetype><displayname>egor</displayname></prop></propstat></response>
<response><href>/calendars/egor%40example.ru/events-default/</href><propstat><status>HTTP/1.1 200 OK</status><prop><resourcetype><collection/><C:calendar/></resourcetype><displayname>Мои события</displayname><C:supported-calendar-component-set><C:comp name='VEVENT'/></C:supported-calendar-component-set></prop></propstat><propstat><status>HTTP/1.1 404 Not Found</status><prop><calendar-color xmlns='http://apple.com/ns/ical/'/></prop></propstat></response>
<response><href>/calendars/egor%40example.ru/todos-default/</href><propstat><status>HTTP/1.1 200 OK</status><prop><resourcetype><collection/><C:calendar/></resourcetype><displayname>Дела</displayname><C:supported-calendar-component-set><C:comp name='VTODO'/></C:supported-calendar-component-set></prop></propstat></response>
<response><href>/calendars/egor%40example.ru/team/</href><propstat><status>HTTP/1.1 200 OK</status><prop><resourcetype><collection/><C:calendar/></resourcetype></prop></propstat></response>
</multistatus>"#;
    const EVENTS: &str = r#"<?xml version='1.0' encoding='UTF-8'?>
<D:multistatus xmlns:D='DAV:' xmlns:C='urn:ietf:params:xml:ns:caldav'><D:response><D:href>/calendars/egor%40example.ru/events-default/a.ics</D:href><D:propstat><D:status>HTTP/1.1 200 OK</D:status><D:prop><D:getetag>"1"</D:getetag><C:calendar-data>BEGIN:VCALENDAR&#13;
BEGIN:VEVENT&#13;
UID:a&#13;
SUMMARY:Дейли &amp; кофе&#13;
END:VEVENT&#13;
END:VCALENDAR&#13;
</C:calendar-data></D:prop></D:propstat></D:response></D:multistatus>"#;

    #[test]
    fn discovers_the_calendars() {
        let principal = principal(PRINCIPAL).unwrap().unwrap();
        assert_eq!(principal, "/principals/users/egor%40example.ru/");
        assert_eq!(email_of(&principal).as_deref(), Some("egor@example.ru"));
        let home = home(HOME).unwrap().unwrap();
        assert_eq!(home, "/calendars/egor%40example.ru/");
        assert_eq!(
            absolute(&home),
            "https://caldav.yandex.ru/calendars/egor%40example.ru/"
        );
        assert_eq!(absolute("https://other.example/x/"), "https://other.example/x/");
        let calendars = calendars(CALENDARS).unwrap();
        assert_eq!(
            calendars,
            vec![
                CalendarInfo {
                    href: "/calendars/egor%40example.ru/events-default/".into(),
                    name: "Мои события".into(),
                },
                CalendarInfo {
                    href: "/calendars/egor%40example.ru/team/".into(),
                    name: "team".into(),
                },
            ]
        );
    }

    #[test]
    fn reads_the_events() {
        let data = calendar_data(EVENTS).unwrap();
        assert_eq!(data.len(), 1);
        assert!(data[0].contains("SUMMARY:Дейли & кофе\r\n"));
        assert!(data[0].starts_with("BEGIN:VCALENDAR"));
    }

    #[test]
    fn asks_for_a_time_range() {
        // 2026-10-14 00:00 UTC … 2026-10-16 00:00 UTC.
        let from = time::days_from_civil(2026, 10, 14) * 86_400;
        let body = events_body(from, from + 2 * 86_400);
        assert!(body.contains(r#"<C:time-range start="20261014T000000Z" end="20261016T000000Z"/>"#));
        assert!(body.contains(r#"<C:expand start="20261014T000000Z""#));
    }
}
