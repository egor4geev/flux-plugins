//! Text for Telegram — agent B's: Claude's Markdown as the HTML of `parse_mode` (drafts, plain
//! messages: bold, italics, code, code blocks with their language, links, lists as «•», quotes,
//! headings as bold, tables as `<pre>`) or as a rich message's Markdown (the final answer: tables,
//! `<details>`), cut to Telegram's limits on a line's border.
//!
//! Telegram refuses a whole message for one tag it can't pair, so every tag here is written with
//! its end: an emphasis without its closing mark stays as it is, a code block still being written
//! is closed for the draft. Lengths are counted as Telegram counts them — in UTF-16 units — and on
//! the HTML itself (a little stricter than Telegram, which counts the text without the tags).

use crate::host::Host;

/// A message's or a draft's text, in characters.
pub const MESSAGE_LIMIT: usize = 4096;
/// A rich message's.
pub const RICH_LIMIT: usize = 32_768;
/// A step's line in the folded block of steps, in characters.
const STEP_LINE: usize = 120;

/// Text inside HTML.
pub fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Text inside an HTML attribute.
fn escape_attr(text: &str) -> String {
    escape(text).replace('"', "&quot;")
}

/// At most `max` characters: the rest is cut off with «…».
pub fn cut(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.into();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// A text's length as Telegram counts it: UTF-16 units.
pub fn length(text: &str) -> usize {
    text.encode_utf16().count()
}

/// Claude's Markdown as `parse_mode` HTML.
pub fn markdown_html(markdown: &str) -> String {
    let mut blocks = Blocks::default();
    for line in markdown.lines() {
        blocks.line(line);
    }
    blocks.finish()
}

/// A draft's HTML: the whole text while it fits a message, else its end (the lines that fit,
/// after «…»; a code block it starts inside of is opened again).
pub fn draft_html(markdown: &str) -> String {
    let html = markdown_html(markdown);
    if length(&html) <= MESSAGE_LIMIT {
        return html;
    }
    let lines: Vec<&str> = markdown.lines().collect();
    let fences = fences_at(&lines);
    let fits = |start: usize| {
        length(&markdown_html(&piece(
            &lines,
            &fences,
            start,
            lines.len(),
            true,
        ))) <= MESSAGE_LIMIT
    };
    // The earliest line the rest fits from (the whole text doesn't).
    let (mut low, mut high) = (1, lines.len());
    while low < high {
        let middle = (low + high) / 2;
        if fits(middle) {
            high = middle;
        } else {
            low = middle + 1;
        }
    }
    if low < lines.len() {
        return markdown_html(&piece(&lines, &fences, low, lines.len(), true));
    }
    // The last line alone is longer than a message: its end.
    let last: Vec<char> = lines.last().copied().unwrap_or_default().chars().collect();
    let tail: String = last[last.len().saturating_sub(MESSAGE_LIMIT / 2)..]
        .iter()
        .collect();
    format!("…{}", escape(&tail))
}

/// A message's HTML in parts that fit a message each, cut on a line's border (a code block cut in
/// two is closed and opened again).
pub fn html_parts(markdown: &str) -> Vec<String> {
    let lines: Vec<&str> = markdown.lines().collect();
    let fences = fences_at(&lines);
    let mut parts = Vec::new();
    let mut start = 0;
    while start < lines.len() {
        let fits = |end: usize| {
            length(&markdown_html(&piece(&lines, &fences, start, end, false))) <= MESSAGE_LIMIT
        };
        if !fits(start + 1) {
            // One line longer than a message: in pieces.
            let chars: Vec<char> = lines[start].chars().collect();
            for chunk in chars.chunks(MESSAGE_LIMIT / 2) {
                parts.push(escape(&chunk.iter().collect::<String>()));
            }
            start += 1;
            continue;
        }
        // The most lines that fit.
        let (mut low, mut high) = (start + 1, lines.len());
        while low < high {
            let middle = (low + high).div_ceil(2);
            if fits(middle) {
                low = middle;
            } else {
                high = middle - 1;
            }
        }
        let html = markdown_html(&piece(&lines, &fences, start, low, false));
        if !html.trim().is_empty() {
            parts.push(html);
        }
        start = low;
    }
    parts
}

/// A rich message's Markdown: as Claude wrote it, cut on a line's border to fit.
pub fn rich_markdown(markdown: &str) -> String {
    if length(markdown) <= RICH_LIMIT {
        return markdown.to_string();
    }
    let lines: Vec<&str> = markdown.lines().collect();
    let fences = fences_at(&lines);
    let mut out = String::new();
    let mut end = 0;
    for line in &lines {
        if length(&out) + length(line) + 1 > RICH_LIMIT - 16 {
            break;
        }
        out.push_str(line);
        out.push('\n');
        end += 1;
    }
    if end == 0 {
        out = cut(markdown, (RICH_LIMIT - 16) / 2);
        out.push('\n');
    } else if let Some(open) = &fences[end] {
        out.push_str(&fence_of(open));
        out.push('\n');
    }
    out.push('…');
    out
}

/// A group of steps as a folded quote: the summary in bold, a line per step.
pub fn steps_html(summary: &str, lines: &[String]) -> String {
    let mut body = format!("<b>{}</b>", escape(&cut(summary, 200)));
    for (index, line) in lines.iter().enumerate() {
        let line = format!("\n{}", escape(&cut(&line.replace('\n', " "), STEP_LINE)));
        if length(&body) + length(&line) > MESSAGE_LIMIT - 64 {
            body.push_str(&format!("\n… +{}", lines.len() - index));
            break;
        }
        body.push_str(&line);
    }
    format!("<blockquote expandable>{body}</blockquote>")
}

/// A count in words, `{0}` the number: "1 file", "3 files". English takes `one` for 1 and `other`
/// for the rest; Russian has three forms, translated as `"<other>|one"`, `"<other>|few"` and
/// `"<other>|many"` in `ru.toml` ("{0} files|few" = "{0} файла").
pub fn count(host: &dyn Host, n: u64, one: &str, other: &str) -> String {
    let key = format!("{other}|{}", russian_form(n));
    let form = host.tr(&key);
    let template = if form != key {
        form
    } else if n == 1 {
        host.tr(one)
    } else {
        host.tr(other)
    };
    template.replace("{0}", &n.to_string())
}

/// The Russian plural form of `n`: "one" (1, 21, 101), "few" (2–4, 22–24), "many" (the rest).
fn russian_form(n: u64) -> &'static str {
    match (n % 10, n % 100) {
        (1, rest) if rest != 11 => "one",
        (2..=4, rest) if !(12..=14).contains(&rest) => "few",
        _ => "many",
    }
}

