//! Finds the translation keys used in source text: the literal arguments of `tr(`, `tr_fmt!(`,
//! `tr_mark(` and `tr_n(` in Rust, `t(` in the start page's script. Used by the catalog tests,
//! the `extract` example and the app (which hands the start page its share of the catalog).

/// How a source file spells its translation calls.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dialect {
    Rust,
    Script,
    /// `data-t` elements (their text) and `data-t-attrs="a,b"` (those attributes), see `html_keys`.
    Html,
}

/// One key found in a source text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    pub key: String,
    /// 1-based line of the literal.
    pub line: usize,
}

/// (call name, number of leading string literals that are keys)
const RUST_CALLS: [(&str, usize); 4] = [("tr", 1), ("tr_fmt!", 1), ("tr_mark", 1), ("tr_n", 2)];
const SCRIPT_CALLS: [(&str, usize); 1] = [("t", 1)];

/// Every key literal in `text`, in order of appearance.
pub fn keys(text: &str, dialect: Dialect) -> Vec<Found> {
    let calls: &[(&str, usize)] = match dialect {
        Dialect::Rust => &RUST_CALLS,
        Dialect::Script => &SCRIPT_CALLS,
        Dialect::Html => return html_keys(text),
    };
    let bytes = text.as_bytes();
    let mut found = Vec::new();
    let mut line_starts_comment = false;
    let mut i = 0;
    while i < bytes.len() {
        if i == 0 || bytes[i - 1] == b'\n' {
            let rest = text[i..].trim_start_matches([' ', '\t']);
            line_starts_comment = rest.starts_with("//");
        }
        if line_starts_comment
            || !(bytes[i] as char).is_ascii_alphabetic()
            || (i > 0 && is_ident_or_dot(bytes[i - 1]))
        {
            i += 1;
            continue;
        }
        let Some((name, count)) = calls
            .iter()
            .find(|(name, _)| text[i..].starts_with(name) && bytes.get(i + name.len()) == Some(&b'('))
        else {
            i += 1;
            continue;
        };
        let mut at = i + name.len() + 1;
        let mut ok = true;
        let mut parsed = Vec::new();
        for n in 0..*count {
            at = skip_space(text, at);
            if n > 0 {
                if bytes.get(at) != Some(&b',') {
                    ok = false;
                    break;
                }
                at = skip_space(text, at + 1);
            }
            match literal(text, at) {
                Some((value, end)) => {
                    parsed.push((value, at));
                    at = end;
                }
                None => {
                    ok = false;
                    break;
                }
            }
        }
        if ok {
            for (key, pos) in parsed {
                found.push(Found { key, line: text[..pos].matches('\n').count() + 1 });
            }
        }
        i = at.max(i + 1);
    }
    found
}

/// Keys in an HTML page: the trimmed text of every element with a `data-t` attribute (up to the
/// next tag, entities decoded) and the values of the attributes named in `data-t-attrs="a,b"`.
fn html_keys(text: &str) -> Vec<Found> {
    let mut found = Vec::new();
    let mut at = 0;
    while let Some(open) = text[at..].find('<') {
        let start = at + open;
        let Some(len) = text[start..].find('>') else { break };
        let tag = &text[start..start + len];
        let after = start + len + 1;
        at = after;
        if tag.starts_with("<!--") || tag.starts_with("</") {
            continue;
        }
        let line = |pos: usize| text[..pos].matches('\n').count() + 1;
        if has_attribute(tag, "data-t") {
            let end = text[after..].find('<').map_or(text.len(), |e| after + e);
            let key = decode_entities(text[after..end].trim());
            if !key.is_empty() {
                found.push(Found { key, line: line(after) });
            }
        }
        if let Some(names) = attribute_value(tag, "data-t-attrs") {
            for name in names.split(',').map(str::trim) {
                if let Some(value) = attribute_value(tag, name) {
                    found.push(Found { key: decode_entities(value), line: line(start) });
                }
            }
        }
    }
    found
}

fn has_attribute(tag: &str, name: &str) -> bool {
    tag.match_indices(name).any(|(i, _)| {
        let before = tag[..i].chars().next_back();
        let after = tag[i + name.len()..].chars().next();
        before.is_some_and(char::is_whitespace) && after.is_none_or(|c| c.is_whitespace() || c == '/')
    })
}

fn attribute_value<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let needle = format!("{name}=\"");
    let (i, _) = tag.match_indices(&needle).find(|(i, _)| tag[..*i].ends_with(char::is_whitespace))?;
    let rest = &tag[i + needle.len()..];
    Some(&rest[..rest.find('"')?])
}

fn decode_entities(text: &str) -> String {
    text.replace("&hellip;", "\u{2026}")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
}

fn is_ident_or_dot(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'.'
}

fn skip_space(text: &str, mut at: usize) -> usize {
    let bytes = text.as_bytes();
    while at < bytes.len() && (bytes[at] as char).is_whitespace() {
        at += 1;
    }
    at
}

