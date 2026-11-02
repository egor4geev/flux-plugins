//! dotenv files without the host: which files they are, their keys, and the checks — keys set
//! twice, keys of `.env.example` a file doesn't set and keys it doesn't list. Read as the Node.js
//! `dotenv` package reads them, like the grammar: `KEY=value`, `export KEY=value`, values in quotes
//! over several lines, `#` comments.

use std::collections::{HashMap, HashSet};

/// A `KEY=…` line. Columns are in characters, as Flux counts them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub key: String,
    /// Zero-based.
    pub line: u32,
    pub start: u32,
    pub end: u32,
    /// The value isn't empty: `KEY=` and `KEY=""` set nothing worth hiding.
    pub has_value: bool,
}

/// The names of templates: the keys a project needs, without the values — committed on purpose,
/// and what the other files are compared with.
const TEMPLATE_SUFFIXES: [&str; 4] = [".example", ".sample", ".template", ".dist"];
const TEMPLATE_PREFIXES: [&str; 3] = ["example.", "sample.", "template."];

/// The templates a folder's files are compared with, the first one found wins.
pub const EXAMPLES: [&str; 3] = [".env.example", ".env.sample", ".env.template"];

/// A file of the language: `.env`, `.env.*`, `*.env` (the manifest's names), in any case.
pub fn is_dotenv(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name == ".env" || name.starts_with(".env.") || (name.ends_with(".env") && name.len() > 4)
}

/// `.env.example`, `.env.local.sample`, `example.env`…: a template, not a file with secrets.
pub fn is_template(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    TEMPLATE_SUFFIXES
        .iter()
        .any(|suffix| name.ends_with(suffix))
        || (name.ends_with(".env")
            && TEMPLATE_PREFIXES
                .iter()
                .any(|prefix| name.starts_with(prefix)))
}

/// The assignments of a dotenv text, in order.
pub fn parse(text: &str) -> Vec<Entry> {
    let mut entries = Vec::new();
    // The quote of a value that goes on past its line.
    let mut open_quote: Option<char> = None;
    for (number, line) in text.split('\n').enumerate() {
        let chars: Vec<char> = line.trim_end_matches('\r').chars().collect();
        if let Some(quote) = open_quote {
            if closing_quote(&chars, 0, quote).is_some() {
                open_quote = None;
            }
            continue;
        }
        let mut at = skip_blanks(&chars, 0);
        if at >= chars.len() || chars[at] == '#' {
            continue;
        }
        // `export KEY=…`; `export=…` is a key named "export".
        if starts_with(&chars, at, "export") && chars.get(at + 6).is_some_and(|c| c.is_whitespace())
        {
            let after = skip_blanks(&chars, at + 6);
            if after < chars.len() && is_key_start(chars[after]) {
                at = after;
            }
        }
        if !is_key_start(chars[at]) {
            continue;
        }
        let start = at;
        while at < chars.len() && is_key_char(chars[at]) {
            at += 1;
        }
        let end = at;
        at = skip_blanks(&chars, at);
        if chars.get(at) != Some(&'=') {
            continue;
        }
        at = skip_blanks(&chars, at + 1);
        let has_value = match chars.get(at) {
            Some(&quote @ ('"' | '\'' | '`')) => match closing_quote(&chars, at + 1, quote) {
                Some(close) => close > at + 1,
                None => {
                    open_quote = Some(quote);
                    true
                }
            },
            // To a `#` or the end of the line, as dotenv reads an unquoted value.
            Some(_) => chars[at..]
                .iter()
                .take_while(|&&c| c != '#')
                .any(|c| !c.is_whitespace()),
            None => false,
        };
        entries.push(Entry {
            key: chars[start..end].iter().collect(),
            line: number as u32,
            start: start as u32,
            end: end as u32,
            has_value,
        });
    }
    entries
}

