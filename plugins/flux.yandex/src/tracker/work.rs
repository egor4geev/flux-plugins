//! The actions of the settings on a task, and the work on a task: an action with `start` takes the
//! task to work in one click — the status, the branch, the task's tabs, the timer, as the `work-*`
//! settings say — then Claude gets the action's prompt filled from the card; the active task in
//! the status bar with its timer (counted while Flux runs) and menu; Close Active Task and Log Time
//! (forms). The active task is kept in the plugin's storage (`active-task`, read by the board too).

use std::collections::HashMap;
use std::time::Duration;

use flux_plugin_api::dialog::{Form, Values};
use flux_plugin_api::git::{self, Branch};
use flux_plugin_api::picker::{self, Picker};
use flux_plugin_api::{claude, contexts, log, notify, status, storage, timers, tr, trf};
use serde::{Deserialize, Serialize};

use super::actions::{self, Action, Mode, PromptData};
use super::model::{Comment, Issue, IssueLink, Transition};
use super::{Asked, PickerPurpose, Tracker, api, open_in_browser};
use crate::auth::Auth;
use crate::settings::{
    BRANCH_TEMPLATE, COMMIT_TEMPLATE, OFFER_WORKLOG, WORK_BRANCH, WORK_CONTEXT, WORK_STATUS,
    WORK_TIMER,
};
use crate::{settings, text, time};

/// The status bar item of the manifest.
const STATUS_ITEM: &str = "active-task";
/// Where the active task is kept: the board reads it too.
const ACTIVE_TASK: &str = "active-task";
/// The context of the tabs from before any task (Close brings them back).
const NO_TASK_CONTEXT: &str = "default";
/// The timer's step.
const TICK: Duration = Duration::from_secs(30);
/// A longer gap between ticks is a sleep of the Mac, not work: it counts as one step.
const MAX_STEP: i64 = 90;

/// The task in work.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(super) struct ActiveTask {
    pub key: String,
    pub summary: String,
    /// The task's branch.
    pub branch: Option<String>,
    /// The branch the work started from: Close offers to go back to it.
    pub start_branch: Option<String>,
    /// The repository's work tree.
    pub root: Option<String>,
    /// The time worked, in seconds.
    pub seconds: i64,
    /// The timer runs.
    pub running: bool,
}

/// A take to work waiting for its branch: the rest — the tabs, the timer, Claude — comes after.
pub(super) struct Start {
    issue: Issue,
    links: Vec<IssueLink>,
    comments: Vec<Comment>,
    action: Action,
    /// The task in work before this one: its tabs are saved under it.
    previous: Option<ActiveTask>,
    branch: Option<String>,
    start_branch: Option<String>,
    root: Option<String>,
}

pub(super) enum GitOp {
    Start(Box<Start>),
    /// Close's switch back to the branch the work started from.
    Back,
}

#[derive(Default)]
pub(super) struct Work {
    active: Option<ActiveTask>,
    timer: Option<u64>,
    last_tick: Option<i64>,
    pub(super) pending: HashMap<u64, GitOp>,
}

impl Work {
    /// The active task of the last session: its status bar item, its commit template, its timer.
    pub fn restore(&mut self) {
        self.active = storage::get::<ActiveTask>(ACTIVE_TASK).filter(|task| !task.key.is_empty());
        if let Some(task) = &self.active {
            set_commit_template(task);
        }
        self.last_tick = Some(time::now());
        self.show_status_item();
        self.keep_ticking();
    }

    /// Counts the time up to now and keeps it (the plugin is about to stop).
    pub fn save_time(&mut self) {
        self.count();
        self.persist();
    }

    pub fn active(&self) -> Option<&ActiveTask> {
        self.active.as_ref()
    }

    pub fn active_key(&self) -> Option<String> {
        self.active.as_ref().map(|task| task.key.clone())
    }

    /// The timer's reading: "1:05".
    pub fn reading(&self) -> String {
        let seconds = self.active.as_ref().map_or(0, |task| task.seconds);
        time::hours_minutes(seconds)
    }

    pub fn tick_timer(&self) -> Option<u64> {
        self.timer
    }

    /// A step of the timer: the time counted, the status bar item refreshed.
    pub fn tick(&mut self) {
        self.count();
        self.persist();
        self.show_status_item();
    }

