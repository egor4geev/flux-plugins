//! The Tracker window: the list of tasks (a filter, grouped by status) and a task's card inside the
//! window (← back). The card reads top down: the parent, the key with Copy and the status with its
//! transitions, the title, a button per Claude action; the fields as aligned properties — a click
//! on an editable one changes it ([`super::edit`]); the description with Edit; the links by
//! relation; Comments | History. Its input, the filter and status pickers, Open Task (⌥⇧N).

use flux_plugin_api::picker::{self, Picker};
use flux_plugin_api::view::{
    RowSpec, SpanStyle, Tone, Tree, badge, button, column, divider, editable_property,
    icon_button, label, markdown, menu_badge, multiline_field, progress, properties, property, row,
    set_view, spacer, span, text, toolbar,
};
use flux_plugin_api::{UiEvent, UiInput, dialog, log, notify, storage, system, tr, trf};

use super::api::{self, Search};
use super::model::{ChangelogEntry, Comment, Issue, IssueLink, StatusKind};
use super::{Asked, Filter, PickerPurpose, Tracker, WINDOW, actions, open_in_browser};
use crate::auth::Auth;
use crate::settings::{self, MY_QUERY};
use crate::{text as words, time};

/// Changes a page of the history shows.
const HISTORY_PAGE: u32 = 50;
/// Tasks the Open Task picker shows at first.
const OPEN_TASK_FIRST: u32 = 50;

/// The list of tasks.
#[derive(Default)]
pub(super) struct List {
    pub issues: Vec<Issue>,
    pub total: u32,
    pub error: Option<String>,
    pub loaded_at: Option<i64>,
    pub loading: bool,
    /// Load again before showing.
    pub stale: bool,
    /// A new filter gives the tree a new id: its groups expand afresh.
    pub generation: u32,
}

/// The tabs under the card's links.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Tab {
    Comments,
    History,
}

/// The card of a task.
pub(super) struct Card {
    pub key: String,
    pub issue: Option<Issue>,
    pub links: Vec<IssueLink>,
    pub comments: Vec<Comment>,
    pub history: Vec<ChangelogEntry>,
    pub history_loaded: bool,
    pub history_more: bool,
    pub tab: Tab,
    pub error: Option<String>,
}

impl Card {
    pub fn new(key: &str) -> Self {
        Card {
            key: key.to_string(),
            issue: None,
            links: Vec::new(),
            comments: Vec::new(),
            history: Vec::new(),
            history_loaded: false,
            history_more: false,
            tab: Tab::Comments,
            error: None,
        }
    }
}

impl Tracker {
    /// Shows the window's content, if the window is open.
    pub(super) fn render(&mut self, auth: &Auth) {
        if !self.shown {
            return;
        }
        let root = match &self.card {
            Some(_) => self.card_view(),
            None => self.list_view(auth),
        };
        set_view(WINDOW, root);
    }

    fn filter_title(&self) -> String {
        match &self.filter {
            Filter::Mine => tr("My Tasks"),
            Filter::Following => tr("Following"),
            Filter::Queue(key) => self
                .queues
                .iter()
                .find(|queue| queue.key == *key)
                .map(|queue| format!("{} · {}", queue.key, queue.name))
                .unwrap_or_else(|| key.clone()),
            Filter::Query(query) => words::ellipsis(query, 32),
        }
    }

    fn filter_query(&self) -> String {
        match &self.filter {
            Filter::Mine => settings::text(MY_QUERY)
                .unwrap_or_else(|| "Assignee: me() Resolution: empty()".to_string()),
            Filter::Following => {
                "Followers: me() Resolution: empty() \"Sort by\": Updated DESC".to_string()
            }
            Filter::Queue(key) => {
                format!("Queue: {key} Resolution: empty() \"Sort by\": Updated DESC")
            }
            Filter::Query(query) => query.clone(),
        }
    }

    pub(super) fn load_list(&mut self, auth: &Auth) {
        if auth.headers().is_none() || auth.organization().is_none() {
            return;
        }
        self.list.loading = true;
        self.render(auth);
        let query = self.filter_query();
        match api::search(auth, Search::Query(&query), api::PAGE) {
            Ok((issues, total)) => {
                self.list.issues = issues;
                self.list.total = total;
                self.list.error = None;
            }
            Err(err) => self.list.error = Some(err.message()),
        }
        self.list.loading = false;
        self.list.stale = false;
        self.list.loaded_at = Some(time::now());
    }

