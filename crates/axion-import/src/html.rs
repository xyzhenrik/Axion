//! Netscape bookmark files: the HTML every browser exports (`<DL>` lists of `<DT><H3>` folders
//! and `<DT><A HREF>` links). Parsed with a tolerant scanner, not an HTML parser: exports are
//! sloppy (unclosed `<p>`, `<DT>`, no closing quotes in old files) and may be hand-edited.

use crate::model::ImportBookmark;
use crate::text::{clean_title, clean_url, guess_to_unix};

/// Lists nested deeper than this are flattened.
const MAX_DEPTH: usize = 64;

/// Whether `text` starts like a bookmarks export.
pub fn looks_like_bookmarks(text: &str) -> bool {
    let head: String = text.chars().take(8192).collect::<String>().to_ascii_lowercase();
    head.contains("netscape-bookmark")
        || (head.contains("<dl") && (head.contains("<dt") || head.contains("<a ")))
}

struct Frame {
    name: Option<String>,
    toolbar: bool,
}

pub fn parse_bookmarks(src: &str) -> Vec<ImportBookmark> {
    // ASCII lowercasing keeps every byte offset, so tags are matched in `lower` and values
    // are cut from `src`.
    let lower = src.to_ascii_lowercase();
    let mut out = Vec::new();
    let mut frames: Vec<Frame> = Vec::new();
    // Lists beyond MAX_DEPTH: counted so that their closing tags stay balanced.
    let mut overflow = 0usize;
    let mut pending: Option<Frame> = None;
    let mut pos = 0;
    while let Some(rel) = src[pos..].find('<') {
        let start = pos + rel;
        let Some(end_rel) = src[start..].find('>') else { break };
        let end = start + end_rel;
        let tag = &src[start + 1..end];
        let tag_lower = &lower[start + 1..end];
        pos = end + 1;
        let closing = tag_lower.starts_with('/');
        let name = tag_lower
            .trim_start_matches('/')
            .split(|c: char| c.is_whitespace() || c == '/')
            .next()
            .unwrap_or_default();
        match (closing, name) {
            (false, "dl") => {
                if frames.len() >= MAX_DEPTH {
                    overflow += 1;
                } else {
                    frames.push(pending.take().unwrap_or(Frame { name: None, toolbar: false }));
                }
            }
            (true, "dl") => {
                if overflow > 0 {
                    overflow -= 1;
                } else {
                    frames.pop();
                }
                pending = None;
            }
            (false, "h3") => {
                let close = element_end(src, &lower, pos, "h3");
                let title = clean_title(&decode(&strip_tags(&src[pos..close])));
                let toolbar =
                    attribute(tag, "personal_toolbar_folder").is_some_and(|v| v.eq_ignore_ascii_case("true"));
                pending = Some(Frame { name: Some(title), toolbar });
                pos = close;
            }
            (false, "a") => {
                let close = element_end(src, &lower, pos, "a");
                let title = clean_title(&decode(&strip_tags(&src[pos..close])));
                pending = None;
                pos = close;
                let Some(url) = attribute(tag, "href").and_then(|h| clean_url(&decode(&h))) else { continue };
                let added = attribute(tag, "add_date")
                    .and_then(|d| d.trim().parse::<i64>().ok())
                    .map_or(0, guess_to_unix);
                out.push(ImportBookmark { url, title, folders: folders_of(&frames), added });
            }
            _ => {}
        }
    }
    out
}

/// The folder path of a link inside `frames`: relative to the bookmarks bar when it is in
/// one; links outside every folder go to "Imported" so they don't flood the bar.
fn folders_of(frames: &[Frame]) -> Vec<String> {
    let toolbar = frames.iter().position(|f| f.toolbar);
    let inside = &frames[toolbar.map_or(0, |t| t + 1)..];
    let path: Vec<String> = inside.iter().filter_map(|f| f.name.clone()).collect();
    match (toolbar, path.is_empty()) {
        (None, true) => vec!["Imported".to_owned()],
        _ => path,
    }
}

/// Titles are short: a closing tag is only looked for this far ahead.
const CLOSE_WINDOW: usize = 8192;

