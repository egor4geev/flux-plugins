//! Calendar: Yandex Calendar over CalDAV (`https://caldav.yandex.ru`: the principal, the calendar
//! home, the calendars, `REPORT calendar-query` over yesterday … tomorrow — expanded by the server,
//! or recurrences expanded here, [`ical`]), with an app password (`caldav-login` +
//! `caldav-password`) or the sign-in's token.
//!
//! - The status bar item `next-meeting`: "Daily in 12 min", "Daily now · 5 min left", "Review at
//!   15:00"; hidden when nothing more today ([`agenda::reading`]). It changes on the minute; the
//!   meetings are read again every five minutes, at a command, a sign-in, new settings.
//! - Its click (`calendar-agenda`): the agenda of today and tomorrow in a menu at the item, grouped
//!   by day ("11:00–11:15  Daily" with a Telemost badge); a choice joins the meeting (its link), or
//!   opens the day in Yandex Calendar when it has none.
//! - A reminder `remind-minutes` before a meeting: a card with Join (its argument — the meeting),
//!   expired when the meeting ends; once per meeting, across restarts.
//! - `calendar-join` from the palette: the meeting going on, else the next one within the hour.
//!
//! Requests run in the background (`http.start`): the Tracker's window answers meanwhile. A
//! failed round keeps the meetings it had (the network comes back); a refused token or password
//! clears them and says so once.

mod agenda;
mod caldav;
mod ical;
mod xml;

use std::collections::HashMap;
use std::time::Duration;

use base64::Engine as _;
use flux_plugin_api::host::http::HttpError;
use flux_plugin_api::host::notifications;
use flux_plugin_api::host::ui::Tone;
use flux_plugin_api::notify::Notice;
use flux_plugin_api::view::{SpanStyle, span};
use flux_plugin_api::{CommandContext, Event};
use flux_plugin_api::{http, log, picker, secrets, setting, status, storage, system, timers};
use flux_plugin_api::{tr, trf};

use crate::auth::{Auth, Credentials};
use crate::settings;
use crate::time;
use agenda::{Day, Reading};
use caldav::CalendarInfo;
use ical::{Clock, Meeting};

/// The calendar's commands start with it (`calendar-agenda`, `calendar-join`,
/// `calendar-refresh`).
pub const COMMAND_PREFIX: &str = "calendar-";
const AGENDA: &str = "calendar-agenda";
const JOIN: &str = "calendar-join";
const REFRESH: &str = "calendar-refresh";

/// The status bar item of the manifest.
const ITEM: &str = "next-meeting";
/// The meetings are read again this often.
const REFRESH_EVERY: i64 = 5 * 60;
/// The longest answer read (a calendar's events).
const MAX_BODY: usize = 16 << 20;
/// Storage: the calendars found, with the user's emails (a restart doesn't look for them again).
const STORED_CALENDARS: &str = "calendar.calendars";
/// Storage: the meetings reminded of, with their ends.
const STORED_REMINDED: &str = "calendar.reminded";

/// The user's clock, as Flux tells it.
struct UserClock {
    zone: String,
}

impl Clock for UserClock {
    fn offset(&self, at: i64) -> i32 {
        time::offset_at(at)
    }

    fn zone(&self) -> String {
        self.zone.clone()
    }
}

/// What a request in flight is for.
#[derive(Debug, Clone, PartialEq)]
enum Step {
    Principal,
    Home,
    Calendars,
    /// The events of a calendar (its name).
    Events(String),
}

struct Request {
    step: Step,
    status: u16,
    body: Vec<u8>,
}

/// A reading of the meetings in progress.
struct Round {
    /// The `Authorization` header of its requests.
    authorization: String,
    /// The account behind it: the password's or the token's (what a refusal says).
    password: bool,
    from: i64,
    to: i64,
    meetings: Vec<Meeting>,
    /// The calendars still to answer.
    waiting: usize,
}

/// The calendars found, kept between launches.
#[derive(serde::Serialize, serde::Deserialize, Default)]
struct Found {
    /// Which account found them: the `Authorization` header's hash.
    account: u64,
    calendars: Vec<(String, String)>,
    emails: Vec<String>,
}