    pub fn toggle_timer(&mut self) {
        self.count();
        let Some(task) = self.active.as_mut() else {
            return;
        };
        task.running = !task.running;
        self.persist();
        self.show_status_item();
        self.keep_ticking();
    }

    /// The active task's item: "TRACK-142 · 0:42", paused — with ⏸; a click opens its menu.
    pub fn show_status_item(&self) {
        let Some(task) = &self.active else {
            status::hide(STATUS_ITEM);
            return;
        };
        let mut text = task.key.clone();
        if task.running || task.seconds > 0 {
            text.push_str(&format!(" · {}", time::hours_minutes(task.seconds)));
        }
        if !task.running && task.seconds > 0 {
            text.push_str(" ⏸");
        }
        let tooltip = format!("{}\n{}", task.summary, tr("The task in work: its menu"));
        status::item(&text)
            .tooltip(&tooltip)
            .command("task-menu")
            .show(STATUS_ITEM);
    }

    /// Adds the time since the last step, while the timer runs.
    fn count(&mut self) {
        let now = time::now();
        let last = self.last_tick.replace(now).unwrap_or(now);
        if let Some(task) = self.active.as_mut().filter(|task| task.running) {
            task.seconds += (now - last).clamp(0, MAX_STEP);
        }
    }

    fn persist(&self) {
        match &self.active {
            Some(task) => storage::set(ACTIVE_TASK, task),
            None => storage::remove(ACTIVE_TASK),
        }
    }

    /// The timer runs while a task's does.
    fn keep_ticking(&mut self) {
        let running = self.active.as_ref().is_some_and(|task| task.running);
        match (running, self.timer) {
            (true, None) => {
                self.last_tick = Some(time::now());
                self.timer = Some(timers::every(TICK));
            }
            (false, Some(timer)) => {
                timers::cancel(timer);
                self.timer = None;
            }
            _ => {}
        }
    }

    /// Another task becomes the active one: the previous one's time is kept for its return.
    fn set_active(&mut self, task: Option<ActiveTask>) {
        self.count();
        if let Some(previous) = &self.active {
            storage::set(&time_key(&previous.key), &previous.seconds);
        }
        self.active = task;
        self.persist();
        self.show_status_item();
        self.keep_ticking();
    }
}

/// Where a task's time is kept while another one is in work.
fn time_key(key: &str) -> String {
    format!("time:{key}")
}

/// The commit message template of the active task (`commit-template`: {key} {summary}).
fn set_commit_template(task: &ActiveTask) {
    let template = settings::text(COMMIT_TEMPLATE).unwrap_or_else(|| "{key} {summary}".into());
    let message = text::fill(
        &template,
        &[("key", task.key.as_str()), ("summary", task.summary.as_str())],
    );
    git::commit_template(message.trim(), Some(&task.key));
}

/// The transition that takes a task to work: to the status `inProgress`, else one whose target
/// reads as work in progress (not a review).
pub(super) fn in_progress_transition(transitions: &[Transition]) -> Option<&Transition> {
    let target = |transition: &Transition| {
        transition
            .to
            .as_ref()
            .map(|to| to.key_or_id().to_ascii_lowercase())
            .unwrap_or_default()
    };
    transitions
        .iter()
        .find(|transition| target(transition) == "inprogress")
        .or_else(|| {
            transitions
                .iter()
                .find(|transition| target(transition).contains("progress"))
        })
        .or_else(|| {
            transitions.iter().find(|transition| {
                let display = transition.display.to_lowercase();
                display.contains("в работу") || display.contains("start progress")
            })
        })
}

/// The task's branch among the repository's: a local one whose name (or a part of it after `/`)
/// starts with the key, else such a remote one; the name to check out.
pub(super) fn task_branch(branches: &[Branch], key: &str) -> Option<String> {
    let key = key.to_ascii_lowercase();
    let names_it = |name: &str| {
        name.split('/').any(|part| {
            let part = part.to_ascii_lowercase();
            part == key
                || part
                    .strip_prefix(&key)
                    .is_some_and(|rest| rest.starts_with(['-', '_', '.']))
        })
    };
    let local = branches
        .iter()
        .find(|branch| !branch.remote && names_it(&branch.name));
    let remote = || {
        branches.iter().find(|branch| {
            branch.remote
                && names_it(branch.name.split_once('/').map_or(&branch.name[..], |(_, name)| name))
        })
    };
    local.or_else(remote).map(|branch| branch.name.clone())
}