    pub(super) fn set_filter(&mut self, filter: Filter, auth: &Auth) {
        storage::set("filter", &filter);
        self.filter = filter;
        self.list.issues.clear();
        self.list.generation += 1;
        self.load_list(auth);
        self.render(auth);
    }

    fn list_view(&self, auth: &Auth) -> flux_plugin_api::view::Element {
        let bar = toolbar(
            "toolbar",
            [
                menu_badge("filter", &self.filter_title(), Tone::Normal),
                spacer("toolbar-space"),
                icon_button("open-task", "search", &tr("Open Task… (⌥⇧N)")),
                icon_button("open-board", "icons/board.svg", &tr("Open Task Board")),
                icon_button("refresh", "refresh", &tr("Refresh")),
            ],
        );
        let mut children = vec![bar];
        if let Some(active) = self.work.active() {
            children.push(row(
                "active-row",
                [
                    badge("active-label", &tr("In work"), Tone::Accent),
                    button(
                        "active-task",
                        &format!("{} · {}", active.key, self.work.reading()),
                    )
                    .tooltip(&active.summary)
                    .into(),
                ],
            ));
        }
        let message = |id: &str, text_: String, open_settings: bool| {
            let mut parts = vec![label(&format!("{id}-text"), &text_)];
            if open_settings {
                parts.push(
                    button("open-settings", &tr("Open Settings"))
                        .primary()
                        .into(),
                );
            } else {
                parts.push(button("retry", &tr("Try Again")).into());
            }
            column(id, parts)
        };
        if auth.headers().is_none() {
            children.push(message(
                "signed-out",
                tr("Sign in to Yandex in the plugin's settings to see your tasks."),
                true,
            ));
        } else if auth.organization().is_none() {
            children.push(message(
                "no-organization",
                tr("Set your organization's ID in the plugin's settings."),
                true,
            ));
        } else if let Some(error) = &self.list.error {
            children.push(message("list-error", error.clone(), false));
        } else if self.list.loading && self.list.issues.is_empty() {
            children.push(progress("loading", None));
        } else {
            children.push(self.tree());
        }
        column("root", children)
    }

    fn tree(&self) -> flux_plugin_api::view::Element {
        let mut groups: Vec<(StatusKind, String, Vec<&Issue>)> = Vec::new();
        for issue in &self.list.issues {
            let name = issue.status_name().to_string();
            match groups.iter_mut().find(|(_, group, _)| *group == name) {
                Some((_, _, issues)) => issues.push(issue),
                None => groups.push((issue.status_kind(), name, vec![issue])),
            }
        }
        groups.sort_by_key(|(kind, _, _)| *kind);
        let active = self.work.active_key();
        let empty = match &self.filter {
            Filter::Mine => tr("No open tasks of yours"),
            _ => tr("No tasks"),
        };
        let mut tree = Tree::new().empty_text(&empty);
        for (_, name, issues) in &groups {
            let name = if name.is_empty() { tr("No status") } else { name.clone() };
            let group = tree.add(
                RowSpec::spans(&format!("status:{name}"), vec![span(&name).bold()])
                    .badge(&issues.len().to_string())
                    .expanded(),
            );
            for issue in issues {
                let mut row = RowSpec::spans(
                    &format!("task:{}", issue.key),
                    vec![
                        span(&issue.key).code().tone(Tone::Dim),
                        span("  "),
                        span(&issue.summary),
                    ],
                );
                if let Some(deadline) = &issue.deadline {
                    row = row.detail(&deadline_text(deadline));
                }
                if active.as_deref() == Some(issue.key.as_str()) {
                    row = row.icon("clock");
                }
                tree.add_child(group, row);
            }
        }
        if self.list.total > self.list.issues.len() as u32 {
            tree.add(
                RowSpec::spans(
                    "more",
                    vec![
                        span(&trf(
                            "{0} of {1}: narrow the filter to see the rest",
                            &[&self.list.issues.len(), &self.list.total],
                        ))
                        .tone(Tone::Dim),
                    ],
                ),
            );
        }
        tree.into_element(&format!("tasks-{}", self.list.generation))
    }