pub struct Calendar {
    /// The meetings of the last round.
    meetings: Vec<Meeting>,
    /// The calendars to read, once found; none — find them first.
    calendars: Option<Vec<CalendarInfo>>,
    /// The user's emails: what they declined is left out.
    emails: Vec<String>,
    /// The account the calendars were found with (the `Authorization` header's hash).
    account: u64,
    requests: HashMap<u64, Request>,
    round: Option<Round>,
    /// The minute's tick.
    tick: Option<u64>,
    next_refresh: i64,
    /// The agenda's menu, while it's open.
    picker: Option<u64>,
    /// What the status bar shows: (text, tooltip); none — hidden.
    shown: Option<(String, String)>,
    /// The meetings reminded of: (key, end).
    reminded: Vec<(String, i64)>,
    /// The reminders' cards: key → (the notification, when it expires).
    cards: HashMap<String, (u64, i64)>,
    /// The last problem told to the user: not again until it changes.
    told: Option<String>,
    active: bool,
}

impl Calendar {
    pub fn new() -> Self {
        Calendar {
            meetings: Vec::new(),
            calendars: None,
            emails: Vec::new(),
            account: 0,
            requests: HashMap::new(),
            round: None,
            tick: None,
            next_refresh: 0,
            picker: None,
            shown: None,
            reminded: Vec::new(),
            cards: HashMap::new(),
            told: None,
            active: false,
        }
    }

    pub fn activate(&mut self, auth: &Auth) {
        self.active = true;
        self.reminded = storage::get(STORED_REMINDED).unwrap_or_default();
        if let Some(found) = storage::get::<Found>(STORED_CALENDARS) {
            self.account = found.account;
            self.emails = found.emails;
            self.calendars = Some(
                found
                    .calendars
                    .into_iter()
                    .map(|(href, name)| CalendarInfo { href, name })
                    .collect(),
            );
        }
        self.refresh(auth);
        self.schedule_tick();
    }

    pub fn deactivate(&mut self) {
        self.active = false;
        self.stop_requests();
        if let Some(tick) = self.tick.take() {
            timers::cancel(tick);
        }
    }

    pub fn settings_changed(&mut self, auth: &Auth) {
        // The password, the hidden calendars, the reminders: a new round says what they mean.
        self.refresh(auth);
    }

    pub fn auth_changed(&mut self, auth: &Auth) {
        self.refresh(auth);
    }

    pub fn run_command(&mut self, command: &str, context: &CommandContext, auth: &Auth) {
        match command {
            AGENDA => self.open_agenda(auth),
            JOIN => self.join(context.argument.as_deref()),
            REFRESH => self.refresh(auth),
            _ => log::warn(&format!("calendar: unknown command {command}")),
        }
    }

    /// The event is the calendar's: its tick, its requests, its menu.
    pub fn claims(&self, event: &Event) -> bool {
        match event {
            Event::Timer(timer) => self.tick == Some(*timer),
            Event::HttpResponse(head) => self.requests.contains_key(&head.request),
            Event::HttpBody((request, _)) | Event::HttpDone((request, _)) => {
                self.requests.contains_key(request)
            }
            Event::PickerChosen((picker, _)) | Event::PickerQuery((picker, _)) => {
                self.picker == Some(*picker)
            }
            _ => false,
        }
    }

    pub fn on_event(&mut self, event: Event, auth: &Auth) {
        match event {
            Event::Timer(_) => {
                self.tick = None;
                let now = time::now();
                if self.active && now >= self.next_refresh && self.round.is_none() {
                    self.refresh(auth);
                }
                self.update(now);
                self.schedule_tick();
            }
            Event::HttpResponse(head) => {
                if let Some(request) = self.requests.get_mut(&head.request) {
                    request.status = head.status;
                }
            }
            Event::HttpBody((id, chunk)) => {
                if let Some(request) = self.requests.get_mut(&id)
                    && request.body.len() + chunk.len() <= MAX_BODY
                {
                    request.body.extend_from_slice(&chunk);
                }
            }
            Event::HttpDone((id, result)) => {
                if let Some(request) = self.requests.remove(&id) {
                    self.answered(request, result, auth);
                }
            }
            Event::PickerChosen((_, key)) => {
                self.picker = None;
                if let Some(key) = key {
                    self.join(Some(&key));
                }
            }
            _ => {}
        }
    }

    // --- Reading the meetings ---

