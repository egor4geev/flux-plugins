//! The board's view, built from what the board knows (pure, so tests read it): the toolbar in
//! groups — the board (a menu badge) and ☆ · Only Mine · Refresh, Open in Browser — a line about
//! the sprint, and the columns side by side: a heading "Name · N" over a list of cards — the key
//! (the active task marked), the title, tags (the priority, the user's reply awaited, the
//! deadline, story points, the task's tags), the type and the assignee unless it's the user, and
//! the favorite action's button on the card itself; an empty column says so under its heading.
//! Without a board — a message and what to do: sign in, try again, make a board.

use flux_plugin_api::view::*;
#[cfg(not(test))]
use flux_plugin_api::{tr, trf};

use crate::tracker::facts::{self, Day};
use crate::tracker::model::Issue;

use super::model::{Lane, Me};

/// The translations are Flux's, asked of the host — in tests on this Mac there is no host: the
/// English text itself.
#[cfg(test)]
fn tr(text: &str) -> String {
    text.to_string()
}

#[cfg(test)]
fn trf(template: &str, args: &[&dyn std::fmt::Display]) -> String {
    args.iter()
        .enumerate()
        .fold(template.to_string(), |text, (index, arg)| {
            text.replace(&format!("{{{index}}}"), &arg.to_string())
        })
}

/// The ids the board's events name.
pub const BOARD_PICKER: &str = "board-picker";
pub const STAR: &str = "favorite-board";
pub const ONLY_MINE: &str = "only-mine";
pub const REFRESH: &str = "refresh";
pub const BROWSER: &str = "browser";
pub const RETRY: &str = "retry";
pub const SETTINGS: &str = "settings";
pub const CREATE_BOARD: &str = "create-board";
/// The lists of cards: `cards-<column>`.
pub const CARDS: &str = "cards-";
/// The favorite action's button on a card (`row-action`).
pub const TAKE: &str = "take";

/// What the board shows under the toolbar.
pub enum Content<'a> {
    /// Nobody signed in, no token pasted.
    SignedOut,
    /// The first load (or another board's).
    Loading,
    /// The load failed: the client's message.
    Failed(&'a str),
    /// The user has no boards.
    NoBoards,
    /// The board's columns.
    Lanes(&'a [Lane]),
}

/// Everything the view is made of.
pub struct Screen<'a> {
    /// The shown board's name.
    pub board: Option<&'a str>,
    /// The shown board is a favorite one (★).
    pub favorite_board: bool,
    pub only_mine: bool,
    /// The favorite action's name ("Take to Work"); empty — not known yet.
    pub favorite: &'a str,
    /// Who the user is: their own name stays off the cards, their awaited reply is on them.
    pub me: Option<&'a Me>,
    /// Today in the user's time: how near a deadline is.
    pub today: Day,
    /// The interface language: dates.
    pub language: &'a str,
    /// The active task's key: its card is marked.
    pub active: Option<&'a str>,
    /// The sprint shown: "Sprint 8 · 7 Oct – 20 Oct".
    pub sprint: Option<&'a str>,
    /// Only the first so many tasks are shown, of so many: (shown, all).
    pub truncated: Option<(usize, usize)>,
    /// A refresh runs over the shown columns.
    pub refreshing: bool,
    /// A refresh failed; the columns shown are the previous ones.
    pub stale: Option<&'a str>,
    pub content: Content<'a>,
}

/// The board's view.
pub fn render(screen: &Screen) -> Element {
    let mut children = vec![toolbar_of(screen)];
    if screen.refreshing {
        children.push(progress("refreshing", None));
    }
    if let Some(problem) = screen.stale {
        children.push(row(
            "stale",
            [
                text("stale-text", [span(problem).tone(Tone::Error)]),
                button(RETRY, &tr("Try Loading the Board Again")).into(),
            ],
        ));
    }
    if let Some(sprint) = screen.sprint {
        children.push(text("sprint", [span(sprint).tone(Tone::Dim)]));
    }
    if let Some((shown, all)) = screen.truncated {
        children.push(text(
            "truncated",
            [span(&trf("Showing the first {0} of {1} tasks", &[&shown, &all])).tone(Tone::Warning)],
        ));
    }
    children.push(match &screen.content {
        Content::Lanes([]) => message("empty", &tr("The board has no columns."), None),
        Content::Lanes(lanes) => row(
            "lanes",
            lanes.iter().map(|lane| lane_element(lane, screen)),
        ),
        Content::SignedOut => message(
            "signed-out",
            &tr("Sign in to Yandex to see your boards."),
            Some(button(SETTINGS, &tr("Open the Plugin's Settings")).primary()),
        ),
        Content::Loading => column(
            "loading",
            [
                progress("loading-bar", None),
                text("loading-text", [span(&tr("Loading the board…")).tone(Tone::Dim)]),
            ],
        ),
        Content::Failed(problem) => message(
            "failed",
            problem,
            Some(button(RETRY, &tr("Try Loading the Board Again"))),
        ),
        Content::NoBoards => message(
            "no-boards",
            &tr("You have no boards in Tracker yet."),
            Some(button(CREATE_BOARD, &tr("Create a Board in Tracker"))),
        ),
    });
    column("root", children)
}

