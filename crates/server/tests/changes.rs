//! Changes made by AI, and undoing them.

mod common;
mod fixtures;

use axum::http::StatusCode;
use common::{TestApp, TestResponse};
use fixtures::{imported, ledger, ok, postings, spending, statement_row};
use serde_json::{Value, json};

async fn changes(app: &mut TestApp) -> Vec<Value> {
    app.get("/api/changes").await.body["changes"]
        .as_array()
        .unwrap()
        .clone()
}

async fn undo(app: &mut TestApp, id: &Value) -> TestResponse {
    app.post(&format!("/api/changes/{id}/undo"), json!({}))
        .await
}

/// Undoes the change an action's result reports, which must work.
async fn undone(app: &mut TestApp, result: &Value) -> Value {
    let response = undo(app, &result["change"]["id"]).await;
    assert_eq!(response.status, StatusCode::OK, "{}", response.body);
    response.body
}

async fn category_paths(app: &mut TestApp) -> Vec<String> {
    ok(app, "list_categories", json!({})).await["categories"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["path"].as_str().unwrap().to_owned())
        .collect()
}

fn detail(response: &TestResponse) -> &str {
    response.body["detail"].as_str().unwrap()
}

// Recording

#[tokio::test]
async fn changes_are_recorded_with_a_summary() {
    let mut app = TestApp::signed_in().await;
    ledger(&mut app).await;
    assert!(
        ok(&mut app, "list_categories", json!({}))
            .await
            .get("change")
            .is_none()
    );
    let result = ok(
        &mut app,
        "create_category",
        json!({"name": "Amazon Prime", "parent": "Food"}),
    )
    .await;
    assert_eq!(
        result["change"]["summary"],
        "Created category \"Amazon Prime\" under Expenses:Food"
    );
    let logged = changes(&mut app).await;
    assert_eq!(logged.len(), 1);
    assert_eq!(logged[0]["id"], result["change"]["id"]);
    assert_eq!(logged[0]["action"], "create_category");
    assert_eq!(logged[0]["source"], "api");
    assert_eq!(logged[0]["undone_at"], Value::Null);

    let nothing = ok(
        &mut app,
        "recategorise_transactions",
        json!({"transaction_ids": [999999], "category": "Food"}),
    )
    .await;
    assert!(nothing.get("change").is_none());
    assert_eq!(changes(&mut app).await.len(), 1);
    let page = app.get("/api/changes?limit=1&offset=1").await.body;
    assert_eq!(page["total"], 1);
    assert_eq!(page["changes"], json!([]));
}

#[tokio::test]
async fn undo_endpoint_errors() {
    let mut app = TestApp::signed_in().await;
    ledger(&mut app).await;
    undo(&mut app, &json!(12345))
        .await
        .assert_status(StatusCode::NOT_FOUND);
    let result = ok(
        &mut app,
        "create_rule",
        json!({"pattern": "DELIVEROO", "category": "Restaurants"}),
    )
    .await;
    undone(&mut app, &result).await;
    let again = undo(&mut app, &result["change"]["id"]).await;
    again.assert_status(StatusCode::BAD_REQUEST);
    assert!(detail(&again).contains("already been undone"));
    assert_ne!(changes(&mut app).await[0]["undone_at"], Value::Null);
}

#[tokio::test]
async fn changes_require_a_session() {
    let mut app = TestApp::new().await;
    app.get("/api/changes")
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
    undo(&mut app, &json!(1))
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}

// Undoing each kind of change

#[tokio::test]
async fn undo_creating_a_category_unless_it_is_now_used() {
    let mut app = TestApp::signed_in().await;
    ledger(&mut app).await;
    let gym = ok(
        &mut app,
        "create_category",
        json!({"name": "Gym", "parent": "Expenses"}),
    )
    .await;
    undone(&mut app, &gym).await;
    assert!(
        !category_paths(&mut app)
            .await
            .contains(&"Expenses:Gym".into())
    );

    let pets = ok(
        &mut app,
        "create_category",
        json!({"name": "Pets", "parent": "Expenses"}),
    )
    .await;
    ok(
        &mut app,
        "add_transaction",
        json!({"description": "Vet", "amount": "40", "category": "Pets", "account": "Current"}),
    )
    .await;
    let response = undo(&mut app, &pets["change"]["id"]).await;
    response.assert_status(StatusCode::BAD_REQUEST);
    assert!(detail(&response).contains("now has transactions"));
    assert!(
        category_paths(&mut app)
            .await
            .contains(&"Expenses:Pets".into())
    );
}

