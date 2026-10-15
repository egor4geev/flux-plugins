//! What a task's card says about it, pure so tests read it: the board's cards (their tags and
//! foot) and the Tracker card's fields. On a board a card answers "what is it, how urgent, who has
//! it": the priority unless it's the usual one, whether the task waits for the user's reply, a
//! deadline and how near it is, story points, the task's tags; the foot names the type and the
//! assignee — not the user, whose name would sit on every card of their board.

use flux_plugin_api::view::Tone;
#[cfg(not(test))]
use flux_plugin_api::{tr, trf};

use super::model::{Issue, Named};
use crate::time;

/// The translations are Flux's, asked of the host — in tests on this Mac there is no host: the
/// English text itself.
#[cfg(test)]
pub(super) fn tr(text: &str) -> String {
    text.to_string()
}

#[cfg(test)]
pub(super) fn trf(template: &str, args: &[&dyn std::fmt::Display]) -> String {
    args.iter()
        .enumerate()
        .fold(template.to_string(), |text, (index, arg)| {
            text.replace(&format!("{{{index}}}"), &arg.to_string())
        })
}

/// A day in the user's time: (year, month, day).
pub type Day = (i64, u32, u32);

/// The most tags a board's card shows: it is narrow, and the task's card has the rest.
const MAX_CARD_TAGS: usize = 4;
/// The most of the task's own tags a board's card shows.
const MAX_OWN_TAGS: usize = 2;

/// A field of the Tracker card: what names it (a property's id — the main editable ones are the ids
/// [`super::edit`] knows, the others the field's key in the task), its name, its value (empty —
/// none: a dash) and how loud it is, whether the user can change it there.
#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    pub id: String,
    pub name: String,
    pub value: String,
    pub tone: Tone,
    pub editable: bool,
}

/// Today in the user's time.
pub fn today(now: i64, offset: i32) -> Day {
    let civil = time::civil(now, offset);
    (civil.year, civil.month, civil.day)
}

/// The priority as a tag: the error tone for a blocker or a critical one, the warning tone for a
/// high one, muted for a minor one; none for the usual.
pub fn priority_tag(priority: &Named) -> Option<(String, Tone)> {
    if priority.display.is_empty() {
        return None;
    }
    let tone = match priority.key_or_id().to_ascii_lowercase().as_str() {
        "normal" | "medium" | "" => return None,
        "blocker" | "critical" | "highest" => Tone::Error,
        "high" | "major" => Tone::Warning,
        _ => Tone::Muted,
    };
    Some((priority.display.clone(), tone))
}

/// A deadline as a tag, "until 20 Oct": the error tone once it has passed, the warning tone today
/// and tomorrow, muted otherwise.
pub fn deadline_tag(deadline: &str, today: Day, language: &str) -> Option<(String, Tone)> {
    let (year, month, day) = time::parse_date(deadline)?;
    let left = time::days_from_civil(year, month, day)
        - time::days_from_civil(today.0, today.1, today.2);
    let tone = match left {
        ..0 => Tone::Error,
        0 | 1 => Tone::Warning,
        _ => Tone::Muted,
    };
    let date = time::day_month(year, month, day, today.0, language);
    Some((trf("until {0}", &[&date]), tone))
}

/// A deadline on the task's card, where the field's name says what it is: the date, and how near
/// it is when it's near — "16 Oct · tomorrow", "8 Oct · overdue" (the tones of [`deadline_tag`]).
pub fn deadline_value(deadline: &str, today: Day, language: &str) -> Option<(String, Tone)> {
    let (year, month, day) = time::parse_date(deadline)?;
    let left = time::days_from_civil(year, month, day)
        - time::days_from_civil(today.0, today.1, today.2);
    let date = time::day_month(year, month, day, today.0, language);
    Some(match left {
        ..0 => (trf("{0} · overdue", &[&date]), Tone::Error),
        0 => (trf("{0} · today", &[&date]), Tone::Warning),
        1 => (trf("{0} · tomorrow", &[&date]), Tone::Warning),
        _ => (date, Tone::Normal),
    })
}

/// Story points as Tracker counts them: "3", "0.5".
pub fn points(points: f64) -> String {
    if points.fract() == 0.0 {
        format!("{}", points as i64)
    } else {
        format!("{points}")
    }
}

