//! What a provider reports: one release with its notes and downloadable files.

use semver::Version;

/// A downloadable file of a release.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Asset {
    pub name: String,
    pub url: String,
    pub size: Option<u64>,
    /// A SHA-256 the manifest states for this file (lower-case hex).
    pub sha256: Option<String>,
}

/// A published version.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub version: Version,
    /// The tag as published (`v0.2.0`).
    pub tag: String,
    pub name: String,
    /// The release notes as the host stores them (usually Markdown); see [`plain_notes`].
    pub notes: String,
    /// The release's page in a browser.
    pub page_url: String,
    pub prerelease: bool,
    pub assets: Vec<Asset>,
}

impl Release {
    /// The `SHA256SUMS` file of the release, if it has one.
    pub fn checksums_asset(&self) -> Option<&Asset> {
        self.assets.iter().find(|a| {
            let name = a.name.to_ascii_lowercase();
            name == "sha256sums" || name == "sha256sums.txt" || name == "sha256sum.txt"
        })
    }

    /// The AppImage for `arch` (`std::env::consts::ARCH`): the one named for it, or the only one.
    pub fn appimage_asset(&self, arch: &str) -> Option<&Asset> {
        let images: Vec<&Asset> =
            self.assets.iter().filter(|a| a.name.to_ascii_lowercase().ends_with(".appimage")).collect();
        let arch = arch.to_ascii_lowercase();
        let alias = match arch.as_str() {
            "x86_64" => "amd64",
            "aarch64" => "arm64",
            other => other,
        };
        images
            .iter()
            .find(|a| {
                let name = a.name.to_ascii_lowercase();
                name.contains(&arch) || name.contains(alias)
            })
            .copied()
            .or_else(|| (images.len() == 1).then(|| images[0]))
    }

    pub fn title(&self) -> &str {
        if self.name.trim().is_empty() { &self.tag } else { &self.name }
    }
}

/// Release notes as plain text: Markdown markers, HTML tags and link syntax removed, bullets
/// as "•", blank-line runs squeezed, capped at `max_chars`.
pub fn plain_notes(markdown: &str, max_chars: usize) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut in_code = false;
    for raw in markdown.replace("\r\n", "\n").lines() {
        let line = raw.trim_end();
        if line.trim_start().starts_with("```") {
            in_code = !in_code;
            continue;
        }
        let mut text = line.trim_start().to_owned();
        if !in_code {
            // Headings, quotes, rules.
            text = text.trim_start_matches('#').trim_start().to_owned();
            if let Some(rest) = text.strip_prefix('>') {
                text = rest.trim_start().to_owned();
            }
            if !text.is_empty()
                && text.chars().all(|c| matches!(c, '-' | '*' | '_' | '=' | ' '))
                && text.len() >= 3
            {
                text.clear();
            }
            let indent = line.len() - line.trim_start().len();
            for marker in ["- ", "* ", "+ "] {
                if let Some(rest) = text.strip_prefix(marker) {
                    text = format!("{}• {}", " ".repeat((indent / 2).min(4)), rest.trim_start());
                    break;
                }
            }
            text = strip_inline(&text);
        }
        if text.trim().is_empty() && out.last().is_none_or(|l| l.trim().is_empty()) {
            continue;
        }
        out.push(text);
    }
    while out.last().is_some_and(|l| l.trim().is_empty()) {
        out.pop();
    }
    let joined = out.join("\n");
    if joined.chars().count() > max_chars {
        let cut: String = joined.chars().take(max_chars).collect();
        format!("{}…", cut.trim_end())
    } else {
        joined
    }
}

/// Inline Markdown: `**bold**`, `_em_`, `` `code` ``, `[text](url)`, `![alt](url)`, `<tags>`.
fn strip_inline(line: &str) -> String {
    let chars: Vec<char> = line.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '!' if chars.get(i + 1) == Some(&'[') => {
                i += 1;
                continue;
            }
            '[' => {
                // [text](url) keeps the text.
                if let Some(close) = chars[i..].iter().position(|&c| c == ']').map(|p| i + p)
                    && chars.get(close + 1) == Some(&'(')
                    && let Some(end) = chars[close..].iter().position(|&c| c == ')').map(|p| close + p)
                {
                    out.extend(&chars[i + 1..close]);
                    i = end + 1;
                    continue;
                }
                out.push('[');
            }
            '<' => {
                // A tag: drop up to '>' when one follows closely.
                if let Some(end) = chars[i..].iter().take(80).position(|&c| c == '>') {
                    i += end + 1;
                    continue;
                }
                out.push('<');
            }
            '*' | '`' => {}
            '_' if i == 0
                || !chars[i - 1].is_alphanumeric()
                || chars.get(i + 1).is_none_or(|c| !c.is_alphanumeric()) => {}
            c => out.push(c),
        }
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asset(name: &str) -> Asset {
        Asset { name: name.into(), url: format!("https://x/{name}"), size: None, sha256: None }
    }

    fn release(assets: &[&str]) -> Release {
        Release {
            version: Version::new(1, 0, 0),
            tag: "v1.0.0".into(),
            name: String::new(),
            notes: String::new(),
            page_url: String::new(),
            prerelease: false,
            assets: assets.iter().map(|n| asset(n)).collect(),
        }
    }

    #[test]
    fn finds_the_checksums_file() {
        assert_eq!(
            release(&["a.AppImage", "SHA256SUMS"]).checksums_asset().map(|a| a.name.as_str()),
            Some("SHA256SUMS")
        );
        assert!(release(&["a.AppImage"]).checksums_asset().is_none());
    }

    #[test]
    fn picks_the_appimage_for_the_architecture() {
        let r = release(&["Axion-1.0-aarch64.AppImage", "Axion-1.0-x86_64.AppImage", "notes.txt"]);
        assert_eq!(r.appimage_asset("x86_64").map(|a| a.name.as_str()), Some("Axion-1.0-x86_64.AppImage"));
        assert_eq!(r.appimage_asset("aarch64").map(|a| a.name.as_str()), Some("Axion-1.0-aarch64.AppImage"));
        // A single image is taken whatever its name says; several unrelated ones are ambiguous.
        assert!(release(&["Axion.AppImage"]).appimage_asset("x86_64").is_some());
        assert!(release(&["a.AppImage", "b.AppImage"]).appimage_asset("x86_64").is_none());
        assert!(release(&["axion.tar.gz"]).appimage_asset("x86_64").is_none());
    }

    #[test]
    fn markdown_becomes_plain_text() {
        let md = "## What's new\n\n- **Faster** tabs, see [the docs](https://x/y)\n* `Shields` fixes\n\n\n> note\n\n```\ncode _kept_\n```\n---\nDone <br> now";
        assert_eq!(
            plain_notes(md, 1000),
            "What's new\n\n• Faster tabs, see the docs\n• Shields fixes\n\nnote\n\ncode _kept_\n\nDone  now"
        );
    }

    #[test]
    fn plain_notes_caps_the_length() {
        let long = "word ".repeat(100);
        let out = plain_notes(&long, 20);
        assert!(out.chars().count() <= 21);
        assert!(out.ends_with('…'));
        assert_eq!(plain_notes("", 20), "");
    }
}
