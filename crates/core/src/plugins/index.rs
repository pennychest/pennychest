//! Plugin repositories: where Settings finds plugins to install.
//!
//! A repository is a link, usually to a GitHub repository such as a fork of
//! pennychest-plugins, with a `plugins.json` at its root:
//!
//! ```json
//! {
//!   "name": "PennyChest plugins",
//!   "plugins": [
//!     {
//!       "package": "hsbc",
//!       "name": "HSBC statements",
//!       "description": "Import HSBC PDF statements",
//!       "version": "1.0.0",
//!       "url": "https://github.com/…/releases/download/hsbc-v1.0.0/hsbc-1.0.0.tar.gz",
//!       "sha256": "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08"
//!     }
//!   ]
//! }
//! ```
//!
//! `package` is the plugin's name, as in its manifest. `url` is a `.tar.gz` of the plugin's
//! directory, and `sha256` its checksum, which is checked before anything is unpacked. A
//! WASI plugin has one download for every platform. A native one lists a download per
//! platform instead of `url` and `sha256`:
//!
//! ```json
//! "platforms": {
//!   "linux-x86_64": { "url": "https://…", "sha256": "…" },
//!   "linux-aarch64": { "url": "https://…", "sha256": "…" }
//! }
//! ```
//!
//! The official repository is always offered; others are added in Settings, unless
//! `PENNYCHEST_ALLOW_PLUGIN_REPOSITORIES` is off.

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::ai::http::{Request, Transport, TransportError};
use crate::settings::{Json, Setting};