/// A remote branch's local name: "origin/TRACK-1-x" → "TRACK-1-x".
fn local_name(branch: &str, branches: &[Branch]) -> String {
    let remote = branches
        .iter()
        .any(|known| known.remote && known.name == branch);
    match branch.split_once('/') {
        Some((_, name)) if remote => name.to_string(),
        _ => branch.to_string(),
    }
}

impl Tracker {
    /// Runs an action of the settings on the task: with `start` it takes the task to work first;
    /// Claude gets the prompt filled from the card.
    pub(super) fn run_action(&mut self, key: &str, action: Action, auth: &Auth) {
        let (issue, links, comments) = match self.task_data(key, auth) {
            Ok(data) => data,
            Err(err) => {
                notify::warning(&trf("Couldn't read {0}", &[&key]), Some(&err.message()));
                return;
            }
        };
        if action.start {
            self.start_work(issue, links, comments, action, auth);
        } else {
            let branch = self.branch_of(&issue);
            send_prompt(&issue, &links, &comments, &branch, &action);
        }
    }

    /// The task's branch for a prompt: the one it's worked on, else the name it would get.
    fn branch_of(&self, issue: &Issue) -> String {
        if let Some(task) = self.work.active().filter(|task| task.key == issue.key)
            && let Some(branch) = &task.branch
        {
            return branch.clone();
        }
        new_branch_name(issue)
    }

    /// Takes the task to work: the status now, then the branch (Flux's own flow; the rest waits
    /// for it), the tabs, the timer, the commit template, Claude.
    fn start_work(
        &mut self,
        issue: Issue,
        links: Vec<IssueLink>,
        comments: Vec<Comment>,
        action: Action,
        auth: &Auth,
    ) {
        let key = issue.key.clone();
        // A task in review or paused goes back to work too; one in progress already stays.
        let in_progress = issue
            .status
            .as_ref()
            .is_some_and(|status| status.key_or_id().eq_ignore_ascii_case("inProgress"));
        if settings::on(WORK_STATUS) && !in_progress {
            match api::transitions(auth, &key) {
                Ok(transitions) => match in_progress_transition(&transitions) {
                    Some(transition) => match api::execute(auth, &key, &transition.id) {
                        Ok(()) => self.mark_changed(&key),
                        Err(err) => {
                            notify::warning(
                                &trf("Couldn't move {0} to work", &[&key]),
                                Some(&err.message()),
                            );
                        }
                    },
                    None => log::info(&format!(
                        "{key}: no transition to a status in progress from \"{}\"",
                        issue.status_name()
                    )),
                },
                Err(err) => {
                    notify::warning(
                        &trf("Couldn't move {0} to work", &[&key]),
                        Some(&err.message()),
                    );
                }
            }
        }
        let previous = self.work.active().cloned();
        let same = previous.as_ref().is_some_and(|task| task.key == key);
        let mut start = Start {
            issue,
            links,
            comments,
            action,
            previous: previous.filter(|_| !same),
            branch: None,
            start_branch: None,
            root: None,
        };
        if same {
            start.branch = self.work.active().and_then(|task| task.branch.clone());
        } else if settings::on(WORK_BRANCH)
            && let Some(repository) = git::repositories().into_iter().next()
        {
            start.root = Some(repository.root.clone());
            start.start_branch = repository.branch.clone();
            match git::branches(&repository.root) {
                Ok(branches) => {
                    let existing = task_branch(&branches, &key);
                    let wanted = existing
                        .as_deref()
                        .map(|branch| local_name(branch, &branches))
                        .unwrap_or_else(|| new_branch_name(&start.issue));
                    start.branch = Some(wanted.clone());
                    if repository.branch.as_deref() != Some(wanted.as_str()) {
                        let id = match &existing {
                            Some(branch) => git::checkout(&repository.root, branch),
                            None => git::new_branch(&repository.root, &wanted, None),
                        };
                        self.work.pending.insert(id, GitOp::Start(Box::new(start)));
                        return;
                    }
                }
                Err(err) => {
                    notify::warning(&tr("Couldn't read the branches"), Some(&err));
                }
            }
        }
        self.finish_start(start, auth);
    }