/// The tags of a board's card, the loudest first: the priority, the user's reply awaited, the
/// deadline, story points, then a few of the task's own tags ("+N" for the rest) — at most
/// [`MAX_CARD_TAGS`].
pub fn card_tags(
    issue: &Issue,
    is_me: impl Fn(&Named) -> bool,
    today: Day,
    language: &str,
) -> Vec<(String, Tone)> {
    let mut tags = Vec::new();
    if let Some(priority) = issue.priority.as_ref().and_then(priority_tag) {
        tags.push(priority);
    }
    if issue.pending_reply_from.iter().any(&is_me) {
        tags.push((tr("Awaiting your reply"), Tone::Warning));
    }
    if let Some(deadline) = issue
        .deadline
        .as_deref()
        .and_then(|deadline| deadline_tag(deadline, today, language))
    {
        tags.push(deadline);
    }
    if let Some(story_points) = issue.story_points {
        tags.push((trf("{0} SP", &[&points(story_points)]), Tone::Muted));
    }
    let own: Vec<&String> = issue.tags.iter().filter(|tag| !tag.trim().is_empty()).collect();
    let room = MAX_CARD_TAGS.saturating_sub(tags.len());
    if room > 0 && !own.is_empty() {
        let shown = own.len().min(MAX_OWN_TAGS).min(room);
        let hidden = own.len() - shown;
        // The "+N" takes a place of its own: one tag fewer when the rest doesn't fit.
        let shown = if hidden > 0 && shown == room { shown - 1 } else { shown };
        tags.extend(own[..shown].iter().map(|tag| (tag.to_string(), Tone::Muted)));
        let hidden = own.len() - shown;
        if hidden > 0 {
            tags.push((format!("+{hidden}"), Tone::Dim));
        }
    }
    tags.truncate(MAX_CARD_TAGS);
    tags
}

/// The foot of a board's card: the type, and the assignee unless it's the user — "No assignee"
/// when nobody has the task.
pub fn card_foot(issue: &Issue, is_me: impl Fn(&Named) -> bool) -> String {
    let mut parts = Vec::new();
    if let Some(kind) = issue
        .kind
        .as_ref()
        .map(|kind| kind.display.as_str())
        .filter(|kind| !kind.is_empty())
    {
        parts.push(kind.to_string());
    }
    match &issue.assignee {
        Some(assignee) if is_me(assignee) => {}
        Some(assignee) if !assignee.display.is_empty() => parts.push(assignee.display.clone()),
        _ => parts.push(tr("No assignee")),
    }
    parts.join(" · ")
}

