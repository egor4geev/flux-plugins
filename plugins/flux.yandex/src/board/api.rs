//! The board's requests to Tracker, through the shared [`crate::client`]: the user's boards, a
//! board's columns and sprints, its tasks (paged), who the user is.

use serde_json::{Map, Value, json};

use crate::auth::Auth;
use crate::client::{self, ClientError};
use crate::tracker::model::Issue;

use super::model::{BoardInfo, Column, Me, Sprint};

/// Tasks on a page of the search.
const PER_PAGE: usize = 100;
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
        Err(ClientError::NotFound | ClientError::Other(_)) => Ok(Vec::new()),
        result => result,
    }
}

/// Who the user is (`GET /v3/myself`): Only Mine.
pub fn myself(auth: &Auth) -> Result<Me, ClientError> {
    client::get(auth, "myself").map(|value| Me::from_myself(&value))
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
        let path = format!("issues/_search?perPage={PER_PAGE}&page={page}");
        let (value, headers) = client::send(auth, "POST", &path, Some(&body))?;
        total = total.or_else(|| header_number(&headers, "X-Total-Count"));
        let found: Vec<Issue> = match value {
            Value::Null => Vec::new(),
            value => {
                serde_json::from_value(value).map_err(|err| ClientError::Other(err.to_string()))?
            }
        };
        let count = found.len();
        tasks.extend(found);
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
    if let Some(query) = board.query.as_deref().map(str::trim).filter(|query| !query.is_empty()) {
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
        assert_eq!(search_body(&board, None), json!({"query": "Queue: DE Tags: frontend"}));
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
}
