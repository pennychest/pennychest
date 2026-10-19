//! Helpers for the server's tests: a whole app on a fresh database, called in-process.
//!
//! Each test gets its own in-memory database and data directories, so tests can run in parallel.

// Each test binary uses its own subset of these helpers.
#![allow(dead_code)]

use std::sync::Arc;

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{HeaderMap, Method, Request, StatusCode, header};
use pennychest_core::ai::http::FakeApi;
use pennychest_core::db;
use pennychest_server::{AppState, Config, app};
use serde_json::{Value, json};
use sqlx::SqlitePool;
use tempfile::TempDir;
use tower::ServiceExt;

/// The password `TestApp::signed_in` sets.
pub const TEST_PASSWORD: &str = "correct horse battery";

pub struct TestApp {
    router: Router,
    background: Arc<pennychest_server::Background>,
    pub db: SqlitePool,
    pub config: Arc<Config>,
    /// Cookies the app has set, sent back with each request like a browser would.
    cookies: Vec<(String, String)>,
    // Holds the upload and plugin directories until the last device using them is dropped.
    _dir: Arc<TempDir>,
}

/// How `TestApp::build_with` sets up an app.
struct Options {
    importers: pennychest_core::imports::Importers,
    api: Option<Arc<FakeApi>>,
    exporters: Option<pennychest_core::export::Exporters>,
    /// Load plugins, after copying in those from this directory if there's one.
    plugins_from: Option<Option<std::path::PathBuf>>,
    allow_repositories: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            importers: pennychest_core::imports::Importers::default(),
            api: None,
            exporters: None,
            plugins_from: None,
            allow_repositories: true,
        }
    }
}

/// Copies a directory and everything in it.
pub fn copy_dir(from: &std::path::Path, to: &std::path::Path) {
    std::fs::create_dir_all(to).expect("a directory");
    for entry in std::fs::read_dir(from).expect("readable") {
        let entry = entry.expect("readable");
        let target = to.join(entry.file_name());
        if entry.path().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).expect("copied");
        }
    }
}

pub struct TestResponse {
    pub status: StatusCode,
    pub headers: HeaderMap,
    /// The body as JSON; a body that isn't JSON is a JSON string, and an empty one null.
    pub body: Value,
}

impl TestApp {
    /// A new install: an empty database, nobody signed in.
    pub async fn new() -> Self {
        Self::with_importers(pennychest_core::imports::Importers::default()).await
    }

    /// A new install with these importers available, e.g. a fake plugin's.
    pub async fn with_importers(importers: pennychest_core::imports::Importers) -> Self {
        Self::build(importers, None, None).await
    }

    /// A new install, signed in, whose AI providers are this fake.
    pub async fn with_fake_api(api: Arc<FakeApi>) -> Self {
        let mut app = Self::build(
            pennychest_core::imports::Importers::default(),
            Some(api),
            None,
        )
        .await;
        app.post("/api/auth/setup", json!({ "password": TEST_PASSWORD }))
            .await
            .assert_status(StatusCode::NO_CONTENT);
        app
    }

    /// A new install, signed in, with these exporters available, e.g. a fake plugin's, and its
    /// database on disk so it can be downloaded.
    pub async fn with_exporters(exporters: pennychest_core::export::Exporters) -> Self {
        let mut app = Self::build(
            pennychest_core::imports::Importers::default(),
            None,
            Some(exporters),
        )
        .await;
        app.post("/api/auth/setup", json!({ "password": TEST_PASSWORD }))
            .await
            .assert_status(StatusCode::NO_CONTENT);
        app
    }

    /// A new install, signed in, with plugin repositories reached through `api`, and the
    /// plugins in `plugin_dir` (copied into the app's own plugin directory) loaded.
    /// `allow_repositories` is `PENNYCHEST_ALLOW_PLUGIN_REPOSITORIES`.
    pub async fn with_plugins(
        api: Arc<FakeApi>,
        plugin_dir: Option<&std::path::Path>,
        allow_repositories: bool,
    ) -> Self {
        let options = Options {
            api: Some(api),
            plugins_from: Some(plugin_dir.map(std::path::Path::to_path_buf)),
            allow_repositories,
            ..Options::default()
        };
        let mut app = Self::build_with(options).await;
        app.post("/api/auth/setup", json!({ "password": TEST_PASSWORD }))
            .await
            .assert_status(StatusCode::NO_CONTENT);
        app
    }

