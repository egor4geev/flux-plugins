//! Tracker's objects as the API returns them, read tolerantly (missing fields — defaults; ids —
//! strings or numbers). Shared with the board; agent F extends it (transitions, comments, links,
//! the changelog, work log), the board keeps its own objects in `board/`.

use serde::Deserialize;
use serde_json::Value;

/// A reference: a status, a type, a priority, a user, a queue — `{"id", "key", "display"}`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Named {
    pub id: Value,
    pub key: Option<String>,
    pub display: String,
}

impl Named {
    /// The key, else the id as text.
    pub fn key_or_id(&self) -> String {
        match (&self.key, &self.id) {
            (Some(key), _) => key.clone(),
            (None, Value::String(id)) => id.clone(),
            (None, Value::Null) => String::new(),
            (None, id) => id.to_string(),
        }
    }
}

/// A task.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Issue {
    pub key: String,
    pub summary: String,
    pub description: Option<String>,
    pub status: Option<Named>,
    /// The status's kind: new, inProgress, paused, done.
    pub status_type: Option<Named>,
    #[serde(rename = "type")]
    pub kind: Option<Named>,
    pub priority: Option<Named>,
    pub assignee: Option<Named>,
    pub created_by: Option<Named>,
    pub queue: Option<Named>,
    /// "2026-10-20".
    pub deadline: Option<String>,
    /// One sprint or a list of them, as the queue has it.
    pub sprint: Option<Value>,
    pub parent: Option<Named>,
    pub resolution: Option<Named>,
    /// "2026-10-14T10:00:00.000+0000".
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    /// The task's tags.
    #[serde(deserialize_with = "list_or_null")]
    pub tags: Vec<String>,
    #[serde(deserialize_with = "list_or_null")]
    pub components: Vec<Named>,
    /// Story points, as the queue counts them.
    pub story_points: Option<f64>,
    #[serde(deserialize_with = "list_or_null")]
    pub followers: Vec<Named>,
    /// Whose reply the task waits for: the people summoned to it.
    #[serde(deserialize_with = "list_or_null")]
    pub pending_reply_from: Vec<Named>,
    /// The comments, the external messages aside.
    #[serde(rename = "commentWithoutExternalMessageCount")]
    pub comment_count: Option<u32>,
    /// The boards the task is on: `[{"id": 85}]` — their sprints are the task's to choose from.
    #[serde(deserialize_with = "list_or_null")]
    pub boards: Vec<Named>,
}

/// A list that may come as `null` (or not at all: the struct's default).
fn list_or_null<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Ok(Option::<Vec<T>>::deserialize(deserializer)?.unwrap_or_default())
}

impl Issue {
    /// The status's display name; "" without one.
    pub fn status_name(&self) -> &str {
        self.status.as_ref().map_or("", |status| status.display.as_str())
    }

    /// What the status is: by its type when Tracker gives it, else by its key.
    pub fn status_kind(&self) -> StatusKind {
        let by_type = self.status_type.as_ref().map(Named::key_or_id);
        let key = by_type
            .or_else(|| self.status.as_ref().map(Named::key_or_id))
            .unwrap_or_default();
        StatusKind::of(&key)
    }

    /// The sprints as text: one, or several joined.
    pub fn sprint_name(&self) -> String {
        match &self.sprint {
            Some(Value::Array(sprints)) => sprints
                .iter()
                .filter_map(|sprint| sprint["display"].as_str())
                .collect::<Vec<_>>()
                .join(", "),
            Some(sprint) => sprint["display"].as_str().unwrap_or_default().to_string(),
            None => String::new(),
        }
    }
}

/// The kind of a status, for its color and the order of the groups.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum StatusKind {
    InProgress,
    Paused,
    New,
    Done,
}

impl StatusKind {
    /// By a status type's key (`new`, `inProgress`, `paused`, `done` — Tracker's), or a status's
    /// own key when the type isn't given.
    pub fn of(key: &str) -> StatusKind {
        let key = key.to_ascii_lowercase();
        let has = |words: &[&str]| words.iter().any(|word| key.contains(word));
        if has(&["done", "closed", "resolved", "cancel", "complete", "fixed", "rejected"]) {
            StatusKind::Done
        } else if has(&["progress", "review", "testing", "test", "work", "started"]) {
            StatusKind::InProgress
        } else if has(&["pause", "need", "wait", "block", "hold"]) {
            StatusKind::Paused
        } else {
            StatusKind::New
        }
    }
}

