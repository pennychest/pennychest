# Testing

```sh
cargo test
```

## Server tests

Tests that go through the API live in `crates/server/tests/`. Each file starts with `mod common;`
and uses `common::TestApp`:

```rust
mod common;

use axum::http::StatusCode;
use common::TestApp;
use serde_json::json;

#[tokio::test]
async fn creates_an_account() {
    let mut app = TestApp::new().await;
    let response = app
        .post("/api/accounts", json!({ "name": "Food", "full_path": "Expenses:Food", "type": "expense" }))
        .await;
    response.assert_status(StatusCode::CREATED);
    assert_eq!(response.body["full_path"], "Expenses:Food");
}
```

`TestApp::new()` is a new install with a fresh in-memory database and its own upload and
plugin directories, so tests run in parallel without sharing anything. It calls the router
in-process, keeps the cookies the app sets like a browser, and exposes the database as
`app.db` for setting up or checking data directly. `assert_status` shows the body when a status
is wrong.

`TestApp::signed_in()` is the same with a signed-in client (the password is `TEST_PASSWORD`).
Tests of AI features fake the provider's API, so they get canned answers.

## Core tests

Logic that doesn't need HTTP is tested next to it in `crates/core`, with
`db::open_in_memory()` for a database.
