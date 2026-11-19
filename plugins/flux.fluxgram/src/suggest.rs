//! The field's suggestions (part 5, no host: tested natively): the members a "@" mentions, the
//! bots' commands a "/" sends, and the mentions of people without a username in the sent text.
//!
//! A member goes into the field as "@username ", or — without a username — as a Markdown link
//! "[First Last](tg://user?id=N) ": [`mention_names`] makes such links of the parsed text the
//! mentions TDLib sends (`textEntityTypeMentionName`), in a new message and in an edit. A chat's
//! commands ([`commands_of`]) are its bot's (a chat with a bot: `userFullInfo.bot_info`) or its
//! bots' (a group: `bot_commands` of its full info); with several bots a command names its bot
//! ("/start@quiz_bot"). The "/" menu of the bots' part keeps to [`commands_of`] too.

use flux_plugin_api::conversation::{AvatarSpec, Suggestion};
use serde_json::{Value, json};

use crate::store::User;

/// The link of a mention of someone without a username, as the field writes it.
pub const USER_LINK: &str = "tg://user?id=";

/// A bot's command.
#[derive(Debug, Clone, PartialEq)]
pub struct Command {
    /// Without "/": "start".
    pub name: String,
    pub description: String,
    /// The bot's username, when the chat has several bots: the command names it.
    pub bot: Option<String>,
}

impl Command {
    /// "/start", "/start@quiz_bot".
    pub fn text(&self) -> String {
        match &self.bot {
            Some(bot) => format!("/{}@{bot}", self.name),
            None => format!("/{}", self.name),
        }
    }
}

/// A member to mention: "@anna ", or a link to the person when they have no username.
pub fn member(user: &User, avatar: AvatarSpec) -> Suggestion {
    let name = user.name();
    let (detail, insert) = match &user.username {
        Some(username) => (Some(format!("@{username}")), format!("@{username} ")),
        None => (
            None,
            format!("[{}]({USER_LINK}{}) ", link_text(&name), user.id),
        ),
    };
    let item = Suggestion::new(&user.id.to_string(), &name, &insert).avatar(avatar);
    match detail {
        Some(detail) => item.detail(&detail),
        None => item,
    }
}

/// A name as a link's text: the brackets that would end it early become round ones.
fn link_text(name: &str) -> String {
    name.replace('[', "(").replace(']', ")")
}

