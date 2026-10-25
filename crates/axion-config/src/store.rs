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
    /// `$XDG_CONFIG_HOME/axion` (usually `~/.config/axion`), or `AXION_CONFIG_DIR` if set.
    pub fn discover() -> Result<Self, ConfigError> {
        let dir = match std::env::var_os("AXION_CONFIG_DIR") {
            Some(dir) => PathBuf::from(dir),
            None => directories::ProjectDirs::from("", "", "axion")
                .ok_or(ConfigError::NoHome)?
                .config_dir()
                .to_path_buf(),
        };
        Ok(Self::in_dir(dir))
    }

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

/// Loads `config.toml`; a missing file yields the defaults.
pub fn load_config(paths: &ConfigPaths) -> Result<Config, ConfigError> {
    let text = read_optional(&paths.config)?;
    toml::from_str(&text).map_err(|source| ConfigError::Parse { path: paths.config.clone(), source })
}

/// Loads `keymap.toml` merged over the default shortcuts.
pub fn load_keymap(paths: &ConfigPaths) -> Result<Keymap, ConfigError> {
    let text = read_optional(&paths.keymap)?;
    let overrides: BTreeMap<String, Vec<String>> =
        toml::from_str(&text).map_err(|source| ConfigError::Parse { path: paths.keymap.clone(), source })?;
    Keymap::with_overrides(&overrides)
}

/// Writes `config.toml` atomically (temp file + rename), creating the directory if needed.
pub fn save_config(paths: &ConfigPaths, config: &Config) -> Result<(), ConfigError> {
    let text = toml::to_string_pretty(config)?;
    let write_err = |source| ConfigError::Write { path: paths.config.clone(), source };
    std::fs::create_dir_all(&paths.dir).map_err(write_err)?;
    let tmp = paths.config.with_extension("toml.tmp");
    std::fs::write(&tmp, text).map_err(write_err)?;
    std::fs::rename(&tmp, &paths.config).map_err(write_err)
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
    fn save_then_load() {
        let paths = ConfigPaths::in_dir(temp_dir("save"));
        let mut config = Config::default();
        config.layout.sidebar_width = 280.0;
        save_config(&paths, &config).unwrap();
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
