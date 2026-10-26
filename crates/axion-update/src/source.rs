//! Where releases are looked up, decided from `[updates]`.

use axion_config::Updates;

use crate::UpdateError;

/// A repository on a forge: `owner/name`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Repository {
    pub owner: String,
    pub name: String,
}

impl std::fmt::Display for Repository {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.owner, self.name)
    }
}

impl Repository {
    /// Whether a Git remote address (`https://host/owner/name.git`, `git@host:owner/name`,
    /// `ssh://git@host/owner/name`, a local path ending in `owner/name.git`) points at this
    /// repository: its last two path parts match (case-insensitively).
    pub fn matches_remote(&self, remote: &str) -> bool {
        let mut rest = remote.trim();
        if let Some((_, after)) = rest.split_once("://") {
            rest = after;
        } else if let Some((_, path)) = rest.split_once(':') {
            rest = path;
        }
        let rest = rest.trim_end_matches('/');
        let rest = rest.strip_suffix(".git").unwrap_or(rest);
        let mut parts = rest.rsplit('/');
        let (Some(name), Some(owner)) = (parts.next(), parts.next()) else { return false };
        owner.eq_ignore_ascii_case(&self.owner) && name.eq_ignore_ascii_case(&self.name)
    }
}

/// Accepts `owner/name`, a web address (`https://github.com/owner/name`, with or without
/// `.git` or a trailing path) or an SSH remote (`git@github.com:owner/name.git`).
pub fn parse_repository(text: &str) -> Result<Repository, UpdateError> {
    let invalid = || UpdateError::InvalidRepository(text.to_owned());
    let mut rest = text.trim();
    if rest.is_empty() {
        return Err(invalid());
    }
    if let Some((_, after)) = rest.split_once("://") {
        // Host, then the path.
        rest = after.split_once('/').map_or("", |(_, path)| path);
    } else if let Some((_, path)) = rest.split_once(':') {
        // scp-style: git@host:owner/name
        rest = path;
    }
    let mut parts = rest.trim_matches('/').split('/');
    let (Some(owner), Some(name)) = (parts.next(), parts.next()) else { return Err(invalid()) };
    let name = name.strip_suffix(".git").unwrap_or(name);
    let valid = |s: &str| {
        !s.is_empty()
            && s != "."
            && s != ".."
            && s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
    };
    if valid(owner) && valid(name) {
        Ok(Repository { owner: owner.to_owned(), name: name.to_owned() })
    } else {
        Err(invalid())
    }
}

/// What the updater asks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// GitHub's releases API (or a compatible one at `api_url`).
    GitHub { api_url: String, repository: Repository },
    /// A JSON document at this address.
    Manifest { url: String },
}

impl Source {
    /// The configured source; `Err(NoSource)` while neither a manifest nor a repository is set,
    /// which keeps the whole updater inert.
    pub fn from_config(updates: &Updates) -> Result<Self, UpdateError> {
        let manifest = updates.manifest_url.trim();
        if !manifest.is_empty() {
            let valid = url::Url::parse(manifest)
                .is_ok_and(|u| matches!(u.scheme(), "http" | "https") && u.host_str().is_some());
            return if valid {
                Ok(Source::Manifest { url: manifest.to_owned() })
            } else {
                Err(UpdateError::InvalidUrl(manifest.to_owned()))
            };
        }
        if updates.repository.trim().is_empty() {
            return Err(UpdateError::NoSource);
        }
        let repository = parse_repository(&updates.repository)?;
        let api_url = updates.api_url.trim().trim_end_matches('/').to_owned();
        if url::Url::parse(&api_url).is_err() {
            return Err(UpdateError::InvalidUrl(api_url));
        }
        Ok(Source::GitHub { api_url, repository })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo(s: &str) -> String {
        parse_repository(s).map(|r| r.to_string()).unwrap_or_else(|e| format!("err: {e}"))
    }

    #[test]
    fn repository_spellings() {
        assert_eq!(repo("henrik/axion"), "henrik/axion");
        assert_eq!(repo(" henrik/axion "), "henrik/axion");
        assert_eq!(repo("https://github.com/henrik/axion"), "henrik/axion");
        assert_eq!(repo("https://github.com/henrik/axion.git"), "henrik/axion");
        assert_eq!(repo("https://github.com/henrik/axion/releases"), "henrik/axion");
        assert_eq!(repo("git@github.com:henrik/axion.git"), "henrik/axion");
        assert_eq!(repo("ssh://git@codeberg.org/henrik/axion.git"), "henrik/axion");
        assert_eq!(repo("my.org/axion-browser_2"), "my.org/axion-browser_2");
    }

    #[test]
    fn invalid_repositories_are_refused() {
        for bad in [
            "",
            "   ",
            "axion",
            "a/",
            "/b",
            "a b/c",
            "a/../b",
            "../x",
            "a/b c",
            "https://github.com",
            "o/n?x=1",
        ] {
            assert!(repo(bad).starts_with("err"), "{bad:?} was accepted as {}", repo(bad));
        }
    }

    #[test]
    fn remote_matching() {
        let r = parse_repository("Henrik/Axion").expect("valid");
        assert!(r.matches_remote("https://github.com/henrik/axion.git"));
        assert!(r.matches_remote("git@github.com:henrik/axion"));
        assert!(!r.matches_remote("https://github.com/henrik/axion-fork.git"));
        assert!(!r.matches_remote("https://github.com/other/axion.git"));
        assert!(!r.matches_remote("/home/user/some/local/path"));
    }

    #[test]
    fn source_from_config() {
        let mut updates = Updates::default();
        assert!(matches!(Source::from_config(&updates), Err(UpdateError::NoSource)));
        updates.repository = "henrik/axion".into();
        assert_eq!(
            Source::from_config(&updates).expect("repository set"),
            Source::GitHub {
                api_url: "https://api.github.com".into(),
                repository: Repository { owner: "henrik".into(), name: "axion".into() }
            }
        );
        // A manifest wins.
        updates.manifest_url = "https://example.org/latest.json".into();
        assert_eq!(
            Source::from_config(&updates).expect("manifest set"),
            Source::Manifest { url: "https://example.org/latest.json".into() }
        );
        updates.manifest_url = "ftp://example.org/latest.json".into();
        assert!(matches!(Source::from_config(&updates), Err(UpdateError::InvalidUrl(_))));
        updates.manifest_url.clear();
        updates.repository = "nonsense".into();
        assert!(matches!(Source::from_config(&updates), Err(UpdateError::InvalidRepository(_))));
    }
}