    async fn build(
        importers: pennychest_core::imports::Importers,
        api: Option<Arc<FakeApi>>,
        exporters: Option<pennychest_core::export::Exporters>,
    ) -> Self {
        Self::build_with(Options {
            importers,
            api,
            exporters,
            ..Options::default()
        })
        .await
    }

    async fn build_with(options: Options) -> Self {
        let Options {
            importers,
            api,
            exporters,
            plugins_from,
            allow_repositories,
        } = options;
        let dir = tempfile::tempdir().expect("a temporary directory");
        let data = dir.path().to_owned();
        let allow = if allow_repositories { "true" } else { "false" };
        let config = Config::from_lookup(|name| match name {
            "PENNYCHEST_DATABASE_PATH" => Some(data.join("unused.db").display().to_string()),
            "PENNYCHEST_UPLOAD_DIR" => Some(data.join("uploads").display().to_string()),
            "PENNYCHEST_PLUGIN_DIR" => Some(data.join("plugins").display().to_string()),
            "PENNYCHEST_ALLOW_PLUGIN_REPOSITORIES" => Some(allow.to_owned()),
            _ => None,
        })
        .expect("test configuration");
        // In memory, except where exporting needs it on disk: SQLite writes a backup of an
        // in-memory database into memory too.
        let db = if exporters.is_some() {
            db::open(&data.join("pennychest.db")).await
        } else {
            db::open_in_memory().await
        }
        .expect("a test database");
        let mut state = AppState::new(config, db.clone()).with_importers(importers);
        if let Some(api) = api {
            state = state.with_http(api);
        }
        if let Some(exporters) = exporters {
            state = state.with_exporters(exporters);
        }
        if let Some(from) = plugins_from {
            let plugin_dir = state.config.plugin_dir.clone();
            if let Some(from) = from {
                copy_dir(&from, &plugin_dir);
            }
            state = state.with_plugins(pennychest_core::plugins::Plugins::load(&plugin_dir));
        }
        let config = state.config.clone();
        let background = state.background.clone();
        Self {
            background,
            router: app(state),
            db,
            config,
            cookies: Vec::new(),
            _dir: Arc::new(dir),
        }
    }

    /// Waits for the work requests left running in the background, such as insights after
    /// an import.
    pub async fn settle(&self) {
        self.background.settle().await;
    }

    /// A new install where the password has been set to `TEST_PASSWORD`, which signs in.
    pub async fn signed_in() -> Self {
        let mut app = Self::new().await;
        app.post("/api/auth/setup", json!({ "password": TEST_PASSWORD }))
            .await
            .assert_status(StatusCode::NO_CONTENT);
        app
    }

    /// Another browser on the same server and database, with no cookies of its own yet.
    pub fn other_device(&self) -> Self {
        Self {
            router: self.router.clone(),
            background: self.background.clone(),
            db: self.db.clone(),
            config: self.config.clone(),
            cookies: Vec::new(),
            _dir: self._dir.clone(),
        }
    }

