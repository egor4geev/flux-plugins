//! Bots (part 5, no host: tested natively): the buttons under their messages, their keyboards
//! over the field, their menu of commands.
//!
//! - A message's inline keyboard (`replyMarkupInlineKeyboard`) becomes its buttons
//!   ([`inline_keyboard`]); a button's id carries what a press does ([`Press`]), so the window
//!   acts on it without asking: a callback goes to the bot (`getCallbackQueryAnswer`, its answer
//!   a notification or, as an alert, a calm dialog; its link opened), a link opens (asked first
//!   unless it is Telegram's), a switch-inline query goes into the field as "@bot query", a text
//!   to copy to the clipboard, a person's button to their profile, a login button opens its page.
//!   Web apps, games, payments and callbacks behind a password are shown, and a press says calmly
//!   that FluxGram can't open them (a web app's page may open in the browser).
//! - The chat's keyboard over the field ([`reply_keyboard`]): TDLib names the message whose
//!   keyboard the chat shows (`reply_markup_message_id` — it applies Telegram's rules: in a group
//!   only the keyboards meant for the user); a press sends the button's text, then a one-time
//!   keyboard goes (`deleteChatReplyMarkup`). A bot asking for a reply (`replyMarkupForceReply`)
//!   puts its message's quote over the field, with its placeholder. Buttons that would share the
//!   user's contact, location, a poll, chats or open a web app say calmly they can't here.

use flux_plugin_api::conversation::{self as conv, KeyboardButton, KeyboardKind, ReplyKeyboard};
use serde_json::Value;

/// What a press of a bot's button does: carried in the button's id (`<row>.<column> <what>`).
#[derive(Debug, Clone, PartialEq)]
pub enum Press {
    /// Ask the bot (`getCallbackQueryAnswer`): the button's data, base64 as TDLib's JSON has it.
    Callback(String),
    /// Open a page (a URL button, a login button).
    Link(String),
    /// Put "@bot query" into the field (a switch-inline button for this chat).
    Field(String),
    /// Copy a text to the clipboard.
    Copy(String),
    /// Open a person's profile.
    User(i64),
    /// Send the button's text (a button of the keyboard over the field).
    Text(String),
    /// What FluxGram can't do here, and the page that may open in the browser.
    Unsupported(Unsupported, Option<String>),
}

/// The buttons FluxGram can't act on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unsupported {
    WebApp,
    Game,
    Payment,
    Password,
    Contact,
    Location,
    Poll,
    Chats,
}

impl Unsupported {
    fn tag(self) -> &'static str {
        match self {
            Unsupported::WebApp => "web-app",
            Unsupported::Game => "game",
            Unsupported::Payment => "payment",
            Unsupported::Password => "password",
            Unsupported::Contact => "contact",
            Unsupported::Location => "location",
            Unsupported::Poll => "poll",
            Unsupported::Chats => "chats",
        }
    }

    fn of(tag: &str) -> Option<Self> {
        Some(match tag {
            "web-app" => Unsupported::WebApp,
            "game" => Unsupported::Game,
            "payment" => Unsupported::Payment,
            "password" => Unsupported::Password,
            "contact" => Unsupported::Contact,
            "location" => Unsupported::Location,
            "poll" => Unsupported::Poll,
            "chats" => Unsupported::Chats,
            _ => return None,
        })
    }

    /// The calm sentence of the dialog a press opens (English: the window translates it).
    pub fn sentence(self) -> &'static str {
        match self {
            Unsupported::WebApp => "FluxGram can't open the bot's app here.",
            Unsupported::Game => "FluxGram can't open games yet.",
            Unsupported::Payment => "FluxGram can't make payments.",
            Unsupported::Password => "This button needs your password: use Telegram's app.",
            Unsupported::Contact => "FluxGram can't share your phone number with a bot.",
            Unsupported::Location => "FluxGram can't share your location.",
            Unsupported::Poll => "FluxGram can't create polls yet.",
            Unsupported::Chats => "FluxGram can't share chats with a bot yet.",
        }
    }
}