/// A chat's commands from TDLib's full info: a bot's (`userFullInfo`: its `bot_info.commands`)
/// or a group's (`supergroupFullInfo`, `basicGroupFullInfo`: `bot_commands`, per bot — `bot` names
/// a bot by its user id, its username). With several bots each command names its bot.
pub fn commands_of(full: &Value, bot: impl Fn(i64) -> Option<String>) -> Vec<Command> {
    let list = |commands: &Value, bot: Option<String>| -> Vec<Command> {
        commands
            .as_array()
            .map(|commands| {
                commands
                    .iter()
                    .filter_map(|command| {
                        Some(Command {
                            name: command["command"].as_str()?.to_string(),
                            description: command["description"]
                                .as_str()
                                .unwrap_or_default()
                                .to_string(),
                            bot: bot.clone(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default()
    };
    if full["bot_info"].is_object() {
        return list(&full["bot_info"]["commands"], None);
    }
    let bots = full["bot_commands"].as_array().cloned().unwrap_or_default();
    let several = bots.len() > 1;
    bots.iter()
        .flat_map(|commands| {
            let name = several
                .then(|| commands["bot_user_id"].as_i64().and_then(&bot))
                .flatten();
            list(&commands["commands"], name)
        })
        .collect()
}

/// The commands that start with `query` (without "/", any case), as the field's items.
pub fn command_items(commands: &[Command], query: &str) -> Vec<Suggestion> {
    let query = query.to_lowercase();
    commands
        .iter()
        .filter(|command| command.text()[1..].to_lowercase().starts_with(&query))
        .map(|command| {
            let text = command.text();
            let item = Suggestion::new(&text, &text, &format!("{text} "));
            if command.description.is_empty() {
                item
            } else {
                item.detail(&command.description)
            }
        })
        .collect()
}

/// The links to people the field wrote ("[Anna](tg://user?id=42)", which TDLib's Markdown may
/// leave as links) as mentions of them: `textEntityTypeMentionName`.
pub fn mention_names(formatted: &mut Value) {
    let Some(entities) = formatted["entities"].as_array_mut() else {
        return;
    };
    for entity in entities {
        let user = (entity["type"]["@type"] == "textEntityTypeTextUrl")
            .then(|| {
                entity["type"]["url"]
                    .as_str()?
                    .strip_prefix(USER_LINK)?
                    .parse::<i64>()
                    .ok()
            })
            .flatten();
        if let Some(user) = user {
            entity["type"] = json!({"@type": "textEntityTypeMentionName", "user_id": user});
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(id: i64, first: &str, last: &str, username: Option<&str>) -> User {
        User {
            id,
            first_name: first.into(),
            last_name: last.into(),
            username: username.map(str::to_string),
            ..User::default()
        }
    }

    fn avatar() -> AvatarSpec {
        AvatarSpec {
            image: None,
            initials: "A".into(),
            color: 1,
        }
    }

    #[test]
    fn a_member_is_mentioned_by_username_or_by_a_link() {
        let anna = member(&user(42, "Anna", "Smirnova", Some("anna")), avatar());
        assert_eq!(anna.title, "Anna Smirnova");
        assert_eq!(anna.detail.as_deref(), Some("@anna"));
        assert_eq!(anna.insert, "@anna ");
        let olga = member(&user(7, "Ольга", "[Петрова]", None), avatar());
        assert_eq!(olga.detail, None);
        assert_eq!(olga.insert, "[Ольга (Петрова)](tg://user?id=7) ");
        assert_eq!(olga.key, "7");
    }

    #[test]
    fn commands_come_from_a_bot_or_a_group_s_bots() {
        let bot = json!({"@type": "userFullInfo", "bot_info": {"commands": [
            {"command": "start", "description": "Start the bot"},
            {"command": "help", "description": ""}]}});
        let commands = commands_of(&bot, |_| None);
        assert_eq!(commands.len(), 2);
        assert_eq!(commands[0].text(), "/start");
        let items = command_items(&commands, "");
        assert_eq!(items[0].insert, "/start ");
        assert_eq!(items[0].detail.as_deref(), Some("Start the bot"));
        assert_eq!(items[1].detail, None);
        assert_eq!(command_items(&commands, "HE").len(), 1);

        let one = json!({"bot_commands": [{"bot_user_id": 5, "commands": [
            {"command": "quiz", "description": "A quiz"}]}]});
        assert_eq!(
            commands_of(&one, |_| Some("quiz_bot".into()))[0].text(),
            "/quiz"
        );
        let two = json!({"bot_commands": [
            {"bot_user_id": 5, "commands": [{"command": "quiz", "description": "A quiz"}]},
            {"bot_user_id": 6, "commands": [{"command": "roll", "description": "Dice"}]}]});
        let names = |id: i64| Some(if id == 5 { "quiz_bot" } else { "dice_bot" }.to_string());
        let commands = commands_of(&two, names);
        assert_eq!(commands[1].text(), "/roll@dice_bot");
        // "/ro" and "/roll@d" both find it.
        assert_eq!(command_items(&commands, "ro")[0].insert, "/roll@dice_bot ");
        assert_eq!(command_items(&commands, "roll@d").len(), 1);
        assert!(commands_of(&json!({"bot_commands": []}), |_| None).is_empty());
    }

    #[test]
    fn links_to_people_become_mentions() {
        let mut text = json!({"@type": "formattedText", "text": "Hi Anna and docs", "entities": [
            {"@type": "textEntity", "offset": 3, "length": 4,
                "type": {"@type": "textEntityTypeTextUrl", "url": "tg://user?id=42"}},
            {"@type": "textEntity", "offset": 12, "length": 4,
                "type": {"@type": "textEntityTypeTextUrl", "url": "https://docs.rs"}},
            {"@type": "textEntity", "offset": 0, "length": 2,
                "type": {"@type": "textEntityTypeBold"}}]});
        mention_names(&mut text);
        assert_eq!(
            text["entities"][0]["type"],
            json!({"@type": "textEntityTypeMentionName", "user_id": 42})
        );
        assert_eq!(text["entities"][1]["type"]["url"], "https://docs.rs");
        assert_eq!(text["entities"][2]["type"]["@type"], "textEntityTypeBold");
    }
}