    /// The rest of taking to work, after the branch: the tabs, the active task and its timer, the
    /// commit template, Claude.
    fn finish_start(&mut self, start: Start, auth: &Auth) {
        let key = start.issue.key.clone();
        let same = self.work.active_key().as_deref() == Some(key.as_str());
        if !same {
            if settings::on(WORK_CONTEXT) {
                let saved = match &start.previous {
                    Some(previous) => (previous.key.clone(), context_title(previous)),
                    None => (NO_TASK_CONTEXT.to_string(), tr("Without a task")),
                };
                if let Err(err) =
                    contexts::switch(Some((saved.0.as_str(), saved.1.as_str())), Some(&key))
                {
                    log::warn(&format!("Switching the tabs to {key}: {err}"));
                }
            }
            let task = ActiveTask {
                key: key.clone(),
                summary: start.issue.summary.clone(),
                branch: start.branch.clone(),
                start_branch: start.start_branch.clone(),
                root: start.root.clone(),
                seconds: storage::get(&time_key(&key)).unwrap_or(0),
                running: settings::on(WORK_TIMER),
            };
            storage::remove(&time_key(&key));
            set_commit_template(&task);
            self.work.set_active(Some(task));
        }
        let branch = start.branch.clone().unwrap_or_else(|| new_branch_name(&start.issue));
        send_prompt(
            &start.issue,
            &start.links,
            &start.comments,
            &branch,
            &start.action,
        );
        self.mark_changed(&key);
        self.render(auth);
    }

    /// A Git operation of the work ended.
    pub(super) fn git_done(&mut self, id: u64, result: Result<(), String>, auth: &Auth) {
        match self.work.pending.remove(&id) {
            Some(GitOp::Start(start)) => {
                if let Err(err) = result {
                    notify::warning(
                        &trf("The branch of {0} didn't work out", &[&start.issue.key]),
                        Some(&err),
                    );
                }
                self.finish_start(*start, auth);
            }
            Some(GitOp::Back) => {
                if let Err(err) = result {
                    notify::warning(&tr("Couldn't switch back to the branch"), Some(&err));
                }
            }
            None => {}
        }
    }

    /// Discuss with Claude: a new session with the task attached; the user writes the request.
    pub(super) fn discuss(&mut self, key: &str, auth: &Auth) {
        let (issue, links, comments) = match self.task_data(key, auth) {
            Ok(data) => data,
            Err(err) => {
                notify::warning(&trf("Couldn't read {0}", &[&key]), Some(&err.message()));
                return;
            }
        };
        let branch = self.branch_of(&issue);
        let data = PromptData {
            issue: &issue,
            links: &links,
            comments: &comments,
            branch: &branch,
        };
        let session = claude::session()
            .title(&format!("{}: {}", issue.key, text::ellipsis(&issue.summary, 60)))
            .attach(&issue.key, &actions::task_markdown(&data));
        if let Err(err) = session.start() {
            notify::warning(&tr("Couldn't open a Claude session"), Some(&err));
        }
    }

    /// The active task's menu, above its status bar item.
    pub(super) fn task_menu(&mut self) {
        let Some(task) = self.work.active().cloned() else {
            return;
        };
        let mut menu = Picker::menu().item(picker::item("show", &tr("Show Task")).icon("info"));
        for (index, action) in actions::actions().iter().enumerate() {
            menu = menu.item(
                picker::item(&format!("action:{index}"), &action.title())
                    .icon(if action.mode == Mode::Plan { "plan" } else { "claude" })
                    .group(&tr("Claude")),
            );
        }
        let timer = if task.running {
            tr("Pause the Timer")
        } else {
            tr("Resume the Timer")
        };
        let work = tr("Work");
        menu = menu
            .item(picker::item("discuss", &tr("Discuss with Claude")).icon("claude").group(&tr("Claude")))
            .item(picker::item("log", &tr("Log Time…")).icon("clock").group(&work))
            .item(picker::item("timer", &timer).group(&work))
            .item(picker::item("close", &tr("Close Task…")).group(&work))
            .item(picker::item("browser", &tr("Open in Browser")).icon("globe").group(&work));
        let id = menu.at_status_item(STATUS_ITEM).open();
        self.pickers.insert(id, PickerPurpose::TaskMenu);
    }