    /// The `Authorization` header and whether it is the app password's: the password beats the
    /// sign-in (a federated account may have only a password for CalDAV).
    fn authorization(&self, auth: &Auth) -> Option<(String, bool)> {
        if let Some(login) = settings::text(settings::CALDAV_LOGIN)
            && let Some(password) =
                secrets::get(settings::CALDAV_PASSWORD).filter(|password| !password.is_empty())
        {
            let pair = base64::engine::general_purpose::STANDARD.encode(format!("{login}:{password}"));
            return Some((format!("Basic {pair}"), true));
        }
        match auth.credentials() {
            Some(Credentials::OAuth(token)) => Some((format!("OAuth {token}"), false)),
            Some(Credentials::Iam(token)) => Some((format!("Bearer {token}"), false)),
            None => None,
        }
    }

    /// Reads the meetings again (a round in progress is dropped): the calendars first, if not
    /// found yet or found with another account.
    fn refresh(&mut self, auth: &Auth) {
        if !self.active {
            return;
        }
        self.stop_requests();
        let enabled = setting::<bool>(settings::CALENDAR_ENABLED).unwrap_or(true);
        let Some((authorization, password)) = self.authorization(auth).filter(|_| enabled) else {
            self.meetings.clear();
            self.update(time::now());
            return;
        };
        self.next_refresh = time::now() + REFRESH_EVERY;
        let account = fingerprint(&authorization);
        if account != self.account {
            self.account = account;
            self.calendars = None;
            self.emails.clear();
        }
        let now = time::now();
        let offset = time::offset_at(now);
        let today = agenda::day_of(now, offset);
        let start_of = |day: i64| day * 86_400 - i64::from(time::offset_at(day * 86_400));
        self.round = Some(Round {
            authorization,
            password,
            from: start_of(today - 1),
            to: start_of(today + 2),
            meetings: Vec::new(),
            waiting: 0,
        });
        match self.calendars.clone() {
            Some(calendars) => self.read_events(&calendars),
            None => self.send("PROPFIND", caldav::BASE, "0", caldav::PRINCIPAL_BODY, Step::Principal),
        }
    }

    fn send(&mut self, method: &str, url: &str, depth: &str, body: &str, step: Step) {
        let Some(round) = &self.round else {
            return;
        };
        let id = http::request(method, url)
            .header("Authorization", &round.authorization)
            .header("Depth", depth)
            .header("Content-Type", "application/xml; charset=utf-8")
            .body(body.as_bytes().to_vec())
            .timeout(Duration::from_secs(30))
            .start();
        self.requests.insert(
            id,
            Request {
                step,
                status: 0,
                body: Vec::new(),
            },
        );
    }

    /// The events of the visible calendars.
    fn read_events(&mut self, calendars: &[CalendarInfo]) {
        let hidden: Vec<String> = setting::<Vec<String>>(settings::HIDDEN_CALENDARS)
            .unwrap_or_default()
            .iter()
            .map(|name| name.trim().to_lowercase())
            .filter(|name| !name.is_empty())
            .collect();
        let visible: Vec<&CalendarInfo> = calendars
            .iter()
            .filter(|calendar| !hidden.contains(&calendar.name.trim().to_lowercase()))
            .collect();
        let Some(round) = &mut self.round else {
            return;
        };
        round.waiting = visible.len();
        let body = caldav::events_body(round.from, round.to);
        if visible.is_empty() {
            self.finish_round();
            return;
        }
        for calendar in visible {
            let url = caldav::absolute(&calendar.href);
            self.send("REPORT", &url, "1", &body, Step::Events(calendar.name.clone()));
        }
    }

