//! Requests to Tracker, shared by the Tracker window and the board: `https://api.tracker.yandex.net/
//! v3/…` with the headers of [`Auth`] (authorization, the organization) and the interface
//! language, JSON in and out; a failure in words for the user: signed out, 401 (sign in again), 403
//! (the organization or access), 404, 429 (Tracker allows 5–10 requests a second, one search at a
//! time), Tracker's own `errorMessages`. Requests block the plugin, not Flux (`fetch`; the wait
//! doesn't count toward its time limit).
//!
//! Agent F extends it (paging with `X-Total-Count`, scroll); the board (F2) uses it as it is.
//!
//! A 429 holds the plugin's background work back ([`backoff_left`]): as long as Tracker's
//! `Retry-After` says, else exponentially longer at each 429 in a row (2 s, 4 s … 2 min, ±50 %
//! jitter: the windows don't come back at once); an answer that isn't 429 ends it.

use std::cell::RefCell;
use std::time::{Duration, SystemTime};

use flux_plugin_api::http;
use flux_plugin_api::{tr, trf};
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::auth::Auth;

/// The API's base.
pub const API: &str = "https://api.tracker.yandex.net/v3/";
/// Tracker's web interface: a task's page is `<WEB><KEY>`.
pub const WEB: &str = "https://tracker.yandex.ru/";

const TIMEOUT: Duration = Duration::from_secs(20);

/// Why a request failed.
#[derive(Debug, Clone, PartialEq)]
pub enum ClientError {
    /// No credentials: nobody signed in, no token pasted.
    SignedOut,
    /// 401: the token expired or was revoked.
    Unauthorized,
    /// 403: no access — to the organization, to the queue; Tracker's words.
    Forbidden(String),
    /// 404.
    NotFound,
    /// 429.
    TooMany,
    /// 400: Tracker didn't take the request (a query it doesn't know); its words.
    BadRequest(String),
    /// Anything else: the network, a status with Tracker's words, a response that isn't JSON.
    Other(String),
}

impl ClientError {
    /// What to tell the user.
    pub fn message(&self) -> String {
        match self {
            ClientError::SignedOut => tr("Sign in to Yandex in the plugin's settings").to_string(),
            ClientError::Unauthorized => {
                tr("Yandex didn't accept the token: sign in again").to_string()
            }
            ClientError::Forbidden(text) if text.is_empty() => {
                tr("No access: check the organization's ID and type in the settings").to_string()
            }
            ClientError::Forbidden(text) => trf("No access: {0}", &[text]),
            ClientError::NotFound => tr("Not found").to_string(),
            ClientError::TooMany => {
                tr("Too many requests to Tracker: try again in a moment").to_string()
            }
            ClientError::BadRequest(text) if text.is_empty() => {
                trf("Tracker answered {0}", &[&400])
            }
            ClientError::BadRequest(text) | ClientError::Other(text) => text.clone(),
        }
    }
}

/// The 429s in a row and when the background may ask again.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Backoff {
    strikes: u32,
    /// Unix milliseconds.
    until: i64,
}

/// The first wait without `Retry-After`, and the longest.
const BACKOFF_FIRST_MS: i64 = 2_000;
const BACKOFF_MAX_MS: i64 = 120_000;
/// The longest `Retry-After` honoured.
const RETRY_AFTER_MAX_MS: i64 = 600_000;

impl Backoff {
    /// A 429 at `now` (unix ms): waits `retry_after` seconds when Tracker says, else 2 s × 2ⁿ⁻¹
    /// up to 2 min, scaled by `jitter` (0…1 → 50…150 %). Returns the wait (ms).
    pub fn strike(&mut self, now: i64, retry_after: Option<u64>, jitter: f64) -> i64 {
        self.strikes = self.strikes.saturating_add(1);
        let wait = match retry_after {
            Some(seconds) => (seconds as i64 * 1000).min(RETRY_AFTER_MAX_MS),
            None => {
                let exponent = self.strikes.saturating_sub(1).min(16);
                let base = (BACKOFF_FIRST_MS << exponent).min(BACKOFF_MAX_MS);
                (base as f64 * (0.5 + jitter.clamp(0.0, 1.0))) as i64
            }
        };
        self.until = now + wait;
        wait
    }

    /// An answer that isn't 429.
    pub fn clear(&mut self) {
        *self = Backoff::default();
    }

    /// How long the background still waits at `now` (ms); none — it may ask.
    pub fn left(&self, now: i64) -> Option<i64> {
        (self.until > now).then(|| self.until - now)
    }
}

thread_local! {
    static BACKOFF: RefCell<Backoff> = RefCell::new(Backoff::default());
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

/// 0…1, random: the backoff's jitter.
fn jitter() -> f64 {
    let mut bytes = [0u8; 4];
    match getrandom::fill(&mut bytes) {
        Ok(()) => f64::from(u32::from_le_bytes(bytes)) / f64::from(u32::MAX),
        Err(_) => 0.5,
    }
}

/// `Retry-After` in seconds (an HTTP date isn't read: the backoff's own wait then).
fn retry_after(headers: &[(String, String)]) -> Option<u64> {
    headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("retry-after"))
        .and_then(|(_, value)| value.trim().parse().ok())
}

