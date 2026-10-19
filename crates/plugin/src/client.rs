//! PennyChest's side of the protocol: sending requests to a plugin and reading its answers,
//! over any reader and writer. The plugin host connects it to a running plugin's stdout
//! and stdin, and enforces the timeouts; tests connect it to a plugin in the same process.

use std::io::{self, BufRead, Read, Write};

use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::protocol::{
    Base64, DetectParams, ExportParams, Exported, FileParams, Handshake, InitializeParams, Ledger,
    MAX_MESSAGE_BYTES, PROTOCOL_VERSION, Parsed, Response, Row, StatementInfo, code, method,
};

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("the plugin couldn't be reached: {0}")]
    Io(#[from] io::Error),
    #[error("the plugin stopped without answering")]
    Closed,
    #[error("the plugin's answer is larger than {} MiB", MAX_MESSAGE_BYTES / 1024 / 1024)]
    TooLarge,
    /// The plugin broke the protocol: an answer that isn't JSON-RPC, or doesn't fit.
    #[error("the plugin sent an answer PennyChest can't read: {0}")]
    Malformed(String),
    /// The plugin speaks a protocol version PennyChest doesn't.
    #[error(
        "the plugin speaks protocol version {0}, but PennyChest speaks version {PROTOCOL_VERSION}"
    )]
    Version(u32),
    /// The plugin answered with an error. `code::FAILED` messages are meant for the user.
    #[error("{message}")]
    Plugin { code: i64, message: String },
}

impl ClientError {
    /// What to tell the user: the plugin's own message for a problem it reported, or what went
    /// wrong with the plugin itself.
    pub fn for_user(&self, plugin: &str) -> String {
        match self {
            Self::Plugin {
                code: code::FAILED,
                message,
            } => message.clone(),
            other => format!("The {plugin} plugin failed: {other}."),
        }
    }
}

/// One request to a plugin. Built separately from sending it, so a host can send a whole
/// run's requests at once (see [`Client::send`]).
#[derive(Debug, Clone)]
pub struct Request {
    method: &'static str,
    params: Value,
}

impl Request {
    pub fn initialize() -> Self {
        let params = InitializeParams {
            protocol_version: PROTOCOL_VERSION,
        };
        Self::new(method::INITIALIZE, to_value(&params))
    }

    pub fn detect(importer: &str, file: &[u8], filename: &str) -> Self {
        let params = DetectParams {
            importer: importer.into(),
            file: Base64(file.to_vec()),
            filename: filename.into(),
        };
        Self::new(method::DETECT, to_value(&params))
    }

    pub fn parse(importer: &str, file: &[u8]) -> Self {
        Self::new(method::PARSE, file_params(importer, file))
    }

    pub fn statement_info(importer: &str, file: &[u8]) -> Self {
        Self::new(method::STATEMENT_INFO, file_params(importer, file))
    }

    pub fn export(exporter: &str, ledger: &Ledger) -> Self {
        let params = ExportParams {
            exporter: exporter.into(),
            ledger: ledger.clone(),
        };
        Self::new(method::EXPORT, to_value(&params))
    }

    pub fn shutdown() -> Self {
        Self::new(method::SHUTDOWN, Value::Null)
    }

    /// Any method, such as one a newer protocol version adds.
    pub fn new(method: &'static str, params: Value) -> Self {
        Self { method, params }
    }
}

/// Talks to one plugin. Requests get ids 1, 2, 3… in the order they're sent, and answers are
/// expected in the same order.
///
/// Usually each request is sent and its answer read before the next ([`Client::call`] and the
/// typed methods). A host can also send every request first and read the answers afterwards
/// (`send`, then `receive` and the `receive_` methods), which is how PennyChest runs plugins:
/// all the requests on stdin, then the answers from stdout once the plugin has exited.
pub struct Client<R, W> {
    reader: R,
    writer: W,
    sent: u64,
    received: u64,
}

impl<R: BufRead, W: Write> Client<R, W> {
    /// `reader` is the plugin's stdout, `writer` its stdin.
    pub fn new(reader: R, writer: W) -> Self {
        Self {
            reader,
            writer,
            sent: 0,
            received: 0,
        }
    }

    /// One line from the plugin, refusing one longer than the protocol allows.
    fn read_line(&mut self) -> Result<Vec<u8>, ClientError> {
        let limit = u64::try_from(MAX_MESSAGE_BYTES).unwrap_or(u64::MAX) + 1;
        let mut line = Vec::new();
        let read = (&mut self.reader)
            .take(limit)
            .read_until(b'\n', &mut line)?;
        if read == 0 {
            return Err(ClientError::Closed);
        }
        if line.last() != Some(&b'\n') {
            return Err(if line.len() > MAX_MESSAGE_BYTES {
                ClientError::TooLarge
            } else {
                ClientError::Closed
            });
        }
        Ok(line)
    }

