//! Signing in, sessions and the password. Tests of other areas' routes use the dashboard.

mod common;

use axum::http::{Method, StatusCode};
use common::{TEST_PASSWORD, TestApp};
use pennychest_server::COOKIE_NAME;
use serde_json::json;

/// Any route that needs a session.
const PRIVATE: &str = "/api/dashboard/layout";

#[tokio::test]
async fn fresh_install_requires_setup() {
    let mut app = TestApp::new().await;
    assert_eq!(
        app.get("/api/auth/status").await.body,
        json!({ "setup_required": true, "authenticated": false })
    );
}

#[tokio::test]
async fn api_requires_session() {
    let mut app = TestApp::new().await;
    let response = app.get(PRIVATE).await;
    response.assert_status(StatusCode::UNAUTHORIZED);
    assert_eq!(response.body, json!({ "detail": "Not signed in" }));
    app.put(PRIVATE, json!({}))
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn health_is_public() {
    let mut app = TestApp::new().await;
    app.get("/api/health").await.assert_status(StatusCode::OK);
}

#[tokio::test]
async fn unknown_routes_are_not_found_even_when_signed_out() {
    let mut app = TestApp::new().await;
    app.get("/api/no-such-thing")
        .await
        .assert_status(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn setup_rejects_short_password() {
    let mut app = TestApp::new().await;
    let response = app
        .post("/api/auth/setup", json!({ "password": "short" }))
        .await;
    response.assert_status(StatusCode::BAD_REQUEST);
    assert_eq!(
        response.body,
        json!({ "detail": "Password must be at least 8 characters" })
    );
    assert_eq!(
        app.get("/api/auth/status").await.body["setup_required"],
        true
    );
}

#[tokio::test]
async fn setup_signs_in_and_can_only_run_once() {
    let mut app = TestApp::signed_in().await;
    assert_eq!(
        app.get("/api/auth/status").await.body,
        json!({ "setup_required": false, "authenticated": true })
    );
    app.get(PRIVATE).await.assert_status(StatusCode::OK);

    app.post(
        "/api/auth/setup",
        json!({ "password": "a-different-password" }),
    )
    .await
    .assert_status(StatusCode::CONFLICT);
}

#[tokio::test]
async fn session_cookie_flags() {
    let mut app = TestApp::new().await;
    let response = app
        .post("/api/auth/setup", json!({ "password": TEST_PASSWORD }))
        .await;
    let cookie = response.headers["set-cookie"].to_str().unwrap().to_owned();
    assert!(cookie.contains("HttpOnly"), "{cookie}");
    assert!(cookie.contains("SameSite=lax"), "{cookie}");
    assert!(cookie.contains("Max-Age=2592000"), "{cookie}");
    assert!(cookie.contains("Path=/"), "{cookie}");
    assert!(!cookie.contains("Secure"), "{cookie}");

    app.post("/api/auth/logout", json!({})).await;
    let response = app
        .send_with_headers(
            Method::POST,
            "/api/auth/login",
            Some(json!({ "password": TEST_PASSWORD })),
            &[("x-forwarded-proto", "https")],
        )
        .await;
    response.assert_status(StatusCode::NO_CONTENT);
    let cookie = response.headers["set-cookie"].to_str().unwrap();
    assert!(cookie.contains("Secure"), "{cookie}");
}

#[tokio::test]
async fn login_and_logout() {
    let mut app = TestApp::signed_in().await;
    app.post("/api/auth/logout", json!({}))
        .await
        .assert_status(StatusCode::NO_CONTENT);
    app.get(PRIVATE)
        .await
        .assert_status(StatusCode::UNAUTHORIZED);

    let wrong = app
        .post("/api/auth/login", json!({ "password": "wrong-password" }))
        .await;
    wrong.assert_status(StatusCode::UNAUTHORIZED);
    assert_eq!(wrong.body, json!({ "detail": "Incorrect password" }));
    app.get(PRIVATE)
        .await
        .assert_status(StatusCode::UNAUTHORIZED);

    app.post("/api/auth/login", json!({ "password": TEST_PASSWORD }))
        .await
        .assert_status(StatusCode::NO_CONTENT);
    app.get(PRIVATE).await.assert_status(StatusCode::OK);
}

#[tokio::test]
async fn login_before_setup() {
    let mut app = TestApp::new().await;
    app.post("/api/auth/login", json!({ "password": TEST_PASSWORD }))
        .await
        .assert_status(StatusCode::CONFLICT);
}

#[tokio::test]
async fn logout_invalidates_the_token() {
    let mut app = TestApp::signed_in().await;
    let token = app.cookie(COOKIE_NAME).unwrap().to_owned();
    app.post("/api/auth/logout", json!({})).await;
    assert_eq!(
        app.cookie(COOKIE_NAME),
        None,
        "logging out deletes the cookie"
    );
    app.set_cookie(COOKIE_NAME, &token);
    app.get(PRIVATE)
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn forged_cookie_is_rejected() {
    let mut app = TestApp::new().await;
    app.post("/api/auth/setup", json!({ "password": TEST_PASSWORD }))
        .await;
    app.set_cookie(COOKIE_NAME, "not-a-real-token");
    app.get(PRIVATE)
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn expired_session_is_rejected() {
    let mut app = TestApp::signed_in().await;
    sqlx::query("UPDATE auth_sessions SET expires_at = unixepoch() - 1")
        .execute(&app.db)
        .await
        .unwrap();
    app.get(PRIVATE)
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn repeated_failures_are_throttled() {
    let mut app = TestApp::signed_in().await;
    app.post("/api/auth/logout", json!({})).await;
    for _ in 0..10 {
        app.post("/api/auth/login", json!({ "password": "wrong-password" }))
            .await
            .assert_status(StatusCode::UNAUTHORIZED);
    }

    // Even the right password is refused until the window passes
    let response = app
        .post("/api/auth/login", json!({ "password": TEST_PASSWORD }))
        .await;
    response.assert_status(StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(
        response.body,
        json!({ "detail": "Too many failed attempts. Try again later." })
    );
}

#[tokio::test]
async fn change_password_signs_out_other_devices() {
    let mut app = TestApp::signed_in().await;
    let mut other_device = app.other_device();
    other_device
        .post("/api/auth/login", json!({ "password": TEST_PASSWORD }))
        .await
        .assert_status(StatusCode::NO_CONTENT);
    other_device
        .get(PRIVATE)
        .await
        .assert_status(StatusCode::OK);

    let wrong = app
        .post(
            "/api/auth/password",
            json!({ "current_password": "wrong-password", "new_password": "new-password-123" }),
        )
        .await;
    wrong.assert_status(StatusCode::BAD_REQUEST);
    assert_eq!(
        wrong.body,
        json!({ "detail": "Current password is incorrect" })
    );

    app.post(
        "/api/auth/password",
        json!({ "current_password": TEST_PASSWORD, "new_password": "short" }),
    )
    .await
    .assert_status(StatusCode::BAD_REQUEST);

    app.post(
        "/api/auth/password",
        json!({ "current_password": TEST_PASSWORD, "new_password": "new-password-123" }),
    )
    .await
    .assert_status(StatusCode::NO_CONTENT);

    app.get(PRIVATE).await.assert_status(StatusCode::OK);
    other_device
        .get(PRIVATE)
        .await
        .assert_status(StatusCode::UNAUTHORIZED);

    app.post("/api/auth/logout", json!({})).await;
    app.post("/api/auth/login", json!({ "password": TEST_PASSWORD }))
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
    app.post("/api/auth/login", json!({ "password": "new-password-123" }))
        .await
        .assert_status(StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn a_stored_password_hash_signs_in() {
    let mut app = TestApp::new().await;
    sqlx::query(
        "INSERT INTO app_settings (key, value) VALUES ('auth_password_hash', \
         'scrypt$16384$8$1$3329456a0c1b24efc287d5f5675260ee$\
         317a6f744a86d689b603ffc1e9b959e86fe447e472e95723c456cf455026ee17')",
    )
    .execute(&app.db)
    .await
    .unwrap();
    app.post("/api/auth/login", json!({ "password": TEST_PASSWORD }))
        .await
        .assert_status(StatusCode::NO_CONTENT);
    app.get(PRIVATE).await.assert_status(StatusCode::OK);
}