    fn card_view(&self) -> flux_plugin_api::view::Element {
        let Some(card) = &self.card else {
            return column("root", []);
        };
        let mut children = vec![toolbar(
            "toolbar",
            [
                icon_button("back", "arrow-left", &tr("Back to the List")),
                spacer("toolbar-space"),
                icon_button("browser", "globe", &tr("Open in Browser")),
                icon_button("card-refresh", "refresh", &tr("Refresh")),
            ],
        )];
        let Some(issue) = &card.issue else {
            children.push(match &card.error {
                Some(error) => column(
                    "card-error",
                    [
                        text("card-error-text", [span(error).tone(Tone::Error)]),
                        button("card-refresh-button", &tr("Try Again")).into(),
                    ],
                ),
                None => progress("card-loading", None),
            });
            return column("root", children);
        };
        // Where the task belongs: its parent, a click away.
        if let Some(parent) = &issue.parent
            && let Some(key) = &parent.key
        {
            children.push(
                button(
                    "parent",
                    &format!("↑ {key}  {}", words::ellipsis(&parent.display, 40)),
                )
                .tooltip(&tr("The parent task"))
                .into(),
            );
        }
        // The header: the key with Copy and the status with its transitions on one line, the
        // title under them.
        let status = if issue.status_name().is_empty() {
            tr("No status")
        } else {
            issue.status_name().to_string()
        };
        children.push(row(
            "key-row",
            [
                text("key", [span(&issue.key).code().tone(Tone::Muted)]),
                icon_button("copy-key", "copy", &tr("Copy the Key")),
                spacer("key-space"),
                menu_badge("status", &status, status_tone(issue.status_kind())),
            ],
        ));
        children.push(text("title", [span(&issue.summary).bold()]));
        children.push(self.action_buttons(issue));
        // The fields: aligned names and values; a click on an editable one changes it — a picker
        // opens under it, or a form.
        children.push(divider("fields-divider"));
        let now = time::now();
        let language = flux_plugin_api::host::i18n::language();
        let fields = super::facts::card_fields(issue, now, time::offset_at(now), &language);
        let list = fields
            .iter()
            .map(|field| {
                let value = if field.value.is_empty() {
                    Vec::new()
                } else {
                    vec![span(&field.value).tone(field.tone)]
                };
                if field.editable {
                    editable_property(field.id, &field.name, value)
                } else {
                    property(field.id, &field.name, value)
                }
            })
            .collect();
        children.push(properties("fields", list));
        children.push(divider("description-divider"));
        children.push(row(
            "description-head",
            [
                text("description-title", [span(&tr("Description")).bold()]),
                spacer("description-space"),
                button("edit-task", &tr("Edit…"))
                    .icon("pencil")
                    .tooltip(&tr("Edit the title and the description"))
                    .into(),
            ],
        ));
        match issue.description.as_deref().map(str::trim) {
            Some(description) if !description.is_empty() => {
                children.push(markdown("description", description));
            }
            _ => children.push(text(
                "no-description",
                [span(&tr("No description")).tone(Tone::Dim)],
            )),
        }
        if !card.links.is_empty() {
            children.push(divider("links-divider"));
            children.push(text(
                "links-title",
                [
                    span(&tr("Links")).bold(),
                    span(&format!("  {}", card.links.len())).tone(Tone::Dim),
                ],
            ));
            let mut relations: Vec<&str> = Vec::new();
            for link in &card.links {
                if !relations.contains(&link.relation()) {
                    relations.push(link.relation());
                }
            }
            for (group, relation) in relations.iter().enumerate() {
                children.push(text(
                    &format!("relation-{group}"),
                    [span(relation).tone(Tone::Muted)],
                ));
                for (index, link) in card.links.iter().enumerate() {
                    if link.relation() != *relation {
                        continue;
                    }
                    let key = link.object.key.clone().unwrap_or_default();
                    let mut parts = vec![
                        button(
                            &format!("link-{index}"),
                            &format!("{key}  {}", words::ellipsis(&link.object.display, 30)),
                        )
                        .into(),
                    ];
                    if let Some(status) = &link.status {
                        parts.push(badge(
                            &format!("link-{index}-status"),
                            &status.display,
                            status_tone(StatusKind::of(&status.key_or_id())),
                        ));
                    }
                    children.push(row(&format!("link-{index}-row"), parts));
                }
            }
        }
        children.push(divider("tabs-divider"));
        let mut comments_tab = button(
            "tab-comments",
            &trf("Comments · {0}", &[&card.comments.len()]),
        );
        let mut history_tab = button("tab-history", &tr("History"));
        match card.tab {
            Tab::Comments => comments_tab = comments_tab.primary(),
            Tab::History => history_tab = history_tab.primary(),
        }
        children.push(row("tabs", [comments_tab.into(), history_tab.into()]));
        match card.tab {
            Tab::Comments => self.comments_view(card, &mut children),
            Tab::History => self.history_view(card, &mut children),
        }
        column("root", children)
    }

