//! Running a plugin. Each piece of work is one run: PennyChest starts the plugin, writes every
//! request on its stdin at once (the handshake, the work, then `shutdown`), and reads the
//! answers from its stdout when it exits. The protocol's answers come in order, so it's the
//! same conversation as one request at a time, without keeping plugins running between uses.
//!
//! - **WASI modules** run in wasmtime with nothing but stdin, stdout and stderr: no files, no
//!   network, no environment, and at most 1 GiB of memory.
//! - **Native programs** run as the user PennyChest runs as, with an empty environment apart
//!   from `PATH`, in the plugin's own directory. Nothing stops one reading files or the network,
//!   which is why Settings warns about installing plugins from repositories you don't trust.
//!
//! Either way, a run that goes on too long is stopped, and stderr goes to PennyChest's log.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::LazyLock;
use std::sync::mpsc;
use std::time::Duration;

use pennychest_plugin::manifest::Run;
use pennychest_plugin::protocol::MAX_MESSAGE_BYTES;
use wasmtime::{Config, Engine, Linker, Module, Store, StoreLimits, StoreLimitsBuilder};
use wasmtime_wasi::p1::{self, WasiP1Ctx};
use wasmtime_wasi::p2::pipe::{MemoryInputPipe, MemoryOutputPipe};
use wasmtime_wasi::{I32Exit, WasiCtxBuilder};

/// How much a run may write to stdout: a few answers, each up to the protocol's limit.
const MAX_OUTPUT: usize = 3 * MAX_MESSAGE_BYTES;
/// How much of a run's stderr is kept for the log.
const MAX_LOG: usize = 1024 * 1024;
/// A WASI plugin's memory limit.
const MAX_MEMORY: usize = 1024 * 1024 * 1024;
/// How often wasmtime checks whether a WASI plugin has run out of time.
const TICK: Duration = Duration::from_millis(100);

#[derive(Debug, thiserror::Error)]
pub enum RunError {
    #[error("it didn't answer within {0} seconds")]
    Timeout(u64),
    #[error("it couldn't be started: {0}")]
    Start(String),
    #[error("it stopped with an error: {0}")]
    Crashed(String),
}

/// One engine for every WASI plugin, with a clock that lets runs be stopped.
static ENGINE: LazyLock<Engine> = LazyLock::new(|| {
    let mut config = Config::new();
    config.epoch_interruption(true);
    let engine = Engine::new(&config).expect("wasmtime's configuration is valid");
    let ticking = engine.weak();
    std::thread::Builder::new()
        .name("plugin-clock".into())
        .spawn(move || {
            while let Some(engine) = ticking.upgrade() {
                engine.increment_epoch();
                drop(engine);
                std::thread::sleep(TICK);
            }
        })
        .expect("the plugin clock starts");
    engine
});

struct WasiState {
    wasi: WasiP1Ctx,
    limits: StoreLimits,
}

/// How to start one installed plugin.
pub enum Runner {
    Wasm {
        module: Module,
    },
    Native {
        dir: PathBuf,
        program: PathBuf,
        args: Vec<String>,
    },
}

impl Runner {
    /// Gets a plugin in `dir` ready to run as its manifest says. A WASI module is compiled
    /// here, once, so each run only has to start it.
    pub fn new(dir: &Path, run: &Run) -> Result<Self, String> {
        match run {
            Run::Wasm(file) => {
                let path = inside(dir, file)?;
                let module = Module::from_file(&ENGINE, &path).map_err(|e| {
                    format!("{file} isn't a WebAssembly module PennyChest can run: {e}")
                })?;
                Ok(Self::Wasm { module })
            }
            Run::Command(command) => {
                let (program, args) = command.split_first().ok_or("the command is empty")?;
                Ok(Self::Native {
                    dir: dir.to_path_buf(),
                    program: inside(dir, program)?,
                    args: args.to_vec(),
                })
            }
        }
    }

    /// Runs the plugin with `input` on stdin and returns its stdout.
    pub fn run(&self, name: &str, input: Vec<u8>, timeout: Duration) -> Result<Vec<u8>, RunError> {
        match self {
            // On a thread of its own: wasmtime's WASI drives its I/O with a Tokio runtime of
            // its own, which can't start on a thread already running one.
            Self::Wasm { module } => std::thread::scope(|scope| {
                scope
                    .spawn(|| run_wasm(module, name, input, timeout))
                    .join()
                    .unwrap_or_else(|_| Err(RunError::Crashed("PennyChest's runner failed".into())))
            }),
            Self::Native { dir, program, args } => {
                run_native(dir, program, args, name, input, timeout)
            }
        }
    }
}

