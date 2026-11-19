//! The messages between the service and the windows' instances (JSON over `service.send`): a
//! window asks for the state and passes the settings block's buttons; the service tells every
//! window the bot's connection.

use serde::{Deserialize, Serialize};

/// A window's instance to the service.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum ToService {
    /// Send me the state.
    Hello,
    /// A new binding link (the old code ends).
    NewCode,
    /// Forget the owner: the bot waits for a new binding.
    Unbind,
    /// Ask Telegram again: the topics turned on, the other program gone.
    CheckAgain,
}

/// The service to the windows.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum ToWindow {
    Connection(Connection),
}

/// The bot's state, as the settings show it.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub enum Connection {
    /// No token in the settings.
    #[default]
    NoToken,
    /// `getMe` is on its way.
    Checking,
    /// The token doesn't work (401).
    BadToken,
    /// The token works, but the bot's topics are off in @BotFather.
    NoTopics { bot: String },
    /// Waiting for the owner: the link with the one-time code (`https://t.me/<bot>?start=<code>`).
    Unbound { bot: String, link: String },
    /// Bound and polling.
    Ready { bot: String, owner: String },
    /// Another program polls this bot (409).
    Conflict { bot: String },
    /// No connection to Telegram; it retries by itself.
    Offline { bot: String, error: String },
}
