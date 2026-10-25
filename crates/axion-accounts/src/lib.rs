//! Local accounts. Each account is a separate profile, like Chrome's: its own browsing data
//! (`<profile base>/accounts/<id>/`: Chromium's cache and cookies, history, bookmarks,
//! passwords, favicons …) and its own settings (`<config base>/accounts/<id>/config.toml` and
//! `keymap.toml`). The list of accounts and the last one used are shared, in
//! `<config base>/accounts.toml`.
//!
//! The bases are `~/.config/axion` and `~/.local/share/axion/profile`, or `AXION_CONFIG_DIR`
//! and `AXION_PROFILE_DIR`. Data from before accounts existed (directly in the bases) is moved
//! into a first account on the first start.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use axion_config::{ACCENT_SWATCHES, Color};
use serde::{Deserialize, Serialize};

/// The shared list of accounts, in the configuration base.
pub const ACCOUNTS_FILE: &str = "accounts.toml";
/// Where the accounts' directories live, inside each base.
const ACCOUNTS_DIR: &str = "accounts";
/// The files of the configuration directory that belong to an account.
const CONFIG_FILES: [&str; 2] = ["config.toml", "keymap.toml"];
/// Chromium's marker of a profile that a running browser uses.
const IN_USE_MARKER: &str = "SingletonLock";
/// The first account's name when the system's user name is unknown.
const FALLBACK_NAME: &str = "Default";
/// Account names are cut to this many characters.
pub const MAX_NAME_CHARS: usize = 40;

#[derive(Debug, thiserror::Error)]
pub enum AccountsError {
    #[error("no home directory for Axion's data")]
    NoHome,
    #[error("reading {path}: {source}")]
    Read { path: PathBuf, source: std::io::Error },
    #[error("writing {path}: {source}")]
    Write { path: PathBuf, source: std::io::Error },
    #[error("{path} is not valid: {source}")]
    Parse { path: PathBuf, source: toml::de::Error },
    #[error("serializing the accounts: {0}")]
    Serialize(#[from] toml::ser::Error),
    #[error("there is no account {0:?}")]
    Unknown(String),
    #[error("the only account can't be deleted")]
    LastAccount,
}

pub type Result<T> = std::result::Result<T, AccountsError>;

/// One account.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Account {
    /// Short and stable; names the account's directories.
    pub id: String,
    pub name: String,
    /// The avatar's colour.
    pub color: Color,
    /// When it was created (Unix seconds).
    pub created: i64,
    /// Set when the account's data stayed where it was (a migration that couldn't move it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_dir: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_dir: Option<PathBuf>,
}

impl Account {
    /// The avatar's letters: the first letter of the first two words ("Henrik Grüter" → "HG").
    pub fn initials(&self) -> String {
        let initials: String = self
            .name
            .split_whitespace()
            .filter_map(|w| w.chars().next())
            .take(2)
            .flat_map(char::to_uppercase)
            .collect();
        if initials.is_empty() { "?".to_owned() } else { initials }
    }
}

/// Where an account's data and settings are.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccountPaths {
    /// Chromium's root cache path and Axion's databases.
    pub profile: PathBuf,
    /// `config.toml` and `keymap.toml`.
    pub config: PathBuf,
}

/// The base directories everything lives in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bases {
    pub config: PathBuf,
    pub profile: PathBuf,
}

