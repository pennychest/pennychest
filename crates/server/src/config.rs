//! Settings read from the environment, each named with the `PENNYCHEST_` prefix.

use std::env;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Config {
    /// The SQLite database file.
    pub database_path: PathBuf,
    /// Where uploaded statements are kept.
    pub upload_dir: PathBuf,
    /// Where plugins installed from Settings are kept, next to the database.
    pub plugin_dir: PathBuf,
    /// False to offer only the official plugin repository in Settings.
    pub allow_plugin_repositories: bool,
    pub listen: SocketAddr,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("{name} must be {expected}, not {value:?}")]
    Invalid {
        name: &'static str,
        expected: &'static str,
        value: String,
    },
}

impl Config {
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_lookup(|name| env::var(name).ok())
    }

    /// Reads settings through `lookup`, so tests don't have to change the process environment.
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let path =
            |name: &str, default: &str| PathBuf::from(lookup(name).unwrap_or(default.into()));

        let allow_plugin_repositories = match lookup("PENNYCHEST_ALLOW_PLUGIN_REPOSITORIES") {
            None => true,
            Some(value) => parse_bool(&value).ok_or(ConfigError::Invalid {
                name: "PENNYCHEST_ALLOW_PLUGIN_REPOSITORIES",
                expected: "true or false",
                value,
            })?,
        };
        let host = match lookup("PENNYCHEST_HOST") {
            None => IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            Some(value) => value.parse().map_err(|_| ConfigError::Invalid {
                name: "PENNYCHEST_HOST",
                expected: "an IP address",
                value,
            })?,
        };
        let port = match lookup("PENNYCHEST_PORT") {
            None => 8000,
            Some(value) => value.parse().map_err(|_| ConfigError::Invalid {
                name: "PENNYCHEST_PORT",
                expected: "a port number",
                value,
            })?,
        };

        Ok(Self {
            database_path: path("PENNYCHEST_DATABASE_PATH", "/data/pennychest.db"),
            upload_dir: path("PENNYCHEST_UPLOAD_DIR", "/data/uploads"),
            plugin_dir: path("PENNYCHEST_PLUGIN_DIR", "/data/plugins"),
            allow_plugin_repositories,
            listen: SocketAddr::new(host, port),
        })
    }
}

/// Booleans in any of the usual spellings, ignoring case.
fn parse_bool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn config(vars: &[(&str, &str)]) -> Result<Config, ConfigError> {
        let vars: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        Config::from_lookup(|name| vars.get(name).cloned())
    }

    #[test]
    fn defaults() {
        let config = config(&[]).unwrap();
        assert_eq!(config.database_path, PathBuf::from("/data/pennychest.db"));
        assert_eq!(config.upload_dir, PathBuf::from("/data/uploads"));
        assert_eq!(config.plugin_dir, PathBuf::from("/data/plugins"));
        assert!(config.allow_plugin_repositories);
        assert_eq!(config.listen, "0.0.0.0:8000".parse().unwrap());
    }

    #[test]
    fn reads_overrides() {
        let config = config(&[
            ("PENNYCHEST_DATABASE_PATH", "/tmp/p.db"),
            ("PENNYCHEST_ALLOW_PLUGIN_REPOSITORIES", "False"),
            ("PENNYCHEST_HOST", "127.0.0.1"),
            ("PENNYCHEST_PORT", "9000"),
        ])
        .unwrap();
        assert_eq!(config.database_path, PathBuf::from("/tmp/p.db"));
        assert!(!config.allow_plugin_repositories);
        assert_eq!(config.listen, "127.0.0.1:9000".parse().unwrap());
    }

    #[test]
    fn rejects_bad_values() {
        assert!(config(&[("PENNYCHEST_PORT", "eighty")]).is_err());
        assert!(config(&[("PENNYCHEST_ALLOW_PLUGIN_REPOSITORIES", "maybe")]).is_err());
    }
}
