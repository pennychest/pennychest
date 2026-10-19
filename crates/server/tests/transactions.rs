//! Listing, updating, deleting and bulk confirming transactions.

mod common;

use axum::http::StatusCode;
use common::TestApp;
use serde_json::{Value, json};

const MANUAL: i64 = 2;

async fn create_accounts(app: &mut TestApp) -> (i64, i64) {
    let checking = app
        .post(
            "/api/accounts",
            json!({"name": "Checking", "full_path": "Assets:Checking", "type": "asset"}),
        )
        .await;
    let groceries = app
        .post(
            "/api/accounts",
            json!({"name": "Groceries", "full_path": "Expenses:Groceries", "type": "expense"}),
        )
        .await;
    (
        checking.body["id"].as_i64().unwrap(),
        groceries.body["id"].as_i64().unwrap(),
    )
}

async fn manual_source_id(app: &mut TestApp) -> i64 {
    let sources = app.get("/api/lookup/categorisation-sources").await.body;
    sources
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == "manual")
        .unwrap()["id"]
        .as_i64()
        .unwrap()
}

fn spend(date: &str, description: &str, category: i64, bank: i64, amount: &str) -> Value {
    json!({
        "date": date,
        "description": description,
        "postings": [
            {"account_id": category, "amount": amount, "categorised_by_id": MANUAL},
            {"account_id": bank, "amount": format!("-{amount}"), "categorised_by_id": MANUAL},
        ]
    })
}

#[tokio::test]
async fn create_transaction() {
    let mut app = TestApp::signed_in().await;
    let (a1, a2) = create_accounts(&mut app).await;
    let source = manual_source_id(&mut app).await;
    let response = app
        .post(
            "/api/transactions",
            json!({
                "date": "2026-01-15",
                "description": "Tesco groceries",
                "postings": [
                    {"account_id": a2, "amount": "50.00", "categorised_by_id": source},
                    {"account_id": a1, "amount": "-50.00", "categorised_by_id": source},
                ]
            }),
        )
        .await;
    response.assert_status(StatusCode::CREATED);
    assert_eq!(response.body["description"], "Tesco groceries");
    assert_eq!(response.body["postings"].as_array().unwrap().len(), 2);
    assert_eq!(response.body["postings"][0]["amount"], "50.0000");
    assert_eq!(
        response.body["postings"][1]["account_full_path"],
        "Assets:Checking"
    );
}

#[tokio::test]
async fn transaction_must_balance() {
    let mut app = TestApp::signed_in().await;
    let (a1, a2) = create_accounts(&mut app).await;
    let response = app
        .post(
            "/api/transactions",
            json!({
                "date": "2026-01-15",
                "description": "Unbalanced",
                "postings": [
                    {"account_id": a2, "amount": "50.00", "categorised_by_id": MANUAL},
                    {"account_id": a1, "amount": "-30.00", "categorised_by_id": MANUAL},
                ]
            }),
        )
        .await;
    response.assert_status(StatusCode::BAD_REQUEST);
    assert_eq!(
        response.body,
        json!({"detail": "Postings must sum to zero, got 20"})
    );
}

#[tokio::test]
async fn transaction_needs_two_postings() {
    let mut app = TestApp::signed_in().await;
    let (a1, _) = create_accounts(&mut app).await;
    let response = app
        .post(
            "/api/transactions",
            json!({
                "date": "2026-01-15",
                "description": "Single posting",
                "postings": [{"account_id": a1, "amount": "0", "categorised_by_id": MANUAL}]
            }),
        )
        .await;
    response.assert_status(StatusCode::BAD_REQUEST);
    assert_eq!(
        response.body,
        json!({"detail": "Transaction must have at least 2 postings"})
    );
}

