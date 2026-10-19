//! PennyChest's web server: the JSON API under `/api` and the embedded React app.

pub mod config;
pub mod error;
mod frontend;
pub mod openapi;
mod routes;

use std::sync::Arc;

use axum::http::{HeaderValue, header};
use axum::response::Response;
use axum::routing::get;
use axum::{Json, Router, middleware};
use pennychest_core::ai::http::{Reqwest, Transport};
use pennychest_core::auth::LoginThrottle;
use pennychest_core::export::Exporters;
use pennychest_core::imports::Importers;
use pennychest_core::learned::Learned;
use pennychest_core::plugins::Plugins;
use serde::Serialize;
use sqlx::SqlitePool;

pub use config::Config;
pub use routes::auth::COOKIE_NAME;

/// What every request handler can reach.
#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub db: SqlitePool,
    pub login_throttle: Arc<LoginThrottle>,
    /// The importers built in: the CSV importer (or others in tests).
    pub built_in_importers: Arc<Importers>,
    /// The exporters built in: none (or some in tests).
    pub built_in_exporters: Arc<Exporters>,
    /// The plugins installed from Settings, which add importers and exporters.
    pub plugins: Arc<Plugins>,
    /// The model learnt from the user's own categories, kept until their history changes.
    pub learned: Arc<Learned>,
    /// How requests reach AI providers: the network, or a fake in tests.
    pub http: Arc<dyn Transport>,
    /// Work started by a request that its response doesn't wait for.
    pub background: Arc<Background>,
}

/// Work that runs after a response has been sent, such as judging an import's charges.
#[derive(Default)]
pub struct Background {
    running: std::sync::Mutex<Vec<tokio::task::JoinHandle<()>>>,
}

impl Background {
    pub fn spawn(&self, work: impl std::future::Future<Output = ()> + Send + 'static) {
        let mut running = self.running.lock().expect("not poisoned");
        running.retain(|task| !task.is_finished());
        running.push(tokio::spawn(work));
    }

    /// Waits for everything started so far to finish.
    pub async fn settle(&self) {
        loop {
            let task = self.running.lock().expect("not poisoned").pop();
            let Some(task) = task else {
                return;
            };
            if let Err(e) = task.await {
                tracing::warn!("Background work failed: {e}");
            }
        }
    }
}

impl AppState {
    pub fn new(config: Config, db: SqlitePool) -> Self {
        Self {
            config: Arc::new(config),
            db,
            login_throttle: Arc::new(LoginThrottle::default()),
            built_in_importers: Arc::new(Importers::default()),
            built_in_exporters: Arc::new(Exporters::default()),
            plugins: Arc::new(Plugins::none()),
            learned: Arc::new(Learned::default()),
            http: Arc::new(Reqwest::default()),
            background: Arc::new(Background::default()),
        }
    }

    /// With AI providers reached through `http`, such as a `FakeApi` in tests.
    pub fn with_http(mut self, http: Arc<dyn Transport>) -> Self {
        self.http = http;
        self
    }

    pub fn with_importers(mut self, importers: Importers) -> Self {
        self.built_in_importers = Arc::new(importers);
        self
    }

    pub fn with_exporters(mut self, exporters: Exporters) -> Self {
        self.built_in_exporters = Arc::new(exporters);
        self
    }

    /// With the plugins installed in a plugin directory.
    pub fn with_plugins(mut self, plugins: Plugins) -> Self {
        self.plugins = Arc::new(plugins);
        self
    }

    /// The importers available now: the built-in ones, then the installed plugins'.
    pub fn importers(&self) -> Importers {
        self.plugins.importers(&self.built_in_importers)
    }

    /// The exporters available now, from the installed plugins.
    pub fn exporters(&self) -> Exporters {
        self.plugins.exporters(&self.built_in_exporters)
    }
}

/// The whole app, ready to serve.
pub fn app(state: AppState) -> Router {
    let api = Router::new()
        .route("/api/health", get(health))
        .route("/api/openapi.json", get(openapi::serve))
        .nest("/api/accounts", routes::accounts::router())
        .nest("/api/actions", routes::actions::actions_router())
        .nest("/api/ai", routes::ai::router())
        .nest("/api/auth", routes::auth::router())
        .nest("/api/budgets", routes::budgets::router())
        .nest("/api/changes", routes::actions::changes_router())
        .nest("/api/chat", routes::chat::router())
        .nest("/api/dashboard", routes::dashboard::router())
        .nest("/api/export", routes::export::router())
        .nest("/api/imports", routes::imports::router())
        .nest("/api/lookup", routes::setup::lookup_router())
        .nest("/api/mcp/tokens", routes::mcp::tokens_router())
        .nest("/api/oauth", routes::mcp::consent_router())
        .nest("/api/plugins", routes::plugins::router())
        .nest("/api/reports", routes::reports::router())
        .nest("/api/rules", routes::rules::router())
        .nest("/api/setup", routes::setup::setup_router())
        .nest("/api/taps", routes::taps::router())
        .nest("/api/transactions", routes::transactions::router())
        // Added last, so it covers every route above, and only those.
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            routes::auth::require_session,
        ));
    api.merge(routes::mcp::public_router())
        .fallback(frontend::serve)
        .layer(middleware::map_response(never_framed))
        .with_state(state)
}

/// Nothing in PennyChest is meant to be embedded, and the OAuth consent page must never be
/// (another site could frame it and trick a click on Allow).
async fn never_framed(mut response: Response) -> Response {
    let headers = response.headers_mut();
    headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("frame-ancestors 'none'"),
    );
    response
}

#[derive(Serialize, utoipa::ToSchema)]
pub(crate) struct Health {
    /// Always `"ok"`.
    #[schema(example = "ok")]
    status: &'static str,
    /// The release, e.g. `"0.5.0"`.
    version: &'static str,
}

/// Whether the server is up, and which release it is.
#[utoipa::path(get, path = "/api/health", tag = "health", responses(
    (status = 200, description = "The server is up", body = Health),
))]
pub(crate) async fn health() -> Json<Health> {
    Json(Health {
        status: "ok",
        version: env!("CARGO_PKG_VERSION"),
    })
}