fn toolbar_of(screen: &Screen) -> Element {
    let board = screen
        .board
        .map(str::to_string)
        .unwrap_or_else(|| tr("Choose a Board"));
    let mut items = vec![menu_badge(BOARD_PICKER, &board, Tone::Normal)];
    if screen.board.is_some() {
        items.push(if screen.favorite_board {
            icon_button(STAR, "star-filled", &tr("Remove the Board from Favorites"))
        } else {
            icon_button(STAR, "star", &tr("Add the Board to Favorites: First in the Menu"))
        });
    }
    items.extend([
        divider("toolbar-board-end"),
        checkbox(ONLY_MINE, &tr("Only mine"), screen.only_mine),
        spacer("toolbar-gap"),
        icon_button(REFRESH, "refresh", &tr("Refresh the Board")),
        icon_button(BROWSER, "globe", &tr("Open the Board in Tracker")),
    ]);
    toolbar("toolbar", items)
}

/// A column: its heading and its cards.
fn lane_element(lane: &Lane, screen: &Screen) -> Element {
    let heading = text(
        &format!("lane-title-{}", lane.key),
        [
            span(&lane.name).bold(),
            span(&format!(" · {}", lane.tasks.len())).tone(Tone::Dim),
        ],
    );
    // An empty column says so right under its heading, not in the middle of the board.
    if lane.tasks.is_empty() {
        return column(
            &format!("lane-{}", lane.key),
            [
                heading,
                text(
                    &format!("lane-empty-{}", lane.key),
                    [span(&tr("No tasks")).tone(Tone::Dim)],
                ),
            ],
        );
    }
    let mut cards = Tree::new().cards();
    for task in &lane.tasks {
        cards.add(card(task, screen));
    }
    column(
        &format!("lane-{}", lane.key),
        [heading, cards.into_element(&format!("{CARDS}{}", lane.key))],
    )
}

/// A task's card: the key (the task you're working on — marked), the title under it, the tags
/// of what matters on a board, the type and the assignee unless it's the user, and the favorite
/// action's button.
fn card(task: &Issue, screen: &Screen) -> RowSpec {
    let is_active = screen.active == Some(task.key.as_str());
    let mut label = Vec::new();
    if is_active {
        label.push(span("▶ ").tone(Tone::Accent));
    }
    let key = span(&task.key).code();
    label.push(if is_active {
        key.tone(Tone::Accent).bold()
    } else {
        key.tone(Tone::Muted)
    });
    let is_me = |user: &crate::tracker::model::Named| screen.me.is_some_and(|me| me.is(user));
    let mut row = RowSpec::spans(&task.key, label).subtitle(vec![span(&task.summary)]);
    for (text_, tone) in facts::card_tags(task, is_me, screen.today, screen.language) {
        row = row.tag(&text_, tone);
    }
    let foot = facts::card_foot(task, is_me);
    if !foot.is_empty() {
        row = row.detail(&foot);
    }
    let favorite = match screen.favorite {
        "" => tr("Take to Work"),
        name => name.to_string(),
    };
    row.action(TAKE, Some(&favorite), Some("claude"), true)
}

/// A message in place of the columns, with what to do.
fn message(id: &str, text_: &str, action: Option<Button>) -> Element {
    let mut children = vec![text(&format!("{id}-text"), [span(text_)])];
    if let Some(action) = action {
        children.push(action.into());
    }
    column(id, children)
}

/// The selection after the columns change: the selected task while it's on the board, else none.
pub fn keep_selection(selected: Option<&str>, lanes: &[Lane]) -> Option<String> {
    let selected = selected?;
    lanes
        .iter()
        .flat_map(|lane| &lane.tasks)
        .any(|task| task.key == selected)
        .then(|| selected.to_string())
}

#[cfg(test)]
mod tests {
    use flux_plugin_api::host::ui::{ElementKind, View};
    use serde_json::Value;

    use super::*;
    use crate::tracker::model::Named;

    fn named(key: &str, display: &str) -> Named {
        Named {
            id: Value::Null,
            key: Some(key.into()),
            display: display.into(),
        }
    }

    fn lanes() -> Vec<Lane> {
        let task = |key: &str, priority: &str| Issue {
            key: key.into(),
            summary: format!("Task {key}"),
            kind: Some(named("bug", "Bug")),
            priority: Some(named(priority, priority)),
            assignee: Some(named("egor", "Egor")),
            ..Issue::default()
        };
        vec![
            Lane {
                key: "1".into(),
                name: "Open".into(),
                tasks: vec![task("TRACK-1", "critical"), task("TRACK-2", "normal")],
            },
            Lane {
                key: "2".into(),
                name: "In Progress".into(),
                tasks: vec![task("TRACK-3", "normal")],
            },
        ]
    }

