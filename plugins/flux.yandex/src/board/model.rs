//! A board's objects as Tracker's API returns them (read tolerantly, as the Tracker's own), and
//! what the board makes of them: the tasks put into the columns, "mine", the current sprint.

use serde::Deserialize;
use serde_json::Value;

use crate::tracker::model::{Issue, Named};

/// A board as `GET /v3/boards` lists it.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct BoardInfo {
    pub id: Value,
    pub name: String,
    /// The tasks' order on the board: a field ("updated", "status"), ascending or not.
    pub order_by: Option<String>,
    pub order_asc: Option<bool>,
    /// The board's own filter — a board of a queue, of tags: its tasks are the ones the filter
    /// finds (as `_search` takes `filter`), not only the ones added to the board.
    pub filter: Option<Value>,
    /// A board made of a query in Tracker's language.
    pub query: Option<String>,
    /// "2026-10-06T…": with nothing chosen yet, the most recently changed board opens.
    pub updated_at: Option<String>,
}

impl BoardInfo {
    /// The id as text: storage, element ids, the picker's keys.
    pub fn key(&self) -> String {
        id_text(&self.id)
    }

    /// The search's order, as `_search` takes it: "-updated" by default.
    pub fn order(&self) -> String {
        let field = self
            .order_by
            .as_deref()
            .filter(|field| !field.is_empty())
            .unwrap_or("updated");
        let sign = if self.order_asc.unwrap_or(false) {
            '+'
        } else {
            '-'
        };
        format!("{sign}{field}")
    }
}

/// A column of a board and the statuses it takes.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Column {
    pub id: Value,
    pub name: String,
    pub statuses: Vec<Named>,
}

impl Column {
    pub fn key(&self) -> String {
        id_text(&self.id)
    }

    /// The column takes tasks in `status`: by the status's key, else its id, else its name.
    fn takes(&self, status: &Named) -> bool {
        self.statuses.iter().any(|own| {
            if let (Some(own), Some(other)) = (&own.key, &status.key) {
                return own == other;
            }
            if !own.id.is_null() && !status.id.is_null() {
                return id_text(&own.id) == id_text(&status.id);
            }
            !own.display.is_empty() && own.display == status.display
        })
    }
}

/// A sprint of a scrum board.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Sprint {
    pub id: Value,
    pub name: String,
    /// "draft", "in_progress", "released".
    pub status: String,
    pub archived: bool,
    /// "2026-10-07".
    pub start_date: Option<String>,
    pub end_date: Option<String>,
}

impl Sprint {
    pub fn key(&self) -> String {
        id_text(&self.id)
    }

    fn in_progress(&self) -> bool {
        let status = self.status.to_ascii_lowercase().replace('_', "");
        !self.archived && status == "inprogress"
    }

    /// Today (`YYYY-MM-DD`) is within the sprint's dates.
    fn covers(&self, today: &str) -> bool {
        let starts = self.start_date.as_deref().map(|date| date.get(..10).unwrap_or(date));
        let ends = self.end_date.as_deref().map(|date| date.get(..10).unwrap_or(date));
        !self.archived
            && starts.is_some_and(|start| start <= today)
            && ends.is_some_and(|end| today <= end)
    }
}

/// The sprint the board shows: the one in progress, else the one whose dates hold today; none
/// for a kanban board, or a scrum board between sprints (then the board shows all its tasks).
pub fn current_sprint(sprints: &[Sprint], today: &str) -> Option<Sprint> {
    sprints
        .iter()
        .find(|sprint| sprint.in_progress())
        .or_else(|| sprints.iter().find(|sprint| sprint.covers(today)))
        .cloned()
}

/// The task is in the sprint (its `sprint` field: one reference or a list of them).
pub fn in_sprint(issue: &Issue, sprint: &Sprint) -> bool {
    let wanted = sprint.key();
    let matches = |value: &Value| match value {
        Value::Object(object) => object.get("id").is_some_and(|id| id_text(id) == wanted),
        id => id_text(id) == wanted,
    };
    match &issue.sprint {
        Some(Value::Array(sprints)) => sprints.iter().any(matches),
        Some(Value::Null) | None => false,
        Some(sprint) => matches(sprint),
    }
}

/// Who the user is to Tracker: the ids a task's assignee may carry (Tracker's, Yandex ID's, the
/// cloud's) and the login.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Me {
    ids: Vec<String>,
}

impl Me {
    /// From `GET /v3/myself`.
    pub fn from_myself(myself: &Value) -> Me {
        let ids = ["uid", "trackerUid", "passportUid", "cloudUid", "login"]
            .iter()
            .filter_map(|field| myself.get(*field))
            .map(id_text)
            .filter(|id| !id.is_empty())
            .collect();
        Me { ids }
    }