    /// A button per action of the settings; with the task in work — Log Time and Close too.
    fn action_buttons(&self, issue: &Issue) -> flux_plugin_api::view::Element {
        let all = actions::actions();
        let favorite = actions::favorite(&all).map(|action| action.name.clone());
        let mut buttons: Vec<flux_plugin_api::view::Element> = all
            .iter()
            .enumerate()
            .map(|(index, action)| {
                let mut button_ = button(&format!("action-{index}"), &action.title())
                    .icon(if action.mode == actions::Mode::Plan {
                        "plan"
                    } else {
                        "claude"
                    })
                    .tooltip(&action_tooltip(action));
                if favorite.as_deref() == Some(action.name.as_str()) {
                    button_ = button_.primary();
                }
                button_.into()
            })
            .collect();
        buttons.push(
            button("discuss", &tr("Discuss"))
                .icon("claude")
                .tooltip(&tr("Discuss with Claude: the task goes along, you write the question"))
                .into(),
        );
        if self.work.active_key().as_deref() == Some(issue.key.as_str()) {
            buttons.push(button("log-time", &tr("Log Time…")).icon("clock").into());
            buttons.push(button("close-task", &tr("Close Task…")).into());
        }
        // One row: Flux wraps the buttons that don't fit the narrow island.
        row("actions", buttons)
    }

    fn comments_view(&self, card: &Card, children: &mut Vec<flux_plugin_api::view::Element>) {
        let now = time::now();
        let language = flux_plugin_api::host::i18n::language();
        if card.comments.is_empty() {
            children.push(text(
                "no-comments",
                [span(&tr("No comments yet")).tone(Tone::Dim)],
            ));
        }
        for (index, comment) in card.comments.iter().enumerate() {
            let author = comment
                .created_by
                .as_ref()
                .map(|named| named.display.clone())
                .unwrap_or_default();
            let when = comment
                .created_at
                .as_deref()
                .and_then(time::parse_iso)
                .map(|at| time::moment(at, now, time::offset_at(at), &language))
                .unwrap_or_default();
            if index > 0 {
                children.push(divider(&format!("comment-{index}-divider")));
            }
            children.push(text(
                &format!("comment-{index}-head"),
                [span(&author).bold(), span(&format!("   {when}")).tone(Tone::Dim)],
            ));
            children.push(markdown(&format!("comment-{index}"), &comment.text));
        }
        children.push(divider("comment-field-divider"));
        children.push(multiline_field(
            "comment",
            &self.draft,
            Some(&tr("Write a comment… ⌘↵ sends")),
        ));
        children.push(row(
            "comment-actions",
            [
                spacer("comment-space"),
                button("send-comment", &tr("Send"))
                    .primary()
                    .enabled(!self.draft.trim().is_empty())
                    .into(),
            ],
        ));
    }

