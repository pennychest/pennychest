//! Settings > Plugins: repositories, listing and installing, with the network faked and the
//! QIF example plugin, built for WASI, as the plugin.

mod common;

use std::path::Path;
use std::sync::Arc;

use axum::http::StatusCode;
use common::{TestApp, TestResponse};
use pennychest_core::ai::http::FakeApi;
use pennychest_core::plugins::index::{OFFICIAL, index_url};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const FORK: &str = "https://github.com/someone/pennychest-plugins";
const QIF_DIR: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../core/tests/fixtures/plugins/qif"
);
const DOWNLOAD: &str = "https://example.com/qif-0.1.0.tar.gz";
const STATEMENT: &str = "!Type:Bank\nD20/09/2026\nT-12.50\nPTESCO STORES 2041\n^\n";

/// The body of a 200.
trait Ok {
    fn ok(self) -> Value;
}

impl Ok for TestResponse {
    #[track_caller]
    fn ok(self) -> Value {
        self.assert_status(StatusCode::OK);
        self.body
    }
}

fn qif_archive() -> Vec<u8> {
    let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(
        Vec::new(),
        flate2::Compression::fast(),
    ));
    builder.append_dir_all("qif", QIF_DIR).unwrap();
    builder.into_inner().unwrap().finish().unwrap()
}

fn index(plugins: Value) -> Value {
    json!({ "name": "Test plugins", "plugins": plugins })
}

fn qif_entry(sha256: &str) -> Value {
    json!({
        "package": "qif",
        "name": "QIF statements",
        "description": "Import QIF statements",
        "version": "0.1.0",
        "url": DOWNLOAD,
        "sha256": sha256,
    })
}

/// A fake network where `repository` serves `index` (every time it's asked).
fn serve_index(api: &FakeApi, repository: &str, index: Value) {
    api.respond("GET", &index_url(repository), move |_| (200, index.clone()));
}

async fn app(api: &Arc<FakeApi>) -> TestApp {
    TestApp::with_plugins(api.clone(), None, true).await
}

#[tokio::test]
async fn lists_the_official_repository() {
    let api = FakeApi::new();
    serve_index(&api, OFFICIAL, index(json!([qif_entry(&"a".repeat(64))])));
    let mut app = app(&api).await;
    let body = app.get("/api/plugins").await.ok();
    let [official] = body["repositories"].as_array().unwrap().as_slice() else {
        panic!("{body}")
    };
    assert_eq!(official["official"], true);
    assert_eq!(official["name"], "Test plugins");
    assert_eq!(
        official["plugins"],
        json!([{
            "package": "qif",
            "name": "QIF statements",
            "description": "Import QIF statements",
            "version": "0.1.0",
            "installed_version": null,
            "built_in": false,
            "available": true,
        }])
    );
    assert_eq!(body["can_add_repositories"], true);
}

#[tokio::test]
async fn an_unreadable_repository_is_listed_with_an_error() {
    let api = FakeApi::new();
    api.respond("GET", &index_url(OFFICIAL), |_| (404, json!("Not Found")));
    let mut app = app(&api).await;
    let body = app.get("/api/plugins").await.ok();
    let official = &body["repositories"][0];
    assert!(official["error"].as_str().unwrap().contains("plugins.json"));
    assert_eq!(official["plugins"], json!([]));
}

#[tokio::test]
async fn adds_and_removes_a_repository() {
    let api = FakeApi::new();
    serve_index(&api, OFFICIAL, index(json!([])));
    serve_index(&api, FORK, index(json!([])));
    let mut app = app(&api).await;
    app.post(
        "/api/plugins/repositories",
        json!({ "url": format!("{FORK}/") }),
    )
    .await
    .assert_status(StatusCode::NO_CONTENT);
    let urls = |body: Value| -> Vec<String> {
        body["repositories"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["url"].as_str().unwrap().to_owned())
            .collect()
    };
    assert_eq!(urls(app.get("/api/plugins").await.ok()), [OFFICIAL, FORK]);

    let path = format!("/api/plugins/repositories?url={}", urlencode(FORK));
    app.delete(&path)
        .await
        .assert_status(StatusCode::NO_CONTENT);
    assert_eq!(urls(app.get("/api/plugins").await.ok()), [OFFICIAL]);
}

fn urlencode(text: &str) -> String {
    url::form_urlencoded::byte_serialize(text.as_bytes()).collect()
}