    fn answered(&mut self, request: Request, result: Result<(), HttpError>, auth: &Auth) {
        if self.round.is_none() {
            return;
        }
        if let Err(err) = result {
            if err != HttpError::Cancelled {
                self.failed(&trf("Couldn't read Yandex Calendar: {0}", &[&err.message()]), false);
            }
            return;
        }
        let password = self.round.as_ref().is_some_and(|round| round.password);
        match request.status {
            200..=299 => {}
            401 => {
                let text = if password {
                    tr("Yandex Calendar didn't accept the app password: check the CalDAV login and password")
                } else {
                    tr("Yandex Calendar didn't accept the token: sign in again")
                };
                return self.failed(&text, true);
            }
            403 => return self.failed(&tr("No access to Yandex Calendar"), true),
            404 if matches!(request.step, Step::Events(_)) => {
                // A calendar went away: look for them again next time.
                self.calendars = None;
                return self.calendar_done(Vec::new());
            }
            status => {
                return self.failed(
                    &trf("Yandex Calendar answered {0}", &[&status.to_string()]),
                    false,
                );
            }
        }
        let body = String::from_utf8_lossy(&request.body).into_owned();
        match request.step {
            Step::Principal => match caldav::principal(&body) {
                Ok(Some(principal)) => {
                    self.learn_email(caldav::email_of(&principal));
                    let url = caldav::absolute(&principal);
                    self.send("PROPFIND", &url, "0", caldav::HOME_BODY, Step::Home);
                }
                _ => self.failed(&tr("Yandex Calendar didn't say where the calendars are"), false),
            },
            Step::Home => match caldav::home(&body) {
                Ok(Some(home)) => {
                    self.learn_email(caldav::email_of(&home));
                    let url = caldav::absolute(&home);
                    self.send("PROPFIND", &url, "1", caldav::CALENDARS_BODY, Step::Calendars);
                }
                _ => self.failed(&tr("Yandex Calendar didn't say where the calendars are"), false),
            },
            Step::Calendars => match caldav::calendars(&body) {
                Ok(calendars) => {
                    self.learn_email(settings::text(settings::CALDAV_LOGIN));
                    self.learn_email(auth.email());
                    storage::set(
                        STORED_CALENDARS,
                        &Found {
                            account: self.account,
                            calendars: calendars
                                .iter()
                                .map(|calendar| (calendar.href.clone(), calendar.name.clone()))
                                .collect(),
                            emails: self.emails.clone(),
                        },
                    );
                    self.calendars = Some(calendars.clone());
                    self.read_events(&calendars);
                }
                Err(err) => self.failed(&trf("Couldn't read Yandex Calendar: {0}", &[&err]), false),
            },
            Step::Events(name) => {
                let Some(round) = &self.round else {
                    return;
                };
                let clock = UserClock {
                    zone: system::time_zone(),
                };
                let mut found = Vec::new();
                match caldav::calendar_data(&body) {
                    Ok(texts) => {
                        for text in texts {
                            found.extend(ical::meetings(
                                &text,
                                &name,
                                &self.emails,
                                round.from,
                                round.to,
                                &clock,
                            ));
                        }
                    }
                    Err(err) => log::warn(&format!("calendar \"{name}\": {err}")),
                }
                self.calendar_done(found);
            }
        }
    }

    fn learn_email(&mut self, email: Option<String>) {
        if let Some(email) = email.map(|email| email.trim().to_lowercase())
            && email.contains('@')
            && !self.emails.contains(&email)
        {
            self.emails.push(email);
        }
    }

    /// A calendar's events came: the round ends with the last one.
    fn calendar_done(&mut self, found: Vec<Meeting>) {
        let Some(round) = &mut self.round else {
            return;
        };
        round.meetings.extend(found);
        round.waiting = round.waiting.saturating_sub(1);
        if round.waiting == 0 {
            self.finish_round();
        }
    }

    fn finish_round(&mut self) {
        let Some(round) = self.round.take() else {
            return;
        };
        let mut meetings = round.meetings;
        meetings.sort_by(|a, b| (a.start, !a.all_day, &a.title).cmp(&(b.start, !b.all_day, &b.title)));
        // One meeting in two calendars (an invitation, a shared calendar): once.
        let mut seen = Vec::new();
        meetings.retain(|meeting| {
            let key = meeting.key();
            let new = !seen.contains(&key);
            seen.push(key);
            new
        });
        self.meetings = meetings;
        if self.told.take().is_some() {
            log::info("calendar: read again");
        }
        self.update(time::now());
        if let Some(picker) = self.picker {
            picker::update(picker, self.agenda_items(), false);
        }
    }

    /// A round failed: a refusal clears the meetings, a passing trouble keeps them; the user hears
    /// once per problem.
    fn failed(&mut self, text: &str, refused: bool) {
        self.stop_requests();
        log::warn(&format!("calendar: {text}"));
        if refused {
            self.meetings.clear();
            self.calendars = None;
            self.update(time::now());
        }
        if self.told.as_deref() != Some(text) {
            self.told = Some(text.to_string());
            if refused {
                flux_plugin_api::notify::warning(text, None);
            }
        }
    }

