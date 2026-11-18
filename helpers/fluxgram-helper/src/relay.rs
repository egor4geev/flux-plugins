//! The protocol without TDLib: the plugin's lines, the accounts' clients, TDLib's messages as the
//! plugin gets them, the batches (README.md → Protocol).

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};

use serde_json::Value;

use crate::render::{self, Render};

/// A batch is written once it reaches this size; a single bigger message goes on its own.
pub const BATCH_LIMIT: usize = 1 << 20;

/// What a line of the plugin asks for.
#[derive(Debug, PartialEq)]
pub enum Input {
    /// A TDLib request for the account `key`: the object without `@client`, as JSON.
    Request { key: String, request: String },
    /// `fluxgram.execute`: TDLib's synchronous request, as JSON, and the line's `@extra`.
    Execute { query: String, extra: Option<Value> },
    /// `fluxgram.keep` or `fluxgram.drop`: which updates pass from now on.
    Filter(Filter),
    /// `fluxgram.chats`: the chats whose updates pass for the account `key`; none — every chat's.
    Chats {
        key: String,
        chats: Option<HashSet<i64>>,
    },
    /// `fluxgram.render`: an animated sticker into Flux's file of frames, and the line's `@extra`.
    Render {
        render: Render,
        extra: Option<Value>,
    },
    /// `fluxgram.quit`.
    Quit,
    /// An empty line: nothing to do.
    Nothing,
}

/// A line the program couldn't take: `fluxgram.error`.
#[derive(Debug, PartialEq)]
pub struct InputError {
    pub message: String,
    pub extra: Option<Value>,
}

impl InputError {
    pub fn new(message: impl Into<String>, extra: Option<&Value>) -> Self {
        Self {
            message: message.into(),
            extra: extra.cloned(),
        }
    }

    /// The `fluxgram.error` line.
    pub fn line(&self) -> String {
        let mut line = format!(
            "{{\"@type\":\"fluxgram.error\",\"message\":{}",
            Value::from(self.message.as_str())
        );
        if let Some(extra) = &self.extra {
            line.push_str(",\"@extra\":");
            line.push_str(&extra.to_string());
        }
        line.push_str("}\n");
        line
    }
}

/// Reads a line of the plugin (without its `\n`; a `\r` before it is taken off too).
pub fn parse_input(line: &[u8]) -> Result<Input, InputError> {
    let line = line.strip_suffix(b"\r").unwrap_or(line);
    if line.iter().all(u8::is_ascii_whitespace) {
        return Ok(Input::Nothing);
    }
    let value: Value = serde_json::from_slice(line)
        .map_err(|error| InputError::new(format!("Not a JSON line: {error}"), None))?;
    let Value::Object(mut object) = value else {
        return Err(InputError::new("Not a JSON object", None));
    };
    let extra = object.get("@extra").cloned();
    let kind = object.get("@type").and_then(Value::as_str).unwrap_or("");

    if let Some(command) = kind.strip_prefix("fluxgram.") {
        return match command {
            "execute" => match object.get("query") {
                Some(query @ Value::Object(_)) => Ok(Input::Execute {
                    query: query.to_string(),
                    extra,
                }),
                _ => Err(InputError::new(
                    "fluxgram.execute needs \"query\": a TDLib request object",
                    extra.as_ref(),
                )),
            },
            "keep" | "drop" => {
                let types = object
                    .get("types")
                    .and_then(Value::as_array)
                    .and_then(|types| {
                        types
                            .iter()
                            .map(|kind| kind.as_str().map(String::from))
                            .collect::<Option<HashSet<_>>>()
                    });
                let filter = match command {
                    "keep" => types.map(Filter::Keep),
                    _ => types.map(Filter::Drop),
                };
                filter.map(Input::Filter).ok_or_else(|| {
                    InputError::new(
                        format!("{kind} needs \"types\": an array of update types"),
                        extra.as_ref(),
                    )
                })
            }
            "chats" => {
                let key = match object.get("@client") {
                    Some(Value::String(key)) if !key.is_empty() => key.clone(),
                    _ => {
                        return Err(InputError::new(
                            "fluxgram.chats needs \"@client\": the account's key",
                            extra.as_ref(),
                        ));
                    }
                };
                let chats = match object.get("chats") {
                    Some(Value::Null) => Some(None),
                    Some(Value::Array(ids)) => ids
                        .iter()
                        .map(Value::as_i64)
                        .collect::<Option<HashSet<_>>>()
                        .map(Some),
                    _ => None,
                };
                chats
                    .map(|chats| Input::Chats { key, chats })
                    .ok_or_else(|| {
                        InputError::new(
                            "fluxgram.chats needs \"chats\": an array of chat ids, or null",
                            extra.as_ref(),
                        )
                    })
            }
            "render" => {
                let path = |name: &str| match object.get(name) {
                    Some(Value::String(path)) if !path.is_empty() => Some(path.clone()),
                    _ => None,
                };
                let size = object
                    .get("size")
                    .and_then(Value::as_u64)
                    .filter(|size| (1..=u64::from(render::MAX_SIZE)).contains(size));
                let fps = match object.get("fps") {
                    None | Some(Value::Null) => Some(u64::from(render::DEFAULT_FPS)),
                    Some(fps) => fps.as_u64().filter(|fps| (1..=60).contains(fps)),
                };
                match (path("input"), path("output"), size, fps) {
                    (Some(input), Some(output), Some(size), Some(fps)) => Ok(Input::Render {
                        render: Render {
                            input,
                            output,
                            size: size as u32,
                            fps: fps as u32,
                        },
                        extra,
                    }),
                    _ => Err(InputError::new(
                        format!(
                            "fluxgram.render needs \"input\" and \"output\" (files), \"size\" (1 to {} pixels) and maybe \"fps\" (1 to 60)",
                            render::MAX_SIZE
                        ),
                        extra.as_ref(),
                    )),
                }
            }
            "quit" => Ok(Input::Quit),
            _ => Err(InputError::new(
                format!("Unknown request {kind}"),
                extra.as_ref(),
            )),
        };
    }

    match object.remove("@client") {
        Some(Value::String(key)) if !key.is_empty() => Ok(Input::Request {
            key,
            request: Value::Object(object).to_string(),
        }),
        Some(_) => Err(InputError::new(
            "\"@client\" must be a non-empty string",
            extra.as_ref(),
        )),
        None => Err(InputError::new(
            "A TDLib request needs \"@client\": the account's key",
            extra.as_ref(),
        )),
    }
}