impl Press {
    /// The button's id: its place (a row's buttons may say the same) and what it does.
    fn id(&self, row: usize, column: usize) -> String {
        let what = match self {
            Press::Callback(data) => format!("callback {data}"),
            Press::Link(url) => format!("link {url}"),
            Press::Field(query) => format!("field {query}"),
            Press::Copy(text) => format!("copy {text}"),
            Press::User(user) => format!("user {user}"),
            Press::Text(text) => format!("text {text}"),
            Press::Unsupported(what, url) => match url {
                Some(url) => format!("no-{} {url}", what.tag()),
                None => format!("no-{}", what.tag()),
            },
        };
        format!("{row}.{column} {what}")
    }

    /// What a pressed button's id says.
    pub fn of(id: &str) -> Option<Press> {
        let (_, what) = id.split_once(' ')?;
        let (kind, rest) = what.split_once(' ').unwrap_or((what, ""));
        Some(match kind {
            "callback" => Press::Callback(rest.to_string()),
            "link" => Press::Link(rest.to_string()),
            "field" => Press::Field(rest.to_string()),
            "copy" => Press::Copy(rest.to_string()),
            "user" => Press::User(rest.parse().ok()?),
            "text" => Press::Text(rest.to_string()),
            _ => {
                let what = Unsupported::of(kind.strip_prefix("no-")?)?;
                let url = (!rest.is_empty()).then(|| rest.to_string());
                Press::Unsupported(what, url)
            }
        })
    }
}

/// A message's inline keyboard as its buttons, rows of them; `bot` — the bot's username, for its
/// switch-inline buttons ("@bot query"). Other markups — none.
pub fn inline_keyboard(markup: &Value, bot: Option<&str>) -> Vec<Vec<KeyboardButton>> {
    if markup["@type"] != "replyMarkupInlineKeyboard" {
        return Vec::new();
    }
    rows(markup, |button| {
        let kind = &button["type"];
        let text = |key: &str| kind[key].as_str().unwrap_or_default().to_string();
        let press = match kind["@type"].as_str().unwrap_or_default() {
            "inlineKeyboardButtonTypeCallback" => Press::Callback(data(&kind["data"])),
            "inlineKeyboardButtonTypeUrl" | "inlineKeyboardButtonTypeLoginUrl" => {
                Press::Link(text("url"))
            }
            "inlineKeyboardButtonTypeSwitchInline" => match bot {
                Some(bot) => Press::Field(format!("@{bot} {}", text("query"))),
                None => Press::Field(text("query")),
            },
            "inlineKeyboardButtonTypeCopyText" => Press::Copy(text("text")),
            "inlineKeyboardButtonTypeUser" => Press::User(kind["user_id"].as_i64()?),
            "inlineKeyboardButtonTypeWebApp" => {
                Press::Unsupported(Unsupported::WebApp, Some(text("url")))
            }
            "inlineKeyboardButtonTypeCallbackGame" => Press::Unsupported(Unsupported::Game, None),
            "inlineKeyboardButtonTypeBuy" => Press::Unsupported(Unsupported::Payment, None),
            "inlineKeyboardButtonTypeCallbackWithPassword" => {
                Press::Unsupported(Unsupported::Password, None)
            }
            _ => return None,
        };
        Some(press)
    })
}

