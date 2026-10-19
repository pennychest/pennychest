//! The accounts endpoints, with the setup and lookup routes: creating, exporting and moving
//! accounts (subaccounts move with them), and refusing to delete one in use.

mod common;

use axum::http::StatusCode;
use common::TestApp;
use serde_json::{Value, json};

async fn seeded() -> TestApp {
    let mut app = TestApp::signed_in().await;
    app.post("/api/setup/seed", json!({}))
        .await
        .assert_status(StatusCode::OK);
    app
}

async fn id_of(app: &mut TestApp, path: &str) -> i64 {
    let accounts = app.get("/api/accounts").await.body;
    accounts
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["full_path"] == path)
        .unwrap_or_else(|| panic!("no account {path}"))["id"]
        .as_i64()
        .unwrap()
}

#[tokio::test]
async fn create_account() {
    let mut app = TestApp::signed_in().await;
    let response = app
        .post(
            "/api/accounts",
            json!({"name": "Bank", "full_path": "Assets:Bank", "type": "asset", "currency": "GBP"}),
        )
        .await;
    response.assert_status(StatusCode::CREATED);
    assert_eq!(
        response.body,
        json!({
            "id": response.body["id"], "name": "Bank", "full_path": "Assets:Bank",
            "parent_id": null, "type": "asset", "currency": "GBP",
            "bank_identifier": null, "icon": null
        })
    );
}

