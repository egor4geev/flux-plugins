//! Every field of a task, pure so tests read it (the author, 2026-10-15: «показываются не все
//! параметры задачи»). The card's main fields come first ([`super::facts::card_fields`]); after them,
//! in the same aligned list, all the others the task has, as Tracker's own issue page lists them:
//! the system fields by Tracker's categories and order, then the queue's own (local) fields — each
//! named as Tracker names it in the interface language (`GET /v3/fields`,
//! `GET /v3/queues/{key}/localFields`), its value read by its type. Empty fields are left out. Those
//! simple to change — a line, a number, a date, a person, one value of a list — can be changed from
//! the card, as the main ones are.

use flux_plugin_api::view::Tone;
#[cfg(not(test))]
use flux_plugin_api::tr;
use serde_json::Value;

#[cfg(test)]
use super::facts::tr;
use super::facts::{Field, points};
use super::model::{FieldMeta, Issue};
use crate::time;

/// Fields the card shows elsewhere: in its header, its main fields, its description, its tabs.
const SHOWN_ELSEWHERE: &[&str] = &[
    "self",
    "id",
    "key",
    "version",
    "summary",
    "description",
    "status",
    "statusType",
    "type",
    "priority",
    "assignee",
    "createdBy",
    "queue",
    "deadline",
    "sprint",
    "parent",
    "createdAt",
    "updatedAt",
    "tags",
    "components",
    "storyPoints",
    "followers",
    "pendingReplyFrom",
    "commentWithoutExternalMessageCount",
    "commentWithExternalMessageCount",
];

/// Tracker's bookkeeping, not a task's parameters: ids of boards, the previous status and queue,
/// who voted, the user's own star, the checklist's items, SLA timers.
const BOOKKEEPING: &[&str] = &[
    "boards",
    "favorite",
    "unique",
    "aliases",
    "previousStatus",
    "previousStatusLastAssignee",
    "previousQueue",
    "lastQueue",
    "votedBy",
    "lastCommentUpdatedAt",
    "statusStartTime",
    "updatedBy",
    "checklistItems",
    "attachments",
    "sla",
    "possibleSpam",
];

/// Counts that say nothing while they're zero.
const COUNTS: &[&str] = &["votes", "checklistDone", "checklistTotal"];

/// How a field is changed from the card.
#[derive(Debug, Clone, PartialEq)]
pub enum EditKind {
    /// A line of text.
    Text,
    /// Several lines.
    Multiline,
    /// A number; `whole` — an integer.
    Number { whole: bool },
    /// A day: quick choices, a date of one's own, none.
    Date,
    /// A person of the organization.
    User,
    /// One value of a fixed list.
    Choice(Vec<String>),
}

/// How the field `meta` describes can be changed from the card; none — it can't (read-only, a
/// list of values, a reference, a duration).
pub fn edit_kind(meta: &FieldMeta) -> Option<EditKind> {
    if meta.readonly {
        return None;
    }
    let options = meta.options();
    match meta.schema.kind.as_str() {
        "array" => None,
        _ if !options.is_empty() => Some(EditKind::Choice(options)),
        "string" => Some(EditKind::Text),
        "text" => Some(EditKind::Multiline),
        "integer" => Some(EditKind::Number { whole: true }),
        "float" | "number" => Some(EditKind::Number { whole: false }),
        "date" => Some(EditKind::Date),
        "user" => Some(EditKind::User),
        _ => None,
    }
}

