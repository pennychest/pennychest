//! The report endpoints. The sums behind them are tested in the core crate.

mod common;

use axum::http::StatusCode;
use common::TestApp;
use serde_json::{Value, json};

const MANUAL: i64 = 2;

async fn account(app: &mut TestApp, path: &str, kind: &str) -> i64 {
    let name = path.rsplit(':').next().unwrap();
    let response = app
        .post(
            "/api/accounts",
            json!({"name": name, "full_path": path, "type": kind}),
        )
        .await;
    response.body["id"].as_i64().unwrap()
}

async fn txn(app: &mut TestApp, date: &str, status: &str, postings: &[(i64, &str)]) {
    let postings: Vec<Value> = postings
        .iter()
        .map(
            |(id, amount)| json!({"account_id": id, "amount": amount, "categorised_by_id": MANUAL}),
        )
        .collect();
    app.post(
        "/api/transactions",
        json!({"date": date, "description": "x", "status": status, "postings": postings}),
    )
    .await
    .assert_status(StatusCode::CREATED);
}

#[tokio::test]
async fn summary_and_spending_by_category() {
    let mut app = TestApp::signed_in().await;
    let bank = account(&mut app, "Assets:Current", "asset").await;
    let card = account(&mut app, "Liabilities:Visa", "liability").await;
    let salary = account(&mut app, "Income:Salary", "income").await;
    let food = account(&mut app, "Expenses:Food", "expense").await;
    let fuel = account(&mut app, "Expenses:Fuel", "expense").await;
    txn(
        &mut app,
        "2026-08-01",
        "confirmed",
        &[(bank, "1000"), (salary, "-1000")],
    )
    .await;
    txn(
        &mut app,
        "2026-08-02",
        "confirmed",
        &[(food, "0.1"), (bank, "-0.1")],
    )
    .await;
    txn(
        &mut app,
        "2026-08-03",
        "confirmed",
        &[(food, "0.2"), (card, "-0.2")],
    )
    .await;
    txn(
        &mut app,
        "2026-08-04",
        "confirmed",
        &[(fuel, "50"), (card, "-50")],
    )
    .await;
    txn(
        &mut app,
        "2026-08-05",
        "pending",
        &[(fuel, "20"), (bank, "-20")],
    )
    .await;
    txn(
        &mut app,
        "2026-09-01",
        "confirmed",
        &[(food, "9"), (bank, "-9")],
    )
    .await;

    let summary = app
        .get("/api/reports/summary?date_from=2026-08-01&date_to=2026-08-31")
        .await;
    summary.assert_status(StatusCode::OK);
    assert_eq!(
        summary.body,
        json!({
            // Exact: 1000 - 0.1 - 0.2 - 50 - 9
            "net_worth": {"GBP": 940.7},
            "income": 1000.0,
            "expenses": 50.3,
            "savings_rate": 95.0,
            "date_from": "2026-08-01",
            "date_to": "2026-08-31",
        })
    );

    let spending = app
        .get("/api/reports/expenses-by-category?date_from=2026-08-01&date_to=2026-08-31")
        .await;
    assert_eq!(
        spending.body,
        json!([
            {"full_path": "Expenses:Fuel", "name": "Fuel", "total": 50.0},
            {"full_path": "Expenses:Food", "name": "Food", "total": 0.3},
        ])
    );
    let under = app
        .get("/api/reports/expenses-by-category?date_from=2026-08-01&date_to=2026-08-31&parent_path=Expenses:Fuel")
        .await;
    assert_eq!(under.body, json!([]));

    // This month so far, by default.
    let summary = app.get("/api/reports/summary").await;
    assert_eq!(summary.body["date_from"].as_str().unwrap().len(), 10);
    assert!(summary.body["date_from"].as_str().unwrap().ends_with("-01"));

    let bad = app.get("/api/reports/summary?date_from=yesterday").await;
    bad.assert_status(StatusCode::UNPROCESSABLE_ENTITY);
}