// --- Blocks ---

/// Markdown's blocks, line by line: code blocks, tables and quotes gather their lines.
#[derive(Default)]
struct Blocks {
    out: Vec<String>,
    /// A code block being read: its fence, language and lines.
    code: Option<(String, String, Vec<String>)>,
    table: Vec<String>,
    quote: Vec<String>,
}

impl Blocks {
    fn line(&mut self, line: &str) {
        if let Some((fence, _, lines)) = &mut self.code {
            if closes(line, fence) {
                self.close_code();
            } else {
                lines.push(line.to_string());
            }
            return;
        }
        let trimmed = line.trim_start();
        if let Some(fence) = opens_fence(trimmed) {
            self.flush();
            let language = trimmed[fence.len()..]
                .split_whitespace()
                .next()
                .unwrap_or_default()
                .chars()
                .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '+' | '-' | '.' | '#'))
                .collect();
            self.code = Some((fence, language, Vec::new()));
            return;
        }
        if trimmed.starts_with('|') {
            self.flush_quote();
            self.table.push(trimmed.to_string());
            return;
        }
        if let Some(rest) = trimmed.strip_prefix('>') {
            self.flush_table();
            self.quote
                .push(rest.strip_prefix(' ').unwrap_or(rest).to_string());
            return;
        }
        self.flush();
        self.out.push(block_line(line));
    }

    fn finish(mut self) -> String {
        // A code block still being written is closed.
        self.close_code();
        self.flush();
        self.out.join("\n")
    }

    fn flush(&mut self) {
        self.flush_table();
        self.flush_quote();
    }

    fn close_code(&mut self) {
        let Some((_, language, lines)) = self.code.take() else {
            return;
        };
        if lines.iter().all(|line| line.trim().is_empty()) {
            return;
        }
        let body = escape(&lines.join("\n"));
        self.out.push(if language.is_empty() {
            format!("<pre>{body}</pre>")
        } else {
            format!("<pre><code class=\"language-{language}\">{body}</code></pre>")
        });
    }

    fn flush_quote(&mut self) {
        if self.quote.is_empty() {
            return;
        }
        let lines: Vec<String> = std::mem::take(&mut self.quote)
            .iter()
            .map(|line| block_line(line))
            .collect();
        if lines.iter().all(|line| line.trim().is_empty()) {
            return;
        }
        self.out
            .push(format!("<blockquote>{}</blockquote>", lines.join("\n")));
    }

    /// A table as monospaced columns; rows without the separator row are only lines.
    fn flush_table(&mut self) {
        if self.table.is_empty() {
            return;
        }
        let rows = std::mem::take(&mut self.table);
        let cells: Vec<Vec<String>> = rows.iter().map(|row| row_cells(row)).collect();
        let separator = |row: &Vec<String>| {
            row.iter()
                .all(|cell| !cell.is_empty() && cell.chars().all(|c| matches!(c, '-' | ':' | ' ')))
        };
        if rows.len() < 2 || !cells.iter().any(separator) {
            self.out.extend(rows.iter().map(|row| inline(row)));
            return;
        }
        let body: Vec<Vec<String>> = cells
            .into_iter()
            .filter(|row| !separator(row))
            .map(|row| {
                row.iter()
                    .map(|cell| cell.replace("**", "").replace('`', ""))
                    .collect()
            })
            .collect();
        let columns = body.iter().map(Vec::len).max().unwrap_or_default();
        let widths: Vec<usize> = (0..columns)
            .map(|column| {
                body.iter()
                    .filter_map(|row| row.get(column))
                    .map(|cell| cell.chars().count())
                    .max()
                    .unwrap_or_default()
            })
            .collect();
        let lines: Vec<String> = body
            .iter()
            .map(|row| {
                let cells: Vec<String> = (0..columns)
                    .map(|column| {
                        let cell = row.get(column).map(String::as_str).unwrap_or_default();
                        let pad = widths[column].saturating_sub(cell.chars().count());
                        format!("{cell}{}", " ".repeat(pad))
                    })
                    .collect();
                cells.join("  ").trim_end().to_string()
            })
            .collect();
        self.out
            .push(format!("<pre>{}</pre>", escape(&lines.join("\n"))));
    }
}