pub const OFFICIAL: &str = "https://github.com/pennychest/pennychest-plugins";
pub const INDEX_FILE: &str = "plugins.json";
/// The repositories added in Settings.
pub const REPOSITORIES: Setting<Json<Vec<String>>> = Setting::new("plugins.repositories");

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Download {
    pub url: String,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct IndexPlugin {
    pub package: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub version: String,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    sha256: Option<String>,
    #[serde(default)]
    platforms: BTreeMap<String, Download>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Index {
    pub name: String,
    pub plugins: Vec<IndexPlugin>,
}

/// This machine, as `platforms` names it, e.g. "linux-x86_64".
pub fn platform() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

impl IndexPlugin {
    /// What to download on this machine, if there's anything.
    pub fn download(&self) -> Option<Download> {
        match (&self.url, &self.sha256) {
            (Some(url), Some(sha256)) => Some(Download {
                url: url.clone(),
                sha256: sha256.clone(),
            }),
            _ => self.platforms.get(&platform()).cloned(),
        }
    }

    fn check(&self) -> Result<(), String> {
        if !pennychest_plugin::manifest::valid_name(&self.package) {
            return Err(format!("{:?} isn't a plugin name", self.package));
        }
        let downloads: Vec<Download> = match (&self.url, &self.sha256) {
            (Some(url), Some(sha256)) if self.platforms.is_empty() => vec![Download {
                url: url.clone(),
                sha256: sha256.clone(),
            }],
            (None, None) if !self.platforms.is_empty() => {
                self.platforms.values().cloned().collect()
            }
            _ => {
                return Err(format!(
                    "{} must have either url and sha256, or platforms",
                    self.package
                ));
            }
        };
        for download in downloads {
            check_https(&download.url)?;
            if download.sha256.len() != 64
                || !download.sha256.chars().all(|c| c.is_ascii_hexdigit())
            {
                return Err(format!("{}: sha256 must be 64 hex digits", self.package));
            }
        }
        Ok(())
    }
}

impl Index {
    /// Reads a `plugins.json`, refusing one with a plugin that can't be installed safely.
    pub fn parse(text: &[u8]) -> Result<Self, String> {
        let index: Self = serde_json::from_slice(text).map_err(|e| e.to_string())?;
        for plugin in &index.plugins {
            plugin.check()?;
        }
        Ok(index)
    }

    pub fn get(&self, package: &str) -> Option<&IndexPlugin> {
        self.plugins.iter().find(|p| p.package == package)
    }
}

fn check_https(url: &str) -> Result<(), String> {
    let parsed = url::Url::parse(url).map_err(|_| format!("{url:?} isn't a link"))?;
    if parsed.scheme() != "https" || parsed.host_str().is_none() {
        return Err(format!("{url} must be an https:// link"));
    }
    Ok(())
}

/// A repository link in one form, so the same repository can't be added twice.
pub fn normalise(url: &str) -> Result<String, String> {
    let url = url.trim();
    let url = url.strip_suffix('/').unwrap_or(url);
    let url = url.strip_suffix(".git").unwrap_or(url);
    check_https(url).map_err(|_| "Enter an https:// link to the repository.".to_owned())?;
    Ok(url.to_owned())
}

/// Where a repository's `plugins.json` is: GitHub repositories are read through
/// raw.githubusercontent.com, and any other link is taken to be a folder or the file itself.
pub fn index_url(repository: &str) -> String {
    if let Some(rest) = repository.strip_prefix("https://github.com/") {
        let parts: Vec<&str> = rest.split('/').collect();
        if let [owner, repo] = parts[..] {
            return format!("https://raw.githubusercontent.com/{owner}/{repo}/HEAD/{INDEX_FILE}");
        }
    }
    if repository.ends_with(".json") {
        return repository.to_owned();
    }
    format!("{repository}/{INDEX_FILE}")
}

/// The most a download may be.
pub const MAX_DOWNLOAD: usize = 100 * 1024 * 1024;

/// Fetches a URL's body, refusing anything but a 200.
pub async fn fetch(http: &dyn Transport, url: &str, timeout: u64) -> Result<Vec<u8>, String> {
    let response = http
        .send(Request::get(url).timeout(timeout))
        .await
        .map_err(|e| match e {
            TransportError::Timeout => "it didn't answer in time".to_owned(),
            TransportError::Connect => "couldn't connect to it".to_owned(),
        })?;
    if response.status != 200 {
        return Err(format!("it answered {}", response.status));
    }
    let body = response
        .bytes()
        .await
        .map_err(|_| "the download was cut off".to_owned())?;
    if body.len() > MAX_DOWNLOAD {
        return Err(format!(
            "it's larger than {} MiB",
            MAX_DOWNLOAD / 1024 / 1024
        ));
    }
    Ok(body)
}

/// A repository's plugin list.
pub async fn fetch_index(http: &dyn Transport, repository: &str) -> Result<Index, String> {
    let body = fetch(http, &index_url(repository), 10).await?;
    Index::parse(&body)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const FORK: &str = "https://github.com/someone/pennychest-plugins";

    fn hsbc() -> serde_json::Value {
        json!({
            "package": "hsbc",
            "name": "HSBC statements",
            "description": "Import HSBC PDF statements",
            "version": "1.0.0",
            "url": "https://example.com/hsbc.tar.gz",
            "sha256": "a".repeat(64),
        })
    }

    fn index(plugin: serde_json::Value) -> Result<Index, String> {
        Index::parse(
            json!({"name": "Test", "plugins": [plugin]})
                .to_string()
                .as_bytes(),
        )
    }

    #[test]
    fn index_urls() {
        assert_eq!(
            index_url(OFFICIAL),
            "https://raw.githubusercontent.com/pennychest/pennychest-plugins/HEAD/plugins.json"
        );
        assert_eq!(
            index_url("https://example.com/plugins"),
            "https://example.com/plugins/plugins.json"
        );
        assert_eq!(
            index_url("https://example.com/my-index.json"),
            "https://example.com/my-index.json"
        );
    }

    #[test]
    fn normalises_repository_links() {
        assert_eq!(normalise(&format!(" {FORK}.git/ ")).unwrap(), FORK);
        assert!(normalise("http://github.com/someone/plugins").is_err());
        assert!(normalise("github.com/someone/plugins").is_err());
    }

    #[test]
    fn reads_an_index() {
        let index = index(hsbc()).unwrap();
        let download = index.get("hsbc").unwrap().download().unwrap();
        assert_eq!(download.url, "https://example.com/hsbc.tar.gz");
    }

    #[test]
    fn picks_this_platforms_download() {
        let mut plugin = hsbc();
        let object = plugin.as_object_mut().unwrap();
        object.remove("url");
        object.remove("sha256");
        object.insert(
            "platforms".into(),
            json!({
                platform(): {"url": "https://example.com/here.tar.gz", "sha256": "b".repeat(64)},
                "plan9-mips": {"url": "https://example.com/there.tar.gz", "sha256": "c".repeat(64)},
            }),
        );
        let index = index(plugin).unwrap();
        let download = index.plugins[0].download().unwrap();
        assert_eq!(download.url, "https://example.com/here.tar.gz");
    }

    #[test]
    fn refuses_plugins_it_cant_install_safely() {
        let cases = [
            ("url", json!("file:///etc/passwd"), "https://"),
            ("url", json!("http://example.com/hsbc.tar.gz"), "https://"),
            ("sha256", json!("abc"), "64 hex digits"),
            (
                "package",
                json!("--index-url=https://evil.example"),
                "isn't a plugin name",
            ),
            ("package", json!("../../etc"), "isn't a plugin name"),
        ];
        for (field, value, problem) in cases {
            let mut plugin = hsbc();
            plugin[field] = value;
            let error = index(plugin).unwrap_err();
            assert!(error.contains(problem), "{field}: {error}");
        }
        let mut plugin = hsbc();
        plugin.as_object_mut().unwrap().remove("sha256");
        assert!(index(plugin).unwrap_err().contains("either url and sha256"));
    }
}
