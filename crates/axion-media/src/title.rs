//! What the fullscreen player says is playing: "Title - Channel - Site".

use crate::SessionInfo;

/// The three parts of the heading; any may be empty (the site of a local page, no channel).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Heading {
    pub title: String,
    pub channel: String,
    pub site: String,
}

impl Heading {
    pub fn is_empty(&self) -> bool {
        self.title.is_empty()
    }
}

/// Sites with a name that is not their capitalised domain.
const SITE_NAMES: &[(&str, &str)] = &[
    ("youtube.com", "YouTube"),
    ("youtu.be", "YouTube"),
    ("youtube-nocookie.com", "YouTube"),
    ("twitch.tv", "Twitch"),
    ("vimeo.com", "Vimeo"),
    ("dailymotion.com", "Dailymotion"),
    ("netflix.com", "Netflix"),
    ("disneyplus.com", "Disney+"),
    ("primevideo.com", "Prime Video"),
    ("crunchyroll.com", "Crunchyroll"),
    ("soundcloud.com", "SoundCloud"),
    ("spotify.com", "Spotify"),
    ("bandcamp.com", "Bandcamp"),
    ("arte.tv", "ARTE"),
    ("ardmediathek.de", "ARD Mediathek"),
    ("zdf.de", "ZDF"),
    ("github.com", "GitHub"),
    ("tiktok.com", "TikTok"),
    ("x.com", "X"),
    ("twitter.com", "X"),
];

/// Second-level labels of public suffixes made of two parts (`co.uk`, `com.au`).
const SECOND_LEVEL: &[&str] = &["co", "com", "org", "net", "gov", "edu", "ac"];

/// The site's name for a page address: "YouTube" for `www.youtube.com`, "Example" for
/// `video.example.org`; empty for anything but a web address.
pub fn site_name(url: &str) -> String {
    let Some(rest) = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://")) else {
        return String::new();
    };
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = host.rsplit('@').next().unwrap_or("");
    let host = host.split(':').next().unwrap_or("").trim_end_matches('.').to_ascii_lowercase();
    if host.is_empty() {
        return String::new();
    }
    for (domain, name) in SITE_NAMES {
        if host == *domain || host.ends_with(&format!(".{domain}")) {
            return (*name).to_owned();
        }
    }
    let labels: Vec<&str> = host.split('.').collect();
    // An address (192.168.0.1) or a bare name (localhost) is shown as it is.
    if labels.len() < 2 || labels.iter().all(|l| l.chars().all(|c| c.is_ascii_digit())) {
        return host;
    }
    let mut at = labels.len() - 2;
    if at > 0
        && labels[at].len() <= 3
        && SECOND_LEVEL.contains(&labels[at])
        && labels[labels.len() - 1].len() == 2
    {
        at -= 1;
    }
    capitalise(labels[at])
}

fn capitalise(word: &str) -> String {
    let mut chars = word.chars();
    chars.next().map(|first| first.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

/// A page title without the site's own tag ("Song - YouTube", "x | Twitch") and without the
/// unread count in front of it ("(3) Song").
pub fn clean_page_title(title: &str, site: &str) -> String {
    let mut t = title.trim();
    if let Some(rest) = t.strip_prefix('(')
        && let Some((count, after)) = rest.split_once(')')
        && !count.is_empty()
        && count.chars().all(|c| c.is_ascii_digit())
    {
        t = after.trim_start();
    }
    if !site.is_empty() {
        for sep in [" - ", " – ", " — ", " | ", " · ", " • "] {
            if let Some((head, tail)) = t.rsplit_once(sep)
                && tail.trim().eq_ignore_ascii_case(site)
                && !head.trim().is_empty()
            {
                return head.trim().to_owned();
            }
        }
    }
    t.to_owned()
}

/// A file name from a media address, without extension and with escapes undone
/// ("https://h/a/My%20Song.mp4?x" -> "My Song"); empty for `blob:` and the like.
pub fn file_title(src: &str) -> String {
    if !(src.starts_with("http://") || src.starts_with("https://") || src.starts_with("file://")) {
        return String::new();
    }
    let path = src.split(['?', '#']).next().unwrap_or("");
    let name = path.rsplit('/').next().unwrap_or("");
    let name = name.rsplit_once('.').map_or(name, |(stem, _)| stem);
    let mut bytes = Vec::with_capacity(name.len());
    let raw = name.as_bytes();
    let mut i = 0;
    while i < raw.len() {
        if raw[i] == b'%'
            && let Some(digits) = raw.get(i + 1..i + 3).and_then(|d| std::str::from_utf8(d).ok())
            && let Ok(hex) = u8::from_str_radix(digits, 16)
        {
            bytes.push(hex);
            i += 3;
        } else {
            bytes.push(raw[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&bytes).trim().to_owned()
}

/// The heading for what plays: the page's media session (title, artist = channel), else the
/// page's title, else the media file's name. The site comes from the page's address.
pub fn heading(session: Option<&SessionInfo>, page_title: &str, page_url: &str, media_src: &str) -> Heading {
    let site = site_name(page_url);
    let mut out = Heading { site, ..Heading::default() };
    if let Some(s) = session
        && !s.title.trim().is_empty()
    {
        out.title = s.title.trim().to_owned();
        out.channel = s.artist.trim().to_owned();
        return out;
    }
    out.title = clean_page_title(page_title, &out.site);
    if out.title.is_empty() || out.title == page_url {
        out.title = file_title(media_src);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn site_names() {
        assert_eq!(site_name("https://www.youtube.com/watch?v=1"), "YouTube");
        assert_eq!(site_name("https://m.youtube.com/"), "YouTube");
        assert_eq!(site_name("https://www.twitch.tv/x"), "Twitch");
        assert_eq!(site_name("https://player.vimeo.com/video/1"), "Vimeo");
        assert_eq!(site_name("https://video.example.org/a"), "Example");
        assert_eq!(site_name("https://www.bbc.co.uk/iplayer"), "Bbc");
        assert_eq!(site_name("http://localhost:8080/x"), "localhost");
        assert_eq!(site_name("axion://media/?s=1"), "");
        assert_eq!(site_name("file:///a.mp4"), "");
    }

    #[test]
    fn page_titles_lose_site_tags() {
        assert_eq!(clean_page_title("(3) Grief 82% - YouTube", "YouTube"), "Grief 82%");
        assert_eq!(clean_page_title("Stream | Twitch", "Twitch"), "Stream");
        assert_eq!(clean_page_title("A - B", "YouTube"), "A - B");
        assert_eq!(clean_page_title("YouTube", "YouTube"), "YouTube");
    }

    #[test]
    fn headings_prefer_the_media_session() {
        let session =
            SessionInfo { title: "Grief 82%, 80%...".into(), artist: "Zoink".into(), ..Default::default() };
        let h = heading(Some(&session), "other - YouTube", "https://www.youtube.com/watch?v=1", "blob:x");
        assert_eq!(
            (h.title.as_str(), h.channel.as_str(), h.site.as_str()),
            ("Grief 82%, 80%...", "Zoink", "YouTube")
        );
        let h = heading(None, "Clip - YouTube", "https://www.youtube.com/watch?v=1", "blob:x");
        assert_eq!((h.title.as_str(), h.channel.as_str()), ("Clip", ""));
        let h = heading(
            None,
            "",
            "https://cdn.example.com/v/My%20Movie.mp4",
            "https://cdn.example.com/v/My%20Movie.mp4",
        );
        assert_eq!((h.title.as_str(), h.site.as_str()), ("My Movie", "Example"));
        assert!(heading(None, "", "https://a.b/", "blob:x").is_empty());
    }
}