#[tokio::test]
async fn list_accounts() {
    let mut app = TestApp::signed_in().await;
    app.post(
        "/api/accounts",
        json!({"name": "Assets", "full_path": "Assets", "type": "asset"}),
    )
    .await;
    let response = app.get("/api/accounts").await;
    response.assert_status(StatusCode::OK);
    assert!(!response.body.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn create_duplicate_account() {
    let mut app = TestApp::signed_in().await;
    let body = json!({"name": "Assets", "full_path": "Assets", "type": "asset"});
    app.post("/api/accounts", body.clone()).await;
    let response = app.post("/api/accounts", body).await;
    response.assert_status(StatusCode::CONFLICT);
    assert_eq!(
        response.body,
        json!({"detail": "Account with this path already exists"})
    );
}

#[tokio::test]
async fn delete_account() {
    let mut app = TestApp::signed_in().await;
    let created = app
        .post(
            "/api/accounts",
            json!({"name": "ToDelete", "full_path": "ToDelete", "type": "expense"}),
        )
        .await;
    let id = created.body["id"].as_i64().unwrap();
    app.delete(&format!("/api/accounts/{id}"))
        .await
        .assert_status(StatusCode::NO_CONTENT);
    app.get(&format!("/api/accounts/{id}"))
        .await
        .assert_status(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn accounts_need_a_session() {
    let mut app = TestApp::new().await;
    for path in [
        "/api/accounts",
        "/api/setup/status",
        "/api/lookup/match-types",
    ] {
        app.get(path).await.assert_status(StatusCode::UNAUTHORIZED);
    }
}

#[tokio::test]
async fn seeding_a_new_install() {
    let mut app = TestApp::signed_in().await;
    assert_eq!(
        app.get("/api/setup/status").await.body,
        json!({"has_accounts": false, "has_rules": false})
    );
    app.post("/api/setup/seed-rules", json!({}))
        .await
        .assert_status(StatusCode::BAD_REQUEST);

    let seeded = app.post("/api/setup/seed", json!({})).await;
    assert_eq!(
        seeded.body,
        json!({"accounts_created": 39, "rules_created": 20})
    );
    assert_eq!(
        app.get("/api/setup/status").await.body,
        json!({"has_accounts": true, "has_rules": true})
    );
    let again = app.post("/api/setup/seed", json!({})).await;
    again.assert_status(StatusCode::BAD_REQUEST);
    assert_eq!(again.body, json!({"detail": "Accounts already exist"}));
    let rules_again = app.post("/api/setup/seed-rules", json!({})).await;
    assert_eq!(rules_again.body, json!({"detail": "Rules already exist"}));
    assert_eq!(
        app.post("/api/setup/sync-seed-icons", json!({})).await.body,
        json!({"ok": true})
    );
}

#[tokio::test]
async fn lookups() {
    let mut app = TestApp::signed_in().await;
    assert_eq!(
        app.get("/api/lookup/match-types").await.body,
        json!([
            {"id": 1, "name": "substring"},
            {"id": 2, "name": "prefix"},
            {"id": 3, "name": "regex"}
        ])
    );
    let sources = app.get("/api/lookup/categorisation-sources").await.body;
    assert_eq!(sources.as_array().unwrap().len(), 6);
    assert_eq!(sources[1], json!({"id": 2, "name": "manual"}));
}

#[tokio::test]
async fn the_tree_can_be_one_type() {
    let mut app = seeded().await;
    let liabilities = app
        .get("/api/accounts/tree/hierarchy?account_type=liability")
        .await
        .body;
    let paths: Vec<&str> = liabilities
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["full_path"].as_str().unwrap())
        .collect();
    assert_eq!(
        paths,
        ["Liabilities", "Liabilities:CreditCard", "Liabilities:Loan"]
    );
    let all = app.get("/api/accounts/tree/hierarchy").await.body;
    assert_eq!(all.as_array().unwrap().len(), 39);
}

#[tokio::test]
async fn opening_balances() {
    let mut app = seeded().await;
    let current = id_of(&mut app, "Assets:Bank:Current").await;
    let path = format!("/api/accounts/{current}/opening-balance");
    assert_eq!(app.get(&path).await.body, Value::Null);

    let created = app
        .put(&path, json!({"amount": "12.5", "date": "2026-01-01"}))
        .await;
    created.assert_status(StatusCode::OK);
    assert_eq!(created.body["amount"], "12.5000");
    assert_eq!(created.body["date"], "2026-01-01");

    let updated = app
        .put(&path, json!({"amount": "13.25", "date": "2026-01-02"}))
        .await;
    assert_eq!(
        updated.body["transaction_id"],
        created.body["transaction_id"]
    );
    assert_eq!(app.get(&path).await.body, updated.body);
    assert_eq!(updated.body["amount"], "13.2500");

    app.put(&path, json!({"amount": "abc", "date": "2026-01-02"}))
        .await
        .assert_status(StatusCode::UNPROCESSABLE_ENTITY);
    let food = id_of(&mut app, "Expenses:Food").await;
    app.put(
        &format!("/api/accounts/{food}/opening-balance"),
        json!({"amount": "1", "date": "2026-01-02"}),
    )
    .await
    .assert_status(StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn moving_an_account_moves_its_subaccounts() {
    let mut app = seeded().await;
    let bank = id_of(&mut app, "Assets:Bank").await;
    let moved = app
        .put(
            &format!("/api/accounts/{bank}"),
            json!({"full_path": "Assets:Banks"}),
        )
        .await;
    moved.assert_status(StatusCode::OK);
    assert_eq!(moved.body["name"], "Bank", "only what's given changes");
    id_of(&mut app, "Assets:Banks:Current").await;

    let cleared = app
        .put(&format!("/api/accounts/{bank}"), json!({"icon": null}))
        .await;
    assert_eq!(cleared.body["full_path"], "Assets:Banks");
}

#[tokio::test]
async fn accounts_in_use_cant_be_deleted() {
    let mut app = seeded().await;
    let food = id_of(&mut app, "Expenses:Food").await;
    let refused = app.delete(&format!("/api/accounts/{food}")).await;
    refused.assert_status(StatusCode::BAD_REQUEST);
    assert_eq!(
        refused.body,
        json!({"detail": "Cannot delete account with children"})
    );

    let current = id_of(&mut app, "Assets:Bank:Current").await;
    app.put(
        &format!("/api/accounts/{current}/opening-balance"),
        json!({"amount": "10", "date": "2026-01-01"}),
    )
    .await;
    let in_use = app.delete(&format!("/api/accounts/{current}")).await;
    in_use.assert_status(StatusCode::BAD_REQUEST);
    assert!(
        in_use.body["detail"]
            .as_str()
            .unwrap()
            .contains("transactions")
    );
}

#[tokio::test]
async fn export_and_import() {
    let mut app = seeded().await;
    let exported = app.get("/api/accounts/export").await;
    exported.assert_status(StatusCode::OK);
    let accounts = exported.body.as_array().unwrap();
    assert_eq!(accounts.len(), 39);
    assert_eq!(
        accounts
            .iter()
            .find(|a| a["full_path"] == "Assets:Bank")
            .unwrap(),
        &json!({"name": "Bank", "full_path": "Assets:Bank", "type": "asset", "parent_path": "Assets"})
    );

    let refused = app
        .post("/api/accounts/import", exported.body.clone())
        .await;
    refused.assert_status(StatusCode::BAD_REQUEST);

    let mut fresh = TestApp::signed_in().await;
    let imported = fresh
        .post("/api/accounts/import", exported.body.clone())
        .await;
    assert_eq!(imported.body, json!({"accounts_created": 39}));
    assert_eq!(fresh.get("/api/accounts/export").await.body, exported.body);
}

#[tokio::test]
async fn ids_that_arent_numbers_are_refused() {
    let mut app = TestApp::signed_in().await;
    let response = app.get("/api/accounts/abc").await;
    response.assert_status(StatusCode::UNPROCESSABLE_ENTITY);
    assert!(response.body["detail"].is_string());
}