/// The chat's keyboard over the field, of the message TDLib names (`reply_markup_message_id`):
/// a bot's keyboard (`replyMarkupShowKeyboard`: its rows, its placeholder, shown at once), or
/// its ask for a reply (`replyMarkupForceReply`: no rows, the placeholder). Other markups — none.
pub fn reply_keyboard(markup: &Value) -> Option<ReplyKeyboard> {
    let placeholder = markup["input_field_placeholder"]
        .as_str()
        .filter(|placeholder| !placeholder.is_empty());
    let keyboard = match markup["@type"].as_str().unwrap_or_default() {
        "replyMarkupShowKeyboard" => {
            let rows = rows(markup, |button| {
                let text = button["text"].as_str().unwrap_or_default().to_string();
                let press = match button["type"]["@type"].as_str().unwrap_or_default() {
                    "keyboardButtonTypeText" => Press::Text(text),
                    "keyboardButtonTypeRequestPhoneNumber" => {
                        Press::Unsupported(Unsupported::Contact, None)
                    }
                    "keyboardButtonTypeRequestLocation" => {
                        Press::Unsupported(Unsupported::Location, None)
                    }
                    "keyboardButtonTypeRequestPoll" => Press::Unsupported(Unsupported::Poll, None),
                    "keyboardButtonTypeRequestUsers" | "keyboardButtonTypeRequestChat" => {
                        Press::Unsupported(Unsupported::Chats, None)
                    }
                    "keyboardButtonTypeWebApp" => Press::Unsupported(
                        Unsupported::WebApp,
                        button["type"]["url"].as_str().map(str::to_string),
                    ),
                    // An unknown kind: its text, as Telegram's apps send it.
                    _ => Press::Text(text),
                };
                Some(press)
            });
            conv::reply_keyboard_spec(rows, true)
        }
        "replyMarkupForceReply" => conv::reply_keyboard_spec(Vec::new(), false),
        _ => return None,
    };
    Some(match placeholder {
        Some(placeholder) => keyboard.placeholder(placeholder),
        None => keyboard,
    })
}

/// The keyboard is one-time: it goes once used (`deleteChatReplyMarkup`), as does an ask for a
/// reply once answered.
pub fn used_once(markup: &Value) -> bool {
    markup["one_time"] == true || markup["@type"] == "replyMarkupForceReply"
}