    /// What was written to the plugin: every request, when the writer is a buffer.
    pub fn into_writer(self) -> W {
        self.writer
    }

    /// Writes a request, without waiting for its answer.
    pub fn send(&mut self, request: Request) -> Result<(), ClientError> {
        let id = self.sent + 1;
        let message = json!({
            "jsonrpc": "2.0", "id": id, "method": request.method, "params": request.params
        });
        let mut line = serde_json::to_vec(&message).expect("JSON serialises");
        line.push(b'\n');
        if line.len() > MAX_MESSAGE_BYTES {
            return Err(ClientError::Plugin {
                code: code::FAILED,
                message: format!(
                    "The file is too large for a plugin: the limit is {} MiB.",
                    MAX_MESSAGE_BYTES / 1024 / 1024 * 3 / 4
                ),
            });
        }
        self.writer.write_all(&line)?;
        self.writer.flush()?;
        self.sent = id;
        Ok(())
    }

    /// Reads the answer to the oldest request not yet answered.
    pub fn receive<T: DeserializeOwned>(&mut self) -> Result<T, ClientError> {
        let id = self.received + 1;
        let line = self.read_line()?;
        self.received = id;
        let response: Response =
            serde_json::from_slice(&line).map_err(|e| ClientError::Malformed(e.to_string()))?;
        if response.id != json!(id) {
            return Err(ClientError::Malformed(format!(
                "an answer to request {} when {id} was asked",
                response.id
            )));
        }
        if let Some(error) = response.error {
            return Err(ClientError::Plugin {
                code: error.code,
                message: error.message,
            });
        }
        serde_json::from_value(response.result.unwrap_or(Value::Null))
            .map_err(|e| ClientError::Malformed(e.to_string()))
    }

    /// The answer to `initialize`.
    pub fn receive_handshake(&mut self) -> Result<Handshake, ClientError> {
        let handshake: Handshake = self.receive()?;
        if handshake.protocol_version != PROTOCOL_VERSION {
            return Err(ClientError::Version(handshake.protocol_version));
        }
        Ok(handshake)
    }

    /// The answer to `importer/parse`.
    pub fn receive_rows(&mut self) -> Result<Vec<Row>, ClientError> {
        Ok(self.receive::<Parsed>()?.rows)
    }

    /// The answer to `exporter/export`.
    pub fn receive_file(&mut self) -> Result<Vec<u8>, ClientError> {
        Ok(self.receive::<Exported>()?.file.0)
    }

    /// Sends a request and waits for its answer.
    pub fn call<T: DeserializeOwned>(&mut self, request: Request) -> Result<T, ClientError> {
        self.send(request)?;
        self.receive()
    }

    /// The handshake: who the plugin is and what it provides.
    pub fn initialize(&mut self) -> Result<Handshake, ClientError> {
        self.send(Request::initialize())?;
        self.receive_handshake()
    }

    pub fn detect(
        &mut self,
        importer: &str,
        file: &[u8],
        filename: &str,
    ) -> Result<bool, ClientError> {
        self.call(Request::detect(importer, file, filename))
    }

    pub fn parse(&mut self, importer: &str, file: &[u8]) -> Result<Vec<Row>, ClientError> {
        self.send(Request::parse(importer, file))?;
        self.receive_rows()
    }

    pub fn statement_info(
        &mut self,
        importer: &str,
        file: &[u8],
    ) -> Result<Option<StatementInfo>, ClientError> {
        self.call(Request::statement_info(importer, file))
    }

    pub fn export(&mut self, exporter: &str, ledger: &Ledger) -> Result<Vec<u8>, ClientError> {
        self.send(Request::export(exporter, ledger))?;
        self.receive_file()
    }

    /// Asks the plugin to exit.
    pub fn shutdown(&mut self) -> Result<(), ClientError> {
        self.call::<Value>(Request::shutdown()).map(|_| ())
    }
}

fn to_value(params: &impl serde::Serialize) -> Value {
    serde_json::to_value(params).expect("JSON serialises")
}

fn file_params(importer: &str, file: &[u8]) -> Value {
    to_value(&FileParams {
        importer: importer.into(),
        file: Base64(file.to_vec()),
    })
}