fn row_cells(row: &str) -> Vec<String> {
    let row = row.trim();
    let row = row.strip_prefix('|').unwrap_or(row);
    let row = row.strip_suffix('|').unwrap_or(row);
    row.split('|').map(|cell| cell.trim().to_string()).collect()
}

/// "```" or "~~~" (or longer) opening a code block.
fn opens_fence(trimmed: &str) -> Option<String> {
    ['`', '~'].into_iter().find_map(|mark| {
        let count = trimmed.chars().take_while(|c| *c == mark).count();
        (count >= 3).then(|| mark.to_string().repeat(count))
    })
}

fn closes(line: &str, fence: &str) -> bool {
    let trimmed = line.trim();
    let mark = fence.chars().next().unwrap_or('`');
    trimmed.chars().count() >= fence.chars().count() && trimmed.chars().all(|c| c == mark)
}

/// The fence that closes a code block opened by this line.
fn fence_of(open: &str) -> String {
    opens_fence(open.trim_start()).unwrap_or_else(|| "```".into())
}

/// For each line (and after the last): the line that opened the code block it is inside of.
fn fences_at(lines: &[&str]) -> Vec<Option<String>> {
    let mut out = Vec::with_capacity(lines.len() + 1);
    let mut open: Option<(String, String)> = None;
    for line in lines {
        out.push(open.as_ref().map(|(_, opening)| opening.clone()));
        match &open {
            Some((fence, _)) if closes(line, fence) => open = None,
            Some(_) => {}
            None => {
                let trimmed = line.trim_start();
                open = opens_fence(trimmed).map(|fence| (fence, trimmed.to_string()));
            }
        }
    }
    out.push(open.map(|(_, opening)| opening));
    out
}