/// A transition of a task's workflow: `GET /v3/issues/{key}/transitions`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Transition {
    pub id: String,
    pub display: String,
    pub to: Option<Named>,
}

/// A comment: `GET /v3/issues/{key}/comments`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Comment {
    pub id: Value,
    pub text: String,
    pub created_by: Option<Named>,
    pub created_at: Option<String>,
}

/// A link of a task to another: `GET /v3/issues/{key}/links`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct IssueLink {
    #[serde(rename = "type")]
    pub kind: LinkType,
    /// "outward" — this task → the other, as `kind.outward` reads ("blocks"); "inward" — the other
    /// way ("is blocked by").
    pub direction: String,
    /// The other task: its key and title.
    pub object: Named,
    pub status: Option<Named>,
    pub assignee: Option<Named>,
}

impl IssueLink {
    /// How the link reads from this task: "Subtask", "Blocks", "Relates".
    pub fn relation(&self) -> &str {
        if self.direction == "inward" {
            &self.kind.inward
        } else {
            &self.kind.outward
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct LinkType {
    pub id: String,
    pub inward: String,
    pub outward: String,
}

/// A change of a task: `GET /v3/issues/{key}/changelog`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ChangelogEntry {
    pub id: String,
    pub updated_by: Option<Named>,
    pub updated_at: Option<String>,
    /// "IssueCreated", "IssueUpdated", "IssueWorkflow", "IssueCommentAdded"…
    #[serde(rename = "type")]
    pub kind: String,
    pub fields: Vec<FieldChange>,
}

/// One field of a change: what it was, what it became.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct FieldChange {
    pub field: Named,
    pub from: Value,
    pub to: Value,
}

/// A value of a changed field as text: a reference's display name, a list joined, a date.
pub fn value_text(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(text) => text.clone(),
        Value::Bool(on) => on.to_string(),
        Value::Number(number) => number.to_string(),
        Value::Array(items) => items
            .iter()
            .map(value_text)
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join(", "),
        Value::Object(object) => ["display", "key", "id", "name"]
            .iter()
            .find_map(|name| match object.get(*name) {
                Some(Value::String(text)) if !text.is_empty() => Some(text.clone()),
                Some(Value::Number(number)) => Some(number.to_string()),
                _ => None,
            })
            .unwrap_or_default(),
    }
}

/// A priority: `GET /v3/priorities` — its name in the language of `Accept-Language`, or (with
/// `localized=false`) by language.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Priority {
    pub id: Value,
    pub key: String,
    pub name: Value,
    pub order: i64,
}

impl Priority {
    /// The name to show: the localized one, else the English one, else the key.
    pub fn display(&self) -> String {
        match &self.name {
            Value::String(name) if !name.is_empty() => name.clone(),
            Value::Object(names) => ["ru", "en"]
                .iter()
                .find_map(|language| names.get(*language)?.as_str().map(str::to_string))
                .unwrap_or_else(|| self.key.clone()),
            _ => self.key.clone(),
        }
    }
}

/// The issue types a queue allows: `GET /v3/queues/{key}?expand=issueTypesConfig`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct QueueTypes {
    #[serde(deserialize_with = "list_or_null")]
    pub issue_types_config: Vec<IssueTypeConfig>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct IssueTypeConfig {
    pub issue_type: Named,
}

/// A user of the organization: `GET /v3/users`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct User {
    pub uid: Value,
    pub login: String,
    pub display: String,
    pub dismissed: bool,
}

impl User {
    /// What names the user in a change of a task: the login, else the uid.
    pub fn handle(&self) -> String {
        if !self.login.is_empty() {
            return self.login.clone();
        }
        match &self.uid {
            Value::String(uid) => uid.clone(),
            Value::Null => String::new(),
            uid => uid.to_string(),
        }
    }
}

/// A component of a queue: `GET /v3/queues/{key}/components`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Component {
    pub id: Value,
    pub name: String,
}

/// A sprint of a board: `GET /v3/boards/{id}/sprints` — draft, in_progress, released, archived.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Sprint {
    pub id: Value,
    pub name: String,
    pub status: String,
    pub archived: bool,
}