impl Bases {
    /// `~/.config/axion` and `~/.local/share/axion/profile`, or `AXION_CONFIG_DIR` and
    /// `AXION_PROFILE_DIR` when set.
    pub fn discover() -> Result<Self> {
        let dirs = directories::ProjectDirs::from("", "", "axion");
        let config = match std::env::var_os("AXION_CONFIG_DIR") {
            Some(dir) => PathBuf::from(dir),
            None => dirs.as_ref().ok_or(AccountsError::NoHome)?.config_dir().to_path_buf(),
        };
        let profile = match std::env::var_os("AXION_PROFILE_DIR") {
            Some(dir) => PathBuf::from(dir),
            None => dirs.as_ref().ok_or(AccountsError::NoHome)?.data_dir().join("profile"),
        };
        Ok(Self { config, profile })
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct File {
    #[serde(skip_serializing_if = "Option::is_none")]
    last_used: Option<String>,
    /// Directories of deleted accounts, removed on the next start (when no browser uses them).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pending_removal: Vec<PathBuf>,
    #[serde(rename = "account")]
    accounts: Vec<Account>,
}

/// The accounts file and the accounts' directories.
#[derive(Debug)]
pub struct Accounts {
    bases: Bases,
    file: File,
    /// Nothing is written: the old profile is in use by another browser, so moving it into an
    /// account waits for the next start.
    transient: bool,
}

impl Accounts {
    /// Reads the accounts file; without one, creates the first account from the data that is
    /// there (see the crate docs).
    pub fn load(bases: Bases, now: i64) -> Result<Self> {
        let path = bases.config.join(ACCOUNTS_FILE);
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == ErrorKind::NotFound => return Self::migrate(bases, now),
            Err(source) => return Err(AccountsError::Read { path, source }),
        };
        let file: File = toml::from_str(&text).map_err(|source| AccountsError::Parse { path, source })?;
        let mut accounts = Self { bases, file, transient: false };
        accounts.purge_pending();
        if accounts.file.accounts.is_empty() {
            accounts.add(&system_user_name(), now)?;
        }
        Ok(accounts)
    }

    /// The first start with accounts: data and settings in the bases move into the first
    /// account's directories. Nothing is ever overwritten or lost: if moving fails, the account
    /// keeps using the old directories in place.
    fn migrate(bases: Bases, now: i64) -> Result<Self> {
        let id = new_id(&[]);
        // The avatar takes the accent colour chosen so far, if it is one of the swatches.
        let accent = axion_config::load_config(&axion_config::ConfigPaths::in_dir(bases.config.clone()))
            .ok()
            .map(|c| c.appearance.accent)
            .filter(|c| ACCENT_SWATCHES.contains(c));
        let mut account = Account {
            id: id.clone(),
            name: system_user_name(),
            color: accent.unwrap_or(ACCENT_SWATCHES[0]),
            created: now,
            profile_dir: None,
            config_dir: None,
        };
        let mut transient = false;
        let profile_entries = entries(&bases.profile, |name| name != ACCOUNTS_DIR);
        if !profile_entries.is_empty() {
            if bases.profile.join(IN_USE_MARKER).symlink_metadata().is_ok() {
                tracing::warn!(
                    dir = %bases.profile.display(),
                    "the profile is in use (or wasn't closed cleanly); moving it into an account on the next start"
                );
                account.profile_dir = Some(bases.profile.clone());
                account.config_dir = Some(bases.config.clone());
                transient = true;
            } else {
                let target = bases.profile.join(ACCOUNTS_DIR).join(&id);
                match move_entries(&bases.profile, &profile_entries, &target) {
                    Ok(()) => {
                        tracing::info!(to = %target.display(), "moved the browsing data into account {id}")
                    }
                    Err(e) => {
                        tracing::warn!("{e}; account {id} keeps its data in {}", bases.profile.display());
                        account.profile_dir = Some(bases.profile.clone());
                    }
                }
            }
        }
        if !transient {
            let config_entries = entries(&bases.config, |name| CONFIG_FILES.contains(&name));
            if !config_entries.is_empty() {
                let target = bases.config.join(ACCOUNTS_DIR).join(&id);
                match move_entries(&bases.config, &config_entries, &target) {
                    Ok(()) => tracing::info!(to = %target.display(), "moved the settings into account {id}"),
                    Err(e) => {
                        tracing::warn!("{e}; account {id} keeps its settings in {}", bases.config.display());
                        account.config_dir = Some(bases.config.clone());
                    }
                }
            }
        }
        let file = File { last_used: Some(id), pending_removal: Vec::new(), accounts: vec![account] };
        let accounts = Self { bases, file, transient };
        accounts.save()?;
        Ok(accounts)
    }

    /// Writes the accounts file atomically.
    pub fn save(&self) -> Result<()> {
        if self.transient {
            tracing::warn!("not saving the accounts until the old profile could be moved");
            return Ok(());
        }
        let path = self.bases.config.join(ACCOUNTS_FILE);
        let text = toml::to_string_pretty(&self.file)?;
        let write_err = |source| AccountsError::Write { path: path.clone(), source };
        std::fs::create_dir_all(&self.bases.config).map_err(write_err)?;
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, text).map_err(write_err)?;
        std::fs::rename(&tmp, &path).map_err(write_err)
    }