#[tokio::test]
async fn confirm_transaction() {
    let mut app = TestApp::signed_in().await;
    let (a1, a2) = create_accounts(&mut app).await;
    let txn = app
        .post(
            "/api/transactions",
            spend("2026-01-15", "Pending transaction", a2, a1, "25.00"),
        )
        .await
        .body;
    assert_eq!(txn["status"], "pending");
    let response = app
        .post(
            &format!("/api/transactions/{}/confirm", txn["id"]),
            json!({}),
        )
        .await;
    response.assert_status(StatusCode::OK);
    assert_eq!(response.body["status"], "confirmed");
}

#[tokio::test]
async fn list_transactions_with_filters() {
    let mut app = TestApp::signed_in().await;
    let (a1, a2) = create_accounts(&mut app).await;
    app.post(
        "/api/transactions",
        spend("2026-01-15", "Searchable transaction", a2, a1, "10.00"),
    )
    .await;
    let found = app.get("/api/transactions?search=Searchable").await;
    found.assert_status(StatusCode::OK);
    assert!(!found.body.as_array().unwrap().is_empty());
    app.get("/api/transactions?status=pending")
        .await
        .assert_status(StatusCode::OK);
}

#[tokio::test]
async fn filter_by_category_includes_subcategories_and_dates() {
    let mut app = TestApp::signed_in().await;
    let (bank, _) = create_accounts(&mut app).await;
    let food = app
        .post(
            "/api/accounts",
            json!({"name": "Food", "full_path": "Expenses:Food", "type": "expense"}),
        )
        .await
        .body["id"]
        .as_i64()
        .unwrap();
    let takeaway = app
        .post(
            "/api/accounts",
            json!({"name": "Takeaway", "full_path": "Expenses:Food:Takeaway", "type": "expense",
                   "parent_id": food}),
        )
        .await
        .body["id"]
        .as_i64()
        .unwrap();
    // A sibling whose name shares the prefix must not count as a subcategory
    let foodbank = app
        .post(
            "/api/accounts",
            json!({"name": "Foodbank", "full_path": "Expenses:Foodbank", "type": "expense"}),
        )
        .await
        .body["id"]
        .as_i64()
        .unwrap();
    for (day, description, category) in [
        ("2026-01-10", "Market", food),
        ("2026-02-10", "Pizza", takeaway),
        ("2026-02-11", "Donation", foodbank),
    ] {
        app.post(
            "/api/transactions",
            spend(day, description, category, bank, "5.00"),
        )
        .await
        .assert_status(StatusCode::CREATED);
    }

    async fn descriptions(app: &mut TestApp, query: &str) -> Vec<String> {
        let response = app.get(&format!("/api/transactions?{query}")).await;
        response.assert_status(StatusCode::OK);
        let mut found: Vec<String> = response
            .body
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["description"].as_str().unwrap().to_owned())
            .collect();
        found.sort();
        found
    }
    assert_eq!(
        descriptions(&mut app, &format!("category_id={food}")).await,
        ["Market", "Pizza"]
    );
    assert_eq!(
        descriptions(&mut app, &format!("category_id={takeaway}")).await,
        ["Pizza"]
    );
    assert_eq!(
        descriptions(
            &mut app,
            &format!("category_id={food}&date_from=2026-02-01&date_to=2026-02-28")
        )
        .await,
        ["Pizza"]
    );
    app.get("/api/transactions?category_id=999999")
        .await
        .assert_status(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn count_matches_the_same_filters_as_the_list() {
    let mut app = TestApp::signed_in().await;
    let (bank, groceries) = create_accounts(&mut app).await;
    for (day, description) in [
        ("2026-01-10", "Tesco"),
        ("2026-02-10", "Tesco"),
        ("2026-02-11", "Petrol"),
    ] {
        app.post(
            "/api/transactions",
            spend(day, description, groceries, bank, "5.00"),
        )
        .await;
    }
    async fn count(app: &mut TestApp, query: &str) -> i64 {
        app.get(&format!("/api/transactions/count?{query}"))
            .await
            .body["count"]
            .as_i64()
            .unwrap()
    }
    assert_eq!(count(&mut app, "").await, 3);
    assert_eq!(count(&mut app, "search=tesco").await, 2);
    assert_eq!(
        count(&mut app, "search=tesco&date_from=2026-02-01").await,
        1
    );
    assert_eq!(
        count(&mut app, &format!("category_id={groceries}")).await,
        3
    );
    assert_eq!(count(&mut app, &format!("account_id={bank}")).await, 3);
}

#[tokio::test]
async fn pages() {
    let mut app = TestApp::signed_in().await;
    let (bank, groceries) = create_accounts(&mut app).await;
    for day in ["2026-01-01", "2026-01-02", "2026-01-03"] {
        app.post("/api/transactions", spend(day, day, groceries, bank, "1"))
            .await;
    }
    let page = app.get("/api/transactions?page=2&per_page=2").await.body;
    assert_eq!(page.as_array().unwrap().len(), 1);
    assert_eq!(page[0]["description"], "2026-01-01");
    for bad in ["page=0", "per_page=0", "per_page=201", "page=abc"] {
        app.get(&format!("/api/transactions?{bad}"))
            .await
            .assert_status(StatusCode::UNPROCESSABLE_ENTITY);
    }
}

#[tokio::test]
async fn update_delete_and_bulk_confirm() {
    let mut app = TestApp::signed_in().await;
    let (bank, groceries) = create_accounts(&mut app).await;
    let first = app
        .post(
            "/api/transactions",
            spend("2026-01-01", "a", groceries, bank, "1"),
        )
        .await
        .body["id"]
        .as_i64()
        .unwrap();
    let second = app
        .post(
            "/api/transactions",
            spend("2026-01-02", "b", groceries, bank, "1"),
        )
        .await
        .body["id"]
        .as_i64()
        .unwrap();

    let updated = app
        .put(
            &format!("/api/transactions/{first}"),
            json!({"description": "A", "date": "2026-01-05"}),
        )
        .await;
    assert_eq!(updated.body["description"], "A");
    assert_eq!(updated.body["date"], "2026-01-05");
    let bad_status = app
        .put(
            &format!("/api/transactions/{first}"),
            json!({"status": "done"}),
        )
        .await;
    bad_status.assert_status(StatusCode::BAD_REQUEST);
    assert_eq!(
        bad_status.body,
        json!({"detail": "Status must be 'pending' or 'confirmed'"})
    );

    let confirmed = app
        .post(
            "/api/transactions/bulk-confirm",
            json!([first, second, 999]),
        )
        .await;
    assert_eq!(confirmed.body, json!({"confirmed": 2}));

    app.delete(&format!("/api/transactions/{first}"))
        .await
        .assert_status(StatusCode::NO_CONTENT);
    app.get(&format!("/api/transactions/{first}"))
        .await
        .assert_status(StatusCode::NOT_FOUND);
    app.delete(&format!("/api/transactions/{first}"))
        .await
        .assert_status(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn bad_sources_and_statuses_are_refused_not_crashes() {
    let mut app = TestApp::signed_in().await;
    let (bank, groceries) = create_accounts(&mut app).await;
    let mut body = spend("2026-01-01", "x", groceries, bank, "1");
    body["postings"][0]["categorised_by_id"] = json!(99);
    let response = app.post("/api/transactions", body).await;
    response.assert_status(StatusCode::BAD_REQUEST);
    assert_eq!(
        response.body,
        json!({"detail": "Categorisation source 99 not found"})
    );

    let mut body = spend("2026-01-01", "x", groceries, bank, "1");
    body["status"] = json!("done");
    app.post("/api/transactions", body)
        .await
        .assert_status(StatusCode::BAD_REQUEST);

    let body = spend("2026-01-01", "x", 999, bank, "1");
    let response = app.post("/api/transactions", body).await;
    assert_eq!(response.body, json!({"detail": "Account 999 not found"}));
}
