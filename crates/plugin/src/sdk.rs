//! Writing a plugin in Rust: implement [`Importer`] or [`Exporter`], add it to a [`Plugin`] and
//! call [`Plugin::serve`] from `main`. The SDK reads requests from stdin and writes answers to
//! stdout; anything the plugin wants logged goes to stderr (e.g. with `eprintln!`).
//!
//! ```no_run
//! use pennychest_plugin::{Importer, ImporterInfo, Plugin, Row};
//!
//! struct Tsv;
//!
//! impl Importer for Tsv {
//!     fn info(&self) -> ImporterInfo {
//!         ImporterInfo::new("tsv", "Tab-separated statements", &["tsv"])
//!     }
//!
//!     fn parse(&self, file: &[u8]) -> Result<Vec<Row>, String> {
//!         Ok(Vec::new())
//!     }
//! }
//!
//! fn main() {
//!     Plugin::new("tsv", env!("CARGO_PKG_VERSION")).importer(Tsv).serve();
//! }
//! ```

use std::io::{self, BufRead, Write};

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::protocol::{
    self, DetectParams, ExportParams, Exported, ExporterInfo, FileParams, Handshake, ImporterInfo,
    InitializeParams, Ledger, PROTOCOL_VERSION, Parsed, Request, Row, StatementInfo, code, method,
};

impl ImporterInfo {
    /// An importer that picks files by extension and doesn't read statement details: say
    /// otherwise with [`ImporterInfo::detects`] and [`ImporterInfo::with_statement_info`].
    pub fn new(name: &str, label: &str, file_types: &[&str]) -> Self {
        Self {
            name: name.into(),
            label: label.into(),
            description: String::new(),
            file_types: file_types.iter().map(|t| (*t).into()).collect(),
            detects: false,
            statement_info: false,
        }
    }

    #[must_use]
    pub fn describe(mut self, description: &str) -> Self {
        self.description = description.into();
        self
    }

    /// It implements [`Importer::detect`].
    #[must_use]
    pub fn detects(mut self) -> Self {
        self.detects = true;
        self
    }

    /// It implements [`Importer::statement_info`].
    #[must_use]
    pub fn with_statement_info(mut self) -> Self {
        self.statement_info = true;
        self
    }
}

impl ExporterInfo {
    pub fn new(name: &str, label: &str, file_extension: &str, media_type: &str) -> Self {
        Self {
            name: name.into(),
            label: label.into(),
            description: String::new(),
            file_extension: file_extension.into(),
            media_type: media_type.into(),
        }
    }

    #[must_use]
    pub fn describe(mut self, description: &str) -> Self {
        self.description = description.into();
        self
    }
}

/// Reads statements. Errors are shown to the user, so say what's wrong with the file.
pub trait Importer {
    fn info(&self) -> ImporterInfo;

    /// Whether this is a file it reads, from its content. Only called if
    /// [`ImporterInfo::detects`] says so.
    fn detect(&self, _file: &[u8], _filename: &str) -> bool {
        false
    }

    fn parse(&self, file: &[u8]) -> Result<Vec<Row>, String>;

    /// The account and summary details, for statements that carry them. Only called if
    /// [`ImporterInfo::with_statement_info`] says so.
    fn statement_info(&self, _file: &[u8]) -> Result<Option<StatementInfo>, String> {
        Ok(None)
    }
}

/// Turns the ledger into a file in another format.
pub trait Exporter {
    fn info(&self) -> ExporterInfo;

    fn export(&self, ledger: &Ledger) -> Result<Vec<u8>, String>;
}

/// A plugin program: what it provides, ready to [`serve`](Plugin::serve).
pub struct Plugin {
    name: String,
    version: String,
    importers: Vec<Box<dyn Importer>>,
    exporters: Vec<Box<dyn Exporter>>,
}

/// A request that can't be answered, as the error it's answered with.
struct Failure {
    code: i64,
    message: String,
}

impl Failure {
    fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

fn params<T: DeserializeOwned>(value: Value) -> Result<T, Failure> {
    serde_json::from_value(value)
        .map_err(|e| Failure::new(code::INVALID_PARAMS, format!("Invalid params: {e}")))
}

fn answer<T: Serialize>(value: &T) -> Result<Value, Failure> {
    serde_json::to_value(value).map_err(|e| Failure::new(code::INTERNAL_ERROR, e.to_string()))
}

impl Plugin {
    /// `name` and `version` are the plugin's, as its manifest gives them.
    pub fn new(name: &str, version: &str) -> Self {
        Self {
            name: name.into(),
            version: version.into(),
            importers: Vec::new(),
            exporters: Vec::new(),
        }
    }