    fn stop_requests(&mut self) {
        for (id, _) in self.requests.drain() {
            http::cancel(id);
        }
        self.round = None;
    }

    // --- The window ---

    /// The tick on the next minute (a little after it): the status bar changes then.
    fn schedule_tick(&mut self) {
        if !self.active {
            return;
        }
        if let Some(tick) = self.tick.take() {
            timers::cancel(tick);
        }
        let millis = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_millis() as u64)
            .unwrap_or(0);
        let delay = 60_000 - millis % 60_000 + 300;
        self.tick = Some(timers::after(Duration::from_millis(delay)));
    }

    /// The status bar, the reminders and their cards, as of `now`.
    fn update(&mut self, now: i64) {
        let offset = time::offset_at(now);
        let enabled = setting::<bool>(settings::CALENDAR_ENABLED).unwrap_or(true);
        let shown = if enabled {
            status_text(&self.meetings, now, offset)
        } else {
            None
        };
        if shown != self.shown {
            match &shown {
                Some((text, tooltip)) => status::item(text)
                    .tooltip(tooltip)
                    .command(AGENDA)
                    .show(ITEM),
                None => status::hide(ITEM),
            }
            self.shown = shown;
        }
        if enabled {
            self.remind(now);
        }
        // A meeting over: its card's Join is over too.
        let over: Vec<String> = self
            .cards
            .iter()
            .filter(|(_, (_, until))| now >= *until)
            .map(|(key, _)| key.clone())
            .collect();
        for key in over {
            if let Some((card, _)) = self.cards.remove(&key) {
                notifications::expire(card);
            }
        }
    }

    fn remind(&mut self, now: i64) {
        let minutes = setting::<i64>(settings::REMIND_MINUTES).unwrap_or(5);
        let keys: Vec<String> = self.reminded.iter().map(|(key, _)| key.clone()).collect();
        let due: Vec<Meeting> = agenda::due(&self.meetings, now, minutes, &keys)
            .into_iter()
            .cloned()
            .collect();
        let before = self.reminded.len();
        self.reminded.retain(|(_, end)| *end > now - 86_400);
        let pruned = self.reminded.len() != before;
        for meeting in &due {
            let title = agenda::short_title(meeting, &tr("A meeting"));
            let minutes = agenda::minutes_until(now, meeting.start);
            let mut body = agenda::span(meeting, time::offset_at);
            if let Some(place) = agenda::place(meeting) {
                body = format!("{body} · {place}");
            }
            let mut card = Notice::info(&trf("{0} in {1} min", &[&title, &minutes]))
                .body(&body)
                .sticky();
            if meeting.link.is_some() {
                card = card.action_with(&tr("Join"), JOIN, &meeting.key());
            }
            let card = card.send();
            self.cards.insert(meeting.key(), (card, meeting.end));
            self.reminded.push((meeting.key(), meeting.end));
        }
        if !due.is_empty() || pruned {
            storage::set(STORED_REMINDED, &self.reminded);
        }
    }

    /// Joins a meeting: the one of a reminder's or the agenda's (its key), else the one going on or
    /// the next within the hour. Without a link — its day in Yandex Calendar.
    fn join(&mut self, key: Option<&str>) {
        let now = time::now();
        let meeting = key
            .and_then(|key| self.meetings.iter().find(|meeting| meeting.key() == key))
            .or_else(|| key.is_none().then(|| agenda::to_join(&self.meetings, now)).flatten());
        let address = match meeting {
            Some(meeting) => meeting
                .link
                .clone()
                .unwrap_or_else(|| agenda::calendar_page(meeting.start, time::offset_at(meeting.start))),
            None if key.is_some() => {
                // The meeting moved or went since the card: what's going on now, if anything.
                match agenda::to_join(&self.meetings, now) {
                    Some(meeting) => meeting.link.clone().unwrap_or_default(),
                    None => agenda::calendar_page(now, time::offset_at(now)),
                }
            }
            None => {
                flux_plugin_api::notify::info(&tr("No meeting to join right now"));
                return;
            }
        };
        if let Err(err) = system::open_url(&address) {
            log::warn(&format!("calendar: {err}"));
        }
    }

    /// The agenda's menu at the status bar item (in the middle of the window when the item is
    /// hidden: a free day).
    fn open_agenda(&mut self, auth: &Auth) {
        let signed_in = self.authorization(auth).is_some();
        let mut menu = picker::Picker::menu()
            .items(self.agenda_items())
            .empty_text(&if signed_in {
                tr("No meetings today or tomorrow")
            } else {
                tr("Sign in to Yandex in the plugin's settings")
            })
            .at_status_item(ITEM);
        if self.round.is_some() && self.meetings.is_empty() {
            menu = menu.loading();
        }
        self.picker = Some(menu.open());
    }

    fn agenda_items(&self) -> Vec<picker::Item> {
        let now = time::now();
        let offset = time::offset_at(now);
        // The first calendar is the user's own: the others are named on their meetings.
        let own = self
            .calendars
            .as_ref()
            .and_then(|calendars| calendars.first())
            .map(|calendar| calendar.name.as_str());
        agenda::agenda(&self.meetings, now, offset)
            .into_iter()
            .map(|(day, meeting)| agenda_item(day, meeting, now, own))
            .collect()
    }
}