/// The `fluxgram.executed` line: `result` is TDLib's answer, JSON as it is.
pub fn executed_line(result: Option<&str>, extra: Option<&Value>) -> String {
    let mut line = format!(
        "{{\"@type\":\"fluxgram.executed\",\"result\":{}",
        result.unwrap_or("null")
    );
    if let Some(extra) = extra {
        line.push_str(",\"@extra\":");
        line.push_str(&extra.to_string());
    }
    line.push_str("}\n");
    line
}

/// Which of TDLib's updates (`update…`) pass to the plugin: the ones it uses, or all but the ones
/// it doesn't. Answers always pass, and so does a client's `authorizationStateClosed`.
#[derive(Debug, PartialEq)]
pub enum Filter {
    /// `fluxgram.keep`: only these types.
    Keep(HashSet<String>),
    /// `fluxgram.drop`: all but these types; an empty list passes everything.
    Drop(HashSet<String>),
}

impl Default for Filter {
    /// Everything passes until the plugin says otherwise.
    fn default() -> Self {
        Filter::Drop(HashSet::new())
    }
}

impl Filter {
    /// An update of `kind` passes.
    fn passes(&self, kind: &str) -> bool {
        match self {
            Filter::Keep(types) => types.contains(kind),
            Filter::Drop(types) => !types.contains(kind),
        }
    }
}

/// The accounts' TDLib clients, and which updates pass to the plugin.
#[derive(Default)]
pub struct Clients {
    /// The live client of each key.
    live: HashMap<String, i32>,
    /// The key of every client made, as a JSON string (a closed one's late messages keep theirs).
    keys: HashMap<i32, String>,
    filter: Filter,
    /// The chats whose updates pass, by the account's key as a JSON string (`fluxgram.chats`); an
    /// account without — every chat's. It may come before the client is made; it goes when the
    /// client closes.
    chats: HashMap<String, HashSet<i64>>,
}

impl Clients {
    /// The client of `key`: the live one, or a new one made by `create` (TDLib's
    /// `td_create_client_id`).
    pub fn client(&mut self, key: &str, create: impl FnOnce() -> i32) -> i32 {
        if let Some(&id) = self.live.get(key) {
            return id;
        }
        let id = create();
        self.live.insert(key.to_string(), id);
        self.keys.insert(id, Value::from(key).to_string());
        id
    }