#[tokio::test]
async fn wont_add_a_repository_twice_or_without_an_index() {
    let api = FakeApi::new();
    serve_index(&api, OFFICIAL, index(json!([])));
    api.respond("GET", &index_url(FORK), |_| (404, json!("Not Found")));
    let mut app = app(&api).await;
    let twice = app
        .post("/api/plugins/repositories", json!({ "url": OFFICIAL }))
        .await;
    twice.assert_status(StatusCode::BAD_REQUEST);
    let missing = app
        .post("/api/plugins/repositories", json!({ "url": FORK }))
        .await;
    missing.assert_status(StatusCode::BAD_REQUEST);
    assert!(
        missing.body["detail"]
            .as_str()
            .unwrap()
            .contains("plugins.json")
    );
    let insecure = app
        .post(
            "/api/plugins/repositories",
            json!({ "url": "http://example.com" }),
        )
        .await;
    insecure.assert_status(StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn adding_repositories_can_be_turned_off() {
    let api = FakeApi::new();
    serve_index(&api, OFFICIAL, index(json!([])));
    serve_index(&api, FORK, index(json!([])));
    let mut app = TestApp::with_plugins(api.clone(), None, false).await;
    app.post("/api/plugins/repositories", json!({ "url": FORK }))
        .await
        .assert_status(StatusCode::FORBIDDEN);
    let body = app.get("/api/plugins").await.ok();
    assert_eq!(body["repositories"].as_array().unwrap().len(), 1);
    assert_eq!(body["can_add_repositories"], false);
}

#[tokio::test]
async fn installs_uses_and_removes_a_plugin() {
    let api = FakeApi::new();
    let archive = qif_archive();
    let sha = hex::encode(Sha256::digest(&archive));
    serve_index(&api, OFFICIAL, index(json!([qif_entry(&sha)])));
    api.on_bytes("GET", DOWNLOAD, 200, archive);
    let mut app = app(&api).await;

    app.post(
        "/api/plugins/install",
        json!({ "repository": OFFICIAL, "package": "qif" }),
    )
    .await
    .assert_status(StatusCode::NO_CONTENT);
    let body = app.get("/api/plugins").await.ok();
    assert_eq!(
        body["repositories"][0]["plugins"][0]["installed_version"],
        "0.1.0"
    );

    // The import page offers it, and it reads a statement, without a restart.
    let importers = app.get("/api/imports/importers").await.ok();
    let names: Vec<&str> = importers
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["csv", "qif"]);
    let detected = app
        .post_form(
            "/api/imports/detect",
            Some(("download.qif", STATEMENT.as_bytes())),
            &[],
        )
        .await
        .ok();
    assert_eq!(detected["importer"], "qif");

    app.post("/api/plugins/uninstall", json!({ "package": "qif" }))
        .await
        .assert_status(StatusCode::NO_CONTENT);
    let importers = app.get("/api/imports/importers").await.ok();
    assert_eq!(importers.as_array().unwrap().len(), 1);
    app.post("/api/plugins/uninstall", json!({ "package": "qif" }))
        .await
        .assert_status(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn wont_install_a_download_that_doesnt_match_its_checksum() {
    let api = FakeApi::new();
    serve_index(&api, OFFICIAL, index(json!([qif_entry(&"0".repeat(64))])));
    api.on_bytes("GET", DOWNLOAD, 200, qif_archive());
    let mut app = app(&api).await;
    let response = app
        .post(
            "/api/plugins/install",
            json!({ "repository": OFFICIAL, "package": "qif" }),
        )
        .await;
    response.assert_status(StatusCode::BAD_REQUEST);
    assert!(
        response.body["detail"]
            .as_str()
            .unwrap()
            .contains("checksum"),
        "{}",
        response.body
    );
    assert!(!app.config.plugin_dir.join("qif").exists());
}

#[tokio::test]
async fn wont_install_from_a_repository_that_isnt_added_or_a_plugin_it_doesnt_list() {
    let api = FakeApi::new();
    serve_index(&api, OFFICIAL, index(json!([])));
    serve_index(&api, FORK, index(json!([qif_entry(&"a".repeat(64))])));
    let mut app = app(&api).await;
    app.post(
        "/api/plugins/install",
        json!({ "repository": FORK, "package": "qif" }),
    )
    .await
    .assert_status(StatusCode::BAD_REQUEST);
    app.post(
        "/api/plugins/install",
        json!({ "repository": OFFICIAL, "package": "qif" }),
    )
    .await
    .assert_status(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn plugins_installed_before_are_loaded_at_start() {
    let api = FakeApi::new();
    serve_index(&api, OFFICIAL, index(json!([])));
    let parent = tempfile::tempdir().unwrap();
    common::copy_dir(Path::new(QIF_DIR), &parent.path().join("qif"));
    let mut app = TestApp::with_plugins(api.clone(), Some(parent.path()), true).await;
    let body = app.get("/api/plugins").await.ok();
    // Not in the official list, so listed on its own.
    assert_eq!(
        body["other_installed"],
        json!([{ "package": "qif", "version": "0.1.0" }])
    );
    assert_eq!(body["broken"], json!([]));
}

#[tokio::test]
async fn plugins_need_signing_in() {
    let mut app = TestApp::new().await;
    app.get("/api/plugins")
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
    app.post(
        "/api/plugins/install",
        json!({ "repository": OFFICIAL, "package": "qif" }),
    )
    .await
    .assert_status(StatusCode::UNAUTHORIZED);
}
