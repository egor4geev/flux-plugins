//! The board's requests to Tracker, through the shared [`crate::client`]: the user's boards, a
//! board's columns and sprints, its tasks (paged), who the user is.

use serde_json::{Map, Value, json};

use crate::auth::Auth;
use crate::client::{self, ClientError};
use crate::tracker::model::Issue;

use super::model::{BoardInfo, Column, Sprint};

/// Tasks on a page of the search.
const PER_PAGE: usize = 100;
/// The fields of a board's tasks (`?fields=`): the columns (the status), a card (the type, the
/// priority, the assignee, the tags, story points, the deadline, the reply awaited), the sprint,
/// the filters (the components too). A server that ignored `fields` sends all: trimmed to these.
pub const BOARD_FIELDS: &[&str] = &[
    "key",
    "summary",
    "status",
    "statusType",
    "type",
    "priority",
    "assignee",
    "sprint",
    "tags",
    "components",
    "storyPoints",
    "pendingReplyFrom",
    "deadline",
    "updatedAt",
];
/// At most this many pages: a board of more tasks shows the first ones and says so.
pub const MAX_PAGES: usize = 5;

/// The boards the user sees (`GET /v3/boards`).
pub fn boards(auth: &Auth) -> Result<Vec<BoardInfo>, ClientError> {
    client::get_as(auth, "boards")
}

/// A board's columns with their statuses.
pub fn columns(auth: &Auth, board: &str) -> Result<Vec<Column>, ClientError> {
    client::get_as(auth, &format!("boards/{board}/columns"))
}

/// A board's sprints. A kanban board has none: Tracker answers 400 («У доски этого типа не может
/// быть спринтов») or 404 — no sprints, as any answer that isn't about the user's access.
pub fn sprints(auth: &Auth, board: &str) -> Result<Vec<Sprint>, ClientError> {
    match client::get_as(auth, &format!("boards/{board}/sprints")) {
        Err(ClientError::NotFound | ClientError::BadRequest(_) | ClientError::Other(_)) => {
            Ok(Vec::new())
        }
        result => result,
    }
}

/// Who the user is (`GET /v3/myself`): Only Mine — as Tracker answers (the cache keeps it).
pub fn myself(auth: &Auth) -> Result<Value, ClientError> {
    client::get(auth, "myself")
}

/// The board's tasks — of `sprint` when given — in the board's order, page by page (at most
/// [`MAX_PAGES`]), and how many the board has (`X-Total-Count`; at least the ones read).
pub fn tasks(
    auth: &Auth,
    board: &BoardInfo,
    sprint: Option<&Sprint>,
) -> Result<(Vec<Issue>, usize), ClientError> {
    let body = search_body(board, sprint);
    let mut tasks = Vec::new();
    let mut total = None;
    for page in 1..=MAX_PAGES {
        let path = format!(
            "issues/_search?perPage={PER_PAGE}&page={page}&fields={}",
            BOARD_FIELDS.join(",")
        );
        let (value, headers) = client::send(auth, "POST", &path, Some(&body))?;
        total = total.or_else(|| header_number(&headers, "X-Total-Count"));
        let found: Vec<Issue> = match value {
            Value::Null => Vec::new(),
            value => {
                serde_json::from_value(value).map_err(|err| ClientError::Other(err.to_string()))?
            }
        };
        let count = found.len();
        tasks.extend(found.into_iter().map(|task| task.trimmed(BOARD_FIELDS)));
        if is_last_page(page, count, &headers) {
            break;
        }
    }
    let total = total.unwrap_or(0).max(tasks.len());
    Ok((tasks, total))
}