    pub fn bases(&self) -> &Bases {
        &self.bases
    }

    pub fn all(&self) -> &[Account] {
        &self.file.accounts
    }

    pub fn get(&self, id: &str) -> Option<&Account> {
        self.file.accounts.iter().find(|a| a.id == id)
    }

    pub fn last_used(&self) -> Option<&str> {
        self.file.last_used.as_deref()
    }

    /// Picks the account to run: `requested` (`--account=ID`) if it exists, else the last one
    /// used, else the first. It becomes the last used one.
    pub fn select(&mut self, requested: Option<&str>) -> Result<String> {
        if let Some(id) = requested.filter(|id| self.get(id).is_none()) {
            tracing::warn!("there is no account {id:?}; opening the last one used");
        }
        let id = requested
            .filter(|id| self.get(id).is_some())
            .or(self.file.last_used.as_deref().filter(|id| self.get(id).is_some()))
            .or(self.file.accounts.first().map(|a| a.id.as_str()))
            .ok_or_else(|| AccountsError::Unknown(requested.unwrap_or_default().to_owned()))?
            .to_owned();
        if self.file.last_used.as_deref() != Some(id.as_str()) {
            self.file.last_used = Some(id.clone());
            self.save()?;
        }
        Ok(id)
    }

    /// The directories of account `id`.
    pub fn paths(&self, id: &str) -> Result<AccountPaths> {
        let account = self.get(id).ok_or_else(|| AccountsError::Unknown(id.to_owned()))?;
        Ok(AccountPaths {
            profile: account.profile_dir.clone().unwrap_or_else(|| self.account_dir(&self.bases.profile, id)),
            config: account.config_dir.clone().unwrap_or_else(|| self.account_dir(&self.bases.config, id)),
        })
    }

    fn account_dir(&self, base: &Path, id: &str) -> PathBuf {
        base.join(ACCOUNTS_DIR).join(id)
    }

    /// Adds an account and returns its id. Its avatar gets the next colour not in use.
    pub fn add(&mut self, name: &str, now: i64) -> Result<String> {
        let ids: Vec<&str> = self.file.accounts.iter().map(|a| a.id.as_str()).collect();
        let id = new_id(&ids);
        let color = ACCENT_SWATCHES
            .iter()
            .copied()
            .find(|c| self.file.accounts.iter().all(|a| a.color != *c))
            .unwrap_or(ACCENT_SWATCHES[self.file.accounts.len() % ACCENT_SWATCHES.len()]);
        let name = clean_name(name).unwrap_or_else(|| format!("Account {}", self.file.accounts.len() + 1));
        self.file.accounts.push(Account {
            id: id.clone(),
            name,
            color,
            created: now,
            profile_dir: None,
            config_dir: None,
        });
        if self.file.last_used.is_none() {
            self.file.last_used = Some(id.clone());
        }
        self.save()?;
        Ok(id)
    }

    /// Renames an account (empty names are ignored). Returns whether it changed.
    pub fn rename(&mut self, id: &str, name: &str) -> Result<bool> {
        let Some(name) = clean_name(name) else { return Ok(false) };
        let account = self.account_mut(id)?;
        if account.name == name {
            return Ok(false);
        }
        account.name = name;
        self.save()?;
        Ok(true)
    }

    pub fn set_color(&mut self, id: &str, color: Color) -> Result<()> {
        self.account_mut(id)?.color = color;
        self.save()
    }

