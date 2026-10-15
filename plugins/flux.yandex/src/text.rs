//! Text helpers: a branch's name from a task (`TRACK-142-oshibka-vkhoda`: Russian in Latin
//! letters, lowercase, `-` between words, length-limited), the templates of the settings
//! (`{key}`, `{summary}`), durations as people write them ("1h 25m", "1ч 25м", "85m", "1.5h",
//! "1:25") and as Tracker does (ISO 8601 `PT1H25M`; a week is 5 days, a day is 8 hours), and the
//! quoting of Tracker's query language.

/// Minutes in Tracker's day and week: a working day is 8 hours, a week is 5 days.
const DAY: u32 = 8 * 60;
const WEEK: u32 = 5 * DAY;

/// A summary in a branch's name is cut at a word to this many characters.
const SUMMARY_IN_BRANCH: usize = 40;

/// Russian letters in Latin ones, as people write them in names (х — kh, ц — ts, щ — shch,
/// ю — yu, я — ya; ъ and ь are dropped).
fn latin(c: char) -> Option<&'static str> {
    Some(match c {
        'а' => "a",
        'б' => "b",
        'в' => "v",
        'г' => "g",
        'д' => "d",
        'е' | 'ё' | 'э' => "e",
        'ж' => "zh",
        'з' => "z",
        'и' => "i",
        'й' | 'ы' => "y",
        'к' => "k",
        'л' => "l",
        'м' => "m",
        'н' => "n",
        'о' => "o",
        'п' => "p",
        'р' => "r",
        'с' => "s",
        'т' => "t",
        'у' => "u",
        'ф' => "f",
        'х' => "kh",
        'ц' => "ts",
        'ч' => "ch",
        'ш' => "sh",
        'щ' => "shch",
        'ъ' | 'ь' => "",
        'ю' => "yu",
        'я' => "ya",
        _ => return None,
    })
}

/// Words of `text` in lowercase Latin letters and digits joined by `-`, cut at a word to `max`
/// characters (a single long word is cut as it is).
pub fn slug(text: &str, max: usize) -> String {
    let mut out = String::new();
    let mut gap = false;
    for c in text.chars().flat_map(char::to_lowercase) {
        let piece: Option<String> = if c.is_ascii_alphanumeric() {
            Some(c.to_string())
        } else {
            latin(c).map(str::to_string)
        };
        match piece {
            Some(piece) if !piece.is_empty() => {
                if gap && !out.is_empty() {
                    out.push('-');
                }
                gap = false;
                out.push_str(&piece);
            }
            // A soft or hard sign: inside a word, not a gap.
            Some(_) => {}
            None => gap = true,
        }
    }
    if out.len() <= max {
        return out;
    }
    // Only ASCII went in: bytes are characters.
    let cut = &out[..max];
    if out[max..].starts_with('-') {
        return cut.to_string();
    }
    match cut.rfind('-') {
        Some(at) if at > 0 => cut[..at].to_string(),
        _ => cut.to_string(),
    }
}

