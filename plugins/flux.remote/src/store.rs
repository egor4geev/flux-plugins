//! What the service keeps between runs — the plugin's storage, one JSON value under [`KEY`]: the
//! bot it belongs to, the owner, «Общее», the topic of each session, where the polling goes on
//! from. A token of another bot starts a new one ([`Store::for_bot`]).

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::host::Host;

pub const KEY: &str = "state";

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Store {
    /// The bot (its Telegram id) this state belongs to.
    #[serde(default)]
    pub bot: Option<i64>,
    /// The only person the bot answers; the private chat with the bot has the same id.
    #[serde(default)]
    pub owner: Option<Owner>,
    /// The binding's one-time code while it waits, and when it ends (ms since 1970).
    #[serde(default)]
    pub code: Option<(String, u64)>,
    /// «Общее»: its topic.
    #[serde(default)]
    pub general: Option<i64>,
    /// The status message in «Общее».
    #[serde(default)]
    pub status_message: Option<i64>,
    /// A topic per session.
    #[serde(default)]
    pub topics: Vec<Topic>,
    /// Where the polling goes on from.
    #[serde(default)]
    pub offset: Option<i64>,
    /// The last draft id given (they only grow: Telegram tells a draft by it).
    #[serde(default)]
    pub last_draft: i64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Owner {
    pub id: i64,
    /// "Egor Ageev (@egor)".
    pub name: String,
}

/// A session's topic.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Topic {
    pub thread: i64,
    /// The CLI's session id.
    pub session: String,
    /// The project's folder.
    pub project: String,
    /// The topic's name as the bot set it last.
    pub title: String,
}

impl Store {
    pub fn load(host: &dyn Host) -> Self {
        host.load(KEY)
            .and_then(|value| serde_json::from_value(value).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, host: &mut dyn Host) {
        let value = serde_json::to_value(self).unwrap_or(Value::Null);
        host.save(KEY, Some(&value));
    }

    /// The state for this bot: the same one, or a new one for another bot.
    pub fn for_bot(&mut self, bot: i64) {
        if self.bot != Some(bot) {
            *self = Store {
                bot: Some(bot),
                last_draft: self.last_draft,
                ..Default::default()
            };
        }
    }

    /// The owner's private chat; none — not bound yet.
    pub fn chat(&self) -> Option<i64> {
        self.owner.as_ref().map(|owner| owner.id)
    }

    pub fn topic_by_thread(&self, thread: i64) -> Option<&Topic> {
        self.topics.iter().find(|topic| topic.thread == thread)
    }

    pub fn topic_by_session(&self, session: &str) -> Option<&Topic> {
        self.topics.iter().find(|topic| topic.session == session)
    }

    /// A session's topic: replaces what the thread or the session had.
    pub fn add_topic(&mut self, topic: Topic) {
        self.topics
            .retain(|old| old.thread != topic.thread && old.session != topic.session);
        self.topics.push(topic);
    }

    pub fn forget_topic(&mut self, thread: i64) {
        self.topics.retain(|topic| topic.thread != thread);
    }

    /// A new draft's id: never 0, never one given before.
    pub fn next_draft(&mut self, now_ms: u64) -> i64 {
        let floor = (now_ms / 1000 % 1_000_000_000) as i64;
        self.last_draft = (self.last_draft + 1).max(floor).max(1);
        self.last_draft
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn topics_are_one_per_session_and_thread() {
        let mut store = Store::default();
        let topic = |thread, session: &str| Topic {
            thread,
            session: session.into(),
            ..Default::default()
        };
        store.add_topic(topic(1, "a"));
        store.add_topic(topic(2, "b"));
        store.add_topic(topic(3, "a"));
        assert_eq!(store.topic_by_session("a").unwrap().thread, 3);
        assert!(store.topic_by_thread(1).is_none());
        store.forget_topic(2);
        assert!(store.topic_by_session("b").is_none());
    }

    #[test]
    fn another_bot_starts_afresh_and_drafts_only_grow() {
        let mut store = Store {
            bot: Some(1),
            general: Some(5),
            ..Default::default()
        };
        let first = store.next_draft(5_000);
        assert!(store.next_draft(1_000) > first);
        store.for_bot(1);
        assert_eq!(store.general, Some(5));
        store.for_bot(2);
        assert_eq!(store.general, None);
        assert!(store.next_draft(0) > first);
    }
}