    fn history_view(&self, card: &Card, children: &mut Vec<flux_plugin_api::view::Element>) {
        if !card.history_loaded {
            children.push(progress("history-loading", None));
            return;
        }
        if card.history.is_empty() {
            children.push(text(
                "no-history",
                [span(&tr("No changes yet")).tone(Tone::Dim)],
            ));
        }
        let now = time::now();
        let language = flux_plugin_api::host::i18n::language();
        // Newest first.
        for (index, entry) in card.history.iter().enumerate().rev() {
            let author = entry
                .updated_by
                .as_ref()
                .map(|named| named.display.clone())
                .unwrap_or_default();
            let when = entry
                .updated_at
                .as_deref()
                .and_then(time::parse_iso)
                .map(|at| time::moment(at, now, time::offset_at(at), &language))
                .unwrap_or_default();
            if index + 1 < card.history.len() {
                children.push(divider(&format!("change-{index}-divider")));
            }
            children.push(text(
                &format!("change-{index}-head"),
                [span(&author).bold(), span(&format!("   {when}")).tone(Tone::Dim)],
            ));
            if entry.fields.is_empty() {
                children.push(text(
                    &format!("change-{index}-kind"),
                    [span(&change_kind(&entry.kind)).tone(Tone::Muted)],
                ));
            }
            for (field, change) in entry.fields.iter().enumerate() {
                let name = if change.field.display.is_empty() {
                    change.field.key_or_id()
                } else {
                    change.field.display.clone()
                };
                children.push(text(
                    &format!("change-{index}-{field}"),
                    [span(&actions::change_line(&name, &change.from, &change.to))
                        .tone(Tone::Muted)],
                ));
            }
        }
        if card.history_more {
            children.push(button("history-more", &tr("Show Older Changes")).into());
        }
    }

    /// Loads the card's task, its links and comments (the history when its tab opens).
    pub(super) fn load_card(&mut self, key: &str, auth: &Auth) {
        let result = api::issue(auth, key);
        let Some(card) = self.card.as_mut().filter(|card| card.key == key) else {
            return;
        };
        match result {
            Ok(issue) => {
                card.issue = Some(issue);
                card.error = None;
                card.links = api::links(auth, key).unwrap_or_default();
                card.comments = api::comments(auth, key).unwrap_or_default();
                card.history_loaded = false;
                card.history.clear();
                if card.tab == Tab::History {
                    self.load_history(auth, false);
                }
            }
            Err(err) => card.error = Some(err.message()),
        }
    }

    /// The history's first page, or the next one (`more`).
    pub(super) fn load_history(&mut self, auth: &Auth, more: bool) {
        let Some(card) = self.card.as_mut() else {
            return;
        };
        let after = more
            .then(|| card.history.last().map(|entry| entry.id.clone()))
            .flatten();
        match api::changelog(auth, &card.key, after.as_deref(), HISTORY_PAGE) {
            Ok(entries) => {
                card.history_more = entries.len() as u32 >= HISTORY_PAGE;
                if !more {
                    card.history.clear();
                }
                card.history.extend(entries);
            }
            Err(err) => {
                notify::warning(&tr("Couldn't read the history"), Some(&err.message()));
                card.history_more = false;
            }
        }
        card.history_loaded = true;
    }