/// The fields of the task the main list doesn't show, in order: the system fields by their
/// category (the categories in the order of their first field), then the queue's own fields, then
/// fields Tracker didn't describe (named by their ids); within a category — Tracker's order, then
/// the name. Empty fields, and zero counts, are left out.
pub fn other_fields(
    issue: &Issue,
    metas: &[FieldMeta],
    now: i64,
    offset: i32,
    language: &str,
) -> Vec<Field> {
    // (category's order, category, field's order, field)
    let mut found: Vec<(i64, String, i64, Field)> = Vec::new();
    let mut values: Vec<(&str, &Value)> = issue
        .other
        .iter()
        .map(|(key, value)| (key.as_str(), value))
        .collect();
    let resolution = issue
        .resolution
        .as_ref()
        .map(|resolution| Value::String(resolution.display.clone()));
    if let Some(resolution) = &resolution {
        values.push(("resolution", resolution));
    }
    for (key, value) in values {
        if SHOWN_ELSEWHERE.contains(&key) || BOOKKEEPING.contains(&key) {
            continue;
        }
        if COUNTS.contains(&key) && value.as_f64() == Some(0.0) {
            continue;
        }
        let meta = find_meta(metas, key);
        let Some(text) = field_text(value, meta, now, offset, language) else {
            continue;
        };
        let name = meta.map(FieldMeta::display).unwrap_or_else(|| humanize(key));
        // An estimate or time spent is a duration: changed in Tracker, not as a line here.
        let duration = value
            .as_str()
            .is_some_and(|text| duration_text(text, "en").is_some());
        let editable = meta.and_then(edit_kind).is_some() && !duration;
        let (category_order, category) = match meta {
            Some(meta) if meta.is_local() => (i64::MAX - 1, String::new()),
            Some(meta) => match &meta.category {
                Some(category) if !category.display.is_empty() => {
                    (category_order(metas, &category.display), category.display.clone())
                }
                _ => (i64::MAX - 2, String::new()),
            },
            None => (i64::MAX, String::new()),
        };
        let order = meta.and_then(|meta| meta.order).unwrap_or(i64::MAX);
        found.push((
            category_order,
            category,
            order,
            Field {
                id: key.to_string(),
                name,
                value: text,
                tone: Tone::Normal,
                editable,
            },
        ));
    }
    found.sort_by(|a, b| {
        (a.0, &a.1, a.2, a.3.name.to_lowercase()).cmp(&(b.0, &b.1, b.2, b.3.name.to_lowercase()))
    });
    found.into_iter().map(|(_, _, _, field)| field).collect()
}

/// The metadata of the field `key` of a task: by its id, or — a local field — by its own key after
/// the queue's id.
pub fn find_meta<'a>(metas: &'a [FieldMeta], key: &str) -> Option<&'a FieldMeta> {
    metas.iter().find(|meta| meta.id == key).or_else(|| {
        let (_, own) = key.split_once("--")?;
        metas
            .iter()
            .find(|meta| meta.is_local() && meta.key.as_deref() == Some(own))
    })
}

/// A category's place: the least order of its fields.
fn category_order(metas: &[FieldMeta], category: &str) -> i64 {
    metas
        .iter()
        .filter(|meta| {
            !meta.is_local()
                && meta
                    .category
                    .as_ref()
                    .is_some_and(|known| known.display == category)
        })
        .filter_map(|meta| meta.order)
        .min()
        .unwrap_or(i64::MAX - 3)
}

/// A field Tracker didn't describe, named by its id: "qaEngineer" → "Qa engineer".
fn humanize(key: &str) -> String {
    let own = key.rsplit("--").next().unwrap_or(key);
    let mut words = String::new();
    for (index, c) in own.chars().enumerate() {
        if c.is_uppercase() && index > 0 {
            words.push(' ');
            words.extend(c.to_lowercase());
        } else if c == '_' || c == '-' {
            words.push(' ');
        } else if index == 0 {
            words.extend(c.to_uppercase());
        } else {
            words.push(c);
        }
    }
    words
}

/// A field's value as the card shows it, by its type: a person's or a reference's name, a list
/// joined, a day, a moment, a duration, a number; none for an empty value.
pub fn field_text(
    value: &Value,
    meta: Option<&FieldMeta>,
    now: i64,
    offset: i32,
    language: &str,
) -> Option<String> {
    let kind = meta.map(|meta| meta.schema.kind.as_str()).unwrap_or("");
    let text = match value {
        Value::Null => return None,
        Value::Bool(on) => {
            if *on {
                tr("Yes")
            } else {
                tr("No")
            }
        }
        Value::Number(number) => number.as_f64().map(points).unwrap_or_default(),
        Value::String(text) => string_text(text, kind, now, offset, language)?,
        Value::Array(items) => {
            let texts: Vec<String> = items
                .iter()
                .filter_map(|item| match item {
                    Value::String(text) => string_text(text, "", now, offset, language),
                    other => field_text(other, None, now, offset, language),
                })
                .collect();
            texts.join(", ")
        }
        Value::Object(object) => reference_text(object)?,
    };
    (!text.trim().is_empty()).then_some(text)
}