/// An id as text: "672" for 672 or "672".
pub fn id_text(id: &Value) -> String {
    match id {
        Value::String(id) => id.clone(),
        Value::Null => String::new(),
        id => id.to_string(),
    }
}

/// A queue: `GET /v3/queues`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Queue {
    pub key: String,
    pub name: String,
}

/// Who the token is: `GET /v3/myself`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Myself {
    pub uid: Value,
    pub login: String,
    pub display: String,
    pub email: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_an_issue_as_tracker_returns_it() {
        let issue: Issue = serde_json::from_str(
            r#"{
                "self": "https://api.tracker.yandex.net/v3/issues/TRACK-142",
                "id": "5f8a", "key": "TRACK-142", "version": 7,
                "summary": "Ошибка входа через SSO",
                "description": "Шаги:\n1. …",
                "status": {"self": "…", "id": "3", "key": "inProgress", "display": "В работе"},
                "statusType": {"id": "inProgress", "display": "В работе", "key": "inProgress"},
                "type": {"id": "1", "key": "bug", "display": "Ошибка"},
                "priority": {"id": "3", "key": "normal", "display": "Средний"},
                "assignee": {"id": "1130000012345678", "display": "Егор Агеев"},
                "queue": {"id": "1", "key": "TRACK", "display": "Трекер"},
                "deadline": "2026-10-20",
                "sprint": [{"id": "12", "display": "Спринт 42"}],
                "updatedAt": "2026-10-14T10:00:00.000+0000",
                "boards": [{"id": 1}]
            }"#,
        )
        .unwrap();
        assert_eq!(issue.key, "TRACK-142");
        assert_eq!(issue.status_name(), "В работе");
        assert_eq!(issue.status_kind(), StatusKind::InProgress);
        assert_eq!(issue.sprint_name(), "Спринт 42");
        assert_eq!(issue.assignee.unwrap().display, "Егор Агеев");
        assert_eq!(issue.kind.unwrap().key_or_id(), "bug");
    }

    #[test]
    fn statuses_by_kind() {
        assert_eq!(StatusKind::of("open"), StatusKind::New);
        assert_eq!(StatusKind::of("needInfo"), StatusKind::Paused);
        assert_eq!(StatusKind::of("inReview"), StatusKind::InProgress);
        assert_eq!(StatusKind::of("closed"), StatusKind::Done);
        assert_eq!(StatusKind::of("done"), StatusKind::Done);
    }

    #[test]
    fn reads_links_and_their_direction() {
        let links: Vec<IssueLink> = serde_json::from_str(
            r#"[{"id": 1, "type": {"id": "subtask", "inward": "Подзадача", "outward": "Родительская задача"},
                 "direction": "outward",
                 "object": {"key": "TRACK-150", "display": "Логи входа"},
                 "status": {"key": "open", "display": "Открыт"}},
                {"id": 2, "type": {"id": "depends", "inward": "Блокирует", "outward": "Зависит от"},
                 "direction": "inward",
                 "object": {"key": "WEB-7", "display": "Форма входа"}}]"#,
        )
        .unwrap();
        assert_eq!(links[0].relation(), "Родительская задача");
        assert_eq!(links[1].relation(), "Блокирует");
        assert_eq!(links[0].object.key.as_deref(), Some("TRACK-150"));
    }

    #[test]
    fn reads_the_changelog() {
        let entries: Vec<ChangelogEntry> = serde_json::from_str(
            r#"[{"id": "a1", "updatedBy": {"display": "Анна"}, "updatedAt": "2026-10-13T09:00:00.000+0000",
                 "type": "IssueWorkflow",
                 "fields": [{"field": {"id": "status", "display": "Статус"},
                             "from": {"key": "open", "display": "Открыт"},
                             "to": {"key": "inProgress", "display": "В работе"}},
                            {"field": {"id": "tags", "display": "Теги"}, "from": null, "to": ["sso", "auth"]}]}]"#,
        )
        .unwrap();
        let fields = &entries[0].fields;
        assert_eq!(value_text(&fields[0].from), "Открыт");
        assert_eq!(value_text(&fields[0].to), "В работе");
        assert_eq!(value_text(&fields[1].from), "");
        assert_eq!(value_text(&fields[1].to), "sso, auth");
    }
}