    pub(super) fn on_ui(&mut self, input: UiInput, auth: &Auth) {
        let element = input.element.as_str();
        match (element, input.event) {
            ("filter", UiEvent::Clicked) => self.filter_picker(),
            ("open-task", UiEvent::Clicked) => self.open_task_picker(auth),
            ("open-board", UiEvent::Clicked) => flux_plugin_api::host::ui::show("board"),
            ("refresh", UiEvent::Clicked) | ("retry", UiEvent::Clicked) => {
                self.list.stale = true;
                self.load_list(auth);
                self.render(auth);
            }
            ("open-settings", UiEvent::Clicked) => system::open_settings(),
            ("active-task", UiEvent::Clicked) => {
                if let Some(key) = self.work.active_key() {
                    self.open_card(&key, auth);
                }
            }
            (tree, UiEvent::Activated(row)) if tree.starts_with("tasks-") => {
                if let Some(key) = row.strip_prefix("task:") {
                    self.open_card(key, auth);
                }
            }
            ("back", UiEvent::Clicked) => {
                self.card = None;
                if self.list.stale {
                    self.load_list(auth);
                }
                self.render(auth);
            }
            ("card-refresh" | "card-refresh-button", UiEvent::Clicked) => {
                if let Some(key) = self.card_key() {
                    self.load_card(&key, auth);
                    self.render(auth);
                }
            }
            ("browser", UiEvent::Clicked) => {
                if let Some(key) = self.card_key() {
                    open_in_browser(&key);
                }
            }
            ("discuss", UiEvent::Clicked) => {
                if let Some(key) = self.card_key() {
                    self.discuss(&key, auth);
                }
            }
            ("status", UiEvent::Clicked) => self.status_picker(auth),
            ("fields", UiEvent::Activated(id)) => self.edit_field(&id, auth),
            // A scenario's `ui:` step can only click: `fields/<id>` stands for the property.
            (property, UiEvent::Clicked) if property.starts_with("fields/") => {
                self.edit_field(&property["fields/".len()..], auth)
            }
            ("edit-task", UiEvent::Clicked) => self.edit_task(),
            ("copy-key", UiEvent::Clicked) => {
                if let Some(key) = self.card_key() {
                    system::copy_text(&key);
                }
            }
            ("parent", UiEvent::Clicked) => {
                let parent = self
                    .card
                    .as_ref()
                    .and_then(|card| card.issue.as_ref()?.parent.as_ref()?.key.clone());
                if let Some(parent) = parent {
                    self.open_card(&parent, auth);
                }
            }
            ("log-time", UiEvent::Clicked) => {
                if let Some(key) = self.card_key() {
                    self.ask_log_time(&key, auth);
                }
            }
            ("close-task", UiEvent::Clicked) => self.close_active(auth),
            ("tab-comments", UiEvent::Clicked) => {
                if let Some(card) = self.card.as_mut() {
                    card.tab = Tab::Comments;
                }
                self.render(auth);
            }
            ("tab-history", UiEvent::Clicked) => {
                if let Some(card) = self.card.as_mut() {
                    card.tab = Tab::History;
                }
                self.render(auth);
                if self.card.as_ref().is_some_and(|card| !card.history_loaded) {
                    self.load_history(auth, false);
                    self.render(auth);
                }
            }
            ("history-more", UiEvent::Clicked) => {
                self.load_history(auth, true);
                self.render(auth);
            }
            ("comment", UiEvent::Changed(text_)) => {
                let was_empty = self.draft.trim().is_empty();
                self.draft = text_;
                if was_empty != self.draft.trim().is_empty() {
                    self.render(auth);
                }
            }
            ("comment", UiEvent::Submitted(text_)) => {
                self.draft = text_;
                self.send_comment(auth);
            }
            ("send-comment", UiEvent::Clicked) => self.send_comment(auth),
            (element, UiEvent::Clicked) => {
                if let Some(index) = element
                    .strip_prefix("action-")
                    .and_then(|index| index.parse::<usize>().ok())
                {
                    self.run_action_at(index, auth);
                } else if let Some(index) = element
                    .strip_prefix("link-")
                    .and_then(|index| index.parse::<usize>().ok())
                {
                    let key = self
                        .card
                        .as_ref()
                        .and_then(|card| card.links.get(index)?.object.key.clone());
                    if let Some(key) = key {
                        self.open_card(&key, auth);
                    }
                }
            }
            _ => {}
        }
    }

    /// Runs the action at `index` of the settings on the card's task.
    fn run_action_at(&mut self, index: usize, auth: &Auth) {
        let Some(key) = self.card_key() else {
            return;
        };
        if let Some(action) = actions::actions().into_iter().nth(index) {
            self.run_action(&key, action, auth);
        }
    }

    fn send_comment(&mut self, auth: &Auth) {
        let text_ = self.draft.trim().to_string();
        let Some(key) = self.card_key() else {
            return;
        };
        if text_.is_empty() {
            return;
        }
        match api::add_comment(auth, &key, &text_) {
            Ok(comment) => {
                self.draft.clear();
                if let Some(card) = self.card.as_mut() {
                    card.comments.push(comment);
                    card.history_loaded = false;
                }
                self.mark_changed(&key);
            }
            Err(err) => {
                notify::warning(&tr("Couldn't send the comment"), Some(&err.message()));
            }
        }
        self.render(auth);
    }