/// A markup's rows of buttons, each made by `press` (none — the button is left out).
fn rows(markup: &Value, press: impl Fn(&Value) -> Option<Press>) -> Vec<Vec<KeyboardButton>> {
    markup["rows"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
        .map(|(row, buttons)| {
            buttons
                .as_array()
                .into_iter()
                .flatten()
                .enumerate()
                .filter_map(|(column, button)| {
                    let press = press(button)?;
                    let kind = match press {
                        Press::Link(_) => KeyboardKind::Link,
                        Press::Field(_) => KeyboardKind::Field,
                        _ => KeyboardKind::Action,
                    };
                    let text = button["text"].as_str().unwrap_or_default();
                    Some(conv::keyboard_button(&press.id(row, column), text, kind))
                })
                .collect::<Vec<_>>()
        })
        .filter(|row| !row.is_empty())
        .collect()
}

/// A callback's data: TDLib's JSON has bytes as base64 already.
fn data(value: &Value) -> String {
    value.as_str().unwrap_or_default().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn button(text: &str, kind: Value) -> Value {
        json!({"@type": "inlineKeyboardButton", "text": text, "type": kind})
    }

    #[test]
    fn an_inline_keyboards_buttons_carry_what_they_do() {
        let markup = json!({"@type": "replyMarkupInlineKeyboard", "rows": [
            [button("Yes", json!({"@type": "inlineKeyboardButtonTypeCallback", "data": "eWVz"})),
             button("Site", json!({"@type": "inlineKeyboardButtonTypeUrl",
                "url": "https://flux.dev"}))],
            [button("Search", json!({"@type": "inlineKeyboardButtonTypeSwitchInline",
                "query": "rain", "target_chat": {"@type": "targetChatCurrent"}})),
             button("Copy", json!({"@type": "inlineKeyboardButtonTypeCopyText",
                "text": "PROMO 42"}))],
            [button("Pay", json!({"@type": "inlineKeyboardButtonTypeBuy"})),
             button("App", json!({"@type": "inlineKeyboardButtonTypeWebApp",
                "url": "https://app.example"})),
             button("Odd", json!({"@type": "inlineKeyboardButtonTypeSomethingNew"}))],
        ]});
        let rows = inline_keyboard(&markup, Some("weather_bot"));
        assert_eq!(rows.len(), 3);
        let press = |row: usize, column: usize| Press::of(&rows[row][column].id).unwrap();
        assert_eq!(press(0, 0), Press::Callback("eWVz".into()));
        assert_eq!(rows[0][0].kind, KeyboardKind::Action);
        assert_eq!(press(0, 1), Press::Link("https://flux.dev".into()));
        assert_eq!(rows[0][1].kind, KeyboardKind::Link);
        assert_eq!(press(1, 0), Press::Field("@weather_bot rain".into()));
        assert_eq!(rows[1][0].kind, KeyboardKind::Field);
        assert_eq!(press(1, 1), Press::Copy("PROMO 42".into()));
        assert_eq!(press(2, 0), Press::Unsupported(Unsupported::Payment, None));
        assert_eq!(
            press(2, 1),
            Press::Unsupported(Unsupported::WebApp, Some("https://app.example".into()))
        );
        // An unknown kind is left out; the same text twice keeps two ids.
        assert_eq!(rows[2].len(), 2);
        assert_ne!(rows[0][0].id, rows[0][1].id);
        // Not an inline keyboard: no buttons.
        assert!(inline_keyboard(&json!({"@type": "replyMarkupShowKeyboard"}), None).is_empty());
        assert!(inline_keyboard(&Value::Null, None).is_empty());
    }

    #[test]
    fn a_bots_keyboard_sends_its_texts_and_says_what_it_cant() {
        let markup = json!({"@type": "replyMarkupShowKeyboard", "one_time": true,
            "input_field_placeholder": "Choose a city", "rows": [
            [{"@type": "keyboardButton", "text": "Moscow",
                "type": {"@type": "keyboardButtonTypeText"}},
             {"@type": "keyboardButton", "text": "Paris",
                "type": {"@type": "keyboardButtonTypeText"}}],
            [{"@type": "keyboardButton", "text": "My location",
                "type": {"@type": "keyboardButtonTypeRequestLocation"}}],
        ]});
        let keyboard = reply_keyboard(&markup).unwrap();
        assert!(keyboard.open);
        assert_eq!(keyboard.placeholder.as_deref(), Some("Choose a city"));
        assert_eq!(keyboard.rows.len(), 2);
        assert_eq!(
            Press::of(&keyboard.rows[0][1].id),
            Some(Press::Text("Paris".into()))
        );
        assert_eq!(
            Press::of(&keyboard.rows[1][0].id),
            Some(Press::Unsupported(Unsupported::Location, None))
        );
        assert!(used_once(&markup));
        // An ask for a reply: no rows, its placeholder, used once.
        let force = json!({"@type": "replyMarkupForceReply", "input_field_placeholder": "Name"});
        let keyboard = reply_keyboard(&force).unwrap();
        assert!(keyboard.rows.is_empty());
        assert_eq!(keyboard.placeholder.as_deref(), Some("Name"));
        assert!(used_once(&force));
        // A keyboard that stays; an inline one isn't over the field.
        let mut stays = markup.clone();
        stays["one_time"] = json!(false);
        assert!(!used_once(&stays));
        assert!(reply_keyboard(&json!({"@type": "replyMarkupInlineKeyboard"})).is_none());
    }

    #[test]
    fn a_texts_spaces_survive_its_id() {
        let press = Press::Text("Two words: yes".into());
        assert_eq!(Press::of(&press.id(3, 1)), Some(press));
        assert_eq!(Press::of("0.0 user 42"), Some(Press::User(42)));
        assert_eq!(
            Press::of("0.0 no-poll"),
            Some(Press::Unsupported(Unsupported::Poll, None))
        );
        assert_eq!(Press::of("open"), None);
    }
}