/// The status bar's text and tooltip; none — a free rest of the day.
fn status_text(meetings: &[Meeting], now: i64, offset: i32) -> Option<(String, String)> {
    let untitled = tr("A meeting");
    let (meeting, text) = match agenda::reading(meetings, now, offset) {
        Reading::Soon(meeting, minutes) => (
            meeting,
            trf(
                "{0} in {1} min",
                &[&agenda::short_title(meeting, &untitled), &minutes],
            ),
        ),
        Reading::Now(meeting, left) => (
            meeting,
            trf(
                "{0} now · {1} min left",
                &[&agenda::short_title(meeting, &untitled), &left],
            ),
        ),
        Reading::Later(meeting) => (
            meeting,
            trf(
                "{0} at {1}",
                &[
                    &agenda::short_title(meeting, &untitled),
                    &time::clock(meeting.start, time::offset_at(meeting.start)),
                ],
            ),
        ),
        Reading::Free => return None,
    };
    let mut tooltip = format!("{} {}", agenda::span(meeting, time::offset_at), meeting.title.trim());
    if let Some(place) = agenda::place(meeting) {
        tooltip = format!("{tooltip} · {place}");
    }
    if meeting.link.as_deref().is_some_and(agenda::is_telemost) {
        tooltip = format!("{tooltip} · {}", tr("Telemost"));
    }
    Some((text, tooltip))
}

/// A line of the agenda: the time (or "All day") and the title — dim once over; "now", the place
/// or another calendar's name after it; Telemost (or a link) on the right.
fn agenda_item(day: Day, meeting: &Meeting, now: i64, own: Option<&str>) -> picker::Item {
    let group = match day {
        Day::Today => tr("Today"),
        Day::Tomorrow => tr("Tomorrow"),
    };
    let when = if meeting.all_day {
        tr("All day")
    } else {
        agenda::span(meeting, time::offset_at)
    };
    let title = match meeting.title.trim() {
        "" => tr("A meeting"),
        title => title.to_string(),
    };
    let over = !meeting.all_day && meeting.end <= now;
    let going = !meeting.all_day && meeting.start <= now && now < meeting.end;
    let tone = if over { Tone::Dim } else { Tone::Normal };
    let label = vec![
        span(&when).tone(if over { Tone::Dim } else { Tone::Muted }),
        span("  "),
        span(&title).tone(tone),
    ];
    let mut item = picker::item(&meeting.key(), &title).spans(label).group(&group);
    if going {
        item = item.detail(&tr("now"));
    } else if let Some(place) = agenda::place(meeting) {
        item = item.detail(&place);
    } else if own.is_some_and(|own| own != meeting.calendar) {
        item = item.detail(&meeting.calendar);
    }
    if let Some(link) = &meeting.link {
        item = item.badge(&if agenda::is_telemost(link) {
            tr("Telemost")
        } else {
            tr("Link")
        });
    }
    item
}

/// A short stable hash of an account's `Authorization` header (not the header: it is a secret).
fn fingerprint(text: &str) -> u64 {
    // FNV-1a.
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    })
}