#[tokio::test]
async fn undo_renaming_a_category() {
    let mut app = TestApp::signed_in().await;
    let ids = ledger(&mut app).await;
    spending(&mut app, &ids).await;
    let renamed = ok(
        &mut app,
        "rename_category",
        json!({"category": "Food", "new_name": "Food & Drink"}),
    )
    .await;
    undone(&mut app, &renamed).await;
    let paths = category_paths(&mut app).await;
    assert!(paths.contains(&"Expenses:Food".into()));
    assert!(paths.contains(&"Expenses:Food:Groceries".into()));
    assert!(!paths.contains(&"Expenses:FoodDrink".into()));
    let food = app
        .get(&format!("/api/accounts/{}", ids["Expenses:Food"]))
        .await;
    assert_eq!(food.body["name"], "Food");

    let first = ok(
        &mut app,
        "rename_category",
        json!({"category": "Transport", "new_name": "Travel"}),
    )
    .await;
    ok(
        &mut app,
        "rename_category",
        json!({"category": "Travel", "new_name": "Getting Around"}),
    )
    .await;
    let response = undo(&mut app, &first["change"]["id"]).await;
    response.assert_status(StatusCode::BAD_REQUEST);
    assert!(detail(&response).contains("renamed again"));
}

#[tokio::test]
async fn undo_budgets() {
    let mut app = TestApp::signed_in().await;
    ledger(&mut app).await;
    let new = ok(
        &mut app,
        "set_budget",
        json!({"category": "Food", "amount": "200"}),
    )
    .await;
    undone(&mut app, &new).await;
    assert_eq!(
        ok(&mut app, "get_budgets", json!({})).await["budgets"],
        json!([])
    );

    ok(
        &mut app,
        "set_budget",
        json!({"category": "Food", "amount": "200"}),
    )
    .await;
    let raised = ok(
        &mut app,
        "set_budget",
        json!({"category": "Food", "amount": "350"}),
    )
    .await;
    undone(&mut app, &raised).await;
    let limit = |budgets: Value| budgets["budgets"][0]["limit"].clone();
    assert_eq!(
        limit(ok(&mut app, "get_budgets", json!({})).await),
        "200.00"
    );

    let removed = ok(&mut app, "delete_budget", json!({"category": "Food"})).await;
    undone(&mut app, &removed).await;
    assert_eq!(
        limit(ok(&mut app, "get_budgets", json!({})).await),
        "200.00"
    );
}

