//! The plugin host: plugins installed in the plugin directory, behind the same
//! `Importer` and `Exporter` traits as everything built in, so imports and export don't know
//! the difference.
//!
//! Each plugin is a directory in `PENNYCHEST_PLUGIN_DIR` holding its `pennychest-plugin.toml`
//! and the files it names. The protocol is in `docs/plugins/protocol.md`, and how a plugin is
//! started and contained is in [`run`].
//!
//! - [`index`]: plugin repositories and their `plugins.json`, which Settings installs from.
//! - [`install`]: downloading, checking and unpacking a plugin.

pub mod index;
pub mod install;
mod run;

use std::io::{self, Cursor};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use pennychest_plugin::client::{Client, ClientError, Request};
use pennychest_plugin::manifest::{self, Manifest};
use pennychest_plugin::protocol::{
    self, CALL_TIMEOUT_SECS, HANDSHAKE_TIMEOUT_SECS, Handshake, Ledger,
};
use serde::Serialize;

use crate::Money;
use crate::export::{Exporter, Exporters};
use crate::imports::{ImportedRow, Importer, Importers, StatementInfo};
pub use run::RunError;

/// Something that went wrong running a plugin.
#[derive(Debug, thiserror::Error)]
pub enum PluginError {
    #[error(transparent)]
    Run(#[from] RunError),
    #[error(transparent)]
    Protocol(#[from] ClientError),
}

impl PluginError {
    /// What to tell the user: the plugin's own message for a problem with the file, or what
    /// went wrong with the plugin.
    pub fn for_user(&self, plugin: &str) -> String {
        match self {
            Self::Protocol(error) => error.for_user(plugin),
            Self::Run(error) => format!("The {plugin} plugin failed: {error}."),
        }
    }
}

/// How long a plugin has: to start and say what it provides, and for each piece of work.
#[derive(Debug, Clone, Copy)]
pub struct Timeouts {
    pub handshake: Duration,
    pub call: Duration,
}

impl Default for Timeouts {
    fn default() -> Self {
        Self {
            handshake: Duration::from_secs(HANDSHAKE_TIMEOUT_SECS),
            call: Duration::from_secs(CALL_TIMEOUT_SECS),
        }
    }
}

/// A plugin's answers, read back after its run.
type Answers = Client<Cursor<Vec<u8>>, io::Sink>;

/// An installed plugin, ready to run.
pub struct Plugin {
    pub manifest: Manifest,
    pub dir: PathBuf,
    /// What it said it provides when it was loaded.
    pub handshake: Handshake,
    runner: run::Runner,
    timeouts: Timeouts,
}

impl Plugin {
    /// Reads the plugin in `dir`, starts it once to ask what it provides, and checks it
    /// agrees with its manifest.
    pub fn load(dir: &Path, timeouts: Timeouts) -> Result<Self, String> {
        let text = std::fs::read_to_string(dir.join(manifest::FILE_NAME))
            .map_err(|e| format!("{}: {e}", manifest::FILE_NAME))?;
        let manifest = Manifest::parse(&text)?;
        let runner = run::Runner::new(dir, &manifest.run)?;
        let mut plugin = Self {
            manifest,
            dir: dir.to_path_buf(),
            handshake: Handshake {
                protocol_version: protocol::PROTOCOL_VERSION,
                name: String::new(),
                version: String::new(),
                importers: Vec::new(),
                exporters: Vec::new(),
            },
            runner,
            timeouts,
        };
        let handshake = plugin
            .start(&[], timeouts.handshake)
            .map_err(|e| e.for_user(&plugin.manifest.name))?
            .1;
        if handshake.name != plugin.manifest.name {
            return Err(format!(
                "it calls itself {:?}, but its manifest says {:?}",
                handshake.name, plugin.manifest.name
            ));
        }
        plugin.handshake = handshake;
        Ok(plugin)
    }

    pub fn name(&self) -> &str {
        &self.manifest.name
    }

    /// Runs the plugin with the handshake, `requests` and `shutdown`, and returns a client
    /// positioned to read the answers to `requests`.
    fn start(
        &self,
        requests: &[Request],
        timeout: Duration,
    ) -> Result<(Answers, Handshake), PluginError> {
        let mut script = Client::new(io::empty(), Vec::new());
        script.send(Request::initialize())?;
        for request in requests {
            script.send(request.clone())?;
        }
        script.send(Request::shutdown())?;
        let output = self
            .runner
            .run(self.name(), script.into_writer(), timeout)?;
        let mut answers = Client::new(Cursor::new(output), io::sink());
        let handshake = answers.receive_handshake()?;
        Ok((answers, handshake))
    }

    /// Runs one piece of work and returns the client to read its answer from.
    fn work(&self, request: Request) -> Result<Answers, PluginError> {
        let timeout = self.timeouts.handshake + self.timeouts.call;
        Ok(self.start(&[request], timeout)?.0)
    }
}

/// One of a plugin's importers.
struct PluginImporter {
    plugin: Arc<Plugin>,
    info: protocol::ImporterInfo,
}

impl Importer for PluginImporter {
    fn name(&self) -> &str {
        &self.info.name
    }

    fn label(&self) -> &str {
        &self.info.label
    }

    fn description(&self) -> &str {
        &self.info.description
    }

    fn file_types(&self) -> Vec<String> {
        self.info.file_types.clone()
    }

    fn detect(&self, content: &[u8], filename: &str) -> bool {
        if !self.info.detects {
            return false;
        }
        let request = Request::detect(&self.info.name, content, filename);
        match self
            .plugin
            .work(request)
            .and_then(|mut a| Ok(a.receive::<bool>()?))
        {
            Ok(detected) => detected,
            Err(error) => {
                tracing::warn!("{}", error.for_user(self.plugin.name()));
                false
            }
        }
    }

    fn parse(&self, content: &[u8]) -> Result<Vec<ImportedRow>, String> {
        let plugin = self.plugin.name();
        let rows = self
            .plugin
            .work(Request::parse(&self.info.name, content))
            .and_then(|mut answers| Ok(answers.receive_rows()?))
            .map_err(|e| e.for_user(plugin))?;
        rows.into_iter()
            .enumerate()
            .map(|(i, row)| {
                let amount = Money::from_decimal(row.amount).map_err(|_| {
                    let line = row.line_number.map_or(i as i64 + 1, |n| n);
                    format!(
                        "The {plugin} plugin read an amount PennyChest can't keep exactly on \
                         line {line}: {}.",
                        row.amount
                    )
                })?;
                Ok(ImportedRow {
                    date: row.date,
                    description: row.description,
                    amount,
                    raw_content: row.raw_content,
                    line_number: row.line_number,
                    metadata: row.metadata,
                })
            })
            .collect()
    }

    fn statement_info(&self, content: &[u8]) -> Option<StatementInfo> {
        if !self.info.statement_info {
            return None;
        }
        let request = Request::statement_info(&self.info.name, content);
        let answer = self
            .plugin
            .work(request)
            .and_then(|mut a| Ok(a.receive::<Option<protocol::StatementInfo>>()?));
        match answer {
            Ok(info) => info.map(|info| StatementInfo {
                statement_type: info.statement_type,
                bank_identifier: info.bank_identifier,
                label: info.label,
                name_hint: info.name_hint,
                period_start: info.period_start,
                period_end: info.period_end,
                opening_balance: info.opening_balance,
                closing_balance: info.closing_balance,
            }),
            Err(error) => {
                tracing::warn!("{}", error.for_user(self.plugin.name()));
                None
            }
        }
    }
}

/// One of a plugin's exporters.
struct PluginExporter {
    plugin: Arc<Plugin>,
    info: protocol::ExporterInfo,
}

impl Exporter for PluginExporter {
    fn name(&self) -> &str {
        &self.info.name
    }

    fn label(&self) -> &str {
        &self.info.label
    }

    fn description(&self) -> &str {
        &self.info.description
    }

    fn file_extension(&self) -> &str {
        &self.info.file_extension
    }

    fn media_type(&self) -> &str {
        &self.info.media_type
    }

    fn export(&self, ledger: &Ledger) -> Result<Vec<u8>, String> {
        self.plugin
            .work(Request::export(&self.info.name, ledger))
            .and_then(|mut answers| Ok(answers.receive_file()?))
            .map_err(|e| e.for_user(self.plugin.name()))
    }
}

/// A plugin as Settings lists it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct InstalledInfo {
    pub name: String,
    pub version: String,
    pub description: String,
}

/// A plugin directory that couldn't be loaded, and why.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Broken {
    pub name: String,
    pub problem: String,
}