    #[must_use]
    pub fn importer(mut self, importer: impl Importer + 'static) -> Self {
        self.importers.push(Box::new(importer));
        self
    }

    #[must_use]
    pub fn exporter(mut self, exporter: impl Exporter + 'static) -> Self {
        self.exporters.push(Box::new(exporter));
        self
    }

    fn handshake(&self) -> Handshake {
        Handshake {
            protocol_version: PROTOCOL_VERSION,
            name: self.name.clone(),
            version: self.version.clone(),
            importers: self.importers.iter().map(|i| i.info()).collect(),
            exporters: self.exporters.iter().map(|e| e.info()).collect(),
        }
    }

    fn find_importer(&self, name: &str) -> Result<&dyn Importer, Failure> {
        self.importers
            .iter()
            .find(|i| i.info().name == name)
            .map(AsRef::as_ref)
            .ok_or_else(|| Failure::new(code::INVALID_PARAMS, format!("No importer called {name}")))
    }

    fn find_exporter(&self, name: &str) -> Result<&dyn Exporter, Failure> {
        self.exporters
            .iter()
            .find(|e| e.info().name == name)
            .map(AsRef::as_ref)
            .ok_or_else(|| Failure::new(code::INVALID_PARAMS, format!("No exporter called {name}")))
    }

    fn call(&self, method: &str, params_: Value) -> Result<Value, Failure> {
        let failed = |message: String| Failure::new(code::FAILED, message);
        match method {
            method::INITIALIZE => {
                let asked: InitializeParams = params(params_)?;
                if asked.protocol_version < PROTOCOL_VERSION {
                    return Err(Failure::new(
                        code::FAILED,
                        format!(
                            "This plugin needs protocol version {PROTOCOL_VERSION}; update \
                             PennyChest to use it."
                        ),
                    ));
                }
                answer(&self.handshake())
            }
            method::DETECT => {
                let asked: DetectParams = params(params_)?;
                let importer = self.find_importer(&asked.importer)?;
                Ok(json!(importer.detect(&asked.file.0, &asked.filename)))
            }
            method::PARSE => {
                let asked: FileParams = params(params_)?;
                let rows = self.find_importer(&asked.importer)?.parse(&asked.file.0);
                answer(&Parsed {
                    rows: rows.map_err(failed)?,
                })
            }
            method::STATEMENT_INFO => {
                let asked: FileParams = params(params_)?;
                let info = self
                    .find_importer(&asked.importer)?
                    .statement_info(&asked.file.0);
                answer(&info.map_err(failed)?)
            }
            method::EXPORT => {
                let asked: ExportParams = params(params_)?;
                let file = self.find_exporter(&asked.exporter)?.export(&asked.ledger);
                answer(&Exported {
                    file: protocol::Base64(file.map_err(failed)?),
                })
            }
            method::SHUTDOWN => Ok(Value::Null),
            other => Err(Failure::new(
                code::METHOD_NOT_FOUND,
                format!("Method not found: {other}"),
            )),
        }
    }

    /// The answer to one line from PennyChest, and whether to stop after sending it.
    fn respond(&self, line: &str) -> (Value, bool) {
        let request: Request = match serde_json::from_str(line) {
            Ok(request) => request,
            Err(e) => {
                let failure = if serde_json::from_str::<Value>(line).is_err() {
                    Failure::new(code::PARSE_ERROR, format!("Parse error: {e}"))
                } else {
                    Failure::new(code::INVALID_REQUEST, format!("Invalid request: {e}"))
                };
                return (error(&Value::Null, &failure), false);
            }
        };
        let stop = request.method == method::SHUTDOWN;
        let reply = match self.call(&request.method, request.params) {
            Ok(result) => json!({"jsonrpc": "2.0", "id": request.id, "result": result}),
            Err(failure) => error(&request.id, &failure),
        };
        (reply, stop)
    }

    /// Answers requests from `input` on `output` until PennyChest asks it to shut down or
    /// closes `input`.
    pub fn serve_on(&self, input: impl BufRead, mut output: impl Write) -> io::Result<()> {
        for line in input.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let (reply, stop) = self.respond(&line);
            serde_json::to_writer(&mut output, &reply)?;
            output.write_all(b"\n")?;
            output.flush()?;
            if stop {
                break;
            }
        }
        Ok(())
    }

    /// Answers requests on stdin and stdout, as PennyChest runs plugins, then exits.
    pub fn serve(self) {
        let stdin = io::stdin().lock();
        let stdout = io::stdout().lock();
        if let Err(e) = self.serve_on(stdin, stdout) {
            eprintln!("{}: {e}", self.name);
            std::process::exit(1);
        }
    }
}

fn error(id: &Value, failure: &Failure) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {"code": failure.code, "message": failure.message},
    })
}