/// A string value: a day, a moment, a duration, or text on one line.
fn string_text(text: &str, kind: &str, now: i64, offset: i32, language: &str) -> Option<String> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let this_year = time::civil(now, offset).year;
    if let Some((year, month, day)) = time::parse_date(text).filter(|_| text.len() == 10) {
        return Some(time::day_month(year, month, day, this_year, language));
    }
    if (matches!(kind, "datetime" | "date") || text.contains('T'))
        && let Some(at) = time::parse_iso(text)
    {
        return Some(time::moment(at, now, offset, language));
    }
    if let Some(duration) = duration_text(text, language) {
        return Some(duration);
    }
    Some(text.split_whitespace().collect::<Vec<_>>().join(" "))
}

/// A reference: a task ("TRACK-100 · New sign-in"), else its name, key or id.
fn reference_text(object: &serde_json::Map<String, Value>) -> Option<String> {
    let text = |name: &str| match object.get(name) {
        Some(Value::String(text)) if !text.trim().is_empty() => Some(text.trim().to_string()),
        Some(Value::Number(number)) => Some(number.to_string()),
        _ => None,
    };
    if let (Some(key), Some(display)) = (text("key"), text("display"))
        && crate::text::is_key(&key)
        && key != display
    {
        return Some(format!("{key} · {display}"));
    }
    text("display")
        .or_else(|| text("name"))
        .or_else(|| text("key"))
        .or_else(|| text("id"))
}

