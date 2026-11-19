//! The data of the bot's buttons: `<kind>:<key>:<choice>`, at most 64 bytes (Telegram's limit).
//! The kind is the module that answers the press; the key is a short number the module gave when
//! it sent the buttons and remembers the meaning of (a session's request ids are too long to fit);
//! the choice is which button.

/// Who answers a press.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// [`crate::requests`]: a permission, Claude's questions, the plan.
    Request,
    /// [`crate::newchat`]: the project of a new chat.
    NewChat,
    /// [`crate::general`]: «Общее».
    General,
    /// [`crate::handoff`]: «Продолжить здесь».
    Handoff,
    /// [`crate::commands`]: the buttons of `/sessions`, `/stop`, `/mode`.
    Command,
    /// [`crate::menu`]: the screens of `/settings`.
    Menu,
    /// [`crate::late`]: «Выполнить» / «Не выполнять» under messages that came while Flux was closed.
    Late,
    /// [`crate::turn`]: «Дифф» and «Откатить» under a turn's «Готово» (the key is the turn).
    Turn,
    /// [`crate::task`]: «Из задачи» in a new chat.
    Task,
    /// A button that only shows something (an answered request's result).
    Noop,
}

impl Kind {
    fn letter(self) -> char {
        match self {
            Kind::Request => 'r',
            Kind::NewChat => 'n',
            Kind::General => 'g',
            Kind::Handoff => 'h',
            Kind::Command => 'c',
            Kind::Menu => 'm',
            Kind::Late => 'l',
            Kind::Turn => 'd',
            Kind::Task => 't',
            Kind::Noop => 'x',
        }
    }

    fn of(letter: &str) -> Option<Kind> {
        Some(match letter {
            "r" => Kind::Request,
            "n" => Kind::NewChat,
            "g" => Kind::General,
            "h" => Kind::Handoff,
            "c" => Kind::Command,
            "m" => Kind::Menu,
            "l" => Kind::Late,
            "d" => Kind::Turn,
            "t" => Kind::Task,
            "x" => Kind::Noop,
            _ => return None,
        })
    }
}

/// The most Telegram takes.
pub const MAX_BYTES: usize = 64;

/// A button's data; a choice too long is cut (on a character's border).
pub fn encode(kind: Kind, key: u64, choice: &str) -> String {
    let mut data = format!("{}:{key}:", kind.letter());
    for c in choice.chars() {
        if data.len() + c.len_utf8() > MAX_BYTES {
            break;
        }
        data.push(c);
    }
    data
}

pub fn decode(data: &str) -> Option<(Kind, u64, String)> {
    let mut parts = data.splitn(3, ':');
    let kind = Kind::of(parts.next()?)?;
    let key = parts.next()?.parse().ok()?;
    Some((kind, key, parts.next().unwrap_or_default().to_string()))
}

/// The data of a button that only shows.
pub fn noop() -> String {
    encode(Kind::Noop, 0, "")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_goes_both_ways_and_fits() {
        let data = encode(Kind::Request, 42, "allow-session");
        assert_eq!(
            decode(&data),
            Some((Kind::Request, 42, "allow-session".into()))
        );
        let long = encode(Kind::NewChat, 7, &"проект".repeat(20));
        assert!(long.len() <= MAX_BYTES);
        assert!(decode(&long).is_some());
        assert_eq!(decode("zz:1:"), None);
        assert_eq!(decode(&noop()), Some((Kind::Noop, 0, String::new())));
    }
}
