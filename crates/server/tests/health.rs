mod common;

use axum::http::StatusCode;
use common::TestApp;
use serde_json::json;

#[tokio::test]
async fn health_check_answers_ok_with_the_version() {
    let mut app = TestApp::new().await;
    let response = app.get("/api/health").await;
    response.assert_status(StatusCode::OK);
    assert_eq!(
        response.body,
        json!({ "status": "ok", "version": env!("CARGO_PKG_VERSION") })
    );
}

#[tokio::test]
async fn unknown_api_routes_are_not_found() {
    let mut app = TestApp::new().await;
    let response = app.get("/api/no-such-thing").await;
    response.assert_status(StatusCode::NOT_FOUND);
    assert_eq!(response.body, json!({ "detail": "Not Found" }));
}

#[tokio::test]
async fn each_test_gets_a_fresh_migrated_database() {
    let app = TestApp::new().await;
    let accounts: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM accounts")
        .fetch_one(&app.db)
        .await
        .unwrap();
    assert_eq!(accounts, 0);
    let sources: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM categorisation_sources")
        .fetch_one(&app.db)
        .await
        .unwrap();
    assert_eq!(sources, 6);
}
