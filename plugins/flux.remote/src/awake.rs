//! Keeping the computer awake while the bot is on (bound and connected, or reconnecting): while a
//! remote session works or waits for an answer — always (a session with a topic, or, while the
//! computer is locked, any session: its questions come to the phone then); otherwise, with the
//! setting «Don't sleep on the power adapter while the bot is on» — on the adapter; otherwise as
//! the system decides. Flux is asked only when the wish changes. A closed lid puts a laptop to
//! sleep anyway (Flux's own rule).

use flux_plugin_api::remote::{Awake, Session, SessionState};
use serde_json::Value;

use crate::bot::Bot;
use crate::host::Host;
use crate::model::Connection;

/// The setting's key.
pub const ON_POWER: &str = "awake-on-power";

#[derive(Default)]
pub struct State {
    /// What Flux was asked for last.
    asked: Option<Awake>,
}

pub fn update(bot: &mut Bot, host: &mut dyn Host) {
    let wanted = wanted(bot, host);
    if bot.awake.asked != Some(wanted) && host.set_awake(wanted).is_ok() {
        bot.awake.asked = Some(wanted);
    }
}

fn wanted(bot: &Bot, host: &dyn Host) -> Awake {
    let on = bot.chat().is_some()
        && matches!(
            bot.connection,
            Connection::Ready { .. } | Connection::Offline { .. } | Connection::Checking
        );
    if !on {
        return Awake::Off;
    }
    let remote = |session: &Session| {
        bot.system.screen_locked || bot.store.topic_by_session(&session.id).is_some()
    };
    if bot
        .sessions
        .values()
        .any(|session| busy(session) && remote(session))
    {
        Awake::Always
    } else if host.setting(ON_POWER) == Some(Value::Bool(true)) {
        Awake::OnPower
    } else {
        Awake::Off
    }
}

/// The session works, or waits for the user.
fn busy(session: &Session) -> bool {
    matches!(
        session.state,
        SessionState::Starting | SessionState::Working | SessionState::Waiting
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bot::ClaudeEvent;
    use crate::store::{Owner, Topic};
    use crate::testing::{FakeHost, session};
    use flux_plugin_api::remote::SystemEvent;

    fn asked(host: &FakeHost) -> Vec<String> {
        host.claude
            .iter()
            .filter(|line| line.starts_with("awake"))
            .cloned()
            .collect()
    }

    fn bound(host: &FakeHost) -> Bot {
        let mut bot = Bot::new(host);
        bot.store.owner = Some(Owner {
            id: 5,
            name: "Egor".into(),
        });
        bot.connection = Connection::Ready {
            bot: "@flux_bot".into(),
            owner: "Egor".into(),
        };
        bot
    }

    #[test]
    fn an_unbound_bot_lets_the_computer_sleep() {
        let mut host = FakeHost::default();
        host.settings.insert(ON_POWER.into(), Value::Bool(true));
        let mut bot = Bot::new(&host);
        bot.sessions
            .insert("s1".into(), session("s1", "/p", SessionState::Working));
        update(&mut bot, &mut host);
        update(&mut bot, &mut host);
        assert_eq!(asked(&host), ["awake Awake::Off"], "asked once");
    }

    #[test]
    fn the_setting_keeps_it_awake_on_power_and_a_remote_session_always() {
        let mut host = FakeHost::default();
        let mut bot = bound(&host);
        update(&mut bot, &mut host);
        host.settings.insert(ON_POWER.into(), Value::Bool(true));
        bot.settings_changed(&mut host);
        bot.store.add_topic(Topic {
            thread: 700,
            session: "s1".into(),
            ..Default::default()
        });
        bot.on_claude(
            &mut host,
            ClaudeEvent::Session(session("s1", "/p", SessionState::Working)),
        );
        bot.on_claude(
            &mut host,
            ClaudeEvent::Session(session("s1", "/p", SessionState::Waiting)),
        );
        bot.on_claude(
            &mut host,
            ClaudeEvent::Session(session("s1", "/p", SessionState::Idle)),
        );
        // A session without a topic counts only while the computer is locked.
        bot.on_claude(
            &mut host,
            ClaudeEvent::Session(session("s2", "/p", SessionState::Working)),
        );
        bot.on_claude(&mut host, ClaudeEvent::System(SystemEvent::ScreenLocked));
        bot.on_claude(&mut host, ClaudeEvent::System(SystemEvent::ScreenUnlocked));
        bot.on_claude(&mut host, ClaudeEvent::Gone("s2".into()));
        assert_eq!(
            asked(&host),
            [
                "awake Awake::Off",
                "awake Awake::OnPower",
                "awake Awake::Always",
                "awake Awake::OnPower",
                "awake Awake::Always",
                "awake Awake::OnPower"
            ]
        );
    }

    #[test]
    fn a_bot_that_stopped_lets_it_sleep() {
        let mut host = FakeHost::default();
        let mut bot = bound(&host);
        bot.store.add_topic(Topic {
            thread: 700,
            session: "s1".into(),
            ..Default::default()
        });
        bot.sessions
            .insert("s1".into(), session("s1", "/p", SessionState::Working));
        update(&mut bot, &mut host);
        bot.connection = Connection::BadToken;
        update(&mut bot, &mut host);
        bot.connection = Connection::Offline {
            bot: "@flux_bot".into(),
            error: "offline".into(),
        };
        update(&mut bot, &mut host);
        assert_eq!(
            asked(&host),
            [
                "awake Awake::Always",
                "awake Awake::Off",
                "awake Awake::Always"
            ]
        );
    }
}