    /// The task is assigned to the user.
    pub fn owns(&self, issue: &Issue) -> bool {
        issue.assignee.as_ref().is_some_and(|assignee| self.is(assignee))
    }

    /// A user Tracker names (an assignee, a follower) is the user.
    pub fn is(&self, user: &Named) -> bool {
        let mut theirs = vec![id_text(&user.id)];
        theirs.extend(user.key.clone());
        theirs
            .iter()
            .any(|id| !id.is_empty() && self.ids.iter().any(|mine| mine == id))
    }
}

/// A column with the tasks it shows.
#[derive(Debug, Clone, PartialEq)]
pub struct Lane {
    /// The column's key; [`OTHER`] for the tasks no column takes.
    pub key: String,
    pub name: String,
    pub tasks: Vec<Issue>,
}

/// The key of the lane of tasks in statuses the board has no column for.
pub const OTHER: &str = "other";

/// Puts the tasks into the board's columns, keeping their order; tasks in a status no column takes
/// go to a last lane named `other` (only when there are such tasks — a board shouldn't hide work
/// because its columns and the tasks' statuses disagree).
pub fn arrange(columns: &[Column], tasks: &[Issue], other: &str) -> Vec<Lane> {
    let mut lanes: Vec<Lane> = columns
        .iter()
        .map(|column| Lane {
            key: column.key(),
            name: column.name.clone(),
            tasks: Vec::new(),
        })
        .collect();
    let mut rest = Vec::new();
    for task in tasks {
        let column = task
            .status
            .as_ref()
            .and_then(|status| columns.iter().position(|column| column.takes(status)));
        match column {
            Some(index) => lanes[index].tasks.push(task.clone()),
            None => rest.push(task.clone()),
        }
    }
    if !rest.is_empty() {
        lanes.push(Lane {
            key: OTHER.to_string(),
            name: other.to_string(),
            tasks: rest,
        });
    }
    lanes
}

