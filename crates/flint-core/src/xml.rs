//! Just enough XML to read Last.fm's answers.
//!
//! Last.fm's replies are a few hundred bytes of elements, attributes and text — no namespaces, no
//! DTDs, no processing instructions worth honouring. A real parser would be a dependency; this is
//! a reader for that shape and nothing else, and it fails by returning `None` rather than by
//! guessing, so a malformed reply reads as "the field is not there" instead of as data.

#[derive(Debug, Default, Clone)]
pub struct Node {
    pub name: String,
    pub attrs: Vec<(String, String)>,
    pub text: String,
    pub children: Vec<Node>,
}

impl Node {
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }

    /// The first direct child with this name.
    pub fn child(&self, name: &str) -> Option<&Node> {
        self.children.iter().find(|c| c.name == name)
    }

    /// Every direct child with this name.
    pub fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Node> {
        self.children.iter().filter(move |c| c.name == name)
    }

    /// `a/b/c` — text of a nested child, trimmed. Missing anywhere on the path is an empty string,
    /// because every caller here treats "absent" and "empty" the same way.
    pub fn text_at(&self, path: &str) -> String {
        let mut node = self;
        for step in path.split('/') {
            match node.child(step) {
                Some(next) => node = next,
                None => return String::new(),
            }
        }
        node.text.trim().to_string()
    }
}

/// Parse a document into its root element. Returns `None` if there is no element at all.
pub fn parse(input: &str) -> Option<Node> {
    let bytes: Vec<char> = input.chars().collect();
    let mut i = 0usize;
    let mut stack: Vec<Node> = Vec::new();
    let mut root: Option<Node> = None;

    while i < bytes.len() {
        if bytes[i] != '<' {
            // Text belongs to whatever element is open; outside one it is whitespace we ignore.
            let start = i;
            while i < bytes.len() && bytes[i] != '<' {
                i += 1;
            }
            if let Some(open) = stack.last_mut() {
                open.text.push_str(&unescape(&bytes[start..i].iter().collect::<String>()));
            }
            continue;
        }
        // <? … ?>, <!-- … -->, <![CDATA[ … ]]> and <!DOCTYPE …>: skipped whole, except CDATA,
        // whose contents are text.
        if starts_with(&bytes, i, "<![CDATA[") {
            let start = i + 9;
            let end = find(&bytes, start, "]]>").unwrap_or(bytes.len());
            if let Some(open) = stack.last_mut() {
                open.text.push_str(&bytes[start..end].iter().collect::<String>());
            }
            i = (end + 3).min(bytes.len());
            continue;
        }
        if starts_with(&bytes, i, "<!--") {
            i = find(&bytes, i + 4, "-->").map_or(bytes.len(), |e| e + 3);
            continue;
        }
        if starts_with(&bytes, i, "<?") || starts_with(&bytes, i, "<!") {
            i = find(&bytes, i, ">").map_or(bytes.len(), |e| e + 1);
            continue;
        }
        let end = find(&bytes, i, ">")?;
        let tag: String = bytes[i + 1..end].iter().collect();
        i = end + 1;

        if let Some(name) = tag.strip_prefix('/') {
            // A close tag: the element is finished, so it joins its parent (or becomes the root).
            let name = name.trim();
            let Some(done) = stack.pop() else { continue };
            if done.name != name {
                // Mismatched close. Keep what we have rather than throwing the document away:
                // the caller checks for the fields it needs and reports their absence.
                return root.or(Some(done));
            }
            match stack.last_mut() {
                Some(parent) => parent.children.push(done),
                None => root = Some(done),
            }
            continue;
        }

        let self_closing = tag.trim_end().ends_with('/');
        let body = tag.trim_end().trim_end_matches('/');
        let (name, attrs) = parse_tag(body);
        let node = Node { name, attrs, ..Node::default() };
        if self_closing {
            match stack.last_mut() {
                Some(parent) => parent.children.push(node),
                None => root = Some(node),
            }
        } else {
            stack.push(node);
        }
    }
    // An unclosed document still yields whatever was open, outermost first.
    while let Some(done) = stack.pop() {
        match stack.last_mut() {
            Some(parent) => parent.children.push(done),
            None => root = Some(done),
        }
    }
    root
}

fn starts_with(chars: &[char], at: usize, needle: &str) -> bool {
    needle.chars().enumerate().all(|(offset, c)| chars.get(at + offset) == Some(&c))
}