/// Lines `start..end` as Markdown of their own: the code block they start inside of is opened
/// again; «…» first when the text goes on from before.
fn piece(
    lines: &[&str],
    fences: &[Option<String>],
    start: usize,
    end: usize,
    ellipsis: bool,
) -> String {
    let mut text = String::new();
    if ellipsis {
        text.push_str("…\n");
    }
    if let Some(open) = &fences[start] {
        text.push_str(open);
        text.push('\n');
    }
    text.push_str(&lines[start..end].join("\n"));
    text
}

/// A line outside code blocks, tables and quotes: a heading, a rule, a list's item, or text.
fn block_line(line: &str) -> String {
    let trimmed = line.trim_start();
    let indent = "  ".repeat(((line.len() - trimmed.len()) / 2).min(4));
    let hashes = trimmed.chars().take_while(|c| *c == '#').count();
    if (1..=6).contains(&hashes) && trimmed[hashes..].starts_with(' ') {
        let title = trimmed[hashes..].trim().trim_end_matches('#').trim_end();
        return if title.is_empty() {
            String::new()
        } else {
            format!("<b>{}</b>", inline(title))
        };
    }
    if is_rule(trimmed) {
        return "———".into();
    }
    for bullet in ["- ", "* ", "+ "] {
        if let Some(rest) = trimmed.strip_prefix(bullet) {
            let item = if let Some(rest) = rest.strip_prefix("[ ] ") {
                format!("☐ {}", inline(rest))
            } else if let Some(rest) = rest.strip_prefix("[x] ").or(rest.strip_prefix("[X] ")) {
                format!("☑ {}", inline(rest))
            } else {
                inline(rest)
            };
            return format!("{indent}• {item}");
        }
    }
    let digits = trimmed.chars().take_while(char::is_ascii_digit).count();
    if (1..=9).contains(&digits) {
        let rest = &trimmed[digits..];
        if let Some(item) = rest.strip_prefix(". ").or(rest.strip_prefix(") ")) {
            return format!("{indent}{}. {}", &trimmed[..digits], inline(item));
        }
    }
    inline(line)
}

/// "---", "***", "___" (spaces between allowed).
fn is_rule(trimmed: &str) -> bool {
    let marks: Vec<char> = trimmed.chars().filter(|c| !c.is_whitespace()).collect();
    marks.len() >= 3
        && ['-', '*', '_']
            .iter()
            .any(|mark| marks.iter().all(|c| c == mark))
}

// --- Inline ---

/// A line's inline Markdown: `code`, **bold**, *italics*, ~~strikes~~, [links](https://…),
/// <https://…>; marks without their pair stay as they are.
fn inline(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::new();
    let mut at = 0;
    while at < chars.len() {
        let c = chars[at];
        match c {
            '`' => {
                let count = run(&chars, at, '`');
                if let Some(end) = find_run(&chars, at + count, '`', count) {
                    let inner: String = chars[at + count..end].iter().collect();
                    let inner = inner.trim();
                    if !inner.is_empty() {
                        out.push_str(&format!("<code>{}</code>", escape(inner)));
                    }
                    at = end + count;
                } else {
                    out.push_str(&"`".repeat(count));
                    at += count;
                }
                continue;
            }
            '*' | '_' | '~' => {
                let count = run(&chars, at, c);
                if let Some((open, close, len)) = emphasis(c, count)
                    && let Some(end) = find_close(&chars, at, c, len)
                {
                    let inner: String = chars[at + len..end].iter().collect();
                    out.push_str(open);
                    out.push_str(&inline(&inner));
                    out.push_str(close);
                    at = end + len;
                } else {
                    out.extend(std::iter::repeat_n(c, count));
                    at += count;
                }
                continue;
            }
            '[' => {
                if let Some((label, url, next)) = link(&chars, at) {
                    if web(&url) {
                        out.push_str(&format!(
                            "<a href=\"{}\">{}</a>",
                            escape_attr(&url),
                            inline(&label)
                        ));
                    } else {
                        out.push_str(&inline(&label));
                    }
                    at = next;
                    continue;
                }
            }
            '<' => {
                if let Some(close) = chars[at + 1..].iter().position(|c| *c == '>') {
                    let url: String = chars[at + 1..at + 1 + close].iter().collect();
                    if web(&url) && !url.contains(char::is_whitespace) {
                        out.push_str(&format!(
                            "<a href=\"{}\">{}</a>",
                            escape_attr(&url),
                            escape(&url)
                        ));
                        at += close + 2;
                        continue;
                    }
                }
            }
            _ => {}
        }
        out.push_str(&escape(&c.to_string()));
        at += 1;
    }
    out
}

