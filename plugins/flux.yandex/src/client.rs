//! Requests to Tracker, shared by the Tracker window and the board: `https://api.tracker.yandex.net/
//! v3/…` with the headers of [`Auth`] (authorization, the organization) and the interface
//! language, JSON in and out; a failure in words for the user: signed out, 401 (sign in again), 403
//! (the organization or access), 404, 429 (Tracker allows 5–10 requests a second, one search at a
//! time), Tracker's own `errorMessages`. Requests block the plugin, not Flux (`fetch`; the wait
//! doesn't count toward its time limit).
//!
//! Agent F extends it (paging with `X-Total-Count`, scroll); the board (F2) uses it as it is.

use std::time::Duration;

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
            ClientError::TooMany => tr("Too many requests to Tracker: try again in a moment").to_string(),
            ClientError::Other(text) => text.clone(),
        }
    }
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
    let text = response.text();
    let json = if text.trim().is_empty() {
        Value::Null
    } else {
        serde_json::from_str(&text).unwrap_or(Value::Null)
    };
    match response.status {
        200..=299 => Ok((json, response.headers.clone())),
        401 => Err(ClientError::Unauthorized),
        403 => Err(ClientError::Forbidden(tracker_words(&json))),
        404 => Err(ClientError::NotFound),
        429 => Err(ClientError::TooMany),
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
