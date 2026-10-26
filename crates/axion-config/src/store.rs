use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::{Config, ConfigError, Keymap};

/// Where the configuration files live.
#[derive(Clone, Debug)]
pub struct ConfigPaths {
    pub dir: PathBuf,
    pub config: PathBuf,
    pub keymap: PathBuf,
}

impl ConfigPaths {
    /// The files in `dir` (an account's configuration directory, see `axion-accounts`).
    pub fn in_dir(dir: PathBuf) -> Self {
        Self { config: dir.join("config.toml"), keymap: dir.join("keymap.toml"), dir }
    }
}

/// Reads a file, treating "not found" as empty.
fn read_optional(path: &Path) -> Result<String, ConfigError> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(source) => Err(ConfigError::Read { path: path.to_owned(), source }),
    }
}

/// Loads `config.toml` over the defaults; a missing file yields the defaults. Partial
/// sections (even one key of `[glass.light]`) keep every other default.
pub fn load_config(paths: &ConfigPaths) -> Result<Config, ConfigError> {
    let text = read_optional(&paths.config)?;
    let parse_err = |source| ConfigError::Parse { path: paths.config.clone(), source };
    let user: toml::Table = toml::from_str(&text).map_err(parse_err)?;
    let mut merged = toml::Table::try_from(Config::default())?;
    merge(&mut merged, user);
    // Round-trip through text so errors (wrong types, unknown keys) carry the file's name.
    let mut config: Config = toml::from_str(&toml::to_string(&merged)?).map_err(parse_err)?;
    config.general.upgrade_homepage();
    Ok(config)
}

/// Deep-merges `over` into `base`: tables merge key by key, everything else replaces.
fn merge(base: &mut toml::Table, over: toml::Table) {
    for (key, value) in over {
        match (base.get_mut(&key), value) {
            (Some(toml::Value::Table(b)), toml::Value::Table(o)) => merge(b, o),
            (_, value) => {
                base.insert(key, value);
            }
        }
    }
}

/// Loads `keymap.toml` merged over the default shortcuts.
pub fn load_keymap(paths: &ConfigPaths) -> Result<Keymap, ConfigError> {
    let text = read_optional(&paths.keymap)?;
    let overrides: BTreeMap<String, Vec<String>> =
        toml::from_str(&text).map_err(|source| ConfigError::Parse { path: paths.keymap.clone(), source })?;
    Keymap::with_overrides(&overrides)
}

/// Writes `config.toml` atomically (temp file + rename), creating the directory if needed.
/// Only settings that differ from the defaults are written, so improved defaults in later
/// versions still reach the user.
pub fn save_config(paths: &ConfigPaths, config: &Config) -> Result<(), ConfigError> {
    let text = toml::to_string_pretty(&changed_settings(config)?)?;
    let write_err = |source| ConfigError::Write { path: paths.config.clone(), source };
    std::fs::create_dir_all(&paths.dir).map_err(write_err)?;
    let tmp = paths.config.with_extension("toml.tmp");
    std::fs::write(&tmp, text).map_err(write_err)?;
    std::fs::rename(&tmp, &paths.config).map_err(write_err)
}

/// `config` as a TOML table without the values that equal the defaults.
fn changed_settings(config: &Config) -> Result<toml::Table, ConfigError> {
    fn diff(value: &mut toml::Table, default: &toml::Table) {
        value.retain(|key, v| match (v, default.get(key)) {
            (toml::Value::Table(t), Some(toml::Value::Table(d))) => {
                diff(t, d);
                !t.is_empty()
            }
            (v, Some(d)) => v != d,
            (_, None) => true,
        });
    }
    let mut table = toml::Table::try_from(config)?;
    diff(&mut table, &toml::Table::try_from(Config::default())?);
    Ok(table)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("axion-config-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn missing_files_give_defaults() {
        let paths = ConfigPaths::in_dir(temp_dir("missing"));
        assert_eq!(load_config(&paths).unwrap(), Config::default());
        assert_eq!(load_keymap(&paths).unwrap(), Keymap::default());
    }

    #[test]
    fn empty_or_old_homepage_becomes_the_start_page() {
        for old in ["", "https://duckduckgo.com/"] {
            let paths = ConfigPaths::in_dir(temp_dir("homepage"));
            std::fs::create_dir_all(&paths.dir).unwrap();
            std::fs::write(&paths.config, format!("[general]\nhomepage = \"{old}\"\n")).unwrap();
            assert_eq!(load_config(&paths).unwrap().general.homepage, crate::START_PAGE_URL);
            std::fs::remove_dir_all(&paths.dir).unwrap();
        }
    }

    #[test]
    fn save_then_load() {
        let paths = ConfigPaths::in_dir(temp_dir("save"));
        let mut config = Config::default();
        config.layout.sidebar_width = 280.0;
        save_config(&paths, &config).unwrap();
        assert_eq!(load_config(&paths).unwrap(), config);
        std::fs::remove_dir_all(&paths.dir).unwrap();
    }

    #[test]
    fn the_playlist_volume_is_saved_and_loaded() {
        let paths = ConfigPaths::in_dir(temp_dir("playlist-volume"));
        let mut config = Config::default();
        assert_eq!(config.playlist.volume, 1.0);
        assert!(!config.playlist.muted);
        config.playlist.volume = 0.3;
        config.playlist.muted = true;
        save_config(&paths, &config).unwrap();
        let text = std::fs::read_to_string(&paths.config).unwrap();
        assert!(text.contains("volume") && text.contains("muted"), "{text}");
        let loaded = load_config(&paths).unwrap();
        assert!((loaded.playlist.volume - 0.3).abs() < 1e-6 && loaded.playlist.muted);
        std::fs::remove_dir_all(&paths.dir).unwrap();
    }

    #[test]
    fn saves_only_changes() {
        let paths = ConfigPaths::in_dir(temp_dir("diff"));
        let mut config = Config::default();
        config.appearance.accent = crate::Color::rgb(0xE2457A);
        config.glass.light.dispersion = 0.2;
        save_config(&paths, &config).unwrap();
        let text = std::fs::read_to_string(&paths.config).unwrap();
        assert!(text.contains("accent") && text.contains("dispersion"), "{text}");
        assert!(!text.contains("homepage") && !text.contains("bezel_width"), "{text}");
        assert_eq!(load_config(&paths).unwrap(), config);
        std::fs::remove_dir_all(&paths.dir).unwrap();
    }

    #[test]
    fn parse_errors_name_the_file() {
        let paths = ConfigPaths::in_dir(temp_dir("bad"));
        std::fs::create_dir_all(&paths.dir).unwrap();
        std::fs::write(&paths.config, "[layout]\nmargin = \"wide\"").unwrap();
        let err = load_config(&paths).unwrap_err().to_string();
        assert!(err.contains("config.toml"), "{err}");
        std::fs::remove_dir_all(&paths.dir).unwrap();
    }
}
