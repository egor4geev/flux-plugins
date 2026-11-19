//! A message in one line (part 3): the list's last line, a notification's text, a quote's text —
//! "Photo", "Voice message", the text's first line, a caption — and who sent it. Needs no host:
//! the words come through [`Words`], the names from the [`Store`].

use serde_json::Value;

use crate::model::Words;
use crate::store::Store;

/// Who sent a message: a user, or a chat (a channel's post forwarded into a group, an anonymous
/// admin).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sender {
    /// The user's id, or the chat's.
    pub id: i64,
    pub user: bool,
    /// The user's full name ("Deleted Account" for a deleted one), or the chat's title.
    pub name: String,
}

impl Sender {
    /// The first name: "Petya" of "Petya Smirnov" — the list's "Petya: …".
    pub fn first_name(&self) -> &str {
        if self.user {
            self.name.split_whitespace().next().unwrap_or(&self.name)
        } else {
            &self.name
        }
    }
}

/// Who sent the message (`sender_id`).
pub fn sender(store: &Store, message: &Value, words: &dyn Words) -> Sender {
    let sender = &message["sender_id"];
    match sender["@type"].as_str().unwrap_or_default() {
        "messageSenderChat" => {
            let id = sender["chat_id"].as_i64().unwrap_or_default();
            Sender {
                id,
                user: false,
                name: store
                    .chats
                    .get(&id)
                    .map(|chat| chat.title.clone())
                    .unwrap_or_default(),
            }
        }
        _ => {
            let id = sender["user_id"].as_i64().unwrap_or_default();
            let name = store.user_name(id);
            Sender {
                id,
                user: true,
                name: if name.is_empty() {
                    words.tr("Deleted Account")
                } else {
                    name
                },
            }
        }
    }
}