/// `template` with `{name}` replaced by the values; an unknown name stays as it is.
pub fn fill(template: &str, values: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let value = after.find('}').and_then(|close| {
            let name = &after[..close];
            values
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| (close, *value))
        });
        match value {
            Some((close, value)) => {
                out.push_str(value);
                rest = &after[close + 1..];
            }
            None => {
                out.push('{');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// A branch's name for a task by the `branch-template` setting: `{key}` — the key as it is,
/// `{summary}` — the title in Latin letters, `{type}` — the task's type; made a valid Git name.
pub fn branch_name(template: &str, key: &str, summary: &str, kind: &str) -> String {
    let summary = slug(summary, SUMMARY_IN_BRANCH);
    let kind = slug(kind, 20);
    let name = fill(
        template,
        &[("key", key), ("summary", &summary), ("type", &kind)],
    );
    git_name(&name)
}

/// `name` as a valid Git branch name: no spaces or the characters Git forbids, no `..`, no `-`
/// or `/` at the ends, no empty parts; a key alone when nothing is left.
pub fn git_name(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        let c = match c {
            ' ' | '\t' | '~' | '^' | ':' | '?' | '*' | '[' | '\\' | '@' | '{' | '}' => '-',
            c if c.is_control() => '-',
            c => c,
        };
        if (c == '-' && out.ends_with('-'))
            || (c == '.' && out.ends_with('.'))
            || (c == '/' && out.ends_with('/'))
        {
            continue;
        }
        out.push(c);
    }
    let mut out = out
        .split('/')
        .map(|part| part.trim_matches(|c| c == '-' || c == '.'))
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("/");
    while out.ends_with(".lock") {
        out.truncate(out.len() - ".lock".len());
    }
    out.trim_matches(|c| c == '-' || c == '/' || c == '.')
        .to_string()
}

/// The unit of a piece of a duration, in minutes.
fn unit(name: &str) -> Option<u32> {
    Some(match name {
        "w" | "wk" | "week" | "weeks" | "н" | "нед" | "недели" | "неделя" | "недель" => WEEK,
        "d" | "day" | "days" | "д" | "дн" | "день" | "дня" | "дней" => DAY,
        "h" | "hr" | "hrs" | "hour" | "hours" | "ч" | "час" | "часа" | "часов" => 60,
        "m" | "min" | "mins" | "minute" | "minutes" | "м" | "мин" | "минут" | "минуты"
        | "минута" => 1,
        _ => return None,
    })
}

/// Minutes from a duration as people write it: "1h 25m", "1ч 25м", "1 ч 25 мин", "85m", "85"
/// (minutes), "1.5h", "1,5 ч", "1:25", "2d" (a day is 8 hours), "1w" (5 days); none when it isn't
/// one.
pub fn parse_duration(text: &str) -> Option<u32> {
    let text = text.trim().to_lowercase().replace(',', ".");
    if text.is_empty() {
        return None;
    }
    if let Some((hours, minutes)) = text.split_once(':') {
        let hours: u32 = hours.trim().parse().ok()?;
        let minutes: u32 = minutes.trim().parse().ok()?;
        return (minutes < 60).then_some(hours * 60 + minutes);
    }
    let mut total = 0f64;
    let mut pieces = 0;
    let mut chars = text.chars().peekable();
    loop {
        while chars.peek().is_some_and(|c| c.is_whitespace()) {
            chars.next();
        }
        if chars.peek().is_none() {
            break;
        }
        let mut number = String::new();
        while let Some(&c) = chars.peek() {
            if c.is_ascii_digit() || c == '.' {
                number.push(c);
                chars.next();
            } else {
                break;
            }
        }
        let value: f64 = number.parse().ok()?;
        while chars.peek().is_some_and(|c| c.is_whitespace()) {
            chars.next();
        }
        let mut name = String::new();
        while let Some(&c) = chars.peek() {
            if c.is_alphabetic() {
                name.push(c);
                chars.next();
            } else {
                break;
            }
        }
        let minutes = if name.is_empty() {
            // A bare number is minutes ("85"), and only alone.
            if pieces > 0 || chars.peek().is_some() {
                return None;
            }
            1
        } else {
            unit(&name)?
        };
        total += value * f64::from(minutes);
        pieces += 1;
        while chars
            .peek()
            .is_some_and(|c| c.is_whitespace() || *c == '.')
        {
            chars.next();
        }
    }
    (pieces > 0).then(|| total.round() as u32)
}

/// A duration for Tracker (ISO 8601): `PT1H25M`; hours only, as Tracker's own weeks and days are
/// 40 and 8 hours.
pub fn iso_duration(minutes: u32) -> String {
    let (hours, minutes) = (minutes / 60, minutes % 60);
    match (hours, minutes) {
        (0, minutes) => format!("PT{minutes}M"),
        (hours, 0) => format!("PT{hours}H"),
        (hours, minutes) => format!("PT{hours}H{minutes}M"),
    }
}

/// Minutes of an ISO 8601 duration as Tracker writes it (`P1W2DT3H4M`; a week is 5 days, a day is
/// 8 hours); none when it isn't one. Only the tests read work logs back for now.
#[cfg(test)]
pub fn parse_iso_duration(text: &str) -> Option<u32> {
    let rest = text.strip_prefix('P')?;
    let mut total = 0u32;
    let mut in_time = false;
    let mut number = String::new();
    for c in rest.chars() {
        match c {
            'T' => in_time = true,
            '0'..='9' => number.push(c),
            unit => {
                let value: u32 = number.parse().ok()?;
                number.clear();
                total += value
                    * match (unit, in_time) {
                        ('W', false) => WEEK,
                        ('D', false) => DAY,
                        ('H', true) => 60,
                        ('M', true) => 1,
                        ('S', true) => 0,
                        _ => return None,
                    };
            }
        }
    }
    number.is_empty().then_some(total)
}

/// A duration as the interface shows it: "1h 25m" — "1 ч 25 мин" in Russian; "0m" for nothing.
pub fn format_duration(minutes: u32, language: &str) -> String {
    let (hours, minutes) = (minutes / 60, minutes % 60);
    let (h, m) = if language == "ru" {
        (" ч", " мин")
    } else {
        ("h", "m")
    };
    match (hours, minutes) {
        (0, minutes) => format!("{minutes}{m}"),
        (hours, 0) => format!("{hours}{h}"),
        (hours, minutes) => format!("{hours}{h} {minutes}{m}"),
    }
}

/// A value in Tracker's query language: in quotes, with quotes and backslashes escaped.
pub fn quote(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        if c == '"' || c == '\\' {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

/// A task's key ("TRACK-142"): a queue's key (Latin letters first, then letters, digits or `_`),
/// `-`, the number.
pub fn is_key(text: &str) -> bool {
    let Some((queue, number)) = text.rsplit_once('-') else {
        return false;
    };
    let mut queue_chars = queue.chars();
    queue_chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && queue_chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !number.is_empty()
        && number.chars().all(|c| c.is_ascii_digit())
}

/// Text cut to `max` characters with "…".
pub fn ellipsis(text: &str, max: usize) -> String {
    let mut chars = text.chars();
    let cut: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        format!("{}…", cut.trim_end())
    } else {
        cut
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_in_latin() {
        assert_eq!(
            slug("Ошибка входа через SSO", 40),
            "oshibka-vkhoda-cherez-sso"
        );
        assert_eq!(slug("Объём щёток — ЮЖНЫЙ ёж!", 40), "obem-shchetok-yuzhnyy-ezh");
        assert_eq!(slug("  Fix: the sign-in (OAuth 2.0)  ", 40), "fix-the-sign-in-oauth-2-0");
        assert_eq!(slug("один два три четыре", 12), "odin-dva-tri");
        assert_eq!(slug("один два три четыре", 11), "odin-dva");
        assert_eq!(slug("Превосходительство", 10), "prevoskhod");
        assert_eq!(slug("!!!", 10), "");
    }

    #[test]
    fn branch_names() {
        assert_eq!(
            branch_name("{key}-{summary}", "TRACK-142", "Ошибка входа через SSO", "bug"),
            "TRACK-142-oshibka-vkhoda-cherez-sso"
        );
        assert_eq!(
            branch_name("feature/{key}", "WEB-7", "x", "task"),
            "feature/WEB-7"
        );
        assert_eq!(
            branch_name("{type}/{key}-{summary}", "WEB-7", "Новая страница", "Задача"),
            "zadacha/WEB-7-novaya-stranitsa"
        );
        assert_eq!(git_name("a..b//c~d^e:f? g.lock"), "a.b/c-d-e-f-g");
        assert_eq!(git_name("-x-/"), "x");
        assert_eq!(fill("{key} {summary} {other}", &[("key", "A-1"), ("summary", "S")]), "A-1 S {other}");
    }

    #[test]
    fn durations_as_people_write_them() {
        assert_eq!(parse_duration("1h 25m"), Some(85));
        assert_eq!(parse_duration("1ч 25м"), Some(85));
        assert_eq!(parse_duration("1 ч 25 мин"), Some(85));
        assert_eq!(parse_duration("85m"), Some(85));
        assert_eq!(parse_duration("85"), Some(85));
        assert_eq!(parse_duration("1.5h"), Some(90));
        assert_eq!(parse_duration("1,5 ч"), Some(90));
        assert_eq!(parse_duration("1:25"), Some(85));
        assert_eq!(parse_duration("2d"), Some(16 * 60));
        assert_eq!(parse_duration("1w 1d"), Some(48 * 60));
        assert_eq!(parse_duration("2 часа 5 минут"), Some(125));
        assert_eq!(parse_duration(""), None);
        assert_eq!(parse_duration("soon"), None);
        assert_eq!(parse_duration("1h 25"), None);
        assert_eq!(parse_duration("1:75"), None);
    }

    #[test]
    fn durations_for_tracker() {
        assert_eq!(iso_duration(85), "PT1H25M");
        assert_eq!(iso_duration(120), "PT2H");
        assert_eq!(iso_duration(5), "PT5M");
        assert_eq!(parse_iso_duration("PT1H25M"), Some(85));
        assert_eq!(parse_iso_duration("P1W2DT3H4M"), Some(40 * 60 + 16 * 60 + 184));
        assert_eq!(parse_iso_duration("PT30S"), Some(0));
        assert_eq!(parse_iso_duration("1H"), None);
        assert_eq!(parse_iso_duration("PT5"), None);
        assert_eq!(format_duration(85, "en"), "1h 25m");
        assert_eq!(format_duration(85, "ru"), "1 ч 25 мин");
        assert_eq!(format_duration(120, "en"), "2h");
        assert_eq!(format_duration(0, "ru"), "0 мин");
        assert_eq!(parse_duration(&format_duration(85, "ru")), Some(85));
    }

    #[test]
    fn queries_and_keys() {
        assert_eq!(quote(r#"say "hi" \ bye"#), r#""say \"hi\" \\ bye""#);
        assert!(is_key("TRACK-142"));
        assert!(is_key("Q1_X-7"));
        assert!(!is_key("142"));
        assert!(!is_key("UTF-"));
        assert!(!is_key("1A-2"));
        assert!(!is_key("A-2b"));
        assert_eq!(ellipsis("abcdef", 3), "abc…");
        assert_eq!(ellipsis("abc", 3), "abc");
    }
}
