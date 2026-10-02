//! The configuration file on disk: where the global file is, reading it, and
//! `diffr config set` ([`store`]). What the file says, and compiling it, is
//! [`diffr_core::config`]'s.
pub use diffr_core::config::*;
pub mod store;

use std::path::{Path, PathBuf};

/// The user's global file: `$XDG_CONFIG_HOME/diffr/config.toml`, falling
/// back to `~/.config/diffr/config.toml`.
pub fn global_path() -> Result<PathBuf, ConfigError> {
    let dir = match std::env::var_os("XDG_CONFIG_HOME") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => dirs::home_dir()
            .ok_or_else(|| ConfigError("no home directory for this user".into()))?
            .join(".config"),
    };
    Ok(dir.join("diffr").join("config.toml"))
}

/// Read the global file; a missing one is the defaults.
pub fn load() -> Result<Config, ConfigError> {
    load_from(&global_path()?)
}

/// The file at `path`; a missing file is the defaults.
fn load_from(path: &Path) -> Result<Config, ConfigError> {
    let source = match std::fs::read_to_string(path) {
        Ok(source) => source,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Config::default());
        }
        Err(error) => return Err(ConfigError(format!("{}: {error}", path.display()))),
    };
    Config::from_toml_in(&source, directory_of(path))
        .map_err(|error| ConfigError(format!("{}: {error}", path.display())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_file_overrides_defaults_key_by_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "[diff]\ngraph_limit = 5\nbyte_limit = 6\n[theme]\nname = 'mine'\n",
        )
        .unwrap();
        let config = load_from(&path).unwrap();
        assert_eq!(config.diff.graph_limit, 5);
        assert_eq!(config.diff.byte_limit, 6);
        assert_eq!(
            config.diff.parse_error_limit,
            crate::params::DEFAULT_PARSE_ERROR_LIMIT
        );
        assert_eq!(config.theme.name, "mine");
        assert_eq!(config.theme.path, None);
    }

    #[test]
    fn unknown_keys_name_their_path_and_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[diff]\ngraph_limit = 5\ntypo = 1\n").unwrap();
        let error = load_from(&path).unwrap_err().to_string();
        assert!(
            error.starts_with(&format!("{}: diff.typo: ", path.display())),
            "{error}"
        );
        std::fs::write(&path, "[diff]\ngraph_limit = 'many'\n").unwrap();
        let error = load_from(&path).unwrap_err().to_string();
        assert!(error.contains("diff.graph_limit: "), "{error}");
    }

    #[test]
    fn a_missing_file_is_the_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let config = load_from(&dir.path().join("absent.toml")).unwrap();
        assert_eq!(config.theme.name, Config::default().theme.name);
    }
}