/// A file named in a manifest, which must be inside the plugin's directory.
fn inside(dir: &Path, file: &str) -> Result<PathBuf, String> {
    let path = dir.join(file);
    let resolved = path
        .canonicalize()
        .map_err(|_| format!("{file} isn't in the plugin"))?;
    let dir = dir.canonicalize().map_err(|e| e.to_string())?;
    if !resolved.starts_with(&dir) || !resolved.is_file() {
        return Err(format!("{file} isn't a file in the plugin"));
    }
    Ok(resolved)
}

fn log_stderr(name: &str, text: &[u8]) {
    for line in String::from_utf8_lossy(text).lines() {
        if !line.trim().is_empty() {
            tracing::info!(plugin = name, "{line}");
        }
    }
}

fn run_wasm(
    module: &Module,
    name: &str,
    input: Vec<u8>,
    timeout: Duration,
) -> Result<Vec<u8>, RunError> {
    let stdout = MemoryOutputPipe::new(MAX_OUTPUT);
    let stderr = MemoryOutputPipe::new(MAX_LOG);
    let wasi = WasiCtxBuilder::new()
        .stdin(MemoryInputPipe::new(input))
        .stdout(stdout.clone())
        .stderr(stderr.clone())
        .args(&[name])
        .build_p1();
    let limits = StoreLimitsBuilder::new().memory_size(MAX_MEMORY).build();
    let mut store = Store::new(&ENGINE, WasiState { wasi, limits });
    store.limiter(|state| &mut state.limits);
    let ticks = (timeout.as_millis() / TICK.as_millis()).max(1);
    store.set_epoch_deadline(u64::try_from(ticks).unwrap_or(u64::MAX));
    store.epoch_deadline_trap();

    let mut linker: Linker<WasiState> = Linker::new(&ENGINE);
    p1::add_to_linker_sync(&mut linker, |state| &mut state.wasi)
        .map_err(|e| RunError::Start(e.to_string()))?;
    let instance = linker
        .instantiate(&mut store, module)
        .map_err(|e| RunError::Start(e.to_string()))?;
    let start = instance
        .get_typed_func::<(), ()>(&mut store, "_start")
        .map_err(|_| RunError::Start("it has no _start function".into()))?;
    let result = start.call(&mut store, ());
    drop(store);
    log_stderr(name, &stderr.contents());

    if let Err(error) = result {
        match error.downcast_ref::<I32Exit>() {
            Some(I32Exit(0)) => {}
            Some(I32Exit(code)) => {
                return Err(RunError::Crashed(format!("it exited with code {code}")));
            }
            None if error.downcast_ref::<wasmtime::Trap>() == Some(&wasmtime::Trap::Interrupt) => {
                return Err(RunError::Timeout(timeout.as_secs()));
            }
            None => return Err(RunError::Crashed(error.to_string())),
        }
    }
    Ok(stdout.contents().to_vec())
}

fn run_native(
    dir: &Path,
    program: &Path,
    args: &[String],
    name: &str,
    input: Vec<u8>,
    timeout: Duration,
) -> Result<Vec<u8>, RunError> {
    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(dir)
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(path) = std::env::var_os("PATH") {
        command.env("PATH", path);
    }
    let mut child = command
        .spawn()
        .map_err(|e| RunError::Start(e.to_string()))?;

    // Written, read and logged on their own threads, so a plugin that answers before reading
    // all its input can't leave both sides waiting on full pipes.
    let mut stdin = child.stdin.take().expect("stdin is piped");
    std::thread::spawn(move || {
        // A plugin that exits early closes its stdin; what it said is in its stdout.
        let _ = stdin.write_all(&input);
    });
    let stderr = child.stderr.take().expect("stderr is piped");
    let plugin = name.to_owned();
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).take(MAX_LOG as u64).lines() {
            let Ok(line) = line else { break };
            log_stderr(&plugin, line.as_bytes());
        }
    });
    let mut stdout = child.stdout.take().expect("stdout is piped");
    let (done, finished) = mpsc::channel();
    std::thread::spawn(move || {
        let mut output = Vec::new();
        let read = (&mut stdout)
            .take(MAX_OUTPUT as u64)
            .read_to_end(&mut output);
        let _ = done.send(read.map(|_| output));
    });

    let output = match finished.recv_timeout(timeout) {
        Ok(output) => output,
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(RunError::Timeout(timeout.as_secs()));
        }
    };
    let status = child.wait().map_err(|e| RunError::Crashed(e.to_string()))?;
    let output = output.map_err(|e| RunError::Crashed(e.to_string()))?;
    if !status.success() && output.is_empty() {
        return Err(RunError::Crashed(format!("it {status}")));
    }
    Ok(output)
}
