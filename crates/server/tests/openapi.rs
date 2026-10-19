//! The OpenAPI description: what's committed is what the server describes, and the
//! routes it describes exist.

mod common;

use std::path::PathBuf;

use axum::http::{Method, StatusCode};
use common::TestApp;
use serde_json::{Value, json};

fn committed() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../openapi.json")
}

/// Fails when the API changed without `openapi.json`. Run with `UPDATE_OPENAPI=1` to write it,
/// then `npm run api:types` in `frontend` for the types the frontend uses.
#[test]
fn the_committed_description_is_up_to_date() {
    let generated = pennychest_server::openapi::json();
    if std::env::var_os("UPDATE_OPENAPI").is_some() {
        std::fs::write(committed(), &generated).unwrap();
        return;
    }
    let on_disk = std::fs::read_to_string(committed()).unwrap_or_default();
    assert!(
        on_disk == generated,
        "openapi.json is out of date. Run `UPDATE_OPENAPI=1 cargo test -p pennychest-server \
         --test openapi`, then `npm run api:types` in frontend, and commit both."
    );
}

#[tokio::test]
async fn served_without_signing_in() {
    let mut app = TestApp::new().await;
    let response = app.get("/api/openapi.json").await;
    assert_eq!(response.status, StatusCode::OK);
    let expected: Value = serde_json::from_str(&pennychest_server::openapi::json()).unwrap();
    assert_eq!(response.body, expected);
    assert_eq!(response.body["openapi"], json!("3.1.0"));
}

#[tokio::test]
async fn every_described_route_exists() {
    let doc: Value = serde_json::from_str(&pennychest_server::openapi::json()).unwrap();
    let paths = doc["paths"].as_object().unwrap();
    assert!(!paths.is_empty());
    // Paths with parameters can rightly answer 404 for a made-up id, so only those without.
    for (path, operations) in paths.iter().filter(|(path, _)| !path.contains('{')) {
        for method in operations.as_object().unwrap().keys() {
            let mut app = TestApp::signed_in().await;
            let method = Method::from_bytes(method.to_uppercase().as_bytes()).unwrap();
            let response = app.send(method.clone(), path, Some(json!({}))).await;
            assert!(
                response.status != StatusCode::NOT_FOUND
                    && response.status != StatusCode::METHOD_NOT_ALLOWED,
                "{method} {path} is described but answers {}",
                response.status
            );
        }
    }
}