    fn filter_picker(&mut self) {
        let current = match &self.filter {
            Filter::Mine => "mine".to_string(),
            Filter::Following => "following".to_string(),
            Filter::Queue(key) => format!("queue:{key}"),
            Filter::Query(_) => "query".to_string(),
        };
        let mark = |item: picker::Item, key: &str| {
            if key == current { item.icon("check") } else { item }
        };
        let mut menu = Picker::menu()
            .item(mark(picker::item("mine", &tr("My Tasks")), "mine"))
            .item(mark(picker::item("following", &tr("Following")), "following"));
        for queue in &self.queues {
            let key = format!("queue:{}", queue.key);
            menu = menu.item(mark(
                picker::item(&key, &queue.name)
                    .detail(&queue.key)
                    .group(&tr("Queues")),
                &key,
            ));
        }
        menu = menu.item(mark(
            picker::item("query", &tr("Query…")).group(&tr("Other")),
            "query",
        ));
        let id = menu.under(WINDOW, "filter").open();
        self.pickers.insert(id, PickerPurpose::Filter);
    }

    fn status_picker(&mut self, auth: &Auth) {
        let Some(key) = self.card_key() else {
            return;
        };
        let transitions = match api::transitions(auth, &key) {
            Ok(transitions) => transitions,
            Err(err) => {
                notify::warning(&tr("Couldn't read the transitions"), Some(&err.message()));
                return;
            }
        };
        if transitions.is_empty() {
            notify::info(&tr("The task's status has no transitions"));
            return;
        }
        let items = transitions.iter().map(|transition| {
            let item = picker::item(&transition.id, &transition.display);
            match &transition.to {
                Some(to) if to.display != transition.display => {
                    item.detail(&format!("→ {}", to.display))
                }
                _ => item,
            }
        });
        let id = Picker::menu().items(items).under(WINDOW, "status").open();
        self.pickers.insert(id, PickerPurpose::Status(key));
    }

    /// Open Task (⌥⇧N): a live picker over the tasks — mine at once, a key or words searched.
    pub(super) fn open_task_picker(&mut self, auth: &Auth) {
        if auth.headers().is_none() || auth.organization().is_none() {
            notify::info(&tr("Sign in to Yandex and set the organization in the plugin's settings"));
            system::open_settings();
            return;
        }
        let mine = if self.filter == Filter::Mine && self.list.loaded_at.is_some() {
            Ok(self.list.issues.clone())
        } else {
            let query = settings::text(MY_QUERY)
                .unwrap_or_else(|| "Assignee: me() Resolution: empty()".to_string());
            api::search(auth, Search::Query(&query), OPEN_TASK_FIRST).map(|(issues, _)| issues)
        };
        self.open_task_items = mine.unwrap_or_else(|err| {
            log::warn(&format!("Open Task: {}", err.message()));
            Vec::new()
        });
        let group = tr("My Tasks");
        let id = Picker::new(&tr("Find a task by its key or words"))
            .live()
            .items(self.open_task_items.iter().map(|issue| task_item(issue, &group)))
            .empty_text(&tr("No tasks found"))
            .open();
        self.pickers.insert(id, PickerPurpose::OpenTask);
    }

    pub(super) fn open_task_query(&mut self, id: u64, query: &str, auth: &Auth) {
        let query = query.trim();
        let mine = tr("My Tasks");
        let lower = query.to_lowercase();
        let local: Vec<&Issue> = self
            .open_task_items
            .iter()
            .filter(|issue| {
                lower.is_empty()
                    || issue.key.to_lowercase().contains(&lower)
                    || issue.summary.to_lowercase().contains(&lower)
            })
            .collect();
        if query.is_empty() {
            picker::update(id, local.iter().map(|issue| task_item(issue, &mine)), false);
            return;
        }
        picker::update(id, local.iter().map(|issue| task_item(issue, &mine)), true);
        let found = if words::is_key(query) {
            api::search(auth, Search::Keys(&[query.to_ascii_uppercase()]), OPEN_TASK_FIRST)
        } else {
            let search = format!(
                "Summary: {} \"Sort by\": Updated DESC",
                words::quote(query)
            );
            api::search(auth, Search::Query(&search), OPEN_TASK_FIRST)
        };
        let found = found.map(|(issues, _)| issues).unwrap_or_else(|err| {
            log::warn(&format!("Open Task: {}", err.message()));
            Vec::new()
        });
        let found_group = tr("Found");
        let mut items: Vec<picker::Item> =
            local.iter().map(|issue| task_item(issue, &mine)).collect();
        items.extend(
            found
                .iter()
                .filter(|issue| !local.iter().any(|known| known.key == issue.key))
                .map(|issue| task_item(issue, &found_group)),
        );
        picker::update(id, items, false);
    }