    fn account_mut(&mut self, id: &str) -> Result<&mut Account> {
        self.file
            .accounts
            .iter_mut()
            .find(|a| a.id == id)
            .ok_or_else(|| AccountsError::Unknown(id.to_owned()))
    }

    /// Deletes an account. Its directories are removed on the next start, once no browser
    /// uses them; data an account kept outside the accounts directories (see [`Self::migrate`])
    /// is left where it is.
    pub fn remove(&mut self, id: &str) -> Result<()> {
        if self.file.accounts.len() <= 1 {
            return Err(AccountsError::LastAccount);
        }
        let paths = self.paths(id)?;
        self.file.accounts.retain(|a| a.id != id);
        for dir in [paths.profile, paths.config] {
            if self.is_account_dir(&dir) {
                self.file.pending_removal.push(dir);
            } else {
                tracing::warn!(dir = %dir.display(), "account {id} deleted; its data there is kept");
            }
        }
        if self.file.last_used.as_deref() == Some(id) {
            self.file.last_used = self.file.accounts.first().map(|a| a.id.clone());
        }
        self.save()
    }

    /// Whether `dir` is one account's directory inside a base (and so safe to remove).
    fn is_account_dir(&self, dir: &Path) -> bool {
        [&self.bases.profile, &self.bases.config]
            .iter()
            .any(|base| dir.parent() == Some(base.join(ACCOUNTS_DIR).as_path()) && dir.file_name().is_some())
    }

    /// Removes the directories of accounts deleted earlier.
    fn purge_pending(&mut self) {
        if self.file.pending_removal.is_empty() {
            return;
        }
        let pending = std::mem::take(&mut self.file.pending_removal);
        for dir in pending {
            let in_use = self
                .file
                .accounts
                .iter()
                .any(|a| self.paths(&a.id).is_ok_and(|p| p.profile == dir || p.config == dir));
            if in_use || !self.is_account_dir(&dir) {
                tracing::warn!(dir = %dir.display(), "not removing a directory that isn't a deleted account's");
                continue;
            }
            match std::fs::remove_dir_all(&dir) {
                Ok(()) => tracing::info!(dir = %dir.display(), "removed a deleted account's data"),
                Err(e) if e.kind() == ErrorKind::NotFound => {}
                Err(e) => {
                    tracing::warn!(dir = %dir.display(), "removing a deleted account's data failed: {e}");
                    self.file.pending_removal.push(dir);
                }
            }
        }
        if let Err(e) = self.save() {
            tracing::warn!("{e}");
        }
    }
}

/// The names in `dir` that `keep` accepts (none if `dir` doesn't exist).
fn entries(dir: &Path, keep: impl Fn(&str) -> bool) -> Vec<String> {
    let Ok(read) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut names: Vec<String> =
        read.filter_map(|e| e.ok()?.file_name().into_string().ok()).filter(|n| keep(n)).collect();
    names.sort();
    names
}

/// Moves `names` from `from` into `to` (renames on one filesystem). On failure everything
/// moved so far goes back.
fn move_entries(from: &Path, names: &[String], to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    let mut moved = Vec::new();
    for name in names {
        let target = to.join(name);
        let result = if target.symlink_metadata().is_ok() {
            Err(std::io::Error::new(ErrorKind::AlreadyExists, format!("{} exists", target.display())))
        } else {
            std::fs::rename(from.join(name), &target)
        };
        if let Err(e) = result {
            for name in moved.iter().rev() {
                if let Err(back) = std::fs::rename(to.join(name), from.join(name)) {
                    tracing::error!("moving {name} back to {} failed: {back}", from.display());
                }
            }
            // Only empty directories go; anything left stays.
            let _ = std::fs::remove_dir(to);
            if let Some(parent) = to.parent() {
                let _ = std::fs::remove_dir(parent);
            }
            return Err(std::io::Error::new(e.kind(), format!("moving {name} into {}: {e}", to.display())));
        }
        moved.push(name.clone());
    }
    Ok(())
}

