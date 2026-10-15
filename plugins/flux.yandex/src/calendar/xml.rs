//! A small XML reader for CalDAV's answers (WebDAV `multistatus`): elements by their local name
//! (a namespace prefix dropped — `D:href`, `href` and `ns0:href` are one), attributes, text with
//! entities and CDATA. Enough for what CalDAV servers return; not a validating parser — a
//! malformed tail is cut off rather than refused.

/// An element: its local name, attributes and children, and the text directly inside it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Element {
    pub name: String,
    pub attributes: Vec<(String, String)>,
    pub children: Vec<Element>,
    pub text: String,
}

impl Element {
    /// The first child named `name`.
    pub fn child(&self, name: &str) -> Option<&Element> {
        self.children.iter().find(|child| child.name == name)
    }

    /// The children named `name`.
    pub fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Element> {
        self.children.iter().filter(move |child| child.name == name)
    }

    /// The first descendant named `name`, depth first.
    pub fn find(&self, name: &str) -> Option<&Element> {
        self.children
            .iter()
            .find_map(|child| (child.name == name).then_some(child).or_else(|| child.find(name)))
    }

    /// Whether a descendant is named `name`.
    pub fn has(&self, name: &str) -> bool {
        self.find(name).is_some()
    }

    /// The value of the attribute `name` (its local name).
    pub fn attribute(&self, name: &str) -> Option<&str> {
        self.attributes
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    /// The text inside, trimmed.
    pub fn trimmed(&self) -> &str {
        self.text.trim()
    }
}

/// Reads a document: its root element.
pub fn parse(text: &str) -> Result<Element, String> {
    let mut stack: Vec<Element> = vec![Element::default()];
    let mut rest = text;
    while !rest.is_empty() {
        let Some(open) = rest.find('<') else {
            push_text(&mut stack, rest);
            break;
        };
        push_text(&mut stack, &rest[..open]);
        rest = &rest[open..];
        if let Some(tail) = rest.strip_prefix("<!--") {
            rest = tail.find("-->").map_or("", |end| &tail[end + 3..]);
        } else if let Some(tail) = rest.strip_prefix("<![CDATA[") {
            let end = tail.find("]]>").unwrap_or(tail.len());
            if let Some(top) = stack.last_mut() {
                top.text.push_str(&tail[..end]);
            }
            rest = tail.get(end + 3..).unwrap_or("");
        } else if rest.starts_with("<?") || rest.starts_with("<!") {
            rest = rest.find('>').map_or("", |end| &rest[end + 1..]);
        } else if let Some(tail) = rest.strip_prefix("</") {
            let end = tail.find('>').unwrap_or(tail.len());
            let name = local(tail[..end].trim());
            rest = tail.get(end + 1..).unwrap_or("");
            close(&mut stack, name);
        } else {
            let end = tag_end(rest).ok_or("an unfinished tag")?;
            let inner = &rest[1..end];
            rest = &rest[end + 1..];
            let (inner, empty) = match inner.strip_suffix('/') {
                Some(inner) => (inner, true),
                None => (inner, false),
            };
            let element = open_tag(inner);
            if empty {
                if let Some(top) = stack.last_mut() {
                    top.children.push(element);
                }
            } else {
                stack.push(element);
            }
        }
    }
    while stack.len() > 1 {
        let element = stack.pop().unwrap_or_default();
        if let Some(top) = stack.last_mut() {
            top.children.push(element);
        }
    }
    stack
        .pop()
        .and_then(|document| document.children.into_iter().next())
        .ok_or_else(|| "not XML".to_string())
}

/// Where a tag that starts `text` ends: its `>`, skipping quoted attribute values.
fn tag_end(text: &str) -> Option<usize> {
    let mut quote = None;
    for (index, char) in text.char_indices() {
        match (quote, char) {
            (None, '"' | '\'') => quote = Some(char),
            (Some(open), char) if char == open => quote = None,
            (None, '>') => return Some(index),
            _ => {}
        }
    }
    None
}

/// An element from the inside of its opening tag: `D:prop xmlns:D="DAV:"`.
fn open_tag(inner: &str) -> Element {
    let inner = inner.trim();
    let name_end = inner
        .find(|char: char| char.is_whitespace())
        .unwrap_or(inner.len());
    let mut element = Element {
        name: local(&inner[..name_end]).to_string(),
        ..Element::default()
    };
    let mut rest = inner[name_end..].trim_start();
    while let Some(equals) = rest.find('=') {
        let name = rest[..equals].trim();
        let value = rest[equals + 1..].trim_start();
        let Some(quote) = value.chars().next().filter(|char| *char == '"' || *char == '\'')
        else {
            break;
        };
        let value = &value[1..];
        let end = value.find(quote).unwrap_or(value.len());
        if !name.starts_with("xmlns") {
            element
                .attributes
                .push((local(name).to_string(), decode(&value[..end])));
        }
        rest = value.get(end + 1..).unwrap_or("").trim_start();
    }
    element
}

/// Closes the element `name` (and any left open inside it).
fn close(stack: &mut Vec<Element>, name: &str) {
    if !stack.iter().skip(1).any(|element| element.name == name) {
        return;
    }
    while stack.len() > 1 {
        let element = stack.pop().unwrap_or_default();
        let done = element.name == name;
        if let Some(top) = stack.last_mut() {
            top.children.push(element);
        }
        if done {
            break;
        }
    }
}

fn push_text(stack: &mut [Element], text: &str) {
    if text.is_empty() {
        return;
    }
    if let Some(top) = stack.last_mut() {
        top.text.push_str(&decode(text));
    }
}

/// A name without its namespace prefix.
fn local(name: &str) -> &str {
    name.rsplit_once(':').map_or(name, |(_, local)| local)
}

/// Text with its entities decoded: `&lt;`, `&amp;`, `&#10;`, `&#x41;`…
pub fn decode(text: &str) -> String {
    if !text.contains('&') {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('&') {
        out.push_str(&rest[..start]);
        let tail = &rest[start..];
        let Some(end) = tail.find(';').filter(|end| *end <= 12) else {
            out.push('&');
            rest = &tail[1..];
            continue;
        };
        let entity = &tail[1..end];
        let decoded = match entity {
            "lt" => Some('<'),
            "gt" => Some('>'),
            "amp" => Some('&'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => entity
                .strip_prefix("#x")
                .or_else(|| entity.strip_prefix("#X"))
                .and_then(|hex| u32::from_str_radix(hex, 16).ok())
                .or_else(|| entity.strip_prefix('#').and_then(|dec| dec.parse().ok()))
                .and_then(char::from_u32),
        };
        match decoded {
            Some(char) => {
                out.push(char);
                rest = &tail[end + 1..];
            }
            None => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Text escaped for XML.
#[cfg(test)]
pub fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_multistatus() {
        let text = r#"<?xml version="1.0" encoding="utf-8"?>
<D:multistatus xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
  <!-- a comment -->
  <D:response>
    <D:href>/calendars/egor%40example.ru/events-default/</D:href>
    <D:propstat>
      <D:prop>
        <D:displayname>Мои события &amp; встречи</D:displayname>
        <D:resourcetype><D:collection/><C:calendar/></D:resourcetype>
        <C:supported-calendar-component-set><C:comp name="VEVENT"/></C:supported-calendar-component-set>
      </D:prop>
      <D:status>HTTP/1.1 200 OK</D:status>
    </D:propstat>
  </D:response>
  <response xmlns="DAV:"><href>/x/</href><propstat><prop><C:calendar-data><![CDATA[BEGIN:VCALENDAR
END:VCALENDAR]]></C:calendar-data></prop></propstat></response>
</D:multistatus>"#;
        let root = parse(text).unwrap();
        assert_eq!(root.name, "multistatus");
        let responses: Vec<_> = root.children_named("response").collect();
        assert_eq!(responses.len(), 2);
        assert_eq!(
            responses[0].child("href").unwrap().trimmed(),
            "/calendars/egor%40example.ru/events-default/"
        );
        assert_eq!(
            responses[0].find("displayname").unwrap().trimmed(),
            "Мои события & встречи"
        );
        assert!(responses[0].find("resourcetype").unwrap().has("calendar"));
        assert_eq!(responses[0].find("comp").unwrap().attribute("name"), Some("VEVENT"));
        assert_eq!(
            responses[1].find("calendar-data").unwrap().text,
            "BEGIN:VCALENDAR\nEND:VCALENDAR"
        );
    }

    #[test]
    fn decodes_entities() {
        assert_eq!(decode("a &lt;b&gt; &amp;&#10;&#x41;&quot;"), "a <b> &\nA\"");
        assert_eq!(decode("R&D & co"), "R&D & co");
        assert_eq!(escape("<a & b>"), "&lt;a &amp; b&gt;");
    }

    #[test]
    fn tolerates_unclosed_and_stray_tags() {
        let root = parse("<a><b>one</b><c>two</a>").unwrap();
        assert_eq!(root.child("b").unwrap().text, "one");
        assert_eq!(root.find("c").unwrap().text, "two");
        let root = parse("<a x='1' y=\"2>3\"><b/></x></a>").unwrap();
        assert_eq!(root.attribute("y"), Some("2>3"));
        assert!(root.child("b").is_some());
        assert!(parse("no xml here").is_err());
    }
}