/// The first line of a text, its spaces collapsed.
pub fn first_line(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// The message's content in one line, without its sender.
pub fn line(store: &Store, message: &Value, words: &dyn Words) -> String {
    let content = &message["content"];
    let caption = || first_line(content["caption"]["text"].as_str().unwrap_or_default());
    // A kind's word, or the caption after it: "Photo, the view from the office".
    let with_caption = |word: &str| {
        let caption = caption();
        if caption.is_empty() {
            words.tr(word)
        } else {
            format!("{}, {caption}", words.tr(word))
        }
    };
    match content["@type"].as_str().unwrap_or_default() {
        "messageText" => first_line(content["text"]["text"].as_str().unwrap_or_default()),
        "messagePhoto" => with_caption("Photo"),
        "messageVideo" => with_caption("Video"),
        "messageAnimation" => with_caption("GIF"),
        "messageVoiceNote" => with_caption("Voice message"),
        "messageVideoNote" => words.tr("Video message"),
        "messageAudio" => {
            let audio = &content["audio"];
            let title = audio["title"].as_str().unwrap_or_default();
            let performer = audio["performer"].as_str().unwrap_or_default();
            match (title.is_empty(), performer.is_empty()) {
                (false, false) => format!("{performer} – {title}"),
                (false, true) => title.to_string(),
                _ => audio["file_name"]
                    .as_str()
                    .filter(|name| !name.is_empty())
                    .map_or_else(|| words.tr("Audio"), str::to_string),
            }
        }
        "messageDocument" => {
            let caption = caption();
            if caption.is_empty() {
                content["document"]["file_name"]
                    .as_str()
                    .filter(|name| !name.is_empty())
                    .map_or_else(|| words.tr("File"), str::to_string)
            } else {
                caption
            }
        }
        "messageSticker" => {
            let emoji = content["sticker"]["emoji"].as_str().unwrap_or_default();
            format!("{emoji} {}", words.tr("Sticker"))
                .trim()
                .to_string()
        }
        "messageAnimatedEmoji" => content["emoji"].as_str().unwrap_or_default().to_string(),
        "messageDice" => content["emoji"].as_str().unwrap_or("🎲").to_string(),
        "messageLocation" => words.tr("Location"),
        "messageVenue" => content["venue"]["title"]
            .as_str()
            .filter(|title| !title.is_empty())
            .map_or_else(|| words.tr("Location"), str::to_string),
        "messageContact" => words.tr("Contact"),
        "messagePoll" => {
            let question = content["poll"]["question"]["text"]
                .as_str()
                .or_else(|| content["poll"]["question"].as_str())
                .unwrap_or_default();
            // "📊 Poll: Lunch?" (part 5), a quiz's "📊 Quiz: …".
            let kind = if content["poll"]["type"]["@type"] == "pollTypeQuiz" {
                "Quiz: {0}"
            } else {
                "Poll: {0}"
            };
            format!("📊 {}", words.trf(kind, &[&first_line(question)]))
        }
        "messageGame" => words.tr("Game"),
        "messageInvoice" => words.tr("Invoice"),
        "messageCall" => words.tr("Call"),
        "messageStory" => words.tr("Story"),
        "messagePaidMedia" => words.tr("Paid media"),
        "messageGiveaway" | "messageGiveawayWinners" => words.tr("Giveaway"),
        "messageChatAddMembers" => {
            let joined = content["member_user_ids"]
                .as_array()
                .and_then(|ids| ids.first())
                .and_then(Value::as_i64)
                .map(|id| store.user_name(id))
                .unwrap_or_default();
            words.trf("{0} joined the group", &[&joined])
        }
        "messageChatJoinByLink" | "messageChatJoinByRequest" => {
            let who = sender(store, message, words);
            words.trf("{0} joined the group", &[&who.name])
        }
        "messageChatDeleteMember" => {
            let left = store.user_name(content["user_id"].as_i64().unwrap_or_default());
            words.trf("{0} left the group", &[&left])
        }
        "messageChatChangeTitle" => words.trf(
            "The group was renamed to «{0}»",
            &[content["title"].as_str().unwrap_or_default()],
        ),
        "messageChatChangePhoto" => words.tr("The group's photo was changed"),
        "messagePinMessage" => words.tr("A message was pinned"),
        "messageBasicGroupChatCreate" | "messageSupergroupChatCreate" => words.trf(
            "The group «{0}» was created",
            &[content["title"].as_str().unwrap_or_default()],
        ),
        "messageContactRegistered" => {
            let who = sender(store, message, words);
            words.trf("{0} joined Telegram", &[&who.name])
        }
        "messageScreenshotTaken" => words.tr("A screenshot was taken"),
        _ => words.tr("Message"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::English;
    use serde_json::json;

    fn store() -> Store {
        let mut store = Store::default();
        store.apply(&json!({"@type": "user", "id": 103, "first_name": "Petya",
            "last_name": "Smirnov"}));
        store
    }

    fn message(content: Value) -> Value {
        json!({"@type": "message", "id": 1, "chat_id": -5,
            "sender_id": {"@type": "messageSenderUser", "user_id": 103}, "content": content})
    }

    #[test]
    fn says_what_a_message_is() {
        let store = store();
        let line = |content| line(&store, &message(content), &English);
        assert_eq!(
            line(json!({"@type": "messageText",
                "text": {"text": "\n  Release on   Friday?\nOr Thursday?"}})),
            "Release on Friday?"
        );
        assert_eq!(
            line(json!({"@type": "messagePhoto", "caption": {"text": ""}})),
            "Photo"
        );
        assert_eq!(
            line(json!({"@type": "messagePhoto", "caption": {"text": "The office"}})),
            "Photo, The office"
        );
        assert_eq!(
            line(json!({"@type": "messageDocument", "caption": {"text": ""},
                "document": {"file_name": "staging.env.example"}})),
            "staging.env.example"
        );
        assert_eq!(
            line(json!({"@type": "messageSticker", "sticker": {"emoji": "🚀"}})),
            "🚀 Sticker"
        );
        assert_eq!(
            line(json!({"@type": "messageChatAddMembers", "member_user_ids": [103]})),
            "Petya Smirnov joined the group"
        );
        assert_eq!(line(json!({"@type": "messageSomethingNew"})), "Message");
    }

    #[test]
    fn names_the_sender() {
        let store = store();
        let petya = sender(&store, &message(json!({})), &English);
        assert_eq!(petya.name, "Petya Smirnov");
        assert_eq!(petya.first_name(), "Petya");
        assert!(petya.user);
        let ghost = sender(
            &store,
            &json!({"sender_id": {"@type": "messageSenderUser", "user_id": 9}}),
            &English,
        );
        assert_eq!(ghost.name, "Deleted Account");
    }
}