fn skip_blanks(chars: &[char], mut at: usize) -> usize {
    while at < chars.len() && (chars[at] == ' ' || chars[at] == '\t' || chars[at] == '\u{feff}') {
        at += 1;
    }
    at
}

fn starts_with(chars: &[char], at: usize, word: &str) -> bool {
    word.chars()
        .enumerate()
        .all(|(index, expected)| chars.get(at + index) == Some(&expected))
}

fn is_key_start(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

fn is_key_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-')
}

/// Where `quote` closes the value from `from` on: in double quotes `\` escapes the next character.
fn closing_quote(chars: &[char], from: usize, quote: char) -> Option<usize> {
    let mut at = from;
    while at < chars.len() {
        match chars[at] {
            '\\' if quote == '"' => at += 2,
            c if c == quote => return Some(at),
            _ => at += 1,
        }
    }
    None
}

/// A key set again: the later line and the line that set it first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Duplicate<'a> {
    pub entry: &'a Entry,
    pub first_line: u32,
}

/// The keys set more than once: every later assignment, with the first one's line (dotenv keeps
/// the first value, a shell the last — either way one of them is a mistake).
pub fn duplicates(entries: &[Entry]) -> Vec<Duplicate<'_>> {
    let mut first: HashMap<&str, u32> = HashMap::new();
    let mut found = Vec::new();
    for entry in entries {
        match first.get(entry.key.as_str()) {
            Some(&line) => found.push(Duplicate {
                entry,
                first_line: line,
            }),
            None => {
                first.insert(&entry.key, entry.line);
            }
        }
    }
    found
}

/// A file against its template: the template's keys the file doesn't set (in the template's
/// order, once each), and the file's assignments of keys the template doesn't list.
pub fn compare<'a>(entries: &'a [Entry], example: &[Entry]) -> (Vec<String>, Vec<&'a Entry>) {
    let set: HashSet<&str> = entries.iter().map(|entry| entry.key.as_str()).collect();
    let listed: HashSet<&str> = example.iter().map(|entry| entry.key.as_str()).collect();
    let mut seen = HashSet::new();
    let missing = example
        .iter()
        .filter(|entry| !set.contains(entry.key.as_str()) && seen.insert(entry.key.as_str()))
        .map(|entry| entry.key.clone())
        .collect();
    let extra = entries
        .iter()
        .filter(|entry| !listed.contains(entry.key.as_str()))
        .collect();
    (missing, extra)
}

/// The file sets some value: a template, or a file of empty keys, has nothing to keep out of Git.
pub fn has_values(entries: &[Entry]) -> bool {
    entries.iter().any(|entry| entry.has_value)
}