/// Decodes the double-quoted literal starting at `at`; returns it and the index after it.
fn literal(text: &str, at: usize) -> Option<(String, usize)> {
    let bytes = text.as_bytes();
    if bytes.get(at) != Some(&b'"') {
        return None;
    }
    let mut out = String::new();
    let mut chars = text[at + 1..].char_indices();
    while let Some((offset, c)) = chars.next() {
        match c {
            '"' => return Some((out, at + 1 + offset + 1)),
            '\\' => match chars.next()?.1 {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                '\\' => out.push('\\'),
                '"' => out.push('"'),
                '\'' => out.push('\''),
                '\n' => {
                    // Line continuation: the next line's leading whitespace is dropped.
                    let mut rest = chars.clone();
                    while let Some((_, w)) = rest.next() {
                        if w.is_whitespace() {
                            chars = rest.clone();
                        } else {
                            break;
                        }
                    }
                }
                'u' => {
                    let mut hex = String::new();
                    let mut closed = false;
                    for (_, h) in chars.by_ref() {
                        match h {
                            '{' => {}
                            '}' => {
                                closed = true;
                                break;
                            }
                            h => hex.push(h),
                        }
                    }
                    if !closed {
                        return None;
                    }
                    out.push(char::from_u32(u32::from_str_radix(&hex, 16).ok()?)?);
                }
                _ => return None,
            },
            c => out.push(c),
        }
    }
    None
}

/// The `{name}` placeholders in `text`, in order.
pub fn placeholders(text: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut rest = text;
    while let Some(open) = rest.find('{') {
        let tail = &rest[open + 1..];
        match tail.find('}') {
            Some(close)
                if !tail[..close].is_empty()
                    && tail[..close]
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_') =>
            {
                found.push(tail[..close].to_owned());
                rest = &tail[close + 1..];
            }
            _ => rest = tail,
        }
    }
    found
}

/// A key and where it is used first (`path:line`, relative to the workspace root).
pub type Used = std::collections::BTreeMap<String, String>;

/// Scans every `crates/*/src` tree under `root` (the workspace root) except this crate's own,
/// plus the start page script, for translation keys.
pub fn workspace_keys(root: &std::path::Path) -> Used {
    let mut used = Used::new();
    let mut files = Vec::new();
    if let Ok(crates) = std::fs::read_dir(root.join("crates")) {
        let mut crates: Vec<_> = crates.flatten().map(|e| e.path()).collect();
        crates.sort();
        for krate in crates {
            if krate.file_name().is_some_and(|n| n == "axion-i18n") {
                continue;
            }
            collect_files(&krate.join("src"), &mut files);
        }
    }
    for path in files {
        let dialect = match path.extension().and_then(|e| e.to_str()) {
            Some("rs") => Dialect::Rust,
            Some("js") => Dialect::Script,
            Some("html") => Dialect::Html,
            _ => continue,
        };
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        let relative = path.strip_prefix(root).unwrap_or(&path).display().to_string();
        for found in keys(&text, dialect) {
            used.entry(found.key).or_insert_with(|| format!("{relative}:{}", found.line));
        }
    }
    used
}

fn collect_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = entries.flatten().map(|e| e.path()).collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            collect_files(&path, out);
        } else {
            out.push(path);
        }
    }
}

/// `text` as a TOML basic string.
pub fn toml_quote(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rust(text: &str) -> Vec<String> {
        keys(text, Dialect::Rust).into_iter().map(|f| f.key).collect()
    }

    #[test]
    fn finds_calls() {
        let src = r#"
            a(tr("One"), tr_fmt!("Two {x}", x = 1));
            // tr("commented")
            let s = tr_n("{n} item", "{n} items", 3);
            const X: &str = tr_mark("Mark \"q\" \u{2026}");
            let t = str("nope"); foo.tr("nope"); fn tr(english: &str) {}
            tr("Con\
                tinued");
        "#;
        assert_eq!(
            rust(src),
            ["One", "Two {x}", "{n} item", "{n} items", "Mark \"q\" \u{2026}", "Continued"]
        );
    }

    #[test]
    fn script_calls() {
        let found = keys("x = t(\"Hello\") + not(\"no\") + t('single')", Dialect::Script);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].key, "Hello");
    }

    #[test]
    fn html_keys_found() {
        let html = "<h2 data-t>Start Page</h2>\n<input data-t-attrs=\"placeholder, aria-label\" placeholder=\"Add\" aria-label=\"A &amp; B\">\
                    <span data-t>trackers &amp; ads</span><p data-thing>no</p>";
        let keys: Vec<String> = keys(html, Dialect::Html).into_iter().map(|f| f.key).collect();
        assert_eq!(keys, ["Start Page", "Add", "A & B", "trackers & ads"]);
    }

    #[test]
    fn placeholder_names() {
        assert_eq!(placeholders("{a} of {b} {Not} {} {c_1}"), ["a", "b", "c_1"]);
    }
}