/// How long the run of `c` at `at` is.
fn run(chars: &[char], at: usize, c: char) -> usize {
    chars[at..].iter().take_while(|x| **x == c).count()
}

/// The next run of exactly `count` × `c` from `from`.
fn find_run(chars: &[char], from: usize, c: char, count: usize) -> Option<usize> {
    let mut at = from;
    while at < chars.len() {
        if chars[at] == c {
            let found = run(chars, at, c);
            if found == count {
                return Some(at);
            }
            at += found;
        } else {
            at += 1;
        }
    }
    None
}

/// The tags of an emphasis of `count` × `c`, and how many marks they take.
fn emphasis(c: char, count: usize) -> Option<(&'static str, &'static str, usize)> {
    match (c, count) {
        ('~', 2) => Some(("<s>", "</s>", 2)),
        ('~', _) => None,
        (_, 1) => Some(("<i>", "</i>", 1)),
        (_, 2) => Some(("<b>", "</b>", 2)),
        _ => Some(("<b><i>", "</i></b>", 3)),
    }
}

/// Where the emphasis opened at `at` closes: a run of exactly as many marks, no space just inside
/// them; `_` only between words (snake_case stays as it is).
fn find_close(chars: &[char], at: usize, c: char, len: usize) -> Option<usize> {
    let word = |index: Option<usize>| {
        index
            .and_then(|i| chars.get(i))
            .is_some_and(|x| x.is_alphanumeric())
    };
    if c == '_' && word(at.checked_sub(1)) {
        return None;
    }
    let first = chars.get(at + len)?;
    if first.is_whitespace() {
        return None;
    }
    let mut end = at + len + 1;
    while end + len <= chars.len() {
        if chars[end..end + len].iter().all(|x| *x == c)
            && chars[end - 1] != c
            && chars.get(end + len) != Some(&c)
            && !chars[end - 1].is_whitespace()
            && !(c == '_' && word(Some(end + len)))
        {
            return Some(end);
        }
        end += 1;
    }
    None
}

/// `[label](url)` at `at`: the label, the URL and where the text goes on.
fn link(chars: &[char], at: usize) -> Option<(String, String, usize)> {
    let close = at + 1 + chars[at + 1..].iter().position(|c| *c == ']')?;
    if chars.get(close + 1) != Some(&'(') {
        return None;
    }
    let end = close + 2 + chars[close + 2..].iter().position(|c| *c == ')')?;
    let label: String = chars[at + 1..close].iter().collect();
    let target: String = chars[close + 2..end].iter().collect();
    let url = target
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_string();
    Some((label, url, end + 1))
}