/// The fields of the Tracker card, a line each, in the order a task is read: who has it and how
/// urgent it is, then its planning, then where it lives and its story. The fields the user can
/// change are always there (empty — a dash, to fill in); the others only when the task has them.
pub fn card_fields(issue: &Issue, now: i64, offset: i32, language: &str) -> Vec<Field> {
    let today = today(now, offset);
    let named = |value: &Option<Named>| {
        value
            .as_ref()
            .map(|named| named.display.clone())
            .filter(|display| !display.is_empty())
    };
    let names = |list: &[Named]| {
        list.iter()
            .map(|named| named.display.as_str())
            .filter(|display| !display.is_empty())
            .collect::<Vec<_>>()
            .join(", ")
    };
    let moment = |at: &Option<String>| {
        at.as_deref()
            .and_then(time::parse_iso)
            .map(|at| time::moment(at, now, offset, language))
    };
    let editable = |id: &str, name: String, value: String, tone: Tone| Field {
        id: id.to_string(),
        name,
        value,
        tone,
        editable: true,
    };
    let shown = |id: &str, name: String, value: Option<String>| {
        value.filter(|value| !value.is_empty()).map(|value| Field {
            id: id.to_string(),
            name,
            value,
            tone: Tone::Normal,
            editable: false,
        })
    };
    let (assignee, assignee_tone) = match named(&issue.assignee) {
        Some(assignee) => (assignee, Tone::Normal),
        None => (tr("No assignee"), Tone::Dim),
    };
    let priority_tone = issue
        .priority
        .as_ref()
        .and_then(priority_tag)
        .map(|(_, tone)| tone)
        .filter(|tone| *tone != Tone::Muted)
        .unwrap_or(Tone::Normal);
    let (deadline, deadline_tone) = issue
        .deadline
        .as_deref()
        .and_then(|deadline| deadline_value(deadline, today, language))
        .unwrap_or((String::new(), Tone::Normal));
    let queue = issue.queue.as_ref().map(|queue| {
        match (&queue.key, queue.display.as_str()) {
            (Some(key), display) if !display.is_empty() && display != key => {
                format!("{display} · {key}")
            }
            (Some(key), _) => key.clone(),
            (None, display) => display.to_string(),
        }
    });
    [
        Some(editable("assignee", tr("Assignee"), assignee, assignee_tone)),
        Some(editable(
            "priority",
            tr("Priority"),
            named(&issue.priority).unwrap_or_default(),
            priority_tone,
        )),
        Some(editable("type", tr("Type"), named(&issue.kind).unwrap_or_default(), Tone::Normal)),
        Some(editable("deadline", tr("Deadline"), deadline, deadline_tone)),
        Some(editable(
            "story-points",
            tr("Story points"),
            issue.story_points.map(points).unwrap_or_default(),
            Tone::Normal,
        )),
        Some(editable("sprint", tr("Sprint"), issue.sprint_name(), Tone::Normal)),
        Some(editable("tags", tr("Tags"), issue.tags.join(", "), Tone::Normal)),
        Some(editable(
            "components",
            tr("Components"),
            names(&issue.components),
            Tone::Normal,
        )),
        shown("awaiting", tr("Awaiting reply from"), Some(names(&issue.pending_reply_from))),
        shown("queue", tr("Queue"), queue),
        shown("author", tr("Author"), named(&issue.created_by)),
        shown(
            "followers",
            tr("Followers"),
            (!issue.followers.is_empty()).then(|| issue.followers.len().to_string()),
        ),
        shown("created", tr("Created"), moment(&issue.created_at)),
        shown("updated", tr("Updated"), moment(&issue.updated_at)),
    ]
    .into_iter()
    .flatten()
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn named(id: &str, key: Option<&str>, display: &str) -> Named {
        Named {
            id: Value::String(id.into()),
            key: key.map(str::to_string),
            display: display.into(),
        }
    }

    fn me(named: &Named) -> bool {
        named.id == Value::String("8000000000000121".into())
    }

    /// A task as Tracker's search returns it.
    fn issue(value: Value) -> Issue {
        serde_json::from_value(value).unwrap()
    }

    const TODAY: Day = (2026, 10, 15);

    #[test]
    fn reads_the_fields_a_board_uses() {
        let task = issue(json!({
            "key": "DE-2560",
            "summary": "Ускорить таблицу",
            "tags": ["backend", "notification-service", "bff"],
            "components": [{"id": "11", "display": "АП 2.0"}],
            "storyPoints": 3,
            "followers": [{"id": "8000000000000121", "display": "Егор"}],
            "pendingReplyFrom": [{"id": "1130000063046524", "display": "Игорь"}],
            "commentWithoutExternalMessageCount": 11,
            "sla": null,
        }));
        assert_eq!(task.tags.len(), 3);
        assert_eq!(task.components[0].display, "АП 2.0");
        assert_eq!(task.story_points, Some(3.0));
        assert_eq!(task.comment_count, Some(11));
        assert_eq!(task.pending_reply_from[0].display, "Игорь");
        // A list that comes as null is empty, not an error that loses the whole board.
        let task = issue(json!({"key": "DE-1", "tags": null, "components": null}));
        assert!(task.tags.is_empty() && task.components.is_empty());
    }

    #[test]
    fn the_priority_is_loud_only_when_it_matters() {
        let tag = |key: &str, display: &str| priority_tag(&named("1", Some(key), display));
        assert_eq!(tag("normal", "Средний"), None);
        assert_eq!(tag("critical", "Критичный"), Some(("Критичный".into(), Tone::Error)));
        assert_eq!(tag("blocker", "Блокер"), Some(("Блокер".into(), Tone::Error)));
        assert_eq!(tag("high", "Высокий"), Some(("Высокий".into(), Tone::Warning)));
        assert_eq!(tag("minor", "Низкий"), Some(("Низкий".into(), Tone::Muted)));
    }

    #[test]
    fn a_deadline_is_louder_as_it_nears() {
        let tag = |date: &str| deadline_tag(date, TODAY, "en").unwrap();
        assert_eq!(tag("2026-10-20"), ("until 20 Oct".into(), Tone::Muted));
        assert_eq!(tag("2026-10-16").1, Tone::Warning);
        assert_eq!(tag("2026-10-15").1, Tone::Warning);
        assert_eq!(tag("2026-10-08"), ("until 8 Oct".into(), Tone::Error));
        assert_eq!(tag("2027-01-10").0, "until 10 Jan 2027");
        assert_eq!(deadline_tag("soon", TODAY, "en"), None);
    }

    #[test]
    fn a_cards_deadline_is_its_date() {
        let value = |date: &str| deadline_value(date, TODAY, "en").unwrap();
        assert_eq!(value("2026-10-20"), ("20 Oct".into(), Tone::Normal));
        assert_eq!(value("2026-10-16"), ("16 Oct · tomorrow".into(), Tone::Warning));
        assert_eq!(value("2026-10-15"), ("15 Oct · today".into(), Tone::Warning));
        assert_eq!(value("2026-10-08"), ("8 Oct · overdue".into(), Tone::Error));
        assert_eq!(value("2027-01-10").0, "10 Jan 2027");
    }

    #[test]
    fn a_cards_tags_loudest_first_and_few() {
        let task = issue(json!({
            "key": "DE-2560",
            "priority": {"id": "4", "key": "critical", "display": "Критичный"},
            "pendingReplyFrom": [{"id": "8000000000000121", "display": "Егор"}],
            "deadline": "2026-10-14",
            "storyPoints": 2.5,
            "tags": ["backend", "bff"],
        }));
        let tags = card_tags(&task, me, TODAY, "en");
        assert_eq!(
            tags,
            vec![
                ("Критичный".to_string(), Tone::Error),
                ("Awaiting your reply".to_string(), Tone::Warning),
                ("until 14 Oct".to_string(), Tone::Error),
                ("2.5 SP".to_string(), Tone::Muted),
            ]
        );
        // The usual task: just a couple of its tags and how many more there are.
        let task = issue(json!({
            "key": "DE-2561",
            "priority": {"id": "3", "key": "normal", "display": "Средний"},
            "pendingReplyFrom": [{"id": "1130000063046524", "display": "Игорь"}],
            "tags": ["backend", "notification-service", "bff", "qa"],
        }));
        assert_eq!(
            card_tags(&task, me, TODAY, "en"),
            vec![
                ("backend".to_string(), Tone::Muted),
                ("notification-service".to_string(), Tone::Muted),
                ("+2".to_string(), Tone::Dim),
            ]
        );
        assert!(card_tags(&issue(json!({"key": "DE-1"})), me, TODAY, "en").is_empty());
    }

    #[test]
    fn the_foot_names_someone_else_not_me() {
        let task = |assignee: Value| {
            issue(json!({
                "key": "DE-1",
                "type": {"id": "2", "key": "task", "display": "Задача"},
                "assignee": assignee,
            }))
        };
        let mine = task(json!({"id": "8000000000000121", "display": "Егор Юрьевич Агеев"}));
        assert_eq!(card_foot(&mine, me), "Задача");
        let theirs = task(json!({"id": "8000000000000017", "display": "Анна Петрова"}));
        assert_eq!(card_foot(&theirs, me), "Задача · Анна Петрова");
        assert_eq!(card_foot(&task(Value::Null), me), "Задача · No assignee");
    }

    #[test]
    fn the_task_card_fields() {
        // 2026-10-15 12:00 in Moscow.
        let now = time::days_from_civil(2026, 10, 15) * 86_400 + 9 * 3600;
        let task = issue(json!({
            "key": "DE-2560",
            "type": {"id": "2", "key": "task", "display": "Задача"},
            "priority": {"id": "4", "key": "critical", "display": "Критичный"},
            "createdBy": {"id": "1", "display": "Игорь"},
            "queue": {"id": "71", "key": "DE", "display": "Research & Development"},
            "sprint": [{"id": "672", "display": "Спринт DE 2026_20"}],
            "deadline": "2026-10-14",
            "tags": ["backend", "bff"],
            "components": [{"id": "11", "display": "АП 2.0"}],
            "followers": [{"id": "1", "display": "Игорь"}, {"id": "2", "display": "Анна"}],
            "createdAt": "2026-10-14T07:00:00.000+0000",
            "updatedAt": "2026-10-15T08:30:00.000+0000",
        }));
        let fields = card_fields(&task, now, 3 * 3600, "en");
        let get = |id: &str| fields.iter().find(|field| field.id == id).cloned();
        assert_eq!(get("priority").unwrap().tone, Tone::Error);
        let assignee = get("assignee").unwrap();
        assert_eq!((assignee.value.as_str(), assignee.tone), ("No assignee", Tone::Dim));
        assert_eq!(get("author").unwrap().value, "Игорь");
        assert_eq!(get("sprint").unwrap().value, "Спринт DE 2026_20");
        assert_eq!(get("deadline").unwrap(), Field {
            id: "deadline".into(),
            name: "Deadline".into(),
            value: "14 Oct · overdue".into(),
            tone: Tone::Error,
            editable: true,
        });
        assert_eq!(get("tags").unwrap().value, "backend, bff");
        assert_eq!(get("components").unwrap().value, "АП 2.0");
        assert_eq!(get("queue").unwrap().value, "Research & Development · DE");
        assert_eq!(get("followers").unwrap().value, "2");
        assert_eq!(get("created").unwrap().value, "yesterday, 10:00");
        assert_eq!(get("updated").unwrap().value, "today, 11:30");
        // What the user can change is there even empty (a dash to fill in); the rest only when
        // the task has it.
        let points = get("story-points").unwrap();
        assert!(points.editable && points.value.is_empty());
        assert_eq!(get("awaiting"), None);
        assert!(!get("author").unwrap().editable);
        let order: Vec<&str> = fields.iter().map(|field| field.id.as_str()).collect();
        assert_eq!(&order[..3], &["assignee", "priority", "type"]);
    }
}
