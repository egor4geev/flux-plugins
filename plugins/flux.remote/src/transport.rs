//! The calls to Telegram and the polling: the order the bot's messages keep, the pace Telegram
//! takes, and the news (`getUpdates`, one long request at a time — nothing runs while nothing
//! happens). Every call says which lane it goes by ([`Lane`]) and who wants its answer
//! ([`Purpose`]); the answer comes back to the bot ([`Incoming::Answer`]).
//!
//! - A topic's calls go in order, one in flight; topics go side by side, at most [`MAX_FLIGHT`]
//!   calls in flight together.
//! - A draft has one request in flight; its newest text waits (a newer one takes its place) and
//!   goes at most every [`DRAFT_EVERY_MS`].
//! - A message to a topic ends the topic's drafts in Telegram: their waiting texts are dropped (and
//!   their later ones, [`ENDED_DRAFTS`]), the message waits for a draft's call in flight, and a new
//!   draft of the topic waits for the message — a late draft never shows up after the final
//!   message.
//! - A `Latest` call: the newest wins, at most every [`LATEST_EVERY_MS`].
//! - `Urgent` (a button's answer) waits behind nothing.
//! - 429: everything waits as long as Telegram says, then the call goes again; a network error: the
//!   call goes again after each of [`RETRY_MS`], then its error comes back; a refusal comes back at
//!   once.
//!
//! Nothing ticks at rest: a timer runs only while a call waits for its moment.

use std::collections::{BTreeMap, HashMap, VecDeque};

use serde_json::Value;

use crate::host::Host;
use crate::telegram::{self, API, Call, POLL_SECONDS, TgError, Update};

/// A draft's newest text goes at most this often (the author chose «twice a second» on a live bot;
/// a call takes ≈0.6 s, so one in flight is about it anyway).
pub const DRAFT_EVERY_MS: u64 = 500;
/// The status message in «Общее» changes at most this often.
pub const LATEST_EVERY_MS: u64 = 2000;
/// Calls in flight together, the `Urgent` ones aside.
pub const MAX_FLIGHT: usize = 4;
/// A call that met a network error goes again after these pauses; then its error comes back.
pub const RETRY_MS: [u64; 2] = [1_000, 3_000];
/// The drafts a message ended, remembered: their late texts are dropped.
const ENDED_DRAFTS: usize = 64;
/// A call gives up after this long.
const CALL_TIMEOUT_MS: u32 = 20_000;
/// The polling's pause after a failure: doubles up to the last.
const POLL_RETRY_MS: [u32; 5] = [2_000, 5_000, 10_000, 30_000, 60_000];

/// How a call goes.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Lane {
    /// In order with the other calls to the same topic (0 — the chat outside topics).
    Topic(i64),
    /// A draft by its id: only the newest text matters.
    Draft(i64),
    /// Only the newest call with this key matters (the status message's edit).
    Latest(String),
    /// Before everything else: the answer to a button's press.
    Urgent,
}

impl Lane {
    /// How often the lane's calls may go: a draft's and `Latest`'s are paced.
    fn every(&self) -> u64 {
        match self {
            Lane::Draft(_) => DRAFT_EVERY_MS,
            Lane::Latest(_) => LATEST_EVERY_MS,
            Lane::Topic(_) | Lane::Urgent => 0,
        }
    }

    /// Only the newest call matters: a newer one takes the waiting one's place.
    fn newest_wins(&self) -> bool {
        matches!(self, Lane::Draft(_) | Lane::Latest(_))
    }
}

/// Who wants the answer: the module, and what it was for in the module's own words.
#[derive(Clone, Debug, PartialEq)]
pub struct Purpose {
    pub module: Module,
    pub tag: String,
}

impl Purpose {
    pub fn new(module: Module, tag: impl Into<String>) -> Self {
        Purpose {
            module,
            tag: tag.into(),
        }
    }