/// An id as text: a number or a string ("12", "abc"); empty for none.
pub fn id_text(id: &Value) -> String {
    match id {
        Value::String(text) => text.clone(),
        Value::Null => String::new(),
        Value::Number(number) => number.to_string(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(key: &str, display: &str) -> Named {
        Named {
            id: Value::String(format!("id-{key}")),
            key: Some(key.into()),
            display: display.into(),
        }
    }

    fn issue(key: &str, status: &str) -> Issue {
        Issue {
            key: key.into(),
            summary: format!("Task {key}"),
            status: Some(named(status, status)),
            ..Issue::default()
        }
    }

    #[test]
    fn reads_boards_columns_and_sprints() {
        let boards: Vec<BoardInfo> = serde_json::from_str(
            r#"[{"self": "https://api.tracker.yandex.net/v3/boards/1", "id": 1, "version": 3,
                 "name": "Backend", "columns": [{"id": "1", "display": "Open"}],
                 "orderBy": "status", "orderAsc": true, "useRanking": false},
                {"id": 2, "name": "Web"}]"#,
        )
        .unwrap();
        assert_eq!(boards[0].key(), "1");
        assert_eq!(boards[0].name, "Backend");
        assert_eq!(boards[0].order(), "+status");
        assert_eq!(boards[1].order(), "-updated");

        let columns: Vec<Column> = serde_json::from_str(
            r#"[{"self": "…/columns/1", "id": 1, "version": 1, "name": "Open",
                 "statuses": [{"self": "…", "id": "1", "key": "open", "display": "Open"},
                              {"id": "2", "key": "needInfo", "display": "Need info"}]},
                {"id": 2, "name": "In Progress",
                 "statuses": [{"id": "3", "key": "inProgress", "display": "In progress"}]}]"#,
        )
        .unwrap();
        assert_eq!(columns.len(), 2);
        assert_eq!(columns[0].key(), "1");
        assert_eq!(columns[0].statuses[1].key.as_deref(), Some("needInfo"));

        let sprints: Vec<Sprint> = serde_json::from_str(
            r#"[{"id": 7, "name": "Sprint 7", "status": "released", "archived": false,
                 "startDate": "2026-09-23", "endDate": "2026-10-06"},
                {"id": 8, "name": "Sprint 8", "status": "in_progress", "archived": false,
                 "startDate": "2026-10-07", "endDate": "2026-10-20"},
                {"id": 9, "name": "Sprint 9", "status": "draft"}]"#,
        )
        .unwrap();
        let current = current_sprint(&sprints, "2026-10-14").unwrap();
        assert_eq!(current.key(), "8");
        assert_eq!(current.start_date.as_deref(), Some("2026-10-07"));
    }

    #[test]
    fn a_sprint_by_dates_when_none_says_in_progress() {
        let sprints = vec![
            Sprint {
                id: Value::from(1),
                status: "released".into(),
                start_date: Some("2026-09-01".into()),
                end_date: Some("2026-09-14".into()),
                ..Sprint::default()
            },
            Sprint {
                id: Value::from(2),
                status: "draft".into(),
                start_date: Some("2026-10-10".into()),
                end_date: Some("2026-10-24T00:00:00.000+0000".into()),
                ..Sprint::default()
            },
        ];
        assert_eq!(current_sprint(&sprints, "2026-10-14").unwrap().key(), "2");
        assert_eq!(current_sprint(&sprints, "2026-11-01"), None);
        assert_eq!(current_sprint(&[], "2026-10-14"), None);
    }

    #[test]
    fn puts_tasks_into_columns() {
        let columns = vec![
            Column {
                id: Value::from(1),
                name: "Open".into(),
                statuses: vec![named("open", "Open"), named("needInfo", "Need info")],
            },
            Column {
                id: Value::from(2),
                name: "In Progress".into(),
                statuses: vec![named("inProgress", "In progress")],
            },
            Column {
                id: Value::from(3),
                name: "Done".into(),
                statuses: vec![named("closed", "Closed")],
            },
        ];
        let tasks = vec![
            issue("TRACK-1", "open"),
            issue("TRACK-2", "inProgress"),
            issue("TRACK-3", "needInfo"),
            issue("TRACK-4", "testing"),
            issue("TRACK-5", "open"),
        ];
        let lanes = arrange(&columns, &tasks, "Other statuses");
        let keys = |lane: &Lane| lane.tasks.iter().map(|task| task.key.clone()).collect::<Vec<_>>();
        assert_eq!(lanes.len(), 4);
        assert_eq!(keys(&lanes[0]), ["TRACK-1", "TRACK-3", "TRACK-5"]);
        assert_eq!(keys(&lanes[1]), ["TRACK-2"]);
        assert!(lanes[2].tasks.is_empty());
        assert_eq!(lanes[3].key, OTHER);
        assert_eq!(lanes[3].name, "Other statuses");
        assert_eq!(keys(&lanes[3]), ["TRACK-4"]);
        // Without stray statuses there is no extra lane.
        assert_eq!(arrange(&columns, &tasks[..3], "Other").len(), 3);
    }

    #[test]
    fn columns_match_statuses_by_id_or_name_without_keys() {
        let column = Column {
            id: Value::from(1),
            name: "Open".into(),
            statuses: vec![Named {
                id: Value::from(1),
                key: None,
                display: "Открыт".into(),
            }],
        };
        let by_id = Named {
            id: Value::String("1".into()),
            key: Some("open".into()),
            display: "Open".into(),
        };
        let by_name = Named {
            id: Value::Null,
            key: None,
            display: "Открыт".into(),
        };
        assert!(column.takes(&by_id));
        assert!(column.takes(&by_name));
        assert!(!column.takes(&Named {
            display: "Closed".into(),
            ..Named::default()
        }));
    }

    #[test]
    fn tells_mine_by_any_of_the_users_ids() {
        let me = Me::from_myself(&serde_json::json!({
            "uid": 1120000000016772u64, "login": "egor", "trackerUid": 1120000000016772u64,
            "passportUid": 1120000000016772u64, "cloudUid": "ajeppa7dgp1sguh6fjl5",
            "display": "Egor"
        }));
        let assigned = |id: Value| Issue {
            assignee: Some(Named {
                id,
                key: None,
                display: "Egor".into(),
            }),
            ..Issue::default()
        };
        assert!(me.owns(&assigned(Value::String("1120000000016772".into()))));
        assert!(me.owns(&assigned(Value::String("ajeppa7dgp1sguh6fjl5".into()))));
        assert!(!me.owns(&assigned(Value::String("42".into()))));
        assert!(!me.owns(&Issue::default()));
        assert!(!Me::default().owns(&assigned(Value::Null)));
    }

    #[test]
    fn tells_a_tasks_sprint() {
        let sprint = Sprint {
            id: Value::from(8),
            ..Sprint::default()
        };
        let with = |value: Value| Issue {
            sprint: Some(value),
            ..Issue::default()
        };
        assert!(in_sprint(&with(serde_json::json!([{"id": "8", "display": "Sprint 8"}])), &sprint));
        assert!(in_sprint(&with(serde_json::json!({"id": 8, "display": "Sprint 8"})), &sprint));
        assert!(!in_sprint(&with(serde_json::json!([{"id": "7"}])), &sprint));
        assert!(!in_sprint(&Issue::default(), &sprint));
    }
}