/// A link Telegram opens.
fn web(url: &str) -> bool {
    ["https://", "http://", "mailto:", "tg://"]
        .iter()
        .any(|scheme| url.starts_with(scheme))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::FakeHost;

    #[test]
    fn escapes_and_cuts() {
        assert_eq!(escape("a<b>&c"), "a&lt;b&gt;&amp;c");
        assert_eq!(cut("абвгд", 3), "аб…");
        assert_eq!(cut("аб", 3), "аб");
    }

    #[test]
    fn inline_marks_become_tags_and_lone_marks_stay() {
        assert_eq!(
            markdown_html("**Bold**, *it*, `a<b>` and ~~old~~"),
            "<b>Bold</b>, <i>it</i>, <code>a&lt;b&gt;</code> and <s>old</s>"
        );
        assert_eq!(
            markdown_html("parse_config_with_env and 2 * 3 * 4"),
            "parse_config_with_env and 2 * 3 * 4"
        );
        assert_eq!(markdown_html("**not closed"), "**not closed");
        assert_eq!(markdown_html("__bold__ _it_"), "<b>bold</b> <i>it</i>");
        assert_eq!(
            markdown_html("[docs](https://x.dev/a?b=1&c=\"2\") and [file](src/main.rs)"),
            "<a href=\"https://x.dev/a?b=1&amp;c=&quot;2&quot;\">docs</a> and file"
        );
        assert_eq!(markdown_html("**a *b* c**"), "<b>a <i>b</i> c</b>");
    }

    #[test]
    fn blocks_become_telegram_html() {
        let markdown = "# Title\nText\n- one\n  - two\n1. first\n> quoted\n> more\n\n```rust\nfn main() {}\n```\n---";
        assert_eq!(
            markdown_html(markdown),
            "<b>Title</b>\nText\n• one\n  • two\n1. first\n<blockquote>quoted\nmore</blockquote>\n\n\
             <pre><code class=\"language-rust\">fn main() {}</code></pre>\n———"
        );
    }

    #[test]
    fn an_unclosed_code_block_is_closed_for_the_draft() {
        assert_eq!(
            markdown_html("Look:\n```js\nlet a = 1 < 2;"),
            "Look:\n<pre><code class=\"language-js\">let a = 1 &lt; 2;</code></pre>"
        );
        assert_eq!(markdown_html("Look:\n```"), "Look:");
    }

    #[test]
    fn tables_are_columns_in_pre() {
        let markdown = "| File | + |\n|---|---:|\n| `a.rs` | 12 |\n| **b.rs** | 4 |";
        assert_eq!(
            markdown_html(markdown),
            "<pre>File  +\na.rs  12\nb.rs  4</pre>"
        );
        assert_eq!(markdown_html("| not a table"), "| not a table");
    }

    #[test]
    fn a_long_draft_shows_its_end() {
        let mut markdown = String::from("```text\n");
        for index in 0..1000 {
            markdown.push_str(&format!("line {index}\n"));
        }
        let html = draft_html(&markdown);
        assert!(length(&html) <= MESSAGE_LIMIT);
        assert!(
            html.starts_with("…\n<pre><code class=\"language-text\">"),
            "{}",
            &html[..60]
        );
        assert!(html.contains("line 999</code></pre>"));
        assert_eq!(draft_html("short"), "short");
    }

    #[test]
    fn long_messages_are_cut_on_lines_and_blocks_reopen() {
        let mut markdown = String::from("Intro\n```py\n");
        for index in 0..1500 {
            markdown.push_str(&format!("print({index})\n"));
        }
        markdown.push_str("```\nThe end");
        let parts = html_parts(&markdown);
        assert!(parts.len() >= 2);
        for part in &parts {
            assert!(length(part) <= MESSAGE_LIMIT);
            assert_eq!(
                part.matches("<pre>").count(),
                part.matches("</pre>").count()
            );
        }
        assert!(parts[1].starts_with("<pre><code class=\"language-py\">"));
        assert!(parts.last().unwrap().ends_with("The end"));
        let rich = rich_markdown(&"word ".repeat(10_000));
        assert!(length(&rich) <= RICH_LIMIT);
    }

    #[test]
    fn steps_fold_and_counts_agree() {
        let html = steps_html("Read 2 files", &["Read a.rs".into(), "Read <b>.rs".into()]);
        assert_eq!(
            html,
            "<blockquote expandable><b>Read 2 files</b>\nRead a.rs\nRead &lt;b&gt;.rs</blockquote>"
        );
        let host = FakeHost::default();
        assert_eq!(count(&host, 1, "{0} file", "{0} files"), "1 file");
        assert_eq!(count(&host, 3, "{0} file", "{0} files"), "3 files");
        assert_eq!(russian_form(21), "one");
        assert_eq!(russian_form(23), "few");
        assert_eq!(russian_form(12), "many");
    }
}