    pub(super) fn task_menu_chosen(&mut self, choice: &str, auth: &Auth) {
        let Some(key) = self.work.active_key() else {
            return;
        };
        match choice {
            "show" => self.open_card(&key, auth),
            "discuss" => self.discuss(&key, auth),
            "log" => self.ask_log_time(&key, auth),
            "timer" => {
                self.work.toggle_timer();
                self.render(auth);
            }
            "close" => self.close_active(auth),
            "browser" => open_in_browser(&key),
            action => {
                let index = action
                    .strip_prefix("action:")
                    .and_then(|index| index.parse::<usize>().ok());
                if let Some(action) = index.and_then(|index| actions::actions().into_iter().nth(index))
                {
                    self.run_action(&key, action, auth);
                }
            }
        }
    }

    /// Close Active Task: the form — the time to log, a comment, the status, back to the branch,
    /// the tabs from before the task.
    pub(super) fn close_active(&mut self, auth: &Auth) {
        let Some(task) = self.work.active().cloned() else {
            self.no_active_task();
            return;
        };
        self.work.count();
        let task = self.work.active().cloned().unwrap_or(task);
        let id = self.close_form(&task, auth, None, None).send();
        self.asked.insert(id, Asked::Close(task.key.clone()));
    }

    fn close_form(
        &self,
        task: &ActiveTask,
        auth: &Auth,
        values: Option<&Values>,
        error: Option<&str>,
    ) -> Form {
        let language = flux_plugin_api::host::i18n::language();
        let minutes = (task.seconds / 60).max(1) as u32;
        let mut form = Form::new(&trf("Close {0}", &[&task.key]), &tr("Close"))
            .message(&task.summary);
        if let Some(error) = error {
            form = form.error(error);
        }
        let value = |key: &str, default: String| {
            values
                .and_then(|values| values.get(key))
                .map(str::to_string)
                .unwrap_or(default)
        };
        let on = |key: &str, default: bool| {
            values.map_or(default, |values| values.on(key))
        };
        if settings::on(OFFER_WORKLOG) {
            form = form
                .toggle("log", &tr("Log the time"), on("log", true))
                .text(
                    "spent",
                    &tr("Time spent"),
                    &value("spent", text::format_duration(minutes, &language)),
                )
                .placeholder(&tr("1h 30m"))
                .enabled_by("log")
                .multiline("comment", &tr("Comment"), &value("comment", String::new()))
                .placeholder(&tr("What was done"))
                .enabled_by("log");
        }
        let mut statuses = vec![(String::new(), tr("Don't change"))];
        if let Ok(transitions) = api::transitions(auth, &task.key) {
            statuses.extend(
                transitions
                    .into_iter()
                    .map(|transition| (transition.id, transition.display)),
            );
        }
        let options: Vec<(&str, &str)> = statuses
            .iter()
            .map(|(id, title)| (id.as_str(), title.as_str()))
            .collect();
        let chosen = value("status", String::new());
        form = form.choice("status", &tr("Move to"), &options, Some(&chosen));
        if let Some(branch) = &task.start_branch
            && task.branch.as_deref() != Some(branch.as_str())
        {
            form = form.toggle(
                "back",
                &trf("Switch back to {0}", &[branch]),
                on("back", true),
            );
        }
        if settings::on(WORK_CONTEXT) {
            form = form.toggle(
                "tabs",
                &tr("Bring back the tabs from before the task"),
                on("tabs", true),
            );
        }
        form
    }

    /// Log Time…: the form — the time, a comment.
    pub(super) fn ask_log_time(&mut self, key: &str, auth: &Auth) {
        let _ = auth;
        self.work.count();
        let id = self.log_time_form(key, None, None).send();
        self.asked.insert(id, Asked::LogTime(key.to_string()));
    }

    fn log_time_form(&self, key: &str, values: Option<&Values>, error: Option<&str>) -> Form {
        let language = flux_plugin_api::host::i18n::language();
        let timer = self
            .work
            .active()
            .filter(|task| task.key == key && task.seconds >= 60)
            .map(|task| text::format_duration((task.seconds / 60) as u32, &language))
            .unwrap_or_default();
        let value = |name: &str, default: String| {
            values
                .and_then(|values| values.get(name))
                .map(str::to_string)
                .unwrap_or(default)
        };
        let mut form = Form::new(&trf("Log Time on {0}", &[&key]), &tr("Log"))
            .text("spent", &tr("Time spent"), &value("spent", timer))
            .placeholder(&tr("1h 30m"))
            .multiline("comment", &tr("Comment"), &value("comment", String::new()))
            .placeholder(&tr("What was done"));
        if let Some(error) = error {
            form = form.error(error);
        }
        form
    }

