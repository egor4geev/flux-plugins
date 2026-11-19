//! A chat's pinned messages (part 5): all of them, newest first, as TDLib gives them
//! (`searchChatMessages` with `searchMessagesFilterPinned` when the chat opens, then
//! `updateMessageIsPinned` and the deleted messages), and the bar over the feed that shows one —
//! the newest at first; a click reveals it and the bar goes to the next older one, from the oldest
//! back to the newest, as Telegram's apps do. Its × unpins (asked first) where the user may
//! ([`Store::can_pin`]), otherwise hides the bar in the chat until a newer message is pinned.
//!
//! Needs no host — the words come through [`Words`] — so it is tested natively.

use flux_plugin_api::conversation::{self as conv, ImageSpec, PinnedSpec};
use serde_json::Value;

use crate::model::{Words, downloaded};
use crate::store::Store;
use crate::summary;

/// The bar's action that unpins the message (asked first).
pub const UNPIN: &str = "unpin";
/// …and the one that only hides the bar, where the user can't unpin.
pub const HIDE: &str = "hide";
/// The picture on the bar: the photo's smallest size at least this long (twice its 32 points).
const PICTURE_SIDE: i64 = 64;

/// The pinned messages of an open chat.
#[derive(Debug, Default)]
pub struct Pins {
    /// Newest first.
    messages: Vec<Value>,
    /// The one the bar shows: an index into `messages`.
    at: usize,
    /// The answer to the first search came.
    pub loaded: bool,
}

impl Pins {
    /// The chat's pinned messages, newest first: the first search's answer.
    pub fn set(&mut self, messages: Vec<Value>) {
        self.messages = messages;
        self.messages
            .sort_by_key(|message| std::cmp::Reverse(id_of(message)));
        self.at = 0;
        self.loaded = true;
    }

    /// A message was pinned: it takes its place by its id, and the bar shows it (the newest).
    pub fn pinned(&mut self, message: Value) {
        let id = id_of(&message);
        self.messages.retain(|known| id_of(known) != id);
        let place = self
            .messages
            .iter()
            .position(|known| id_of(known) < id)
            .unwrap_or(self.messages.len());
        self.messages.insert(place, message);
        self.at = place;
    }

    /// Messages were unpinned or deleted: the bar stays on the one it showed, or the next.
    pub fn unpinned(&mut self, ids: &[i64]) -> bool {
        let shown = self.shown_id();
        let before = self.messages.len();
        self.messages
            .retain(|message| !ids.contains(&id_of(message)));
        if self.messages.len() == before {
            return false;
        }
        self.at = match shown.and_then(|id| self.position(id)) {
            Some(at) => at,
            None => self.at.min(self.messages.len().saturating_sub(1)),
        };
        true
    }

    /// A pinned message changed (its text, its file): its new self.
    pub fn changed(&mut self, message: &Value) -> bool {
        let id = id_of(message);
        match self.messages.iter_mut().find(|known| id_of(known) == id) {
            Some(known) => {
                *known = message.clone();
                true
            }
            None => false,
        }
    }

    /// The bar was clicked on `id`: it goes to the next older pinned message, after the oldest
    /// back to the newest.
    pub fn clicked(&mut self, id: i64) {
        if let Some(at) = self.position(id) {
            self.at = (at + 1) % self.messages.len();
        }
    }

    /// The newest pinned message's id: a bar hidden until a newer one is pinned remembers it.
    pub fn newest(&self) -> Option<i64> {
        self.messages.first().map(id_of)
    }

    /// The message the bar shows.
    pub fn shown_id(&self) -> Option<i64> {
        self.messages.get(self.at).map(id_of)
    }

    fn position(&self, id: i64) -> Option<usize> {
        self.messages
            .iter()
            .position(|message| id_of(message) == id)
    }

    /// The file of the bar's picture, when it has one and it isn't downloaded.
    pub fn wanted_file(&self, store: &Store) -> Option<i64> {
        let file = picture_file(self.messages.get(self.at)?)?;
        let id = file["id"].as_i64()?;
        (store.file_path(id).is_none() && downloaded(file).is_none()).then_some(id)
    }

    /// The file of the bar's picture: its download puts the bar again.
    pub fn shown_file(&self) -> Option<i64> {
        picture_file(self.messages.get(self.at)?)?["id"].as_i64()
    }

    /// The bar: the message shown, its picture, which of how many it is, × — unpin (`can_pin`)
    /// or hide. None — no pinned messages, or the bar hidden until a newer one than `hidden` is
    /// pinned.
    pub fn bar(
        &self,
        store: &Store,
        words: &dyn Words,
        can_pin: bool,
        hidden: Option<i64>,
    ) -> Option<PinnedSpec> {
        let message = self.messages.get(self.at)?;
        if hidden.is_some() && hidden == self.newest() {
            return None;
        }
        let count = self.messages.len() as u32;
        let title = match self.at {
            // Telegram's numbers count from the oldest: the newest of three is #3.
            0 => words.tr("Pinned message"),
            at => words.trf(
                "Pinned message #{0}",
                &[&(self.messages.len() - at).to_string()],
            ),
        };
        let mut spec = conv::pinned_spec(
            &id_of(message).to_string(),
            &title,
            &line(store, message, words),
            self.at as u32,
            count,
        );
        if let Some(image) = picture(store, message) {
            spec = spec.image(image);
        }
        spec = match can_pin {
            true => spec.action(UNPIN, &words.tr("Unpin"), "close"),
            false => spec.action(HIDE, &words.tr("Hide"), "close"),
        };
        Some(spec)
    }
}