/// Trimmed and shortened; `None` if empty.
fn clean_name(name: &str) -> Option<String> {
    let name: String = name.trim().chars().take(MAX_NAME_CHARS).collect();
    let name = name.trim_end().to_owned();
    (!name.is_empty()).then_some(name)
}

/// The system's user name, capitalised ("henrik" → "Henrik"), or "Default".
fn system_user_name() -> String {
    let user = std::env::var("USER").or_else(|_| std::env::var("USERNAME")).unwrap_or_default();
    let mut chars = user.trim().chars();
    match chars.next() {
        Some(first) => clean_name(&first.to_uppercase().chain(chars).collect::<String>())
            .unwrap_or_else(|| FALLBACK_NAME.to_owned()),
        None => FALLBACK_NAME.to_owned(),
    }
}

/// Eight random hex digits, not one of `taken`.
fn new_id(taken: &[&str]) -> String {
    loop {
        let mut hasher = RandomState::new().build_hasher();
        let nanos =
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
        hasher.write_u128(nanos);
        hasher.write_u32(std::process::id());
        let id = format!("{:08x}", hasher.finish() as u32);
        if !taken.contains(&id.as_str()) {
            return id;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_bases(name: &str) -> Bases {
        let root = std::env::temp_dir().join(format!("axion-accounts-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        Bases { config: root.join("config"), profile: root.join("profile") }
    }

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn cleanup(bases: &Bases) {
        let _ = std::fs::remove_dir_all(bases.config.parent().unwrap());
    }

    #[test]
    fn migrates_existing_data_into_the_first_account() {
        let bases = temp_bases("migrate");
        write(&bases.profile.join("axion.sqlite"), "store");
        write(&bases.profile.join("vault.sqlite"), "vault");
        write(&bases.profile.join("Default/Cookies"), "cookies");
        write(&bases.profile.join("favicons/example.png"), "icon");
        write(&bases.config.join("config.toml"), "[appearance]\naccent = \"#E2457A\"\n");
        write(&bases.config.join("keymap.toml"), "");
        write(&bases.config.join("notes.txt"), "not ours");

        let accounts = Accounts::load(bases.clone(), 100).unwrap();
        assert_eq!(accounts.all().len(), 1);
        let account = &accounts.all()[0];
        assert!(!account.name.is_empty());
        assert_eq!(account.created, 100);
        assert_eq!(account.color, Color::rgb(0xE2457A), "the avatar takes the accent chosen so far");
        assert_eq!(accounts.last_used(), Some(account.id.as_str()));
        let paths = accounts.paths(&account.id).unwrap();
        assert_eq!(paths.profile, bases.profile.join("accounts").join(&account.id));
        assert_eq!(paths.config, bases.config.join("accounts").join(&account.id));
        assert_eq!(std::fs::read_to_string(paths.profile.join("axion.sqlite")).unwrap(), "store");
        assert_eq!(std::fs::read_to_string(paths.profile.join("Default/Cookies")).unwrap(), "cookies");
        assert!(
            paths.profile.join("vault.sqlite").exists()
                && paths.profile.join("favicons/example.png").exists()
        );
        assert!(paths.config.join("config.toml").exists() && paths.config.join("keymap.toml").exists());
        // Only the accounts directory is left in the profile base; unknown files stay.
        assert_eq!(entries(&bases.profile, |_| true), vec!["accounts".to_owned()]);
        assert!(bases.config.join("notes.txt").exists() && !bases.config.join("config.toml").exists());

        // The next start reads the file and moves nothing again.
        let id = account.id.clone();
        let mut again = Accounts::load(bases.clone(), 200).unwrap();
        assert_eq!(again.all().len(), 1);
        assert_eq!(again.select(None).unwrap(), id);
        cleanup(&bases);
    }

    #[test]
    fn fresh_start_creates_one_account() {
        let bases = temp_bases("fresh");
        let mut accounts = Accounts::load(bases.clone(), 1).unwrap();
        let id = accounts.select(None).unwrap();
        assert_eq!(accounts.paths(&id).unwrap().profile, bases.profile.join("accounts").join(&id));
        assert!(bases.config.join(ACCOUNTS_FILE).exists());
        cleanup(&bases);
    }

    #[test]
    fn keeps_data_in_place_when_it_cant_be_moved() {
        let bases = temp_bases("blocked");
        write(&bases.profile.join("axion.sqlite"), "store");
        // A file where the accounts directory should go makes the move fail.
        write(&bases.profile.join("accounts"), "in the way");
        write(&bases.config.join("config.toml"), "");

        let accounts = Accounts::load(bases.clone(), 1).unwrap();
        let id = accounts.all()[0].id.clone();
        let paths = accounts.paths(&id).unwrap();
        assert_eq!(paths.profile, bases.profile);
        assert_eq!(std::fs::read_to_string(bases.profile.join("axion.sqlite")).unwrap(), "store");
        // The settings could move; the fallback is remembered in the file.
        assert_eq!(paths.config, bases.config.join("accounts").join(&id));
        let reloaded = Accounts::load(bases.clone(), 2).unwrap();
        assert_eq!(reloaded.paths(&id).unwrap(), paths);
        cleanup(&bases);
    }

    #[test]
    fn waits_while_the_old_profile_is_in_use() {
        let bases = temp_bases("in-use");
        write(&bases.profile.join("axion.sqlite"), "store");
        write(&bases.profile.join(IN_USE_MARKER), "host-123");
        write(&bases.config.join("config.toml"), "");
        let accounts = Accounts::load(bases.clone(), 1).unwrap();
        let paths = accounts.paths(&accounts.all()[0].id).unwrap();
        assert_eq!(paths, AccountPaths { profile: bases.profile.clone(), config: bases.config.clone() });
        assert!(!bases.config.join(ACCOUNTS_FILE).exists());
        assert!(bases.profile.join("axion.sqlite").exists() && bases.config.join("config.toml").exists());
        cleanup(&bases);
    }

    #[test]
    fn add_rename_select_and_remove() {
        let bases = temp_bases("manage");
        let mut accounts = Accounts::load(bases.clone(), 1).unwrap();
        let first = accounts.select(None).unwrap();
        assert!(matches!(accounts.remove(&first), Err(AccountsError::LastAccount)));
        let second = accounts.add("  Work  ", 2).unwrap();
        assert_eq!(accounts.get(&second).unwrap().name, "Work");
        assert_ne!(accounts.get(&second).unwrap().color, accounts.get(&first).unwrap().color);
        assert!(accounts.rename(&second, "Work Stuff").unwrap());
        assert!(!accounts.rename(&second, "   ").unwrap());
        assert_eq!(accounts.get(&second).unwrap().initials(), "WS");
        assert_eq!(accounts.select(Some(&second)).unwrap(), second);
        assert_eq!(accounts.select(Some("nope")).unwrap(), second);

        let dir = accounts.paths(&second).unwrap().profile;
        write(&dir.join("axion.sqlite"), "work");
        accounts.remove(&second).unwrap();
        assert_eq!(accounts.last_used(), Some(first.as_str()));
        assert!(dir.exists(), "removed on the next start, not while in use");
        let reloaded = Accounts::load(bases.clone(), 3).unwrap();
        assert_eq!(reloaded.all().len(), 1);
        assert!(!dir.exists());
        assert!(reloaded.paths(&first).is_ok());
        cleanup(&bases);
    }

    #[test]
    fn initials_and_names() {
        let account = |name: &str| Account {
            id: "x".into(),
            name: name.into(),
            color: ACCENT_SWATCHES[0],
            created: 0,
            profile_dir: None,
            config_dir: None,
        };
        assert_eq!(account("henrik").initials(), "H");
        assert_eq!(account("Ada Lovelace Byron").initials(), "AL");
        assert_eq!(account("").initials(), "?");
        assert_eq!(clean_name(&"x".repeat(100)).map(|n| n.len()), Some(MAX_NAME_CHARS));
        assert_eq!(clean_name("  "), None);
    }
}