/// The number of characters of the text's first line: the span a whole-file problem underlines.
pub fn first_line_length(text: &str) -> u32 {
    text.split('\n')
        .next()
        .unwrap_or("")
        .trim_end_matches('\r')
        .chars()
        .count() as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(text: &str) -> Vec<(String, u32, bool)> {
        parse(text)
            .into_iter()
            .map(|entry| (entry.key, entry.line, entry.has_value))
            .collect()
    }

    #[test]
    fn names_of_dotenv_files_and_templates() {
        for name in [
            ".env",
            ".env.local",
            ".ENV.Production",
            "production.env",
            ".env.example",
        ] {
            assert!(is_dotenv(name), "{name}");
        }
        for name in ["env", ".envrc", "environment.ts", ".env-cmdrc", "a.envx"] {
            assert!(!is_dotenv(name), "{name}");
        }
        for name in [
            ".env.example",
            ".env.sample",
            ".env.template",
            ".env.local.example",
            ".env.dist",
            "example.env",
            "Sample.env",
        ] {
            assert!(is_template(name), "{name}");
        }
        for name in [
            ".env",
            ".env.local",
            ".env.production",
            "production.env",
            ".env.examples-old",
        ] {
            assert!(!is_template(name), "{name}");
        }
    }

    #[test]
    fn reads_keys_values_and_their_columns() {
        let text = "# comment\n\nA=1\nexport B = two words # note\n  C=\nD=\"\"\nE=# only a comment\nexport=1\nnot a line\nvite.port=5173\r\nÄ=x\n";
        assert_eq!(
            keys(text),
            [
                ("A".into(), 2, true),
                ("B".into(), 3, true),
                ("C".into(), 4, false),
                ("D".into(), 5, false),
                ("E".into(), 6, false),
                ("export".into(), 7, true),
                ("vite.port".into(), 9, true),
            ]
        );
        let entries = parse("  export  KEY=1\nÉ=1 X=2\n");
        assert_eq!((entries[0].start, entries[0].end), (10, 13));
        assert_eq!(
            entries.len(),
            1,
            "a key must start with a letter, a digit or _"
        );
        let entries = parse("Ж=1\nκλ=2\nKEY=ünïcode\nNEXT=1");
        assert_eq!(
            entries.iter().map(|entry| entry.line).collect::<Vec<_>>(),
            [2, 3]
        );
    }

    #[test]
    fn values_in_quotes_go_on_over_lines() {
        let text = "KEY=\"-----BEGIN-----\nNOT_A_KEY=1\n-----END-----\"\nNEXT='a\nB=2\n' # done\nTICK=`x`\nESCAPED=\"a \\\" b\nC=3\"\nLAST=1\n";
        assert_eq!(
            keys(text),
            [
                ("KEY".into(), 0, true),
                ("NEXT".into(), 3, true),
                ("TICK".into(), 6, true),
                ("ESCAPED".into(), 7, true),
                ("LAST".into(), 9, true),
            ]
        );
        // Single quotes and backticks don't escape.
        assert_eq!(
            keys("A='x\\'\nB=1"),
            [("A".into(), 0, true), ("B".into(), 1, true)]
        );
        // A quote that never closes takes the rest of the file, as dotenv does.
        assert_eq!(keys("A=\"open\nB=1\n"), [("A".into(), 0, true)]);
    }

    #[test]
    fn keys_set_again() {
        let entries = parse("A=1\nB=2\nA=3\n# A=4\nA=5\nB=\n");
        let found: Vec<(&str, u32, u32)> = duplicates(&entries)
            .iter()
            .map(|duplicate| {
                (
                    duplicate.entry.key.as_str(),
                    duplicate.entry.line,
                    duplicate.first_line,
                )
            })
            .collect();
        assert_eq!(found, [("A", 2, 0), ("A", 4, 0), ("B", 5, 1)]);
        assert!(
            duplicates(&parse("A=1\na=2\nA_1=3")).is_empty(),
            "keys are case-sensitive"
        );
    }

    #[test]
    fn a_file_against_its_template() {
        let example = parse("# Fill in:\nAPI_URL=\nSECRET=\nPORT=3000\nSECRET=\n");
        let entries = parse("PORT=8080\nDEBUG=1\nAPI_URL=https://x\nDEBUG=2\n");
        let (missing, extra) = compare(&entries, &example);
        assert_eq!(missing, ["SECRET"]);
        assert_eq!(
            extra
                .iter()
                .map(|entry| (entry.key.as_str(), entry.line))
                .collect::<Vec<_>>(),
            [("DEBUG", 1), ("DEBUG", 3)]
        );
        let (missing, extra) = compare(&entries, &[]);
        assert!(missing.is_empty());
        assert_eq!(extra.len(), 4);
    }

    #[test]
    fn files_worth_keeping_out_of_git() {
        assert!(has_values(&parse("A=\nB=1\n")));
        assert!(!has_values(&parse("# nothing\nA=\nB=''\nC= # comment\n")));
        assert!(!has_values(&parse("")));
        assert_eq!(first_line_length("ÄBC=1\r\nX=2"), 5);
        assert_eq!(first_line_length(""), 0);
    }
}
