use std::net::SocketAddr;
use std::process::ExitCode;

use pennychest_core::plugins::Plugins;
use pennychest_core::{auth, db};
use pennychest_server::{AppState, Config, app};
use sqlx::SqlitePool;
use tracing_subscriber::EnvFilter;

const USAGE: &str = "\
Usage: pennychest [command]

Commands:
  (none)           Run the server
  reset-password   Forget the password and sign everyone out; the next visit shows the
                   setup screen to choose a new one
  healthcheck      Exit successfully if the server is answering, as Docker's HEALTHCHECK
                   runs it (the image has no shell or curl)
";

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        // Colours only for a person at a terminal, not in `docker logs` or a log file.
        .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stdout()))
        .init();

    let command = std::env::args().nth(1);
    let config = match Config::from_env() {
        Ok(config) => config,
        Err(err) => {
            tracing::error!("{err}");
            return ExitCode::FAILURE;
        }
    };

    match command.as_deref() {
        None => serve(config).await,
        Some("reset-password") => reset_password(config).await,
        Some("healthcheck") => healthcheck(&config),
        Some("help" | "--help" | "-h") => {
            print!("{USAGE}");
            ExitCode::SUCCESS
        }
        Some(other) => {
            eprint!("Unknown command {other:?}.\n\n{USAGE}");
            ExitCode::from(2)
        }
    }
}

async fn open_database(config: &Config) -> Option<SqlitePool> {
    if let Some(dir) = config.database_path.parent()
        && let Err(err) = std::fs::create_dir_all(dir)
    {
        tracing::error!("Couldn't create {}: {err}", dir.display());
        return None;
    }
    match db::open(&config.database_path).await {
        Ok(db) => Some(db),
        Err(err) => {
            tracing::error!("{err}");
            None
        }
    }
}

async fn serve(config: Config) -> ExitCode {
    let Some(db) = open_database(&config).await else {
        return ExitCode::FAILURE;
    };
    tracing::info!("Database ready at {}", config.database_path.display());

    let listener = match tokio::net::TcpListener::bind(config.listen).await {
        Ok(listener) => listener,
        Err(err) => {
            tracing::error!("Couldn't listen on {}: {err}", config.listen);
            return ExitCode::FAILURE;
        }
    };
    tracing::info!("PennyChest is listening on http://{}", config.listen);

    // Plugins are started once each to ask what they provide, so they load off the runtime.
    let plugin_dir = config.plugin_dir.clone();
    let plugins = tokio::task::spawn_blocking(move || Plugins::load(&plugin_dir))
        .await
        .unwrap_or_else(|_| Plugins::none());
    let state = AppState::new(config, db.clone()).with_plugins(plugins);
    // Client addresses are passed through for the sign-in throttle.
    let served = axum::serve(
        listener,
        app(state).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await;
    db.close().await;
    match served {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            tracing::error!("The server stopped: {err}");
            ExitCode::FAILURE
        }
    }
}

/// Run inside the container: `pennychest reset-password`.
async fn reset_password(config: Config) -> ExitCode {
    let Some(db) = open_database(&config).await else {
        return ExitCode::FAILURE;
    };
    let result = auth::reset(&db).await;
    db.close().await;
    match result {
        Ok(()) => {
            println!("Password cleared. Open PennyChest to choose a new one.");
            ExitCode::SUCCESS
        }
        Err(err) => {
            tracing::error!("Couldn't clear the password: {err}");
            ExitCode::FAILURE
        }
    }
}

/// Asks the running server for `/api/health` over the loopback interface.
fn healthcheck(config: &Config) -> ExitCode {
    use std::io::{Read, Write};
    use std::time::Duration;

    let mut address = config.listen;
    if address.ip().is_unspecified() {
        address.set_ip(std::net::Ipv4Addr::LOCALHOST.into());
    }
    let answered = (|| -> std::io::Result<bool> {
        let timeout = Duration::from_secs(5);
        let mut stream = std::net::TcpStream::connect_timeout(&address, timeout)?;
        stream.set_read_timeout(Some(timeout))?;
        stream.write_all(b"GET /api/health HTTP/1.0\r\nHost: localhost\r\n\r\n")?;
        let mut reply = String::new();
        stream.take(64).read_to_string(&mut reply)?;
        Ok(reply.starts_with("HTTP/1.0 200") || reply.starts_with("HTTP/1.1 200"))
    })();
    match answered {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => {
            eprintln!("The server at {address} isn't healthy.");
            ExitCode::FAILURE
        }
        Err(err) => {
            eprintln!("The server at {address} isn't answering: {err}");
            ExitCode::FAILURE
        }
    }
}

/// Ctrl-C, or the SIGTERM that Docker and Fly send to stop a container.
async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut signal) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            signal.recv().await;
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }
    tracing::info!("Shutting down");
}