/// The search of a board's tasks: the board's own filter (a board of a queue, of tags) or query,
/// else the tasks added to the board — `{"filter": {"boards": <id>}}`; with the sprint of a scrum
/// board (`"sprint": <id>`), in the board's order.
fn search_body(board: &BoardInfo, sprint: Option<&Sprint>) -> Value {
    if let Some(query) = board
        .query
        .as_deref()
        .map(str::trim)
        .filter(|query| !query.is_empty())
    {
        // The query has its own order ("Sort by").
        return json!({ "query": query });
    }
    let mut filter = match board.filter.as_ref().and_then(Value::as_object) {
        Some(own) if !own.is_empty() => own.clone(),
        _ => {
            let mut filter = Map::new();
            filter.insert("boards".into(), board.id.clone());
            filter
        }
    };
    if let Some(sprint) = sprint {
        filter.insert("sprint".into(), sprint.id.clone());
    }
    json!({ "filter": filter, "order": board.order() })
}

/// A number in a response's header.
fn header_number(headers: &[(String, String)], name: &str) -> Option<usize> {
    headers
        .iter()
        .find(|(header, _)| header.eq_ignore_ascii_case(name))
        .and_then(|(_, value)| value.trim().parse().ok())
}

/// The search's last page: `X-Total-Pages` says so, or (without it) a page short of
/// [`PER_PAGE`].
fn is_last_page(page: usize, count: usize, headers: &[(String, String)]) -> bool {
    match header_number(headers, "X-Total-Pages") {
        Some(pages) => page >= pages,
        None => count < PER_PAGE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn searches_a_board_and_its_sprint() {
        let board = BoardInfo {
            id: Value::from(12),
            name: "Backend".into(),
            order_by: Some("status".into()),
            order_asc: Some(true),
            ..BoardInfo::default()
        };
        assert_eq!(
            search_body(&board, None),
            json!({"filter": {"boards": 12}, "order": "+status"})
        );
        let sprint = Sprint {
            id: Value::from(8),
            ..Sprint::default()
        };
        assert_eq!(
            search_body(&board, Some(&sprint)),
            json!({"filter": {"boards": 12, "sprint": 8}, "order": "+status"})
        );
    }

    #[test]
    fn a_board_of_its_own_filter_or_query_searches_by_it() {
        // Board 99 of the author's Tracker: 5 tasks added to it, 8 found by its filter.
        let board = BoardInfo {
            id: Value::from(99),
            filter: Some(json!({"queue": "DE", "tags": "frontend-optimization"})),
            ..BoardInfo::default()
        };
        assert_eq!(
            search_body(&board, None),
            json!({"filter": {"queue": "DE", "tags": "frontend-optimization"}, "order": "-updated"})
        );
        // An empty filter is no filter.
        let board = BoardInfo {
            id: Value::from(7),
            filter: Some(json!({})),
            ..BoardInfo::default()
        };
        assert_eq!(
            search_body(&board, None),
            json!({"filter": {"boards": 7}, "order": "-updated"})
        );
        let board = BoardInfo {
            id: Value::from(5),
            query: Some("Queue: DE Tags: frontend".into()),
            ..BoardInfo::default()
        };
        assert_eq!(
            search_body(&board, None),
            json!({"query": "Queue: DE Tags: frontend"})
        );
    }

    #[test]
    fn knows_the_last_page() {
        let pages = |total: &str| vec![("x-total-pages".to_string(), total.to_string())];
        assert!(!is_last_page(1, 100, &pages("3")));
        assert!(is_last_page(3, 20, &pages("3")));
        assert!(is_last_page(1, 0, &pages("0")));
        // Without the header: a short page is the last one.
        assert!(is_last_page(1, 37, &[]));
        assert!(!is_last_page(2, PER_PAGE, &[]));
    }

    /// A task as Tracker sends it in full: every field the card and the filters read, and more.
    fn full_task() -> Value {
        json!({
            "self": "https://api.tracker.yandex.net/v3/issues/WEB-7", "id": "a1", "key": "WEB-7",
            "version": 4, "summary": "Кнопка оплаты", "description": "Длинное описание…",
            "status": {"id": "3", "key": "inReview", "display": "Ревью"},
            "statusType": {"id": "inProgress", "display": "В работе"},
            "type": {"id": "2", "key": "task", "display": "Задача"},
            "priority": {"id": "4", "key": "critical", "display": "Критичный"},
            "assignee": {"id": "7", "display": "Егор"},
            "createdBy": {"id": "9", "display": "Анна"},
            "queue": {"id": "1", "key": "WEB", "display": "Веб"},
            "sprint": [{"id": "8", "display": "Спринт 8"}],
            "tags": ["frontend", "payments", "q4"],
            "components": [{"id": "5", "display": "Оплата"}],
            "storyPoints": 3,
            "pendingReplyFrom": [{"id": "7", "display": "Егор"}],
            "followers": [{"id": "9", "display": "Анна"}],
            "commentWithoutExternalMessageCount": 11,
            "deadline": "2026-10-20",
            "updatedAt": "2026-10-14T10:00:00.000+0000",
            "1234--risk": "high"
        })
    }

    /// What Tracker sends when it honours `fields`: the task's references and those fields.
    fn only(task: &Value, fields: &[&str]) -> Issue {
        let object = task.as_object().unwrap();
        let kept: Map<String, Value> = object
            .iter()
            .filter(|(name, _)| {
                ["self", "id", "key"].contains(&name.as_str()) || fields.contains(&name.as_str())
            })
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect();
        serde_json::from_value(Value::Object(kept)).unwrap()
    }

    #[test]
    fn the_fields_asked_are_the_fields_a_card_and_the_filters_use() {
        use crate::board::filter::Facet;
        use crate::board::model::{Me, in_sprint};
        use crate::tracker::facts;
        let task = full_task();
        let full: Issue = serde_json::from_value(task.clone()).unwrap();
        // As the search keeps them, whether the server honoured `fields` or not.
        let served = only(&task, BOARD_FIELDS).trimmed(BOARD_FIELDS);
        assert_eq!(served, full.clone().trimmed(BOARD_FIELDS));
        let me = Me::from_myself(&json!({"uid": 7}));
        let is_me = |user: &crate::tracker::model::Named| me.is(user);
        let today = facts::today(1_760_000_000, 0);
        assert_eq!(
            facts::card_tags(&served, is_me, today, "en"),
            facts::card_tags(&full, is_me, today, "en")
        );
        assert_eq!(
            facts::card_foot(&served, is_me),
            facts::card_foot(&full, is_me)
        );
        for facet in [
            Facet::Assignee,
            Facet::Type,
            Facet::Priority,
            Facet::Tags,
            Facet::Components,
        ] {
            assert_eq!(facet.values(&served), facet.values(&full));
        }
        let sprint = Sprint {
            id: Value::from(8),
            ..Sprint::default()
        };
        assert!(in_sprint(&served, &sprint));
        assert_eq!(served.status, full.status);
        assert!(served.description.is_none() && served.followers.is_empty());
    }

    #[test]
    fn the_list_and_the_poll_ask_for_what_they_show() {
        use crate::tracker::api::{LIST_FIELDS, PICKER_FIELDS, POLL_FIELDS};
        let task = full_task();
        let full: Issue = serde_json::from_value(task.clone()).unwrap();
        for fields in [LIST_FIELDS, POLL_FIELDS, PICKER_FIELDS] {
            let served = only(&task, fields).trimmed(fields);
            assert_eq!(served, full.clone().trimmed(fields));
            // The key, the title, the status (its name, its kind) of every reader.
            assert_eq!(
                (served.key.as_str(), served.summary.as_str()),
                ("WEB-7", "Кнопка оплаты")
            );
            assert_eq!(served.status_name(), "Ревью");
        }
        // The list groups by the status's kind and shows the deadline.
        let listed = only(&task, LIST_FIELDS);
        assert_eq!(listed.status_kind(), full.status_kind());
        assert_eq!(listed.deadline.as_deref(), Some("2026-10-20"));
        // The search's path asks for them.
        assert_eq!(
            crate::tracker::api::search_path(100, 1, POLL_FIELDS),
            "issues/_search?perPage=100&fields=key,summary,status,updatedAt"
        );
        assert_eq!(
            crate::tracker::api::search_path(100, 3, &[]),
            "issues/_search?perPage=100&page=3"
        );
    }
}