    /// The user: Egor, by the key the fixtures give him.
    fn me() -> Me {
        Me::from_myself(&serde_json::json!({"login": "egor"}))
    }

    fn screen<'a>(lanes: &'a [Lane], me: &'a Me) -> Screen<'a> {
        Screen {
            board: Some("Backend"),
            favorite_board: false,
            only_mine: false,
            favorite: "Take to Work",
            me: Some(me),
            today: (2026, 10, 15),
            language: "en",
            active: Some("TRACK-3"),
            sprint: None,
            truncated: None,
            refreshing: false,
            stale: None,
            content: Content::Lanes(lanes),
        }
    }

    fn find<'a>(view: &'a View, id: &str) -> &'a ElementKind {
        &view
            .elements
            .iter()
            .find(|element| element.id == id)
            .unwrap_or_else(|| panic!("no element {id}"))
            .kind
    }

    #[test]
    fn columns_side_by_side_with_their_cards() {
        let lanes = lanes();
        let me = me();
        let view = render(&screen(&lanes, &me)).into_view();
        let lanes_row = view.elements.iter().find(|e| e.id == "lanes").unwrap();
        assert_eq!(lanes_row.kind, ElementKind::Row);
        assert_eq!(lanes_row.children.len(), 2);
        let ElementKind::Tree(open) = find(&view, "cards-1") else {
            panic!("a tree of cards");
        };
        assert_eq!(open.rows.len(), 2);
        assert_eq!(open.rows[0].key, "TRACK-1");
        // The user's own name stays off the card: the type alone.
        assert_eq!(open.rows[0].detail.as_deref(), Some("Bug"));
        // The usual priority isn't shown; an unusual one is a loud tag.
        assert_eq!(open.rows[0].tags[0].text, "critical");
        assert_eq!(open.rows[0].tags[0].tone, Tone::Error);
        assert!(open.rows[1].tags.is_empty());
        // The active task is marked.
        let ElementKind::Tree(in_progress) = find(&view, "cards-2") else {
            panic!("a tree of cards");
        };
        assert_eq!(in_progress.rows[0].label[0].text, "▶ ");
        let ElementKind::Text(heading) = find(&view, "lane-title-1") else {
            panic!("a heading");
        };
        assert_eq!(heading[1].text, " · 2");
    }

    #[test]
    fn every_card_carries_the_favorite_action() {
        let lanes = lanes();
        let me = me();
        let view = render(&screen(&lanes, &me)).into_view();
        let ElementKind::Tree(open) = find(&view, "cards-1") else {
            panic!("a tree of cards");
        };
        for row in &open.rows {
            assert_eq!(row.actions.len(), 1);
            assert_eq!(row.actions[0].id, TAKE);
            assert_eq!(row.actions[0].label.as_deref(), Some("Take to Work"));
            assert!(row.actions[0].primary);
        }
        // The toolbar no longer has it.
        assert!(!view.elements.iter().any(|element| element.id == "favorite"));
    }

    #[test]
    fn someone_elses_task_names_them() {
        let mut lanes = lanes();
        lanes[0].tasks[0].assignee = Some(named("anna", "Anna"));
        lanes[0].tasks[1].assignee = None;
        let me = me();
        let view = render(&screen(&lanes, &me)).into_view();
        let ElementKind::Tree(open) = find(&view, "cards-1") else {
            panic!("a tree of cards");
        };
        assert_eq!(open.rows[0].detail.as_deref(), Some("Bug · Anna"));
        assert_eq!(open.rows[1].detail.as_deref(), Some("Bug · No assignee"));
    }

    #[test]
    fn keeps_the_selection_only_while_the_task_is_shown() {
        let lanes = lanes();
        assert_eq!(keep_selection(Some("TRACK-3"), &lanes), Some("TRACK-3".into()));
        assert_eq!(keep_selection(Some("TRACK-9"), &lanes), None);
        assert_eq!(keep_selection(None, &lanes), None);
        assert_eq!(keep_selection(Some("TRACK-1"), &[]), None);
    }

    #[test]
    fn messages_instead_of_columns() {
        let me = me();
        let signed_out = Screen {
            content: Content::SignedOut,
            ..screen(&[], &me)
        };
        let view = render(&signed_out).into_view();
        assert!(view.elements.iter().any(|e| e.id == SETTINGS));
        assert!(!view.elements.iter().any(|e| e.id == "lanes"));
        let failed = Screen {
            content: Content::Failed("Not found"),
            ..screen(&[], &me)
        };
        let view = render(&failed).into_view();
        assert!(view.elements.iter().any(|e| e.id == RETRY));
    }
}