fn find(chars: &[char], from: usize, needle: &str) -> Option<usize> {
    (from..chars.len()).find(|&at| starts_with(chars, at, needle))
}

fn parse_tag(body: &str) -> (String, Vec<(String, String)>) {
    let mut parts = body.splitn(2, char::is_whitespace);
    let name = parts.next().unwrap_or("").trim().to_string();
    let mut attrs = Vec::new();
    let rest = parts.next().unwrap_or("");
    let chars: Vec<char> = rest.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        while i < chars.len() && chars[i].is_whitespace() {
            i += 1;
        }
        let start = i;
        while i < chars.len() && chars[i] != '=' && !chars[i].is_whitespace() {
            i += 1;
        }
        if start == i {
            break;
        }
        let key: String = chars[start..i].iter().collect();
        while i < chars.len() && (chars[i].is_whitespace() || chars[i] == '=') {
            i += 1;
        }
        let quote = if i < chars.len() && (chars[i] == '"' || chars[i] == '\'') {
            let q = chars[i];
            i += 1;
            q
        } else {
            ' '
        };
        let vstart = i;
        while i < chars.len() && chars[i] != quote {
            i += 1;
        }
        let value: String = chars[vstart..i].iter().collect();
        i += 1;
        attrs.push((key, unescape(&value)));
    }
    (name, attrs)
}

fn unescape(input: &str) -> String {
    if !input.contains('&') {
        return input.to_string();
    }
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        let tail = &rest[at..];
        let end = match tail.find(';') {
            Some(e) if e <= 10 => e,
            _ => {
                out.push('&');
                rest = &tail[1..];
                continue;
            }
        };
        let entity = &tail[1..end];
        let replacement = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            other => other
                .strip_prefix('#')
                .and_then(|n| match n.strip_prefix('x').or_else(|| n.strip_prefix('X')) {
                    Some(hex) => u32::from_str_radix(hex, 16).ok(),
                    None => n.parse::<u32>().ok(),
                })
                .and_then(char::from_u32),
        };
        match replacement {
            Some(c) => out.push(c),
            None => out.push_str(&tail[..=end]),
        }
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::parse;

    const LOVED: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<lfm status="ok"><lovedtracks user="x" totalPages="2" total="3">
  <track><name>Love Is A Losing Game</name><artist><name>Amy Winehouse</name></artist>
  <date uts="1600000000">13 Sep 2020</date></track>
  <track><name>M&amp;M</name><artist><name>Sigur R&#243;s</name></artist></track>
</lovedtracks></lfm>"#;

    #[test]
    fn reads_the_shape_lastfm_returns() {
        let root = parse(LOVED).expect("a root element");
        assert_eq!(root.name, "lfm");
        assert_eq!(root.attr("status"), Some("ok"));
        let loved = root.child("lovedtracks").expect("lovedtracks");
        assert_eq!(loved.attr("totalPages"), Some("2"));
        let tracks: Vec<_> = loved.children_named("track").collect();
        assert_eq!(tracks.len(), 2);
        assert_eq!(tracks[0].text_at("name"), "Love Is A Losing Game");
        assert_eq!(tracks[0].text_at("artist/name"), "Amy Winehouse");
        assert_eq!(tracks[0].child("date").and_then(|d| d.attr("uts")), Some("1600000000"));
        // Entities in both text and attributes, including a numeric one.
        assert_eq!(tracks[1].text_at("name"), "M&M");
        assert_eq!(tracks[1].text_at("artist/name"), "Sigur Rós");
        // A field that is not there reads as empty, never as a panic.
        assert_eq!(tracks[1].text_at("date/nothing"), "");
    }

    #[test]
    fn an_error_reply_is_still_readable() {
        let root = parse(r#"<lfm status="failed"><error code="9">Invalid session key</error></lfm>"#).unwrap();
        assert_eq!(root.attr("status"), Some("failed"));
        let error = root.child("error").unwrap();
        assert_eq!(error.attr("code"), Some("9"));
        assert_eq!(error.text.trim(), "Invalid session key");
    }

    #[test]
    fn truncated_and_empty_input_do_not_panic() {
        assert!(parse("").is_none());
        assert!(parse("not xml at all").is_none());
        let torn = parse("<lfm status=\"ok\"><scrobbles accepted=\"1\">").unwrap();
        assert_eq!(torn.name, "lfm");
        assert_eq!(torn.child("scrobbles").and_then(|s| s.attr("accepted")), Some("1"));
    }
}