/// An ISO 8601 duration as Tracker keeps estimates and time spent ("P1W2DT3H30M") in short words:
/// "1w 2d 3h 30m" ("1нед 2д 3ч 30мин"); none when it isn't one.
pub fn duration_text(iso: &str, language: &str) -> Option<String> {
    let rest = iso.strip_prefix('P')?;
    let (date, clock) = match rest.split_once('T') {
        Some((date, clock)) => (date, clock),
        None => (rest, ""),
    };
    let units = if language == "ru" {
        ["нед", "д", "ч", "мин", "с"]
    } else {
        ["w", "d", "h", "m", "s"]
    };
    let mut parts = Vec::new();
    let mut read = |part: &str, letters: &[(char, usize)]| -> Option<()> {
        let mut number = String::new();
        for c in part.chars() {
            if c.is_ascii_digit() || c == '.' || c == ',' {
                number.push(c);
                continue;
            }
            let (_, unit) = letters.iter().find(|(letter, _)| *letter == c)?;
            if number.is_empty() {
                return None;
            }
            // "0D" says nothing: a zero part is left out.
            if number.chars().any(|c| c.is_ascii_digit() && c != '0') {
                parts.push(format!("{}{}", number.replace(',', "."), units[*unit]));
            }
            number.clear();
        }
        number.is_empty().then_some(())
    };
    read(date, &[('W', 0), ('D', 1)])?;
    read(clock, &[('H', 2), ('M', 3), ('S', 4)])?;
    if parts.is_empty() {
        // "PT0S": nothing spent.
        return (rest.len() > 1).then(|| format!("0{}", units[3]));
    }
    Some(parts.join(" "))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn meta(value: Value) -> FieldMeta {
        serde_json::from_value(value).unwrap()
    }

    fn metas() -> Vec<FieldMeta> {
        [
            json!({"id": "start", "name": "Начало", "schema": {"type": "date"}, "order": 40,
                   "category": {"id": "c-time", "display": "Учёт времени"}}),
            json!({"id": "originalEstimation", "name": "Первоначальная оценка",
                   "schema": {"type": "string"}, "order": 41,
                   "category": {"id": "c-time", "display": "Учёт времени"}}),
            json!({"id": "spent", "name": "Затрачено", "schema": {"type": "string"},
                   "readonly": true, "order": 43,
                   "category": {"id": "c-time", "display": "Учёт времени"}}),
            json!({"id": "qaEngineer", "name": "QA-инженер", "schema": {"type": "user"},
                   "order": 12, "category": {"id": "c-system", "display": "Системные"}}),
            json!({"id": "fixVersions", "name": "Исправить в версиях",
                   "schema": {"type": "array", "items": "version"}, "order": 14,
                   "category": {"id": "c-system", "display": "Системные"}}),
            json!({"id": "votes", "name": "Голоса", "schema": {"type": "integer"}, "readonly": true,
                   "order": 30, "category": {"id": "c-system", "display": "Системные"}}),
            json!({"id": "resolution", "name": "Резолюция", "schema": {"type": "resolution"},
                   "order": 5, "category": {"id": "c-system", "display": "Системные"}}),
            json!({"type": "local", "id": "64f1c0a2e4b0a1b2c3d4e5f6--team", "key": "team",
                   "name": "Команда", "schema": {"type": "string"}, "order": 102,
                   "optionsProvider": {"type": "FixedListOptionsProvider",
                                       "values": ["Платформа", "Платежи"]}}),
            json!({"type": "local", "id": "64f1c0a2e4b0a1b2c3d4e5f6--customers", "key": "customers",
                   "name": "Клиентов затронуто", "schema": {"type": "integer"}, "order": 101}),
        ]
        .into_iter()
        .map(meta)
        .collect()
    }

    /// 2026-10-15 12:00 in Moscow.
    const NOW: i64 = 20_741 * 86_400 + 9 * 3600;
    const MSK: i32 = 3 * 3600;

    fn issue(value: Value) -> Issue {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn a_task_keeps_every_field_it_has() {
        let task = issue(json!({
            "key": "TRACK-142",
            "summary": "Ошибка входа",
            "start": "2026-10-12",
            "qaEngineer": {"id": "17", "display": "Олег Смирнов"},
            "64f1c0a2e4b0a1b2c3d4e5f6--team": "Платформа",
        }));
        assert_eq!(task.summary, "Ошибка входа");
        assert_eq!(task.other["start"], json!("2026-10-12"));
        assert_eq!(task.other["64f1c0a2e4b0a1b2c3d4e5f6--team"], json!("Платформа"));
        // What the card reads by name isn't kept twice.
        assert!(!task.other.contains_key("summary"));
    }

    #[test]
    fn the_other_fields_by_category_then_the_queues_own() {
        let task = issue(json!({
            "key": "TRACK-142",
            "queue": {"id": "1", "key": "TRACK", "display": "Трекер"},
            "summary": "Ошибка входа",
            "assignee": {"id": "1", "display": "Егор"},
            "resolution": {"id": "fixed", "key": "fixed", "display": "Решён"},
            "start": "2026-10-12",
            "originalEstimation": "P3D",
            "spent": "PT6H30M",
            "qaEngineer": {"id": "17", "display": "Олег Смирнов"},
            "fixVersions": [{"id": "1", "display": "2.4"}, {"id": "2", "display": "2.5"}],
            "votes": 0,
            "favorite": false,
            "64f1c0a2e4b0a1b2c3d4e5f6--team": "Платформа",
            "64f1c0a2e4b0a1b2c3d4e5f6--customers": 120,
            "lastQueue": {"key": "WEB"},
            "unknownThing": "Значение",
            "emptyText": "",
            "emptyList": [],
        }));
        let fields = other_fields(&task, &metas(), NOW, MSK, "ru");
        let shown: Vec<(&str, &str)> = fields
            .iter()
            .map(|field| (field.name.as_str(), field.value.as_str()))
            .collect();
        assert_eq!(
            shown,
            [
                // The system fields by category — Системные (from order 5), Учёт времени (40) —
                // then the queue's own, then what Tracker didn't describe.
                ("Резолюция", "Решён"),
                ("QA-инженер", "Олег Смирнов"),
                ("Исправить в версиях", "2.4, 2.5"),
                ("Начало", "12 окт"),
                ("Первоначальная оценка", "3д"),
                ("Затрачено", "6ч 30мин"),
                ("Клиентов затронуто", "120"),
                ("Команда", "Платформа"),
                ("Unknown thing", "Значение"),
            ]
        );
        // A field's id is its key in the task.
        assert_eq!(fields[7].id, "64f1c0a2e4b0a1b2c3d4e5f6--team");
        // What can be changed here: a person, a day, a line, a number, one of a list.
        let editable: Vec<bool> = fields.iter().map(|field| field.editable).collect();
        assert_eq!(editable, [false, true, false, true, false, false, true, true, false]);
    }

    #[test]
    fn how_fields_change() {
        let metas = metas();
        let kind = |id: &str| find_meta(&metas, id).and_then(edit_kind);
        assert_eq!(kind("start"), Some(EditKind::Date));
        assert_eq!(kind("qaEngineer"), Some(EditKind::User));
        assert_eq!(kind("fixVersions"), None);
        assert_eq!(kind("spent"), None);
        assert_eq!(
            kind("64f1c0a2e4b0a1b2c3d4e5f6--team"),
            Some(EditKind::Choice(vec!["Платформа".into(), "Платежи".into()]))
        );
        assert_eq!(
            kind("64f1c0a2e4b0a1b2c3d4e5f6--customers"),
            Some(EditKind::Number { whole: true })
        );
        // A local field found by its own key when the queue's id differs in the metadata.
        assert!(find_meta(&metas, "0000--team").is_some());
        assert_eq!(find_meta(&metas, "team"), None);
    }

    #[test]
    fn values_by_their_type() {
        let text = |value: Value| field_text(&value, None, NOW, MSK, "en");
        assert_eq!(text(json!("2026-10-20")), Some("20 Oct".into()));
        assert_eq!(text(json!("2027-01-10")), Some("10 Jan 2027".into()));
        assert_eq!(text(json!("2026-10-15T07:00:00.000+0000")), Some("today, 10:00".into()));
        assert_eq!(text(json!("P1W2DT3H30M")), Some("1w 2d 3h 30m".into()));
        assert_eq!(text(json!(2.5)), Some("2.5".into()));
        assert_eq!(text(json!(true)), Some("Yes".into()));
        assert_eq!(text(json!("  first line\nsecond  ")), Some("first line second".into()));
        assert_eq!(
            text(json!({"id": "1", "key": "TRACK-100", "display": "Новая авторизация"})),
            Some("TRACK-100 · Новая авторизация".into())
        );
        assert_eq!(text(json!({"id": "5", "display": "Вход 2.0"})), Some("Вход 2.0".into()));
        assert_eq!(
            text(json!([{"id": "1", "display": "Анна"}, {"id": "2", "display": "Олег"}])),
            Some("Анна, Олег".into())
        );
        assert_eq!(text(json!(["sso", "auth"])), Some("sso, auth".into()));
        assert_eq!(text(json!(null)), None);
        assert_eq!(text(json!("")), None);
        assert_eq!(text(json!([])), None);
        assert_eq!(text(json!({})), None);
    }

    #[test]
    fn durations_in_short_words() {
        assert_eq!(duration_text("P3D", "en").as_deref(), Some("3d"));
        assert_eq!(duration_text("PT6H30M", "ru").as_deref(), Some("6ч 30мин"));
        assert_eq!(duration_text("P1W", "ru").as_deref(), Some("1нед"));
        assert_eq!(duration_text("PT0S", "en").as_deref(), Some("0m"));
        assert_eq!(duration_text("PT1.5H", "en").as_deref(), Some("1.5h"));
        assert_eq!(duration_text("Platform", "en"), None);
        assert_eq!(duration_text("P", "en"), None);
        assert_eq!(duration_text("P3X", "en"), None);
    }

    #[test]
    fn unknown_fields_are_named_by_their_id() {
        assert_eq!(humanize("qaEngineer"), "Qa engineer");
        assert_eq!(humanize("6054ae5e2b8f8e3b4ff6c5ac--release_date"), "Release date");
    }
}