/// The plugins installed in the plugin directory.
pub struct Plugins {
    dir: PathBuf,
    timeouts: Timeouts,
    loaded: RwLock<Loaded>,
    /// Installs and removals, one at a time.
    changing: std::sync::Mutex<()>,
}

#[derive(Default)]
struct Loaded {
    plugins: Vec<Arc<Plugin>>,
    broken: Vec<Broken>,
}

impl Plugins {
    /// Loads every plugin in `dir`. One that can't be loaded is logged and listed as broken,
    /// rather than stopping the others.
    pub fn load(dir: &Path) -> Self {
        Self::load_with(dir, Timeouts::default())
    }

    /// As `load`, giving plugins other time limits.
    pub fn load_with(dir: &Path, timeouts: Timeouts) -> Self {
        let plugins = Self {
            dir: dir.to_path_buf(),
            timeouts,
            loaded: RwLock::default(),
            changing: std::sync::Mutex::new(()),
        };
        plugins.reload();
        plugins
    }

    /// No plugins, for tests and for when there's no plugin directory.
    pub fn none() -> Self {
        Self {
            dir: PathBuf::new(),
            timeouts: Timeouts::default(),
            loaded: RwLock::default(),
            changing: std::sync::Mutex::new(()),
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Reads the plugin directory again.
    fn reload(&self) {
        let mut loaded = Loaded::default();
        let entries = match std::fs::read_dir(&self.dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                *self.loaded.write().expect("not poisoned") = loaded;
                return;
            }
            Err(error) => {
                tracing::warn!(
                    "Couldn't read the plugin directory {}: {error}",
                    self.dir.display()
                );
                return;
            }
        };
        let mut dirs: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            // Other things can share the directory: only folders with a manifest are plugins.
            .filter(|path| path.join(manifest::FILE_NAME).is_file())
            .collect();
        dirs.sort();
        for dir in dirs {
            let name = dir
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            match Plugin::load(&dir, self.timeouts) {
                Ok(plugin) => {
                    tracing::info!(
                        "Loaded the {} plugin, version {}",
                        plugin.name(),
                        plugin.manifest.version
                    );
                    loaded.plugins.push(Arc::new(plugin));
                }
                Err(problem) => {
                    tracing::warn!("Couldn't load the {name} plugin: {problem}");
                    loaded.broken.push(Broken { name, problem });
                }
            }
        }
        *self.loaded.write().expect("not poisoned") = loaded;
    }

    /// The installed plugins.
    pub fn installed(&self) -> Vec<InstalledInfo> {
        self.loaded
            .read()
            .expect("not poisoned")
            .plugins
            .iter()
            .map(|p| InstalledInfo {
                name: p.manifest.name.clone(),
                version: p.manifest.version.clone(),
                description: p.manifest.description.clone(),
            })
            .collect()
    }

    /// Plugin directories that couldn't be loaded.
    pub fn broken(&self) -> Vec<Broken> {
        self.loaded.read().expect("not poisoned").broken.clone()
    }

    /// `built_in` followed by every importer the plugins provide. An importer whose name is
    /// already taken is left out.
    pub fn importers(&self, built_in: &Importers) -> Importers {
        let mut all = built_in.all();
        for plugin in &self.loaded.read().expect("not poisoned").plugins {
            for info in &plugin.handshake.importers {
                if all.iter().any(|i| i.name() == info.name) {
                    tracing::warn!(
                        "The {} plugin's importer {:?} has a name already in use",
                        plugin.name(),
                        info.name
                    );
                    continue;
                }
                all.push(Arc::new(PluginImporter {
                    plugin: Arc::clone(plugin),
                    info: info.clone(),
                }));
            }
        }
        Importers::new(all)
    }

    /// `built_in` followed by every exporter the plugins provide.
    pub fn exporters(&self, built_in: &Exporters) -> Exporters {
        let mut all = built_in.all();
        for plugin in &self.loaded.read().expect("not poisoned").plugins {
            for info in &plugin.handshake.exporters {
                if all.iter().any(|e| e.name() == info.name) {
                    continue;
                }
                all.push(Arc::new(PluginExporter {
                    plugin: Arc::clone(plugin),
                    info: info.clone(),
                }));
            }
        }
        Exporters::new(all)
    }
}