    /// The live clients (to close them at the end).
    pub fn live_ids(&self) -> Vec<i32> {
        self.live.values().copied().collect()
    }

    pub fn is_empty(&self) -> bool {
        self.live.is_empty()
    }

    /// A new `fluxgram.keep` or `fluxgram.drop`: it replaces the one before, whichever it was.
    pub fn set_filter(&mut self, filter: Filter) {
        self.filter = filter;
    }

    /// A new `fluxgram.chats` of the account `key`: the chats whose updates pass; none — every
    /// chat's.
    pub fn set_chats(&mut self, key: &str, chats: Option<HashSet<i64>>) {
        let key = Value::from(key).to_string();
        match chats {
            Some(chats) => self.chats.insert(key, chats),
            None => self.chats.remove(&key),
        };
    }

    /// An update of the client `key` passes its chat filter: it is about a listed chat, or about
    /// no chat at all (a user, a file, a group's members), or the account has no filter.
    fn chat_passes(&self, key: Option<&String>, raw: &str) -> bool {
        let Some(listed) = key.and_then(|key| self.chats.get(key)) else {
            return true;
        };
        update_chat(raw).is_none_or(|chat| listed.contains(&chat))
    }

    /// Puts TDLib's message `raw` into `batch` as the plugin gets it: `"@client": <key>` in place
    /// of `@client_id`, unless it's an update the filter holds back. After a client's
    /// `authorizationStateClosed` comes `fluxgram.closed`, and the key is free for a new client:
    /// true then.
    pub fn relay(&mut self, raw: &str, batch: &mut Batch) -> bool {
        let Message { kind, members, id } = Message::read(raw);
        let closed = kind == "updateAuthorizationState"
            && raw.contains("\"@type\":\"authorizationStateClosed\"");
        let key = id.and_then(|id| self.keys.get(&id));
        let held_back = kind.starts_with("update")
            && !closed
            && (!self.filter.passes(&kind) || !self.chat_passes(key, raw));

        if !held_back {
            let comma = if members.is_empty() { "" } else { "," };
            match key {
                Some(key) => batch.push_parts(&["{\"@client\":", key, comma, &members, "}"]),
                None => batch.push_parts(&["{", &members, "}"]),
            }
        }

        if closed && let Some(id) = id {
            if let Some(key) = self.keys.get(&id) {
                batch.push_parts(&["{\"@type\":\"fluxgram.closed\",\"@client\":", key, "}"]);
                // The filter goes with its client: a new one starts without.
                self.chats.remove(key);
            }
            self.live.retain(|_, live| *live != id);
            return true;
        }
        false
    }
}

/// The chat an update is about: its `chat_id`, its message's (`updateNewMessage`,
/// `updateMessageSendSucceeded`), the new chat's (`updateNewChat`), or the person's of
/// `updateUserStatus` (a person's chat has the person's id); none — an update of no chat.
fn update_chat(raw: &str) -> Option<i64> {
    let update: Value = serde_json::from_str(raw).ok()?;
    update["chat_id"]
        .as_i64()
        .or_else(|| update["message"]["chat_id"].as_i64())
        .or_else(|| match update["@type"].as_str()? {
            "updateNewChat" => update["chat"]["id"].as_i64(),
            "updateUserStatus" => update["user_id"].as_i64(),
            _ => None,
        })
}

/// What the program reads of TDLib's message.
struct Message<'a> {
    /// `@type`.
    kind: Cow<'a, str>,
    /// The members without the braces and `@client_id`.
    members: Cow<'a, str>,
    /// `@client_id`.
    id: Option<i32>,
}

impl<'a> Message<'a> {
    /// TDLib writes `@type` first and `@client_id` last — `{"@type":"ok",…,"@client_id":3}` — so
    /// both are found without reading the whole object (a quote inside a string is escaped:
    /// `,"@client_id":` can only be the key). Another order is read whole.
    fn read(raw: &'a str) -> Self {
        const TYPE: &str = "{\"@type\":\"";
        const CLIENT_ID: &str = ",\"@client_id\":";
        if let Some(rest) = raw.strip_prefix(TYPE)
            && let Some((kind, _)) = rest.split_once('"')
            && let Some(at) = raw.rfind(CLIENT_ID)
            && let Some(number) = raw[at + CLIENT_ID.len()..].strip_suffix('}')
            && let Ok(id) = number.trim().parse::<i32>()
        {
            return Self {
                kind: Cow::Borrowed(kind),
                members: Cow::Borrowed(&raw[1..at]),
                id: Some(id),
            };
        }
        let Ok(Value::Object(mut object)) = serde_json::from_str::<Value>(raw) else {
            // TDLib gives objects only.
            return Self {
                kind: Cow::Borrowed(""),
                members: Cow::Borrowed(raw.trim().trim_start_matches('{').trim_end_matches('}')),
                id: None,
            };
        };
        let id = object
            .remove("@client_id")
            .and_then(|id| id.as_i64())
            .and_then(|id| i32::try_from(id).ok());
        let kind = object
            .get("@type")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let text = Value::Object(object).to_string();
        Self {
            kind: Cow::Owned(kind),
            members: Cow::Owned(text[1..text.len() - 1].to_string()),
            id,
        }
    }
}