fn id_of(message: &Value) -> i64 {
    message["id"].as_i64().unwrap_or_default()
}

/// The bar's line: a caption by itself next to its picture, else the message in one line.
fn line(store: &Store, message: &Value, words: &dyn Words) -> String {
    let content = &message["content"];
    let caption = summary::first_line(content["caption"]["text"].as_str().unwrap_or_default());
    let pictured = matches!(
        content["@type"].as_str().unwrap_or_default(),
        "messagePhoto" | "messageVideo" | "messageAnimation"
    );
    match pictured && !caption.is_empty() {
        true => caption,
        false => summary::line(store, message, words),
    }
}

/// The file of a message's small picture: a photo's smallest size at least [`PICTURE_SIDE`]
/// long, a video's or a GIF's thumbnail.
fn picture_file(message: &Value) -> Option<&Value> {
    let content = &message["content"];
    match content["@type"].as_str()? {
        "messagePhoto" => {
            let longest = |size: &Value| {
                let side = |key: &str| size[key].as_i64().unwrap_or_default();
                side("width").max(side("height"))
            };
            let sizes = content["photo"]["sizes"].as_array()?;
            let size = sizes
                .iter()
                .filter(|size| longest(size) >= PICTURE_SIDE)
                .min_by_key(|size| longest(size))
                .or_else(|| sizes.iter().max_by_key(|size| longest(size)))?;
            Some(&size["photo"])
        }
        "messageVideo" => Some(&content["video"]["thumbnail"]["file"]),
        "messageAnimation" => Some(&content["animation"]["thumbnail"]["file"]),
        _ => None,
    }
    .filter(|file| file["id"].is_i64())
}

/// The bar's picture: the file once downloaded, else the message's minithumbnail.
fn picture(store: &Store, message: &Value) -> Option<ImageSpec> {
    let content = &message["content"];
    let mini = match content["@type"].as_str()? {
        "messagePhoto" => &content["photo"]["minithumbnail"],
        "messageVideo" => &content["video"]["minithumbnail"],
        "messageAnimation" => &content["animation"]["minithumbnail"],
        _ => return None,
    };
    let path = picture_file(message).and_then(|file| {
        match file["id"].as_i64().and_then(|id| store.files.get(&id)) {
            Some(known) => known.path.clone(),
            None => downloaded(file),
        }
    });
    if let Some(path) = path {
        return Some(conv::picture(&path, 0, 0));
    }
    let bytes = mini["data"].as_str().and_then(|data| {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.decode(data).ok()
    });
    let mut spec = conv::pending_picture(0, 0);
    if let Some(bytes) = bytes {
        spec = spec.preview(bytes);
    }
    Some(spec)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::English;
    use serde_json::json;

    fn text(id: i64, line: &str) -> Value {
        json!({"id": id, "content": {"@type": "messageText", "text": {"text": line}}})
    }

    #[test]
    fn the_bar_goes_from_the_newest_back_and_wraps() {
        let store = Store::default();
        let mut pins = Pins::default();
        pins.set(vec![text(1, "first"), text(3, "third"), text(2, "second")]);
        let bar = pins.bar(&store, &English, true, None).unwrap();
        assert_eq!((bar.message.as_str(), bar.text.as_str()), ("3", "third"));
        assert_eq!(
            (bar.title.as_str(), bar.position, bar.count),
            ("Pinned message", 0, 3)
        );
        assert_eq!(bar.actions[0].id, UNPIN);
        pins.clicked(3);
        let bar = pins.bar(&store, &English, false, None).unwrap();
        assert_eq!(
            (bar.message.as_str(), bar.title.as_str()),
            ("2", "Pinned message #2")
        );
        assert_eq!(bar.actions[0].id, HIDE);
        pins.clicked(2);
        pins.clicked(1);
        assert_eq!(
            pins.shown_id(),
            Some(3),
            "after the oldest — the newest again"
        );
    }

    #[test]
    fn pinning_and_unpinning_keep_the_bar_in_place() {
        let store = Store::default();
        let mut pins = Pins::default();
        pins.set(vec![text(5, "five"), text(2, "two")]);
        pins.clicked(5);
        assert_eq!(pins.shown_id(), Some(2));
        // A new pin shows itself; unpinning the shown one goes to the next.
        pins.pinned(text(9, "nine"));
        assert_eq!((pins.shown_id(), pins.newest()), (Some(9), Some(9)));
        assert!(pins.unpinned(&[9]));
        assert_eq!(pins.shown_id(), Some(5));
        assert!(!pins.unpinned(&[42]));
        // Hidden until a newer one is pinned.
        assert!(pins.bar(&store, &English, false, Some(5)).is_none());
        pins.pinned(text(10, "ten"));
        assert!(pins.bar(&store, &English, false, Some(5)).is_some());
        assert!(pins.unpinned(&[10, 5, 2]));
        assert!(pins.bar(&store, &English, true, None).is_none());
    }
}