    /// The value of a cookie the app has set, if it's still kept.
    pub fn cookie(&self, name: &str) -> Option<&str> {
        self.cookies
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    /// Sets a cookie as though the app had, e.g. to try a forged or old one.
    pub fn set_cookie(&mut self, name: &str, value: &str) {
        self.cookies.retain(|(n, _)| n != name);
        self.cookies.push((name.to_owned(), value.to_owned()));
    }

    pub async fn get(&mut self, path: &str) -> TestResponse {
        self.send(Method::GET, path, None).await
    }

    pub async fn post(&mut self, path: &str, body: Value) -> TestResponse {
        self.send(Method::POST, path, Some(body)).await
    }

    pub async fn put(&mut self, path: &str, body: Value) -> TestResponse {
        self.send(Method::PUT, path, Some(body)).await
    }

    pub async fn patch(&mut self, path: &str, body: Value) -> TestResponse {
        self.send(Method::PATCH, path, Some(body)).await
    }

    pub async fn delete(&mut self, path: &str) -> TestResponse {
        self.send(Method::DELETE, path, None).await
    }

    /// Posts a multipart form, as a browser uploads a file: `file` is (filename, content).
    pub async fn post_form(
        &mut self,
        path: &str,
        file: Option<(&str, &[u8])>,
        fields: &[(&str, &str)],
    ) -> TestResponse {
        const BOUNDARY: &str = "pennychest-test-boundary";
        let mut body = Vec::new();
        for (name, value) in fields {
            body.extend_from_slice(
                format!(
                    "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
                )
                .as_bytes(),
            );
        }
        if let Some((filename, content)) = file {
            body.extend_from_slice(
                format!(
                    "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"; \
                     filename=\"{filename}\"\r\nContent-Type: application/octet-stream\r\n\r\n"
                )
                .as_bytes(),
            );
            body.extend_from_slice(content);
            body.extend_from_slice(b"\r\n");
        }
        body.extend_from_slice(format!("--{BOUNDARY}--\r\n").as_bytes());
        let request = Request::builder()
            .method(Method::POST)
            .uri(path)
            .header(
                header::CONTENT_TYPE,
                format!("multipart/form-data; boundary={BOUNDARY}"),
            )
            .body(Body::from(body))
            .expect("a valid request");
        self.dispatch(request).await
    }

    pub async fn send(&mut self, method: Method, path: &str, body: Option<Value>) -> TestResponse {
        self.send_with_headers(method, path, body, &[]).await
    }

    pub async fn send_with_headers(
        &mut self,
        method: Method,
        path: &str,
        body: Option<Value>,
        headers: &[(&str, &str)],
    ) -> TestResponse {
        let mut request = Request::builder().method(method).uri(path);
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        if !self.cookies.is_empty() {
            let cookie = self
                .cookies
                .iter()
                .map(|(name, value)| format!("{name}={value}"))
                .collect::<Vec<_>>()
                .join("; ");
            request = request.header(header::COOKIE, cookie);
        }
        let request = match body {
            Some(body) => request
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string())),
            None => request.body(Body::empty()),
        }
        .expect("a valid request");
        self.dispatch(request).await
    }

    /// The body of a GET as it is, for downloads that aren't text.
    pub async fn get_bytes(&mut self, path: &str) -> Vec<u8> {
        let mut request = Request::builder().method(Method::GET).uri(path);
        if !self.cookies.is_empty() {
            let cookie = self
                .cookies
                .iter()
                .map(|(name, value)| format!("{name}={value}"))
                .collect::<Vec<_>>()
                .join("; ");
            request = request.header(header::COOKIE, cookie);
        }
        let request = request.body(Body::empty()).expect("a valid request");
        let response = self
            .router
            .clone()
            .oneshot(request)
            .await
            .expect("the router never fails");
        assert_eq!(response.status(), StatusCode::OK);
        to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("a readable body")
            .to_vec()
    }

    /// Sends this body as it is, e.g. a form or something that isn't JSON at all.
    pub async fn send_raw(
        &mut self,
        method: Method,
        path: &str,
        content_type: &str,
        body: impl Into<Body>,
        headers: &[(&str, &str)],
    ) -> TestResponse {
        let mut request = Request::builder()
            .method(method)
            .uri(path)
            .header(header::CONTENT_TYPE, content_type);
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        let request = request.body(body.into()).expect("a valid request");
        self.dispatch(request).await
    }

    async fn dispatch(&mut self, request: Request<Body>) -> TestResponse {
        let mut request = request;
        if !self.cookies.is_empty() && !request.headers().contains_key(header::COOKIE) {
            let cookie = self
                .cookies
                .iter()
                .map(|(name, value)| format!("{name}={value}"))
                .collect::<Vec<_>>()
                .join("; ");
            request.headers_mut().insert(
                header::COOKIE,
                cookie.parse().expect("a valid cookie header"),
            );
        }
        let response = self
            .router
            .clone()
            .oneshot(request)
            .await
            .expect("the router never fails");
        let status = response.status();
        let headers = response.headers().clone();
        self.remember_cookies(&headers);
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("a readable body");
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes)
                .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()))
        };
        TestResponse {
            status,
            headers,
            body,
        }
    }

    fn remember_cookies(&mut self, headers: &HeaderMap) {
        for set_cookie in headers.get_all(header::SET_COOKIE) {
            let Ok(set_cookie) = set_cookie.to_str() else {
                continue;
            };
            let pair = set_cookie.split(';').next().unwrap_or_default();
            let Some((name, value)) = pair.split_once('=') else {
                continue;
            };
            let (name, value) = (name.trim().to_owned(), value.trim().to_owned());
            self.cookies.retain(|(existing, _)| *existing != name);
            // An emptied or expired cookie is how a server deletes one.
            let expired = set_cookie.to_ascii_lowercase().contains("max-age=0");
            if !value.is_empty() && !expired {
                self.cookies.push((name, value));
            }
        }
    }
}

impl TestResponse {
    /// Panics with the body if the status isn't `expected`, so a failing test shows why.
    #[track_caller]
    pub fn assert_status(&self, expected: StatusCode) -> &Self {
        assert_eq!(
            self.status, expected,
            "expected {expected}, got {} with body {}",
            self.status, self.body
        );
        self
    }
}