/// TDLib's messages that are ready at once, as the lines of one write.
#[derive(Default)]
pub struct Batch {
    bytes: Vec<u8>,
    lines: usize,
}

impl Batch {
    fn push_parts(&mut self, parts: &[&str]) {
        for part in parts {
            self.bytes.extend_from_slice(part.as_bytes());
        }
        self.bytes.push(b'\n');
        self.lines += 1;
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn lines(&self) -> usize {
        self.lines
    }

    pub fn is_empty(&self) -> bool {
        self.lines == 0
    }

    pub fn clear(&mut self) {
        self.bytes.clear();
        self.lines = 0;
    }
}

/// The messages of one batch: `first` (the one TDLib was waited for), then whatever `next` has
/// ready at once (TDLib's `td_receive(0)`), up to [`BATCH_LIMIT`] bytes.
pub fn gather(first: String, mut next: impl FnMut() -> Option<String>) -> Vec<String> {
    let mut size = first.len();
    let mut messages = vec![first];
    while size < BATCH_LIMIT {
        let Some(message) = next() else { break };
        size += message.len();
        messages.push(message);
    }
    messages
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(line: &str) -> (String, Value) {
        match parse_input(line.as_bytes()) {
            Ok(Input::Request { key, request }) => (key, serde_json::from_str(&request).unwrap()),
            other => panic!("not a request: {other:?}"),
        }
    }

    fn error(line: &str) -> InputError {
        parse_input(line.as_bytes()).unwrap_err()
    }

    #[test]
    fn a_request_goes_on_without_its_key() {
        let (key, object) =
            request(r#"{"@client":"a1","@type":"getOption","name":"version","@extra":1}"#);
        assert_eq!(key, "a1");
        assert_eq!(
            object,
            serde_json::json!({"@type": "getOption", "name": "version", "@extra": 1})
        );
        // A line from Windows: `\r\n`.
        let (key, _) = request("{\"@client\":\"a2\",\"@type\":\"close\"}\r");
        assert_eq!(key, "a2");
    }

    #[test]
    fn lines_the_program_cannot_take() {
        assert_eq!(error("not json").extra, None);
        assert!(error("not json").message.starts_with("Not a JSON line"));
        assert_eq!(error("[1, 2]").message, "Not a JSON object");
        let no_key = error(r#"{"@type":"getMe","@extra":"x"}"#);
        assert!(no_key.message.contains("@client"));
        assert_eq!(no_key.extra, Some(Value::from("x")));
        assert_eq!(error(r#"{"@client":"","@type":"getMe"}"#).extra, None);
        assert!(
            error(r#"{"@client":5,"@type":"getMe"}"#)
                .message
                .contains("string")
        );
        let unknown = error(r#"{"@type":"fluxgram.nope","@extra":3}"#);
        assert_eq!(unknown.message, "Unknown request fluxgram.nope");
        assert_eq!(
            unknown.line(),
            "{\"@type\":\"fluxgram.error\",\"message\":\"Unknown request fluxgram.nope\",\"@extra\":3}\n"
        );
        assert!(
            error(r#"{"@type":"fluxgram.execute"}"#)
                .message
                .contains("query")
        );
        assert!(
            error(r#"{"@type":"fluxgram.drop","types":[1]}"#)
                .message
                .contains("types")
        );
        let keep = error(r#"{"@type":"fluxgram.keep","types":"updateUser","@extra":4}"#);
        assert!(keep.message.starts_with("fluxgram.keep needs \"types\""));
        assert_eq!(keep.extra, Some(Value::from(4)));
    }

    #[test]
    fn render_requests() {
        let line = r#"{"@type":"fluxgram.render","input":"/s/1.tgs","output":"/c/1-320.fxa","size":320,"fps":30,"@extra":9}"#;
        assert_eq!(
            parse_input(line.as_bytes()),
            Ok(Input::Render {
                render: Render {
                    input: "/s/1.tgs".into(),
                    output: "/c/1-320.fxa".into(),
                    size: 320,
                    fps: 30,
                },
                extra: Some(Value::from(9)),
            })
        );
        let default_fps = r#"{"@type":"fluxgram.render","input":"a","output":"b","size":144}"#;
        assert!(matches!(
            parse_input(default_fps.as_bytes()),
            Ok(Input::Render {
                render: Render { fps: 30, .. },
                extra: None
            })
        ));
        for line in [
            r#"{"@type":"fluxgram.render","output":"b","size":144,"@extra":2}"#,
            r#"{"@type":"fluxgram.render","input":"a","output":"","size":144,"@extra":2}"#,
            r#"{"@type":"fluxgram.render","input":"a","output":"b","size":0,"@extra":2}"#,
            r#"{"@type":"fluxgram.render","input":"a","output":"b","size":5000,"@extra":2}"#,
            r#"{"@type":"fluxgram.render","input":"a","output":"b","size":144,"fps":0,"@extra":2}"#,
        ] {
            let error = error(line);
            assert!(error.message.starts_with("fluxgram.render needs"), "{line}");
            assert_eq!(error.extra, Some(Value::from(2)));
        }
    }

    #[test]
    fn empty_lines_are_nothing() {
        assert_eq!(parse_input(b""), Ok(Input::Nothing));
        assert_eq!(parse_input(b"  \r"), Ok(Input::Nothing));
    }

    #[test]
    fn the_program_own_requests() {
        assert_eq!(
            parse_input(br#"{"@type":"fluxgram.quit"}"#),
            Ok(Input::Quit)
        );
        assert_eq!(
            parse_input(br#"{"@type":"fluxgram.drop","types":["updateUserStatus"]}"#),
            Ok(Input::Filter(Filter::Drop(HashSet::from([
                "updateUserStatus".to_string()
            ]))))
        );
        assert_eq!(
            parse_input(br#"{"@type":"fluxgram.drop","types":[]}"#),
            Ok(Input::Filter(Filter::Drop(HashSet::new())))
        );
        assert_eq!(
            parse_input(br#"{"@type":"fluxgram.keep","types":["updateUser","updateFile"]}"#),
            Ok(Input::Filter(Filter::Keep(HashSet::from([
                "updateUser".to_string(),
                "updateFile".to_string()
            ]))))
        );
        let Ok(Input::Execute { query, extra }) = parse_input(
            br#"{"@type":"fluxgram.execute","query":{"@type":"getTextEntities","text":"a"},"@extra":7}"#,
        ) else {
            panic!("not an execute");
        };
        assert_eq!(
            serde_json::from_str::<Value>(&query).unwrap(),
            serde_json::json!({"@type": "getTextEntities", "text": "a"})
        );
        assert_eq!(extra, Some(Value::from(7)));
        assert_eq!(
            executed_line(Some(r#"{"@type":"ok"}"#), extra.as_ref()),
            "{\"@type\":\"fluxgram.executed\",\"result\":{\"@type\":\"ok\"},\"@extra\":7}\n"
        );
        assert_eq!(
            executed_line(None, None),
            "{\"@type\":\"fluxgram.executed\",\"result\":null}\n"
        );
    }

    #[test]
    fn a_key_keeps_its_client_until_it_closes() {
        let mut clients = Clients::default();
        let mut made = 0;
        let mut create = || {
            made += 1;
            made
        };
        let a1 = clients.client("a1", &mut create);
        assert_eq!(clients.client("a1", &mut create), a1);
        let a2 = clients.client("a2", &mut create);
        assert_ne!(a1, a2);
        let mut ids = clients.live_ids();
        ids.sort();
        assert_eq!(ids, vec![a1, a2]);

        let mut batch = Batch::default();
        clients.relay(
            &format!(
                r#"{{"@type":"updateAuthorizationState","authorization_state":{{"@type":"authorizationStateClosed"}},"@client_id":{a1}}}"#
            ),
            &mut batch,
        );
        let text = String::from_utf8(batch.bytes().to_vec()).unwrap();
        let lines: Vec<Value> = text
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0]["@client"], "a1");
        assert_eq!(
            lines[0]["authorization_state"]["@type"],
            "authorizationStateClosed"
        );
        assert_eq!(
            lines[1],
            serde_json::json!({"@type": "fluxgram.closed", "@client": "a1"})
        );

        // The key is free: its next request makes a new client; the other one stays.
        assert_eq!(clients.live_ids(), vec![a2]);
        let again = clients.client("a1", &mut create);
        assert_ne!(again, a1);
    }

    #[test]
    fn messages_carry_the_key_in_place_of_the_client_id() {
        let mut clients = Clients::default();
        let id = clients.client("a\"1", || 7);
        let mut batch = Batch::default();
        clients.relay(
            r#"{"@type":"optionValueString","value":"1.8.61","@extra":1,"@client_id":7}"#,
            &mut batch,
        );
        // A text that looks like the key, inside a string, stays as it is.
        clients.relay(
            &format!(r#"{{"@type":"message","text":",\"@client_id\":9}}","@client_id":{id}}}"#),
            &mut batch,
        );
        // A message of an unknown client, and one without a client.
        clients.relay(r#"{"@type":"ok","@client_id":99}"#, &mut batch);
        clients.relay(r#"{"@type":"ok"}"#, &mut batch);
        // `@client_id` in another place than TDLib's: the object is read whole.
        clients.relay(r#"{"@client_id":7,"@type":"ok"}"#, &mut batch);
        assert_eq!(batch.lines(), 5);
        let text = String::from_utf8(batch.bytes().to_vec()).unwrap();
        assert!(text.starts_with("{\"@client\":\"a\\\"1\",\"@type\":\"optionValueString\""));
        let lines: Vec<Value> = text
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(
            lines[0],
            serde_json::json!({"@client": "a\"1", "@type": "optionValueString", "value": "1.8.61", "@extra": 1})
        );
        assert_eq!(lines[1]["@client"], "a\"1");
        assert_eq!(lines[1]["text"], ",\"@client_id\":9}");
        assert!(lines[1].get("@client_id").is_none());
        assert_eq!(lines[2], serde_json::json!({"@type": "ok"}));
        assert_eq!(lines[3], serde_json::json!({"@type": "ok"}));
        assert_eq!(
            lines[4],
            serde_json::json!({"@client": "a\"1", "@type": "ok"})
        );
    }

    #[test]
    fn the_drop_list_holds_back_updates_only() {
        let mut clients = Clients::default();
        clients.client("a1", || 1);
        clients.set_filter(Filter::Drop(HashSet::from([
            "updateUserStatus".to_string(),
            "ok".to_string(),
            "updateAuthorizationState".to_string(),
        ])));
        let mut batch = Batch::default();
        clients.relay(
            r#"{"@type":"updateUserStatus","user_id":5,"@client_id":1}"#,
            &mut batch,
        );
        assert!(batch.is_empty());
        // Answers always go: only updates are held back.
        clients.relay(r#"{"@type":"ok","@extra":2,"@client_id":1}"#, &mut batch);
        assert_eq!(batch.lines(), 1);
        // A client's end always goes, and so does fluxgram.closed.
        clients.relay(
            r#"{"@type":"updateAuthorizationState","authorization_state":{"@type":"authorizationStateClosing"},"@client_id":1}"#,
            &mut batch,
        );
        assert_eq!(batch.lines(), 1);
        clients.relay(
            r#"{"@type":"updateAuthorizationState","authorization_state":{"@type":"authorizationStateClosed"},"@client_id":1}"#,
            &mut batch,
        );
        assert_eq!(batch.lines(), 3);
        assert!(clients.is_empty());

        // In another order than TDLib's the object is read whole: its end is still seen.
        clients.client("a2", || 2);
        let before = batch.lines();
        assert!(clients.relay(
            r#"{"@client_id":2,"authorization_state":{"@type":"authorizationStateClosed"},"@type":"updateAuthorizationState"}"#,
            &mut batch,
        ));
        assert_eq!(batch.lines(), before + 2);
        assert!(clients.is_empty());

        // An empty list passes everything.
        clients.set_filter(Filter::Drop(HashSet::new()));
        batch.clear();
        clients.relay(
            r#"{"@type":"updateUserStatus","user_id":5,"@client_id":1}"#,
            &mut batch,
        );
        assert_eq!(batch.lines(), 1);
    }

    #[test]
    fn the_keep_list_passes_its_updates_only() {
        let mut clients = Clients::default();
        clients.client("a1", || 1);
        // Everything passes before the plugin says otherwise.
        let mut batch = Batch::default();
        clients.relay(
            r#"{"@type":"updateUserStatus","user_id":5,"@client_id":1}"#,
            &mut batch,
        );
        assert_eq!(batch.lines(), 1);

        clients.set_filter(Filter::Keep(HashSet::from([
            "updateUser".to_string(),
            "ok".to_string(),
        ])));
        batch.clear();
        clients.relay(
            r#"{"@type":"updateUserStatus","user_id":5,"@client_id":1}"#,
            &mut batch,
        );
        clients.relay(
            r#"{"@type":"updateNewMessage","message":{},"@client_id":1}"#,
            &mut batch,
        );
        assert!(batch.is_empty());
        clients.relay(
            r#"{"@type":"updateUser","user":{"@type":"user","id":5},"@client_id":1}"#,
            &mut batch,
        );
        assert_eq!(batch.lines(), 1);
        // Answers always pass, kept or not.
        clients.relay(
            r#"{"@type":"user","id":5,"@extra":3,"@client_id":1}"#,
            &mut batch,
        );
        clients.relay(
            r#"{"@type":"error","code":400,"message":"x","@extra":4,"@client_id":1}"#,
            &mut batch,
        );
        assert_eq!(batch.lines(), 3);
        // A client's other states are held back when `updateAuthorizationState` isn't kept; its
        // end always passes, with fluxgram.closed after it.
        clients.relay(
            r#"{"@type":"updateAuthorizationState","authorization_state":{"@type":"authorizationStateClosing"},"@client_id":1}"#,
            &mut batch,
        );
        assert_eq!(batch.lines(), 3);
        assert!(clients.relay(
            r#"{"@type":"updateAuthorizationState","authorization_state":{"@type":"authorizationStateClosed"},"@client_id":1}"#,
            &mut batch,
        ));
        assert_eq!(batch.lines(), 5);
        assert!(clients.is_empty());

        // A drop list replaces the keep list, and a keep list a drop list.
        clients.client("a2", || 2);
        clients.set_filter(Filter::Drop(HashSet::from(["updateUser".to_string()])));
        batch.clear();
        clients.relay(
            r#"{"@type":"updateUserStatus","user_id":5,"@client_id":2}"#,
            &mut batch,
        );
        clients.relay(
            r#"{"@type":"updateUser","user":{"@type":"user","id":5},"@client_id":2}"#,
            &mut batch,
        );
        assert_eq!(batch.lines(), 1);
        clients.set_filter(Filter::Keep(HashSet::new()));
        batch.clear();
        clients.relay(
            r#"{"@type":"updateUserStatus","user_id":5,"@client_id":2}"#,
            &mut batch,
        );
        assert!(batch.is_empty());
    }

    #[test]
    fn the_chats_filter_is_read() {
        assert_eq!(
            parse_input(
                br#"{"@type":"fluxgram.chats","@client":"a1","chats":[101,-1000000001001]}"#
            ),
            Ok(Input::Chats {
                key: "a1".to_string(),
                chats: Some(HashSet::from([101, -1000000001001])),
            })
        );
        assert_eq!(
            parse_input(br#"{"@type":"fluxgram.chats","@client":"a1","chats":[]}"#),
            Ok(Input::Chats {
                key: "a1".to_string(),
                chats: Some(HashSet::new()),
            })
        );
        assert_eq!(
            parse_input(br#"{"@type":"fluxgram.chats","@client":"a1","chats":null}"#),
            Ok(Input::Chats {
                key: "a1".to_string(),
                chats: None,
            })
        );
        assert!(
            error(r#"{"@type":"fluxgram.chats","chats":[1],"@extra":5}"#)
                .message
                .contains("@client")
        );
        assert!(
            error(r#"{"@type":"fluxgram.chats","@client":"a1","chats":["x"]}"#)
                .message
                .contains("chat ids")
        );
        assert!(
            error(r#"{"@type":"fluxgram.chats","@client":"a1"}"#)
                .message
                .contains("chat ids")
        );
    }

    #[test]
    fn only_the_listed_chats_updates_pass() {
        let mut clients = Clients::default();
        // The filter may come before the client.
        clients.set_chats("a1", Some(HashSet::from([101, -201])));
        clients.client("a1", || 1);
        clients.client("a2", || 2);
        fn passes(clients: &mut Clients, line: &str) -> bool {
            let mut batch = Batch::default();
            clients.relay(line, &mut batch);
            batch.lines() == 1
        }
        // By `chat_id`.
        assert!(passes(
            &mut clients,
            r#"{"@type":"updateChatReadInbox","chat_id":101,"unread_count":0,"@client_id":1}"#
        ));
        assert!(!passes(
            &mut clients,
            r#"{"@type":"updateChatReadInbox","chat_id":102,"unread_count":0,"@client_id":1}"#
        ));
        // By the message's chat; a quote of a listed chat inside doesn't count.
        assert!(passes(
            &mut clients,
            r#"{"@type":"updateNewMessage","message":{"id":1,"chat_id":-201},"@client_id":1}"#
        ));
        assert!(!passes(
            &mut clients,
            r#"{"@type":"updateNewMessage","message":{"id":1,"chat_id":-202,"reply_to":{"chat_id":101}},"@client_id":1}"#
        ));
        // By the new chat's id, and the person's of a status.
        assert!(passes(
            &mut clients,
            r#"{"@type":"updateNewChat","chat":{"id":101,"title":"Anna"},"@client_id":1}"#
        ));
        assert!(!passes(
            &mut clients,
            r#"{"@type":"updateNewChat","chat":{"id":102,"title":"Dima"},"@client_id":1}"#
        ));
        assert!(passes(
            &mut clients,
            r#"{"@type":"updateUserStatus","user_id":101,"status":{},"@client_id":1}"#
        ));
        assert!(!passes(
            &mut clients,
            r#"{"@type":"updateUserStatus","user_id":102,"status":{},"@client_id":1}"#
        ));
        // Updates of no chat pass, and so do answers, whatever chat they are of.
        assert!(passes(
            &mut clients,
            r#"{"@type":"updateUser","user":{"id":102},"@client_id":1}"#
        ));
        assert!(passes(
            &mut clients,
            r#"{"@type":"updateFile","file":{"id":7},"@client_id":1}"#
        ));
        assert!(passes(
            &mut clients,
            r#"{"@type":"chat","id":102,"@extra":3,"@client_id":1}"#
        ));
        // Another account has no filter.
        assert!(passes(
            &mut clients,
            r#"{"@type":"updateChatReadInbox","chat_id":102,"@client_id":2}"#
        ));
        // The type filter still comes first.
        clients.set_filter(Filter::Keep(HashSet::from(["updateChatTitle".to_string()])));
        assert!(!passes(
            &mut clients,
            r#"{"@type":"updateChatReadInbox","chat_id":101,"@client_id":1}"#
        ));
        assert!(passes(
            &mut clients,
            r#"{"@type":"updateChatTitle","chat_id":101,"title":"A","@client_id":1}"#
        ));
        clients.set_filter(Filter::default());
        // An empty list passes no chat's updates; null — every chat's.
        clients.set_chats("a1", Some(HashSet::new()));
        assert!(!passes(
            &mut clients,
            r#"{"@type":"updateChatReadInbox","chat_id":101,"@client_id":1}"#
        ));
        clients.set_chats("a1", None);
        assert!(passes(
            &mut clients,
            r#"{"@type":"updateChatReadInbox","chat_id":102,"@client_id":1}"#
        ));
        // The filter goes with its client.
        clients.set_chats("a1", Some(HashSet::new()));
        let mut batch = Batch::default();
        clients.relay(
            r#"{"@type":"updateAuthorizationState","authorization_state":{"@type":"authorizationStateClosed"},"@client_id":1}"#,
            &mut batch,
        );
        clients.client("a1", || 3);
        assert!(passes(
            &mut clients,
            r#"{"@type":"updateChatReadInbox","chat_id":102,"@client_id":3}"#
        ));
    }

    #[test]
    fn a_batch_takes_what_is_ready_up_to_its_limit() {
        // Everything ready at once.
        let mut ready = vec!["b".to_string(), "c".to_string()].into_iter();
        assert_eq!(gather("a".into(), || ready.next()), vec!["a", "b", "c"]);

        // Past the limit, the rest waits for the next batch.
        let big = "x".repeat(BATCH_LIMIT / 2);
        let mut ready = vec![big.clone(), big.clone(), "next".to_string()].into_iter();
        let batch = gather("a".into(), || ready.next());
        assert_eq!(batch.len(), 3);
        assert_eq!(ready.next().as_deref(), Some("next"));

        // A message bigger than the limit goes alone.
        let huge = "x".repeat(BATCH_LIMIT + 1);
        let mut ready = vec!["after".to_string()].into_iter();
        assert_eq!(gather(huge, || ready.next()).len(), 1);
    }
}