    pub(super) fn form_answered(
        &mut self,
        asked: Asked,
        values: Option<Vec<(String, String)>>,
        auth: &Auth,
    ) {
        let Some(values) = values.map(Values::from) else {
            return;
        };
        match asked {
            Asked::Close(key) => self.close_answered(&key, values, auth),
            Asked::LogTime(key) => self.log_time_answered(&key, values, auth),
            Asked::CustomQuery => {}
            asked @ (Asked::Deadline(_)
            | Asked::StoryPoints(_)
            | Asked::Tags(_)
            | Asked::EditTask(_)
            | Asked::OtherField { .. }) => self.edit_form_answered(asked, values, auth),
        }
    }

    fn log_time_answered(&mut self, key: &str, values: Values, auth: &Auth) {
        let ask_again = |tracker: &mut Tracker, error: &str| {
            let id = tracker.log_time_form(key, Some(&values), Some(error)).send();
            tracker.asked.insert(id, Asked::LogTime(key.to_string()));
        };
        let Some(minutes) = text::parse_duration(&values.text("spent")).filter(|m| *m > 0) else {
            return ask_again(self, &tr("Write the time as 1h 30m, 45m or 1.5h"));
        };
        if let Err(err) = log_work(auth, key, minutes, &values.text("comment")) {
            return ask_again(self, &err);
        }
        if let Some(task) = self.work.active.as_mut().filter(|task| task.key == key) {
            task.seconds = (task.seconds - i64::from(minutes) * 60).max(0);
            self.work.persist();
            self.work.show_status_item();
        }
        let language = flux_plugin_api::host::i18n::language();
        notify::success(&trf(
            "Logged {0} on {1}",
            &[&text::format_duration(minutes, &language), &key],
        ));
        self.mark_changed(key);
        self.render(auth);
    }

    fn close_answered(&mut self, key: &str, values: Values, auth: &Auth) {
        let Some(task) = self.work.active().cloned().filter(|task| task.key == key) else {
            return;
        };
        let language = flux_plugin_api::host::i18n::language();
        let mut logged = None;
        if settings::on(OFFER_WORKLOG) && values.on("log") {
            let ask_again = |tracker: &mut Tracker, error: &str| {
                let id = tracker
                    .close_form(&task, auth, Some(&values), Some(error))
                    .send();
                tracker.asked.insert(id, Asked::Close(key.to_string()));
            };
            let Some(minutes) = text::parse_duration(&values.text("spent")).filter(|m| *m > 0)
            else {
                return ask_again(self, &tr("Write the time as 1h 30m, 45m or 1.5h"));
            };
            if let Err(err) = log_work(auth, key, minutes, &values.text("comment")) {
                return ask_again(self, &err);
            }
            logged = Some(minutes);
        }
        let transition = values.text("status");
        if !transition.is_empty()
            && let Err(err) = api::execute(auth, key, &transition)
        {
            notify::warning(&trf("Couldn't change the status of {0}", &[&key]), Some(&err.message()));
        }
        if values.on("back")
            && let (Some(root), Some(branch)) = (&task.root, &task.start_branch)
        {
            let id = git::checkout(root, branch);
            self.work.pending.insert(id, GitOp::Back);
        }
        if settings::on(WORK_CONTEXT) && values.on("tabs") {
            let title = context_title(&task);
            if let Err(err) = contexts::switch(Some((key, title.as_str())), Some(NO_TASK_CONTEXT)) {
                log::warn(&format!("Switching the tabs back: {err}"));
            }
        }
        git::clear_commit_template();
        storage::remove(&time_key(key));
        self.work.active = None;
        self.work.persist();
        self.work.show_status_item();
        self.work.keep_ticking();
        match logged {
            Some(minutes) => notify::success(&trf(
                "{0} is closed: {1} logged",
                &[&key, &text::format_duration(minutes, &language)],
            )),
            None => notify::success(&trf("{0} is closed", &[&key])),
        };
        self.mark_changed(key);
        self.render(auth);
    }
}