#[tokio::test]
async fn undo_recategorising() {
    let mut app = TestApp::signed_in().await;
    let ids = ledger(&mut app).await;
    let s = spending(&mut app, &ids).await;
    let moved = ok(
        &mut app,
        "recategorise_transactions",
        json!({"category": "Transport", "transaction_ids": [s["tesco_aug"], s["pizza"]]}),
    )
    .await;
    undone(&mut app, &moved).await;
    let all = ok(&mut app, "list_transactions", json!({})).await;
    let category = |id: i64| {
        all["transactions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["id"] == id)
            .unwrap()["category"]
            .clone()
    };
    assert_eq!(category(s["tesco_aug"]), "Expenses:Food:Groceries");
    assert_eq!(category(s["pizza"]), "Expenses:Food:Restaurants");
}

#[tokio::test]
async fn undo_added_transactions_and_transfers() {
    let mut app = TestApp::signed_in().await;
    ledger(&mut app).await;
    let coffee = ok(
        &mut app,
        "add_transaction",
        json!({"description": "Coffee", "amount": "3", "category": "Restaurants", "account": "Current"}),
    )
    .await;
    let transfer = ok(
        &mut app,
        "add_transfer",
        json!({"amount": "50", "from_account": "Current", "to_account": "Savings"}),
    )
    .await;
    assert!(
        coffee["change"]["summary"]
            .as_str()
            .unwrap()
            .contains("Coffee")
    );
    assert_eq!(
        transfer["change"]["summary"],
        "Added a £50.00 transfer from Assets:Bank:Current to Assets:Bank:Savings"
    );
    undone(&mut app, &coffee).await;
    undone(&mut app, &transfer).await;
    assert_eq!(
        ok(&mut app, "list_transactions", json!({})).await["total"],
        0
    );
}

#[tokio::test]
async fn undoing_an_edit_restores_every_field() {
    let mut app = TestApp::signed_in().await;
    ledger(&mut app).await;
    let txn = ok(
        &mut app,
        "add_transaction",
        json!({"description": "Coffee", "amount": "3", "category": "Restaurants", "account": "Current", "date": "2026-08-01"}),
    )
    .await["added"]
        .clone();
    let edited = ok(
        &mut app,
        "edit_transaction",
        json!({
            "transaction_id": txn["id"],
            "description": "Pay",
            "date": "2026-08-09",
            "amount": "1500",
            "category": "Salary",
            "account": "Visa",
        }),
    )
    .await;
    assert_eq!(
        edited["change"]["summary"],
        "Edited the description, date, amount, category, account of \"Coffee\" (2026-08-01)"
    );
    undone(&mut app, &edited).await;
    let after = ok(&mut app, "list_transactions", json!({})).await["transactions"][0].clone();
    assert_eq!(after, txn);
}

async fn tap_transaction(app: &TestApp, tap: i64) -> Option<i64> {
    sqlx::query_scalar("SELECT transaction_id FROM card_taps WHERE id = ?")
        .bind(tap)
        .fetch_one(&app.db)
        .await
        .unwrap()
}

#[tokio::test]
async fn undoing_a_delete_puts_transactions_back_with_their_links() {
    let mut app = TestApp::signed_in().await;
    let ids = ledger(&mut app).await;
    let s = spending(&mut app, &ids).await;
    let statement = imported(&mut app, &ids).await;
    let tap: i64 = sqlx::query_scalar(
        "INSERT INTO card_taps (tapped_on, merchant, amount, transaction_id)
         VALUES ('2026-08-05', 'Tesco', 200000, ?) RETURNING id",
    )
    .bind(statement)
    .fetch_one(&app.db)
    .await
    .unwrap();
    let before = ok(&mut app, "list_transactions", json!({})).await["transactions"].clone();
    let postings_before = postings(&mut app, statement).await;

    let deleted = ok(
        &mut app,
        "delete_transactions",
        json!({"transaction_ids": [statement, s["transfer"]]}),
    )
    .await;
    assert_eq!(deleted["change"]["summary"], "Deleted 2 transactions");
    assert_eq!(tap_transaction(&app, tap).await, None);

    undone(&mut app, &deleted).await;
    let after = ok(&mut app, "list_transactions", json!({})).await["transactions"].clone();
    assert_eq!(after, before);
    assert_eq!(postings(&mut app, statement).await, postings_before);
    assert_eq!(statement_row(&app).await.0, Some(statement));
    assert_eq!(tap_transaction(&app, tap).await, Some(statement));

    // Undoing it twice would put back transactions that are already there.
    let again = undo(&mut app, &deleted["change"]["id"]).await;
    again.assert_status(StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn undo_marking_reviewed() {
    let mut app = TestApp::signed_in().await;
    let ids = ledger(&mut app).await;
    let s = spending(&mut app, &ids).await;
    let reviewed = ok(
        &mut app,
        "mark_reviewed",
        json!({"transaction_ids": [s["pending"]]}),
    )
    .await;
    undone(&mut app, &reviewed).await;
    let pending = ok(&mut app, "list_transactions", json!({"status": "pending"})).await;
    assert_eq!(pending["transactions"][0]["id"], s["pending"]);
    assert_eq!(pending["total"], 1);
}