/// Where the element `name` (`a`, `h3`) whose text starts at `from` ends: at its closing tag,
/// or, when that is missing (sloppy exports), at the next list or link tag. Bounded, so that
/// a file full of unclosed elements cannot make the scan quadratic.
fn element_end(src: &str, lower: &str, from: usize, name: &str) -> usize {
    let limit = lower.len().min(from + CLOSE_WINDOW);
    let mut at = from;
    while let Some(rel) = lower.as_bytes()[at..limit].iter().position(|&b| b == b'<') {
        let j = at + rel;
        let tag = &lower[j + 1..];
        let closing = tag.starts_with('/');
        let tag_name: String =
            tag.trim_start_matches('/').chars().take_while(char::is_ascii_alphanumeric).collect();
        if (closing && tag_name == name)
            || matches!(tag_name.as_str(), "dt" | "dd" | "dl" | "h1" | "h3" | "a")
        {
            return j;
        }
        at = j + 1;
    }
    next_tag(src, from)
}

/// Where the next tag starts (or the end), for elements whose closing tag is missing.
fn next_tag(src: &str, from: usize) -> usize {
    src[from..].find('<').map_or(src.len(), |r| from + r)
}

/// The text of `fragment` without tags.
fn strip_tags(fragment: &str) -> String {
    let mut out = String::with_capacity(fragment.len());
    let mut in_tag = false;
    for c in fragment.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    out
}

/// The value of attribute `name` (lowercase) in the text of a start tag, quoted or not.
fn attribute(tag: &str, name: &str) -> Option<String> {
    let bytes = tag.as_bytes();
    let mut i = 0;
    // Skip the tag name.
    while i < bytes.len() && !bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    while i < bytes.len() {
        while i < bytes.len() && (bytes[i].is_ascii_whitespace() || bytes[i] == b'/') {
            i += 1;
        }
        let key_start = i;
        while i < bytes.len() && !bytes[i].is_ascii_whitespace() && bytes[i] != b'=' && bytes[i] != b'/' {
            i += 1;
        }
        let key = &tag[key_start..i];
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let mut value = None;
        if i < bytes.len() && bytes[i] == b'=' {
            i += 1;
            while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            if i < bytes.len() && (bytes[i] == b'"' || bytes[i] == b'\'') {
                let quote = bytes[i];
                let v_start = i + 1;
                i = v_start;
                while i < bytes.len() && bytes[i] != quote {
                    i += 1;
                }
                value = Some(&tag[v_start..i.min(bytes.len())]);
                i += 1;
            } else {
                let v_start = i;
                while i < bytes.len() && !bytes[i].is_ascii_whitespace() {
                    i += 1;
                }
                value = Some(&tag[v_start..i]);
            }
        }
        if key.eq_ignore_ascii_case(name) && !key.is_empty() {
            return Some(value.unwrap_or_default().to_owned());
        }
        if key.is_empty() {
            i += 1;
        }
    }
    None
}

/// Decodes the HTML entities exports use (named: amp, lt, gt, quot, apos, nbsp; numeric).
fn decode(text: &str) -> String {
    if !text.contains('&') {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        let decoded = rest.find(';').filter(|&semi| semi <= 10).and_then(|semi| {
            let entity = &rest[1..semi];
            let c = match entity {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                "nbsp" => Some(' '),
                _ => entity.strip_prefix('#').and_then(|n| {
                    let code = match n.strip_prefix(['x', 'X']) {
                        Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                        None => n.parse().ok()?,
                    };
                    char::from_u32(code)
                }),
            }?;
            Some((c, semi + 1))
        });
        match decoded {
            Some((c, used)) => {
                out.push(c);
                rest = &rest[used..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entities() {
        assert_eq!(
            decode("Tom &amp; Jerry &lt;3 &#233;&#x1F980; &bogus; &"),
            "Tom & Jerry <3 é\u{1F980} &bogus; &"
        );
        assert_eq!(decode("&#99999999999;"), "&#99999999999;");
    }

    #[test]
    fn attributes() {
        let tag = r#"A HREF="https://x.test/?a=1&amp;b=2" add_date=1700000000 ICON='data:x' lone"#;
        assert_eq!(attribute(tag, "href").as_deref(), Some("https://x.test/?a=1&amp;b=2"));
        assert_eq!(attribute(tag, "add_date").as_deref(), Some("1700000000"));
        assert_eq!(attribute(tag, "icon").as_deref(), Some("data:x"));
        assert_eq!(attribute(tag, "lone").as_deref(), Some(""));
        assert_eq!(attribute(tag, "missing"), None);
        // An unterminated quote must not loop or panic.
        assert_eq!(attribute(r#"A HREF="oops"#, "href").as_deref(), Some("oops"));
    }
}
