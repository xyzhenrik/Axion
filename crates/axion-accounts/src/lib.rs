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
/// The stored profile picture's file name.
pub const AVATAR_FILE: &str = "avatar.png";
/// A picture may unpack to at most this many bytes while it is decoded (a small file can still
/// hold a gigantic image).
const AVATAR_DECODE_LIMIT: u64 = 512 * 1024 * 1024;
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
    #[error("The picture is larger than {max_mb} MB.")]
    AvatarTooLarge { max_mb: u32 },
    #[error("That file isn't a picture Axion can read.")]
    AvatarUnreadable,
    #[error("saving the picture failed: {0}")]
    AvatarSave(std::io::Error),
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
    /// The profile picture's file name inside the account's configuration directory (a copy
    /// Axion made; never a path to the user's own file). `None`: the coloured initials.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avatar: Option<String>,
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
            avatar: None,
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
            avatar: None,
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

    /// The stored profile picture of account `id`, if it has one and the file is there.
    pub fn avatar_path(&self, id: &str) -> Option<PathBuf> {
        let name = self.get(id)?.avatar.as_deref()?;
        if !valid_avatar_name(name) {
            return None;
        }
        let path = self.paths(id).ok()?.config.join(name);
        path.is_file().then_some(path)
    }

    /// Makes `picture` (already cropped and scaled, see [`load_avatar`]) the profile picture of
    /// account `id`: stored as PNG in the account's directory, replacing (and deleting) the
    /// old one.
    pub fn set_avatar(&mut self, id: &str, picture: &image::RgbaImage) -> Result<()> {
        let dir = self.paths(id)?.config;
        std::fs::create_dir_all(&dir).map_err(AccountsError::AvatarSave)?;
        let target = dir.join(AVATAR_FILE);
        let tmp = dir.join(format!("{AVATAR_FILE}.tmp"));
        let saved = picture
            .save_with_format(&tmp, image::ImageFormat::Png)
            .map_err(|e| AccountsError::AvatarSave(std::io::Error::other(e)))
            .and_then(|()| std::fs::rename(&tmp, &target).map_err(AccountsError::AvatarSave));
        if saved.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        saved?;
        let old = self.account_mut(id)?.avatar.replace(AVATAR_FILE.to_owned());
        // A file under another name (older versions, hand edits) is replaced too.
        if let Some(old) = old.filter(|o| o != AVATAR_FILE && valid_avatar_name(o)) {
            let _ = std::fs::remove_file(dir.join(old));
        }
        self.save()
    }

    /// Picks `source` (crop, scale, copy; see [`load_avatar`]) as the profile picture.
    pub fn set_avatar_from_file(&mut self, id: &str, source: &Path, size_px: u32, max_mb: u32) -> Result<()> {
        let picture = load_avatar(source, size_px, max_mb)?;
        self.set_avatar(id, &picture)
    }

    /// Back to the coloured initials; deletes the stored picture.
    pub fn remove_avatar(&mut self, id: &str) -> Result<()> {
        let dir = self.paths(id)?.config;
        if let Some(name) = self.account_mut(id)?.avatar.take() {
            remove_avatar_file(&dir, &name);
            self.save()?;
        }
        Ok(())
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
        // Normally inside the config directory that goes below; an account whose settings
        // stayed in the base directory must not leave its picture behind.
        if let Some(name) = self.get(id).and_then(|a| a.avatar.clone()) {
            remove_avatar_file(&paths.config, &name);
        }
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

/// Only plain file names of ours: no paths, no hidden files.
fn valid_avatar_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('.')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

fn remove_avatar_file(dir: &Path, name: &str) {
    if !valid_avatar_name(name) {
        return;
    }
    match std::fs::remove_file(dir.join(name)) {
        Ok(()) => {}
        Err(e) if e.kind() == ErrorKind::NotFound => {}
        Err(e) => tracing::warn!("removing the profile picture failed: {e}"),
    }
}

/// Reads a picture (PNG, JPEG, WebP, GIF: its first frame) for use as a profile picture: cut
/// to a centred square and scaled to `size_px` per side (small pictures are scaled up, so every
/// avatar has the same size). Files over `max_mb` are refused before they are decoded.
pub fn load_avatar(source: &Path, size_px: u32, max_mb: u32) -> Result<image::RgbaImage> {
    let max_bytes = u64::from(max_mb) * 1024 * 1024;
    let meta = std::fs::metadata(source)
        .map_err(|source_err| AccountsError::Read { path: source.to_path_buf(), source: source_err })?;
    if meta.len() > max_bytes {
        return Err(AccountsError::AvatarTooLarge { max_mb });
    }
    let mut reader = image::ImageReader::open(source)
        .and_then(|r| r.with_guessed_format())
        .map_err(|_| AccountsError::AvatarUnreadable)?;
    // The decoded size is limited too: a small file can still unpack to gigabytes.
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(AVATAR_DECODE_LIMIT);
    reader.limits(limits);
    let decoded = reader.decode().map_err(|_| AccountsError::AvatarUnreadable)?;
    Ok(square_avatar(&decoded, size_px))
}

/// The centred square of `picture`, scaled to `size_px` per side (at least 1).
pub fn square_avatar(picture: &image::DynamicImage, size_px: u32) -> image::RgbaImage {
    use image::GenericImageView;
    let size = size_px.max(1);
    let (w, h) = picture.dimensions();
    let side = w.min(h).max(1);
    let cropped = picture.crop_imm((w - side) / 2, (h - side) / 2, side, side);
    image::imageops::resize(&cropped.to_rgba8(), size, size, image::imageops::FilterType::Lanczos3)
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
    use image::GenericImageView;

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
            avatar: None,
        };
        assert_eq!(account("henrik").initials(), "H");
        assert_eq!(account("Ada Lovelace Byron").initials(), "AL");
        assert_eq!(account("").initials(), "?");
        assert_eq!(clean_name(&"x".repeat(100)).map(|n| n.len()), Some(MAX_NAME_CHARS));
        assert_eq!(clean_name("  "), None);
    }

    fn gradient(w: u32, h: u32) -> image::RgbaImage {
        image::RgbaImage::from_fn(w, h, |x, y| image::Rgba([(x % 256) as u8, (y % 256) as u8, 40, 255]))
    }

    fn two_accounts(name: &str) -> (Bases, Accounts, String, String) {
        let bases = temp_bases(name);
        let mut accounts = Accounts::load(bases.clone(), 1).unwrap();
        let first = accounts.all()[0].id.clone();
        let second = accounts.add("Second", 2).unwrap();
        (bases, accounts, first, second)
    }

    #[test]
    fn avatars_are_cropped_to_a_centred_square() {
        // 300x100: the centre square is x 100..200.
        let wide = image::RgbaImage::from_fn(300, 100, |x, _| {
            if (100..200).contains(&x) {
                image::Rgba([255, 0, 0, 255])
            } else {
                image::Rgba([0, 0, 255, 255])
            }
        });
        let square = square_avatar(&image::DynamicImage::ImageRgba8(wide), 64);
        assert_eq!(square.dimensions(), (64, 64));
        assert!(square.pixels().all(|p| p.0 == [255, 0, 0, 255]), "only the centre remains");
        let tall = image::DynamicImage::ImageRgba8(gradient(40, 400));
        assert_eq!(square_avatar(&tall, 256).dimensions(), (256, 256), "small pictures grow to size");
        assert_eq!(square_avatar(&tall, 0).dimensions(), (1, 1));
    }

    #[test]
    fn avatar_file_is_copied_and_survives_the_original() {
        let (bases, mut accounts, id, other) = two_accounts("avatar-copy");
        let source = bases.config.parent().unwrap().join("Downloads/me.jpg");
        std::fs::create_dir_all(source.parent().unwrap()).unwrap();
        image::DynamicImage::ImageRgba8(gradient(800, 500)).to_rgb8().save(&source).unwrap();
        assert!(accounts.avatar_path(&id).is_none());

        accounts.set_avatar_from_file(&id, &source, 128, 5).unwrap();
        let stored = accounts.avatar_path(&id).unwrap();
        assert_eq!(stored, accounts.paths(&id).unwrap().config.join(AVATAR_FILE));
        assert_eq!(image::open(&stored).unwrap().dimensions(), (128, 128));
        assert_eq!(accounts.get(&id).unwrap().avatar.as_deref(), Some(AVATAR_FILE));
        assert!(accounts.avatar_path(&other).is_none(), "other accounts keep their initials");

        std::fs::remove_file(&source).unwrap();
        assert!(accounts.avatar_path(&id).is_some(), "deleting the original keeps the avatar");
        // Reloading (a relaunch) keeps it too.
        let again = Accounts::load(bases.clone(), 3).unwrap();
        assert!(again.avatar_path(&id).is_some());
        cleanup(&bases);
    }

    #[test]
    fn avatar_limits_and_bad_files() {
        let (bases, mut accounts, id, _) = two_accounts("avatar-limits");
        let dir = bases.config.parent().unwrap();
        let big = dir.join("big.png");
        // Two MB of noise over a 1 MB limit is refused before decoding.
        std::fs::write(&big, vec![7u8; 2 * 1024 * 1024]).unwrap();
        assert!(matches!(
            accounts.set_avatar_from_file(&id, &big, 64, 1),
            Err(AccountsError::AvatarTooLarge { max_mb: 1 })
        ));
        assert!(matches!(
            accounts.set_avatar_from_file(&id, &big, 64, 5),
            Err(AccountsError::AvatarUnreadable)
        ));
        assert!(matches!(
            accounts.set_avatar_from_file(&id, &dir.join("missing.png"), 64, 5),
            Err(AccountsError::Read { .. })
        ));
        assert!(accounts.get(&id).unwrap().avatar.is_none());
        cleanup(&bases);
    }

    #[test]
    fn replacing_and_removing_delete_the_stored_file() {
        let (bases, mut accounts, id, _) = two_accounts("avatar-replace");
        let dir = accounts.paths(&id).unwrap().config;
        accounts.set_avatar(&id, &gradient(32, 32)).unwrap();
        // An avatar under an older name goes when the next one is stored.
        write(&dir.join("old-avatar.png"), "x");
        accounts.account_mut(&id).unwrap().avatar = Some("old-avatar.png".into());
        accounts.set_avatar(&id, &gradient(48, 48)).unwrap();
        assert!(!dir.join("old-avatar.png").exists());
        assert_eq!(image::open(dir.join(AVATAR_FILE)).unwrap().dimensions(), (48, 48));
        assert!(!dir.join("avatar.png.tmp").exists());

        accounts.remove_avatar(&id).unwrap();
        assert!(!dir.join(AVATAR_FILE).exists() && accounts.avatar_path(&id).is_none());
        assert!(accounts.get(&id).unwrap().avatar.is_none());
        accounts.remove_avatar(&id).unwrap();
        cleanup(&bases);
    }

    #[test]
    fn deleting_an_account_deletes_its_avatar() {
        let (bases, mut accounts, _, second) = two_accounts("avatar-delete");
        accounts.set_avatar(&second, &gradient(32, 32)).unwrap();
        let stored = accounts.avatar_path(&second).unwrap();
        accounts.remove(&second).unwrap();
        assert!(!stored.exists());
        cleanup(&bases);
    }

    #[test]
    fn avatar_in_accounts_file_round_trips() {
        let (bases, mut accounts, id, other) = two_accounts("avatar-toml");
        accounts.set_avatar(&id, &gradient(16, 16)).unwrap();
        let text = std::fs::read_to_string(bases.config.join(ACCOUNTS_FILE)).unwrap();
        assert_eq!(text.matches("avatar = ").count(), 1, "only the account with a picture has the key");
        assert!(text.contains("avatar = \"avatar.png\""));
        let again = Accounts::load(bases.clone(), 5).unwrap();
        assert_eq!(again.get(&id).unwrap().avatar.as_deref(), Some(AVATAR_FILE));
        assert_eq!(again.get(&other).unwrap().avatar, None);
        // Files from before profile pictures load as before.
        let legacy: File =
            toml::from_str("[[account]]\nid = \"a\"\nname = \"A\"\ncolor = \"#E2457A\"\ncreated = 1\n")
                .unwrap();
        assert_eq!(legacy.accounts[0].avatar, None);
        cleanup(&bases);
    }
}