    pub(super) fn picked(&mut self, purpose: PickerPurpose, key: Option<String>, auth: &Auth) {
        let Some(key) = key else {
            return;
        };
        match purpose {
            PickerPurpose::OpenTask => self.open_card(&key, auth),
            PickerPurpose::Filter => match key.as_str() {
                "mine" => self.set_filter(Filter::Mine, auth),
                "following" => self.set_filter(Filter::Following, auth),
                "query" => {
                    let last = match &self.filter {
                        Filter::Query(query) => query.clone(),
                        _ => self.filter_query(),
                    };
                    let id = dialog::AskText::new(&tr("Tasks by a Query"), &tr("Show"))
                        .message(&tr("A query in the Tracker query language"))
                        .text(&last)
                        .send();
                    self.asked.insert(id, Asked::CustomQuery);
                }
                queue => {
                    if let Some(queue) = queue.strip_prefix("queue:") {
                        self.set_filter(Filter::Queue(queue.to_string()), auth);
                    }
                }
            },
            PickerPurpose::Status(task) => {
                match api::execute(auth, &task, &key) {
                    Ok(()) => self.mark_changed(&task),
                    Err(err) => {
                        notify::warning(&tr("Couldn't change the status"), Some(&err.message()));
                    }
                }
                self.load_card(&task, auth);
                self.render(auth);
            }
            PickerPurpose::TaskMenu => self.task_menu_chosen(&key, auth),
            PickerPurpose::Field { task, field } => self.field_picked(&task, field, &key, auth),
        }
    }
}

/// A task as an item of the Open Task picker.
fn task_item(issue: &Issue, group: &str) -> picker::Item {
    picker::item(&issue.key, &issue.summary)
        .spans([
            span(&issue.key).code().tone(Tone::Dim),
            span("  "),
            span(&issue.summary),
        ])
        .detail(issue.status_name())
        .group(group)
}

/// A status's badge color by its kind.
pub(super) fn status_tone(kind: StatusKind) -> Tone {
    match kind {
        StatusKind::New => Tone::Normal,
        StatusKind::InProgress => Tone::Accent,
        StatusKind::Paused => Tone::Warning,
        StatusKind::Done => Tone::Success,
    }
}

/// "until 20 Oct".
fn deadline_text(deadline: &str) -> String {
    let language = flux_plugin_api::host::i18n::language();
    match time::parse_date(deadline) {
        Some((year, month, day)) => {
            let today = time::civil(time::now(), time::offset_at(time::now()));
            trf(
                "until {0}",
                &[&time::day_month(year, month, day, today.year, &language)],
            )
        }
        None => deadline.to_string(),
    }
}

/// An action's hint: its mode and whether it takes the task to work.
fn action_tooltip(action: &actions::Action) -> String {
    let mut parts = Vec::new();
    if action.start {
        parts.push(tr("Takes the task to work, then sends Claude the prompt"));
    } else {
        parts.push(tr("Sends Claude the prompt"));
    }
    if action.mode == actions::Mode::Plan {
        parts.push(tr("in plan mode"));
    }
    parts.join(" ")
}

/// A change without fields: what happened.
fn change_kind(kind: &str) -> String {
    match kind {
        "IssueCreated" => tr("Created the task"),
        "IssueCommentAdded" => tr("Added a comment"),
        "IssueCommentUpdated" => tr("Edited a comment"),
        "IssueLinked" => tr("Linked a task"),
        "IssueUnlinked" => tr("Removed a link"),
        "IssueAttachmentAdded" => tr("Attached a file"),
        "IssueWorkflow" => tr("Changed the status"),
        "IssueWorklogAdded" => tr("Logged time"),
        other => other.to_string(),
    }
}