/// A context's title: "TRACK-142 Fix the sign-in".
fn context_title(task: &ActiveTask) -> String {
    text::ellipsis(&format!("{} {}", task.key, task.summary), 80)
}

/// The branch a task would get by `branch-template`.
fn new_branch_name(issue: &Issue) -> String {
    let template = settings::text(BRANCH_TEMPLATE).unwrap_or_else(|| "{key}-{summary}".into());
    let kind = issue
        .kind
        .as_ref()
        .map(|kind| kind.key_or_id())
        .unwrap_or_default();
    text::branch_name(&template, &issue.key, &issue.summary, &kind)
}

/// Claude gets the action's prompt filled from the card, at once, in the action's mode.
fn send_prompt(
    issue: &Issue,
    links: &[IssueLink],
    comments: &[Comment],
    branch: &str,
    action: &Action,
) {
    let data = PromptData {
        issue,
        links,
        comments,
        branch,
    };
    let prompt = actions::fill_prompt(&action.prompt, &data);
    if prompt.is_empty() {
        notify::warning(
            &trf("The action \"{0}\" has no prompt", &[&action.title()]),
            Some(&tr("Write it in the plugin's settings: Claude actions")),
        );
        return;
    }
    let mut session = claude::session()
        .title(&format!("{}: {}", issue.key, text::ellipsis(&issue.summary, 60)))
        .prompt(&prompt)
        .send();
    if action.mode == Mode::Plan {
        session = session.plan_mode();
    }
    if let Err(err) = session.start() {
        notify::warning(&tr("Couldn't open a Claude session"), Some(&err));
    }
}

/// Logs `minutes` on the task, as if the work started that long ago.
fn log_work(auth: &Auth, key: &str, minutes: u32, comment: &str) -> Result<(), String> {
    let start = time::now() - i64::from(minutes) * 60;
    let start = time::iso_with_offset(start, time::offset_at(start));
    api::add_worklog(auth, key, &start, minutes, comment).map_err(|err| err.message())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tracker::model::Named;

    fn transition(id: &str, display: &str, to: &str) -> Transition {
        Transition {
            id: id.into(),
            display: display.into(),
            to: Some(Named {
                key: Some(to.into()),
                display: to.into(),
                ..Named::default()
            }),
        }
    }

    #[test]
    fn the_transition_to_work() {
        let transitions = [
            transition("review", "На ревью", "inReview"),
            transition("start", "В работу", "inProgress"),
            transition("close", "Закрыть", "closed"),
        ];
        assert_eq!(in_progress_transition(&transitions).unwrap().id, "start");
        let custom = [
            transition("review", "На ревью", "inReview"),
            transition("go", "Начать", "startProgress"),
        ];
        assert_eq!(in_progress_transition(&custom).unwrap().id, "go");
        let by_name = [transition("t1", "Взять в работу", "doing")];
        assert_eq!(in_progress_transition(&by_name).unwrap().id, "t1");
        assert!(in_progress_transition(&[transition("close", "Закрыть", "closed")]).is_none());
    }

    fn branch(name: &str, remote: bool) -> Branch {
        Branch {
            name: name.into(),
            remote,
            current: false,
        }
    }

    #[test]
    fn the_task_branch_local_first() {
        let branches = [
            branch("main", false),
            branch("TRACK-1420-other", false),
            branch("origin/TRACK-142-sso", true),
            branch("feature/track-142-oshibka", false),
        ];
        assert_eq!(
            task_branch(&branches, "TRACK-142").as_deref(),
            Some("feature/track-142-oshibka")
        );
        let remote_only = [branch("main", false), branch("origin/TRACK-142-sso", true)];
        assert_eq!(
            task_branch(&remote_only, "TRACK-142").as_deref(),
            Some("origin/TRACK-142-sso")
        );
        assert_eq!(local_name("origin/TRACK-142-sso", &remote_only), "TRACK-142-sso");
        assert_eq!(local_name("main", &remote_only), "main");
        assert!(task_branch(&[branch("TRACK-1420", false)], "TRACK-142").is_none());
        assert_eq!(
            task_branch(&[branch("TRACK-142", false)], "TRACK-142").as_deref(),
            Some("TRACK-142")
        );
    }
}