/// How long background requests (polling, refreshes) should still wait after a 429; none — go.
/// The user's own requests (a card, a status) aren't held back.
pub fn backoff_left() -> Option<Duration> {
    BACKOFF
        .with(|backoff| backoff.borrow().left(now_ms()))
        .map(|ms| Duration::from_millis(ms as u64))
}

/// `GET <API><path>` (a path with its query: "issues/TRACK-1/comments?perPage=50").
pub fn get(auth: &Auth, path: &str) -> Result<Value, ClientError> {
    send(auth, "GET", path, None).map(|(value, _)| value)
}

/// `POST <API><path>` with a JSON body.
pub fn post(auth: &Auth, path: &str, body: &Value) -> Result<Value, ClientError> {
    send(auth, "POST", path, Some(body)).map(|(value, _)| value)
}

/// `GET` read as `T`.
pub fn get_as<T: DeserializeOwned>(auth: &Auth, path: &str) -> Result<T, ClientError> {
    let value = get(auth, path)?;
    serde_json::from_value(value).map_err(|err| ClientError::Other(err.to_string()))
}

/// A request: the response's JSON (null for an empty body) and its headers.
pub fn send(
    auth: &Auth,
    method: &str,
    path: &str,
    body: Option<&Value>,
) -> Result<(Value, Vec<(String, String)>), ClientError> {
    let Some(headers) = auth.headers() else {
        return Err(ClientError::SignedOut);
    };
    let mut request = http::request(method, &format!("{API}{}", path.trim_start_matches('/')))
        .header("Accept", "application/json")
        .header("Accept-Language", &flux_plugin_api::host::i18n::language())
        .timeout(TIMEOUT);
    for (name, value) in &headers {
        request = request.header(name, value);
    }
    if let Some(body) = body {
        request = request.json(body);
    }
    let response = request
        .fetch()
        .map_err(|err| ClientError::Other(err.message()))?;
    if response.status == 429 {
        let wait = retry_after(&response.headers);
        let wait = BACKOFF.with(|backoff| backoff.borrow_mut().strike(now_ms(), wait, jitter()));
        flux_plugin_api::log::warn(&format!(
            "Tracker: 429, waiting {wait} ms before polling again"
        ));
    } else {
        BACKOFF.with(|backoff| backoff.borrow_mut().clear());
    }
    let text = response.text();
    let json = if text.trim().is_empty() {
        Value::Null
    } else {
        serde_json::from_str(&text).unwrap_or(Value::Null)
    };
    match response.status {
        200..=299 => Ok((json, response.headers.clone())),
        401 => {
            auth.token_rejected();
            Err(ClientError::Unauthorized)
        }
        403 => Err(ClientError::Forbidden(tracker_words(&json))),
        404 => Err(ClientError::NotFound),
        429 => Err(ClientError::TooMany),
        400 => Err(ClientError::BadRequest(tracker_words(&json))),
        status => {
            let words = tracker_words(&json);
            Err(ClientError::Other(if words.is_empty() {
                trf("Tracker answered {0}", &[&status.to_string()])
            } else {
                words
            }))
        }
    }
}

/// Tracker's own words of an error: `{"errorMessages": […], "errors": {field: text}}`.
fn tracker_words(json: &Value) -> String {
    let mut words: Vec<String> = json["errorMessages"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|text| text.as_str().map(str::to_string))
        .collect();
    if let Some(errors) = json["errors"].as_object() {
        words.extend(
            errors
                .iter()
                .filter_map(|(field, text)| Some(format!("{field}: {}", text.as_str()?))),
        );
    }
    words.join("; ")
}

/// The task's page in Tracker.
pub fn web_url(key: &str) -> String {
    format!("{WEB}{key}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backs_off_after_429() {
        let mut backoff = Backoff::default();
        assert_eq!(backoff.left(0), None);
        // Tracker's Retry-After wins.
        assert_eq!(backoff.strike(1_000, Some(7), 0.9), 7_000);
        assert_eq!(backoff.left(1_000), Some(7_000));
        assert_eq!(backoff.left(8_000), None);
        // Without it: 2 s, 4 s, 8 s … at most 2 min, with jitter of ±50 %.
        let mut backoff = Backoff::default();
        assert_eq!(backoff.strike(0, None, 0.5), 2_000);
        assert_eq!(backoff.strike(0, None, 0.5), 4_000);
        assert_eq!(backoff.strike(0, None, 0.0), 4_000);
        for _ in 0..20 {
            backoff.strike(0, None, 0.5);
        }
        assert_eq!(backoff.strike(0, None, 1.0), 180_000);
        assert_eq!(backoff.strike(0, None, 0.5), 120_000);
        // A huge Retry-After is capped.
        assert_eq!(backoff.strike(0, Some(86_400), 0.5), 600_000);
        backoff.clear();
        assert_eq!(backoff.left(0), None);
        assert_eq!(backoff.strike(0, None, 0.5), 2_000);
        let headers = vec![("Retry-After".to_string(), " 3 ".to_string())];
        assert_eq!(retry_after(&headers), Some(3));
        assert_eq!(
            retry_after(&[("retry-after".into(), "Wed, 21 Oct 2026 07:28:00 GMT".into())]),
            None
        );
    }
}