    /// Nobody waits for the answer (an error is only logged).
    pub fn none() -> Self {
        Purpose::new(Module::None, "")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Module {
    None,
    Binding,
    General,
    Handoff,
    NewChat,
    Conversation,
    Requests,
}

/// What the transport hands to the bot.
#[derive(Clone, Debug, PartialEq)]
pub enum Incoming {
    Answer(Purpose, Result<Value, TgError>),
    /// A poll came back (maybe empty): Telegram is reachable.
    Updates(Vec<Update>),
    /// A poll failed: `Unauthorized` and `Conflict` stop the polling, others retry by themselves.
    PollFailed(TgError),
}

struct Queued {
    call: Call,
    lane: Lane,
    purpose: Purpose,
    /// Network errors so far.
    tries: usize,
    /// Not before this moment (ms since 1970): the pause before a retry.
    not_before: u64,
}

impl Queued {
    /// A draft's topic (0 — outside topics).
    fn draft_topic(&self) -> Option<i64> {
        match self.lane {
            Lane::Draft(_) => Some(
                self.call
                    .params
                    .get("message_thread_id")
                    .and_then(Value::as_i64)
                    .unwrap_or(0),
            ),
            _ => None,
        }
    }

    /// A message to a topic: it ends the topic's drafts.
    fn ending_topic(&self) -> Option<i64> {
        match self.lane {
            Lane::Topic(topic) if ends_drafts(self.call.method) => Some(topic),
            _ => None,
        }
    }
}

/// The methods that post a message: Telegram ends the drafts of its topic then (an edit doesn't).
fn ends_drafts(method: &str) -> bool {
    matches!(
        method,
        "sendMessage" | "sendRichMessage" | "sendPhoto" | "sendDocument"
    )
}

/// When a waiting call may go.
enum Moment {
    Now,
    At(u64),
    /// When something else is answered.
    Later,
}

pub struct Transport {
    token: Option<String>,
    /// The calls waiting: a lane's in its order, `Urgent` ones first.
    queue: VecDeque<Queued>,
    /// The calls in flight, by request.
    flights: BTreeMap<u64, Queued>,
    /// When a paced lane last sent (a draft, `Latest`); forgotten once it doesn't hold anything back.
    sent: HashMap<Lane, u64>,
    /// Drafts a message ended, newest last.
    ended: VecDeque<i64>,
    /// The timer of the next waiting call's moment, and the moment.
    wake: Option<(u64, u64)>,
    /// Telegram asked to wait (429): the timer that ends the wait, and its end.
    pause: Option<(u64, u64)>,
    /// The poll in flight.
    poll: Option<u64>,
    polling: bool,
    offset: Option<i64>,
    /// Failed polls in a row.
    poll_failures: usize,
    /// The timer of the polling's pause.
    poll_timer: Option<u64>,
    statuses: HashMap<u64, u16>,
    bodies: HashMap<u64, Vec<u8>>,
}

impl Default for Transport {
    fn default() -> Self {
        Self::new()
    }
}

impl Transport {
    pub fn new() -> Self {
        Transport {
            token: None,
            queue: VecDeque::new(),
            flights: BTreeMap::new(),
            sent: HashMap::new(),
            ended: VecDeque::new(),
            wake: None,
            pause: None,
            poll: None,
            polling: false,
            offset: None,
            poll_failures: 0,
            poll_timer: None,
            statuses: HashMap::new(),
            bodies: HashMap::new(),
        }
    }

    /// A new token (or none): everything waiting or in flight is dropped.
    pub fn set_token(&mut self, host: &mut dyn Host, token: Option<String>) {
        self.stop_polling(host);
        for request in std::mem::take(&mut self.flights).into_keys() {
            host.http_cancel(request);
        }
        for (timer, _) in [self.wake.take(), self.pause.take()].into_iter().flatten() {
            host.cancel_timer(timer);
        }
        self.queue.clear();
        self.sent.clear();
        self.ended.clear();
        self.statuses.clear();
        self.bodies.clear();
        self.offset = None;
        self.token = token;
    }

    /// Where the polling goes on from (the bot keeps it across restarts).
    pub fn offset(&self) -> Option<i64> {
        self.offset
    }

    pub fn send(&mut self, host: &mut dyn Host, call: Call, lane: Lane, purpose: Purpose) {
        let queued = Queued {
            call,
            lane,
            purpose,
            tries: 0,
            not_before: 0,
        };
        if let Lane::Draft(draft) = queued.lane
            && self.ended.contains(&draft)
        {
            return;
        }
        if let Some(topic) = queued.ending_topic() {
            self.end_drafts(topic);
        }
        let waiting = queued
            .lane
            .newest_wins()
            .then(|| {
                self.queue
                    .iter_mut()
                    .find(|waiting| waiting.lane == queued.lane)
            })
            .flatten();
        if let Some(waiting) = waiting {
            // The newer call in the older one's place, after the older one's pause.
            *waiting = Queued {
                not_before: waiting.not_before,
                ..queued
            };
        } else if queued.lane == Lane::Urgent {
            let first = self
                .queue
                .iter()
                .take_while(|waiting| waiting.lane == Lane::Urgent)
                .count();
            self.queue.insert(first, queued);
        } else {
            self.queue.push_back(queued);
        }
        self.pump(host);
    }

    pub fn start_polling(&mut self, host: &mut dyn Host, offset: Option<i64>) {
        if offset.is_some() {
            self.offset = offset;
        }
        self.polling = true;
        if self.poll.is_none() && self.poll_timer.is_none() {
            self.poll_now(host);
        }
    }

    pub fn stop_polling(&mut self, host: &mut dyn Host) {
        self.polling = false;
        if let Some(request) = self.poll.take() {
            host.http_cancel(request);
        }
        if let Some(timer) = self.poll_timer.take() {
            host.cancel_timer(timer);
        }
        self.poll_failures = 0;
    }

    pub fn on_head(&mut self, request: u64, status: u16) {
        if self.is_ours(request) {
            self.statuses.insert(request, status);
        }
    }

    pub fn on_body(&mut self, request: u64, chunk: &[u8]) {
        if self.is_ours(request) {
            self.bodies
                .entry(request)
                .or_default()
                .extend_from_slice(chunk);
        }
    }

    /// A request ended (`error` — the network's text): what it brings to the bot.
    pub fn on_done(
        &mut self,
        host: &mut dyn Host,
        request: u64,
        error: Option<String>,
    ) -> Vec<Incoming> {
        if !self.is_ours(request) {
            return Vec::new();
        }
        let status = self.statuses.remove(&request).unwrap_or(0);
        let body = self.bodies.remove(&request).unwrap_or_default();
        let result = match error {
            Some(error) => Err(TgError::Network(error)),
            None => telegram::parse(status, &body),
        };
        let mut out = Vec::new();
        if self.poll == Some(request) {
            self.poll = None;
            self.polled(host, result, &mut out);
        } else if let Some(queued) = self.flights.remove(&request) {
            let now = host.now_ms();
            match result {
                Err(TgError::Limited(seconds)) => {
                    self.again(queued);
                    self.pause(host, now + seconds.max(1) * 1000, now);
                }
                Err(TgError::Network(_)) if queued.tries < RETRY_MS.len() => {
                    let not_before = now + RETRY_MS[queued.tries];
                    self.again(Queued {
                        tries: queued.tries + 1,
                        not_before,
                        ..queued
                    });
                }
                result => out.push(Incoming::Answer(queued.purpose, result)),
            }
        }
        self.pump(host);
        out
    }

    /// A timer of the transport's (true — it was ours).
    pub fn on_timer(&mut self, host: &mut dyn Host, timer: u64) -> bool {
        if self.wake.is_some_and(|(wake, _)| wake == timer) {
            self.wake = None;
            self.pump(host);
            true
        } else if self.pause.is_some_and(|(pause, _)| pause == timer) {
            self.pause = None;
            self.pump(host);
            true
        } else if self.poll_timer == Some(timer) {
            self.poll_timer = None;
            if self.polling {
                self.poll_now(host);
            }
            true
        } else {
            false
        }
    }

    fn is_ours(&self, request: u64) -> bool {
        self.poll == Some(request) || self.flights.contains_key(&request)
    }

    fn url(&self, method: &str) -> Option<String> {
        Some(format!("{API}/bot{}/{method}", self.token.as_deref()?))
    }

    /// A message to `topic` is coming: the topic's drafts end — the waiting texts go, and later
    /// ones won't be sent.
    fn end_drafts(&mut self, topic: i64) {
        let mut ended: Vec<i64> = self
            .flights
            .values()
            .chain(&self.queue)
            .filter(|queued| queued.draft_topic() == Some(topic))
            .filter_map(|queued| match queued.lane {
                Lane::Draft(draft) => Some(draft),
                _ => None,
            })
            .collect();
        self.queue
            .retain(|waiting| waiting.draft_topic() != Some(topic));
        ended.retain(|draft| !self.ended.contains(draft));
        self.ended.extend(ended);
        while self.ended.len() > ENDED_DRAFTS {
            self.ended.pop_front();
        }
    }

    /// A call goes again, first of its lane — unless a newer one took its place or a message ended
    /// its draft.
    fn again(&mut self, queued: Queued) {
        let ended = matches!(queued.lane, Lane::Draft(draft) if self.ended.contains(&draft));
        let newer = queued.lane.newest_wins()
            && self.queue.iter().any(|waiting| waiting.lane == queued.lane);
        if !ended && !newer {
            self.queue.push_front(queued);
        }
    }

    /// Telegram asked to wait until `until`: nothing goes before then.
    fn pause(&mut self, host: &mut dyn Host, until: u64, now: u64) {
        if self.pause.is_some_and(|(_, end)| end >= until) {
            return;
        }
        if let Some((timer, _)) = self.pause.take() {
            host.cancel_timer(timer);
        }
        self.pause = Some((host.timer(ms(until - now)), until));
    }

    /// Sends every waiting call whose moment has come; a timer for the nearest of the others.
    fn pump(&mut self, host: &mut dyn Host) {
        if self.pause.is_some() || self.token.is_none() {
            return;
        }
        let now = host.now_ms();
        self.sent.retain(|_, sent| now < *sent + LATEST_EVERY_MS);
        let mut next: Option<u64> = None;
        // The lanes with a call waiting before: theirs go in order.
        let mut before: Vec<Lane> = Vec::new();
        let mut index = 0;
        while index < self.queue.len() {
            match self.moment(&self.queue[index], &before, now) {
                Moment::Now => {
                    if let Some(queued) = self.queue.remove(index) {
                        self.start(host, queued, now);
                    }
                    continue;
                }
                Moment::At(at) => next = Some(next.map_or(at, |next| next.min(at))),
                Moment::Later => {}
            }
            before.push(self.queue[index].lane.clone());
            index += 1;
        }
        self.wake_at(host, next, now);
    }

    fn moment(&self, waiting: &Queued, before: &[Lane], now: u64) -> Moment {
        let lane = &waiting.lane;
        if *lane != Lane::Urgent {
            let busy =
                before.contains(lane) || self.flights.values().any(|flight| flight.lane == *lane);
            let full = self
                .flights
                .values()
                .filter(|flight| flight.lane != Lane::Urgent)
                .count()
                >= MAX_FLIGHT;
            // A message of the draft's topic, waiting or in flight, goes first: it ends the drafts.
            let behind_message = waiting.draft_topic().is_some_and(|topic| {
                self.flights
                    .values()
                    .chain(&self.queue)
                    .any(|other| other.ending_topic() == Some(topic))
            });
            // A message waits for its topic's draft in flight.
            let behind_draft = waiting.ending_topic().is_some_and(|topic| {
                self.flights
                    .values()
                    .any(|flight| flight.draft_topic() == Some(topic))
            });
            if busy || full || behind_message || behind_draft {
                return Moment::Later;
            }
        }
        let paced = self.sent.get(lane).map_or(0, |sent| sent + lane.every());
        let at = paced.max(waiting.not_before);
        if at > now {
            Moment::At(at)
        } else {
            Moment::Now
        }
    }

    fn start(&mut self, host: &mut dyn Host, queued: Queued, now: u64) {
        let Some(url) = self.url(queued.call.method) else {
            return;
        };
        let body = serde_json::to_vec(&queued.call.params).unwrap_or_default();
        let request = host.http_post(&url, body, CALL_TIMEOUT_MS);
        if queued.lane.every() > 0 {
            self.sent.insert(queued.lane.clone(), now);
        }
        self.flights.insert(request, queued);
    }

    /// The timer for the nearest moment a waiting call may go; none — nothing waits for a moment.
    fn wake_at(&mut self, host: &mut dyn Host, next: Option<u64>, now: u64) {
        if let (Some((_, at)), Some(next)) = (self.wake, next)
            && at <= next
        {
            // It comes first: the queue is looked at again then.
            return;
        }
        if let Some((timer, _)) = self.wake.take() {
            host.cancel_timer(timer);
        }
        self.wake = next.map(|at| (host.timer(ms(at.saturating_sub(now))), at));
    }

    fn poll_now(&mut self, host: &mut dyn Host) {
        let call = telegram::get_updates(self.offset, POLL_SECONDS);
        let Some(url) = self.url(call.method) else {
            return;
        };
        let body = serde_json::to_vec(&call.params).unwrap_or_default();
        self.poll = Some(host.http_post(&url, body, (POLL_SECONDS + 15) * 1000));
    }

    fn polled(
        &mut self,
        host: &mut dyn Host,
        result: Result<Value, TgError>,
        out: &mut Vec<Incoming>,
    ) {
        match result {
            Ok(Value::Array(items)) => {
                self.poll_failures = 0;
                let mut updates = Vec::new();
                for item in items {
                    let id = item.get("update_id").and_then(Value::as_i64);
                    if let Some(id) = id {
                        self.offset = Some(self.offset.map_or(id + 1, |offset| offset.max(id + 1)));
                    }
                    match serde_json::from_value::<Update>(item) {
                        Ok(update) => updates.push(update),
                        Err(err) => host.log(&format!("An update the bot doesn't read: {err}")),
                    }
                }
                out.push(Incoming::Updates(updates));
                if self.polling {
                    self.poll_now(host);
                }
            }
            Ok(_) => self.retry_poll(host, TgError::Network("Unexpected answer".into()), out),
            Err(error @ (TgError::Unauthorized | TgError::Conflict)) => {
                self.polling = false;
                out.push(Incoming::PollFailed(error));
            }
            Err(error) => self.retry_poll(host, error, out),
        }
    }

    fn retry_poll(&mut self, host: &mut dyn Host, error: TgError, out: &mut Vec<Incoming>) {
        let wait = match &error {
            TgError::Limited(seconds) => (*seconds * 1000).min(60_000) as u32,
            _ => POLL_RETRY_MS[self.poll_failures.min(POLL_RETRY_MS.len() - 1)],
        };
        self.poll_failures += 1;
        out.push(Incoming::PollFailed(error));
        if self.polling {
            self.poll_timer = Some(host.timer(wait));
        }
    }
}

/// Milliseconds for a timer, at least one.
fn ms(ms: u64) -> u32 {
    ms.clamp(1, u64::from(u32::MAX)) as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::FakeHost;
    use serde_json::json;

    fn transport(host: &mut FakeHost) -> Transport {
        let mut transport = Transport::new();
        transport.set_token(host, Some("1:T".into()));
        transport
    }

    /// Telegram answers a request with `ok` and this result.
    fn ok(tg: &mut Transport, host: &mut FakeHost, request: u64, result: Value) -> Vec<Incoming> {
        let body = serde_json::to_vec(&json!({ "ok": true, "result": result })).unwrap();
        tg.on_head(request, 200);
        tg.on_body(request, &body);
        tg.on_done(host, request, None)
    }

    fn refused(
        tg: &mut Transport,
        host: &mut FakeHost,
        request: u64,
        code: u16,
        body: Value,
    ) -> Vec<Incoming> {
        tg.on_head(request, code);
        tg.on_body(request, &serde_json::to_vec(&body).unwrap());
        tg.on_done(host, request, None)
    }

    /// The only timer runs out: the clock moves on by its time.
    fn fire(tg: &mut Transport, host: &mut FakeHost) {
        assert_eq!(host.timers.len(), 1, "one timer: {:?}", host.timers);
        let (timer, ms) = host.timers.remove(0);
        host.now += u64::from(ms);
        assert!(tg.on_timer(host, timer));
    }

    /// The requests started so far: (id, method, parameters).
    fn sent(host: &FakeHost) -> Vec<(u64, String, Value)> {
        host.tg_calls()
    }

    fn message(thread: i64, text: &str) -> Call {
        telegram::send_message(1, Some(thread), text, None)
    }

    fn draft(thread: i64, draft: i64, text: &str) -> Call {
        telegram::send_draft(1, Some(thread), draft, text, true)
    }

    #[test]
    fn one_call_at_a_time_and_the_answer_goes_back() {
        let mut host = FakeHost::default();
        let mut tg = transport(&mut host);
        tg.send(
            &mut host,
            telegram::get_me(),
            Lane::Topic(0),
            Purpose::new(Module::Binding, "me"),
        );
        tg.send(
            &mut host,
            telegram::get_me(),
            Lane::Topic(0),
            Purpose::none(),
        );
        assert_eq!(host.http.len(), 1);
        let (id, url, _) = host.http[0].clone();
        assert_eq!(url, "https://api.telegram.org/bot1:T/getMe");
        let out = ok(&mut tg, &mut host, id, json!({"id":5}));
        assert_eq!(
            out,
            vec![Incoming::Answer(
                Purpose::new(Module::Binding, "me"),
                Ok(json!({"id":5}))
            )]
        );
        assert_eq!(host.http.len(), 2, "the next call goes");
    }

    #[test]
    fn a_topic_keeps_its_order_and_topics_go_side_by_side() {
        let mut host = FakeHost::default();
        let mut tg = transport(&mut host);
        tg.send(
            &mut host,
            message(1, "a1"),
            Lane::Topic(1),
            Purpose::new(Module::General, "a1"),
        );
        tg.send(&mut host, message(1, "a2"), Lane::Topic(1), Purpose::none());
        tg.send(&mut host, message(2, "b1"), Lane::Topic(2), Purpose::none());
        let texts = |host: &FakeHost| -> Vec<String> {
            sent(host)
                .iter()
                .map(|(_, _, params)| params["text"].as_str().unwrap().to_string())
                .collect()
        };
        assert_eq!(
            texts(&host),
            ["a1", "b1"],
            "a1 and b1 together, a2 after a1"
        );
        let (a1, _, _) = sent(&host)[0].clone();
        let out = ok(&mut tg, &mut host, a1, json!({"message_id": 1}));
        assert!(matches!(&out[0], Incoming::Answer(purpose, Ok(_)) if purpose.tag == "a1"));
        assert_eq!(texts(&host), ["a1", "b1", "a2"]);
    }

    #[test]
    fn at_most_four_in_flight_and_a_press_waits_for_nothing() {
        let mut host = FakeHost::default();
        let mut tg = transport(&mut host);
        for topic in 1..=6 {
            tg.send(
                &mut host,
                message(topic, "x"),
                Lane::Topic(topic),
                Purpose::none(),
            );
        }
        assert_eq!(host.http.len(), MAX_FLIGHT);
        tg.send(
            &mut host,
            telegram::answer_press("q", None),
            Lane::Urgent,
            Purpose::none(),
        );
        assert_eq!(
            host.http.len(),
            MAX_FLIGHT + 1,
            "the press's answer goes at once"
        );
        assert_eq!(sent(&host)[MAX_FLIGHT].1, "answerCallbackQuery");
        let (first, _, _) = sent(&host)[0].clone();
        ok(&mut tg, &mut host, first, json!(true));
        assert_eq!(host.http.len(), MAX_FLIGHT + 2, "the fifth topic goes");
        assert_eq!(sent(&host)[MAX_FLIGHT + 1].2["message_thread_id"], 5);
    }

    #[test]
    fn a_newer_draft_replaces_the_waiting_one() {
        let mut host = FakeHost::default();
        let mut tg = transport(&mut host);
        tg.send(
            &mut host,
            telegram::get_me(),
            Lane::Topic(0),
            Purpose::none(),
        );
        for text in ["a", "ab", "abc"] {
            tg.send(
                &mut host,
                telegram::send_draft(1, Some(2), 9, text, true),
                Lane::Draft(9),
                Purpose::none(),
            );
        }
        assert_eq!(tg.queue.len(), 1);
        assert_eq!(tg.queue[0].call.params["text"], "abc");
    }

    #[test]
    fn a_draft_has_one_call_in_flight_and_goes_twice_a_second() {
        let mut host = FakeHost::default();
        let mut tg = transport(&mut host);
        tg.send(&mut host, draft(7, 9, "a"), Lane::Draft(9), Purpose::none());
        tg.send(
            &mut host,
            draft(7, 9, "ab"),
            Lane::Draft(9),
            Purpose::none(),
        );
        tg.send(
            &mut host,
            draft(7, 9, "abc"),
            Lane::Draft(9),
            Purpose::none(),
        );
        assert_eq!(host.http.len(), 1, "one in flight");
        assert!(
            host.timers.is_empty(),
            "nothing to time while the call is in flight"
        );
        host.now += 300;
        let (first, _, _) = sent(&host)[0].clone();
        ok(&mut tg, &mut host, first, json!(true));
        assert_eq!(
            host.http.len(),
            1,
            "not before half a second since the last"
        );
        assert_eq!(
            host.timers.iter().map(|(_, ms)| *ms).collect::<Vec<_>>(),
            [200]
        );
        fire(&mut tg, &mut host);
        assert_eq!(host.http.len(), 2);
        assert_eq!(sent(&host)[1].2["text"], "abc", "the newest text");
        let (second, _, _) = sent(&host)[1].clone();
        host.now += 600;
        ok(&mut tg, &mut host, second, json!(true));
        assert!(host.timers.is_empty(), "nothing ticks at rest");
    }

    #[test]
    fn a_message_ends_its_topics_drafts() {
        let mut host = FakeHost::default();
        let mut tg = transport(&mut host);
        tg.send(&mut host, draft(7, 9, "a"), Lane::Draft(9), Purpose::none());
        tg.send(
            &mut host,
            draft(8, 10, "x"),
            Lane::Draft(10),
            Purpose::none(),
        );
        tg.send(
            &mut host,
            draft(7, 9, "ab"),
            Lane::Draft(9),
            Purpose::none(),
        );
        assert_eq!(host.http.len(), 2);
        tg.send(
            &mut host,
            message(7, "final"),
            Lane::Topic(7),
            Purpose::new(Module::Conversation, "final"),
        );
        assert_eq!(
            host.http.len(),
            2,
            "the message waits for the draft in flight"
        );
        assert!(
            tg.queue
                .iter()
                .all(|waiting| waiting.lane != Lane::Draft(9)),
            "the waiting text is dropped"
        );
        tg.send(
            &mut host,
            draft(7, 9, "abc"),
            Lane::Draft(9),
            Purpose::none(),
        );
        assert!(
            tg.queue
                .iter()
                .all(|waiting| waiting.lane != Lane::Draft(9)),
            "a late text too"
        );
        tg.send(
            &mut host,
            draft(7, 11, "next"),
            Lane::Draft(11),
            Purpose::none(),
        );
        let (first, _, _) = sent(&host)[0].clone();
        host.now += 600;
        ok(&mut tg, &mut host, first, json!(true));
        assert_eq!(host.http.len(), 3);
        assert_eq!(
            sent(&host)[2].2["text"],
            "final",
            "the message, and the new draft waits for it"
        );
        let (final_message, _, _) = sent(&host)[2].clone();
        ok(&mut tg, &mut host, final_message, json!({"message_id": 3}));
        assert_eq!(sent(&host)[3].2["text"], "next");
        // The other topic's draft goes on.
        tg.send(
            &mut host,
            draft(8, 10, "xy"),
            Lane::Draft(10),
            Purpose::none(),
        );
        let (other, _, _) = sent(&host)[1].clone();
        ok(&mut tg, &mut host, other, json!(true));
        assert_eq!(sent(&host)[4].2["text"], "xy");
    }

    #[test]
    fn the_latest_wins_at_most_every_two_seconds() {
        let mut host = FakeHost::default();
        let mut tg = transport(&mut host);
        let edit = |text: &str| telegram::edit_text(1, 5, text, None);
        let lane = || Lane::Latest("status".into());
        tg.send(&mut host, edit("1"), lane(), Purpose::none());
        tg.send(&mut host, edit("2"), lane(), Purpose::none());
        tg.send(&mut host, edit("3"), lane(), Purpose::none());
        host.now += 100;
        let (first, _, _) = sent(&host)[0].clone();
        ok(&mut tg, &mut host, first, json!(true));
        assert_eq!(host.http.len(), 1);
        assert_eq!(
            host.timers.iter().map(|(_, ms)| *ms).collect::<Vec<_>>(),
            [1900]
        );
        fire(&mut tg, &mut host);
        assert_eq!(sent(&host)[1].2["text"], "3");
        let (second, _, _) = sent(&host)[1].clone();
        ok(&mut tg, &mut host, second, json!(true));
        assert!(host.timers.is_empty());
    }

    #[test]
    fn too_many_requests_pause_everything_then_the_call_goes_again() {
        let mut host = FakeHost::default();
        let mut tg = transport(&mut host);
        tg.send(
            &mut host,
            message(1, "a"),
            Lane::Topic(1),
            Purpose::new(Module::General, "a"),
        );
        let (first, _, _) = sent(&host)[0].clone();
        let limited = json!({"ok": false, "error_code": 429, "description": "Too Many Requests: retry after 3", "parameters": {"retry_after": 3}});
        assert!(refused(&mut tg, &mut host, first, 429, limited).is_empty());
        tg.send(&mut host, message(2, "b"), Lane::Topic(2), Purpose::none());
        tg.send(
            &mut host,
            telegram::answer_press("q", None),
            Lane::Urgent,
            Purpose::none(),
        );
        assert_eq!(host.http.len(), 1, "everything waits");
        assert_eq!(
            host.timers.iter().map(|(_, ms)| *ms).collect::<Vec<_>>(),
            [3000]
        );
        fire(&mut tg, &mut host);
        let methods: Vec<String> = sent(&host)
            .iter()
            .skip(1)
            .map(|(_, method, params)| format!("{method} {}", params["text"]))
            .collect();
        assert_eq!(
            methods,
            [
                "answerCallbackQuery null",
                "sendMessage \"a\"",
                "sendMessage \"b\""
            ]
        );
        let (again, _, _) = sent(&host)[2].clone();
        let out = ok(&mut tg, &mut host, again, json!({"message_id": 1}));
        assert!(matches!(&out[0], Incoming::Answer(purpose, Ok(_)) if purpose.tag == "a"));
    }

    #[test]
    fn a_network_error_is_tried_twice_more_and_a_refusal_comes_back_at_once() {
        let mut host = FakeHost::default();
        let mut tg = transport(&mut host);
        tg.send(
            &mut host,
            message(1, "a"),
            Lane::Topic(1),
            Purpose::new(Module::General, "a"),
        );
        tg.send(&mut host, message(1, "b"), Lane::Topic(1), Purpose::none());
        for pause in RETRY_MS {
            let (request, _, _) = sent(&host).last().unwrap().clone();
            assert!(
                tg.on_done(&mut host, request, Some("timed out".into()))
                    .is_empty()
            );
            assert_eq!(
                host.timers
                    .iter()
                    .map(|(_, ms)| u64::from(*ms))
                    .collect::<Vec<_>>(),
                [pause]
            );
            fire(&mut tg, &mut host);
            assert_eq!(
                sent(&host).last().unwrap().2["text"],
                "a",
                "again, before b"
            );
        }
        let (request, _, _) = sent(&host).last().unwrap().clone();
        let out = tg.on_done(&mut host, request, Some("timed out".into()));
        assert_eq!(
            out,
            vec![Incoming::Answer(
                Purpose::new(Module::General, "a"),
                Err(TgError::Network("timed out".into()))
            )]
        );
        assert_eq!(sent(&host).last().unwrap().2["text"], "b");
        let (request, _, _) = sent(&host).last().unwrap().clone();
        let gone =
            json!({"ok": false, "error_code": 400, "description": "Bad Request: TOPIC_ID_INVALID"});
        let out = refused(&mut tg, &mut host, request, 400, gone);
        assert!(matches!(&out[0], Incoming::Answer(_, Err(err)) if err.topic_gone()));
        assert_eq!(host.http.len(), 4, "a refusal isn't sent again");
        assert!(host.timers.is_empty());
    }

    #[test]
    fn polling_moves_the_offset_and_goes_on() {
        let mut host = FakeHost::default();
        let mut tg = transport(&mut host);
        tg.start_polling(&mut host, None);
        let (id, url, _) = host.http[0].clone();
        assert!(url.ends_with("/getUpdates"));
        tg.on_head(id, 200);
        tg.on_body(
            id,
            br#"{"ok":true,"result":[{"update_id":7,"message":{"message_id":1,"chat":{"id":1}}}]}"#,
        );
        let out = tg.on_done(&mut host, id, None);
        assert!(matches!(&out[0], Incoming::Updates(updates) if updates.len() == 1));
        assert_eq!(tg.offset(), Some(8));
        let (_, _, body) = host.http[1].clone();
        assert_eq!(serde_json::from_slice::<Value>(&body).unwrap()["offset"], 8);
    }

    #[test]
    fn a_conflict_stops_the_polling() {
        let mut host = FakeHost::default();
        let mut tg = transport(&mut host);
        tg.start_polling(&mut host, Some(3));
        let (id, _, _) = host.http[0].clone();
        tg.on_head(id, 409);
        tg.on_body(
            id,
            br#"{"ok":false,"error_code":409,"description":"Conflict"}"#,
        );
        assert_eq!(
            tg.on_done(&mut host, id, None),
            vec![Incoming::PollFailed(TgError::Conflict)]
        );
        assert_eq!(host.http.len(), 1, "no new poll");
    }

    #[test]
    fn a_failed_poll_waits_longer_each_time() {
        let mut host = FakeHost::default();
        let mut tg = transport(&mut host);
        tg.start_polling(&mut host, None);
        for wait in [2_000, 5_000, 10_000] {
            let (id, _, _) = host.http.last().unwrap().clone();
            let out = tg.on_done(&mut host, id, Some("offline".into()));
            assert_eq!(
                out,
                vec![Incoming::PollFailed(TgError::Network("offline".into()))]
            );
            assert_eq!(
                host.timers.iter().map(|(_, ms)| *ms).collect::<Vec<_>>(),
                [wait]
            );
            fire(&mut tg, &mut host);
        }
        let (id, _, _) = host.http.last().unwrap().clone();
        ok(&mut tg, &mut host, id, json!([]));
        assert!(host.timers.is_empty(), "back to one long request, no timer");
    }
}
