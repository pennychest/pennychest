//! The budgets endpoints (the period tests are in the core crate).

mod common;

use std::collections::HashMap;

use axum::http::StatusCode;
use chrono::Local;
use common::{TestApp, TestResponse};
use serde_json::{Value, json};

const MANUAL: i64 = 2;

/// A bank account plus Food (with a Groceries subcategory) and Transport.
async fn ledger(app: &mut TestApp) -> HashMap<&'static str, i64> {
    let mut ids = HashMap::new();
    for (name, path, kind) in [
        ("Bank", "Assets:Bank", "asset"),
        ("Expenses", "Expenses", "expense"),
        ("Food", "Expenses:Food", "expense"),
        ("Groceries", "Expenses:Food:Groceries", "expense"),
        ("Foodbank", "Expenses:Foodbank", "expense"),
        ("Transport", "Expenses:Transport", "expense"),
        ("Salary", "Income:Salary", "income"),
    ] {
        let response = app
            .post(
                "/api/accounts",
                json!({"name": name, "full_path": path, "type": kind}),
            )
            .await;
        ids.insert(path, response.body["id"].as_i64().unwrap());
    }
    ids
}

async fn spend(
    app: &mut TestApp,
    ids: &HashMap<&str, i64>,
    category: &str,
    amount: &str,
    day: &str,
    confirmed: bool,
) {
    let negated = amount
        .strip_prefix('-')
        .map(str::to_owned)
        .unwrap_or_else(|| format!("-{amount}"));
    let status = if confirmed { "confirmed" } else { "pending" };
    app.post(
        "/api/transactions",
        json!({
            "date": day,
            "description": format!("{category} {amount}"),
            "status": status,
            "postings": [
                {"account_id": ids["Assets:Bank"], "amount": negated, "categorised_by_id": MANUAL},
                {"account_id": ids[category], "amount": amount, "categorised_by_id": MANUAL},
            ],
        }),
    )
    .await
    .assert_status(StatusCode::CREATED);
}

async fn budget(
    app: &mut TestApp,
    ids: &HashMap<&str, i64>,
    category: &str,
    amount: &str,
    period: &str,
) -> TestResponse {
    app.post(
        "/api/budgets",
        json!({"account_id": ids[category], "amount": amount, "period": period}),
    )
    .await
}

async fn by_category(app: &mut TestApp, on: &str) -> HashMap<String, Value> {
    let budgets = app.get(&format!("/api/budgets?on={on}")).await.body;
    budgets
        .as_array()
        .unwrap()
        .iter()
        .map(|b| {
            (
                b["account_full_path"].as_str().unwrap().to_owned(),
                b.clone(),
            )
        })
        .collect()
}

#[tokio::test]
async fn budgets_require_a_session() {
    let mut app = TestApp::new().await;
    app.get("/api/budgets")
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
    app.post("/api/budgets", json!({}))
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn create_and_list_a_budget_with_spending() {
    let mut app = TestApp::signed_in().await;
    let ids = ledger(&mut app).await;
    budget(&mut app, &ids, "Expenses:Food", "300", "monthly")
        .await
        .assert_status(StatusCode::CREATED);

    spend(&mut app, &ids, "Expenses:Food", "20.00", "2026-09-02", true).await;
    spend(
        &mut app,
        &ids,
        "Expenses:Food:Groceries",
        "45.50",
        "2026-09-10",
        true,
    )
    .await;
    spend(
        &mut app,
        &ids,
        "Expenses:Food:Groceries",
        "10.00",
        "2026-09-20",
        false,
    )
    .await;
    spend(&mut app, &ids, "Expenses:Food", "99.00", "2026-08-31", true).await; // previous month
    spend(
        &mut app,
        &ids,
        "Expenses:Transport",
        "15.00",
        "2026-09-05",
        true,
    )
    .await; // other category
    spend(
        &mut app,
        &ids,
        "Expenses:Foodbank",
        "7.00",
        "2026-09-05",
        true,
    )
    .await; // similar name only

    let food = &by_category(&mut app, "2026-09-15").await["Expenses:Food"];
    assert_eq!(food["period"], "monthly");
    assert_eq!(food["period_start"], "2026-09-01");
    assert_eq!(food["period_end"], "2026-09-30");
    assert_eq!(food["amount"], "300.0000");
    assert_eq!(food["spent"], "75.5000");
    assert_eq!(food["unreviewed"], "10.0000");
    assert_eq!(food["remaining"], "224.5000");

    let august = by_category(&mut app, "2026-08-10").await;
    assert_eq!(august["Expenses:Food"]["spent"], "99.0000");
}

#[tokio::test]
async fn refunds_reduce_spending_and_overspending_goes_negative() {
    let mut app = TestApp::signed_in().await;
    let ids = ledger(&mut app).await;
    budget(&mut app, &ids, "Expenses:Transport", "50", "monthly").await;
    spend(
        &mut app,
        &ids,
        "Expenses:Transport",
        "80.00",
        "2026-09-03",
        true,
    )
    .await;
    spend(
        &mut app,
        &ids,
        "Expenses:Transport",
        "-10.00",
        "2026-09-04",
        true,
    )
    .await;

    let transport = &by_category(&mut app, "2026-09-30").await["Expenses:Transport"];
    assert_eq!(transport["spent"], "70.0000");
    assert_eq!(transport["remaining"], "-20.0000");
}

#[tokio::test]
async fn an_annual_budget_covers_the_calendar_year() {
    let mut app = TestApp::signed_in().await;
    let ids = ledger(&mut app).await;
    budget(&mut app, &ids, "Expenses:Transport", "1200", "annual").await;
    spend(
        &mut app,
        &ids,
        "Expenses:Transport",
        "100.00",
        "2026-01-01",
        true,
    )
    .await;
    spend(
        &mut app,
        &ids,
        "Expenses:Transport",
        "200.00",
        "2026-12-31",
        true,
    )
    .await;
    spend(
        &mut app,
        &ids,
        "Expenses:Transport",
        "999.00",
        "2025-12-31",
        true,
    )
    .await;

    let annual = &app.get("/api/budgets?on=2026-06-01").await.body[0];
    assert_eq!(annual["period"], "annual");
    assert_eq!(annual["period_start"], "2026-01-01");
    assert_eq!(annual["period_end"], "2026-12-31");
    assert_eq!(annual["spent"], "300.0000");
}

#[tokio::test]
async fn budgets_are_checked() {
    let mut app = TestApp::signed_in().await;
    let ids = ledger(&mut app).await;
    for (category, amount, status) in [
        ("Assets:Bank", "100", StatusCode::BAD_REQUEST),
        ("Income:Salary", "100", StatusCode::BAD_REQUEST),
        ("Expenses:Food", "0", StatusCode::UNPROCESSABLE_ENTITY),
        ("Expenses:Food", "-5", StatusCode::UNPROCESSABLE_ENTITY),
    ] {
        budget(&mut app, &ids, category, amount, "monthly")
            .await
            .assert_status(status);
    }
    app.post("/api/budgets", json!({"account_id": 99999, "amount": "5"}))
        .await
        .assert_status(StatusCode::NOT_FOUND);
    budget(&mut app, &ids, "Expenses:Food", "5", "weekly")
        .await
        .assert_status(StatusCode::UNPROCESSABLE_ENTITY);

    budget(&mut app, &ids, "Expenses:Food", "100", "monthly")
        .await
        .assert_status(StatusCode::CREATED);
    let clash = budget(&mut app, &ids, "Expenses:Food", "200", "monthly").await;
    clash.assert_status(StatusCode::CONFLICT);
    assert_eq!(
        clash.body["detail"],
        "That category already has a budget for this period"
    );
    // The same category can have both a monthly and an annual budget.
    budget(&mut app, &ids, "Expenses:Food", "2000", "annual")
        .await
        .assert_status(StatusCode::CREATED);
}

#[tokio::test]
async fn update_and_delete_a_budget() {
    let mut app = TestApp::signed_in().await;
    let ids = ledger(&mut app).await;
    let id = budget(&mut app, &ids, "Expenses:Food", "100", "monthly")
        .await
        .body["id"]
        .as_i64()
        .unwrap();
    budget(&mut app, &ids, "Expenses:Food", "900", "annual").await;

    let response = app
        .patch(&format!("/api/budgets/{id}"), json!({"amount": "150"}))
        .await;
    response.assert_status(StatusCode::OK);
    assert_eq!(response.body["amount"], "150.0000");
    // Switching to annual would clash with the existing annual budget.
    app.patch(&format!("/api/budgets/{id}"), json!({"period": "annual"}))
        .await
        .assert_status(StatusCode::CONFLICT);
    app.patch("/api/budgets/99999", json!({"amount": "1"}))
        .await
        .assert_status(StatusCode::NOT_FOUND);

    app.delete(&format!("/api/budgets/{id}"))
        .await
        .assert_status(StatusCode::NO_CONTENT);
    assert_eq!(
        app.get("/api/budgets").await.body.as_array().unwrap().len(),
        1
    );
    app.delete(&format!("/api/budgets/{id}"))
        .await
        .assert_status(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_budget_follows_its_category_being_renamed_and_deleted() {
    let mut app = TestApp::signed_in().await;
    let ids = ledger(&mut app).await;
    budget(&mut app, &ids, "Expenses:Transport", "60", "monthly").await;
    let today = Local::now().date_naive().to_string();
    spend(&mut app, &ids, "Expenses:Transport", "12.00", &today, true).await;
    app.put(
        &format!("/api/accounts/{}", ids["Expenses:Transport"]),
        json!({"name": "Travel", "full_path": "Expenses:Travel"}),
    )
    .await
    .assert_status(StatusCode::OK);
    let travel = &app.get("/api/budgets").await.body[0];
    assert_eq!(travel["account_full_path"], "Expenses:Travel");
    assert_eq!(travel["spent"], "12.0000");

    // Deleting a category (once nothing uses it) takes its budget with it.
    budget(&mut app, &ids, "Expenses:Foodbank", "5", "monthly").await;
    app.delete(&format!("/api/accounts/{}", ids["Expenses:Foodbank"]))
        .await
        .assert_status(StatusCode::NO_CONTENT);
    let paths: Vec<Value> = app
        .get("/api/budgets")
        .await
        .body
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["account_full_path"].clone())
        .collect();
    assert_eq!(paths, [json!("Expenses:Travel")]);
}

#[tokio::test]
async fn suggestions_average_recent_full_months() {
    let mut app = TestApp::signed_in().await;
    let ids = ledger(&mut app).await;
    spend(&mut app, &ids, "Expenses:Food", "90.00", "2026-06-10", true).await;
    spend(
        &mut app,
        &ids,
        "Expenses:Food:Groceries",
        "60.00",
        "2026-07-10",
        true,
    )
    .await;
    spend(
        &mut app,
        &ids,
        "Expenses:Food",
        "30.00",
        "2026-08-10",
        false,
    )
    .await;
    spend(
        &mut app,
        &ids,
        "Expenses:Food",
        "500.00",
        "2026-09-10",
        true,
    )
    .await; // this month
    spend(
        &mut app,
        &ids,
        "Expenses:Food",
        "500.00",
        "2026-05-31",
        true,
    )
    .await; // too old

    let food = ids["Expenses:Food"];
    let response = app
        .get(&format!(
            "/api/budgets/suggestion?account_id={food}&on=2026-09-20"
        ))
        .await;
    response.assert_status(StatusCode::OK);
    let months: Vec<&Value> = response.body["months"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| &m["month"])
        .collect();
    assert_eq!(months, ["2026-06", "2026-07", "2026-08"]);
    assert_eq!(response.body["suggested"], "60.0000");

    let annual = app
        .get(&format!(
            "/api/budgets/suggestion?account_id={food}&period=annual&on=2026-09-20"
        ))
        .await
        .body;
    assert_eq!(annual["months"].as_array().unwrap().len(), 12);
    assert_eq!(annual["suggested"], "680.0000");

    // A third of an uneven total is rounded to the penny.
    spend(&mut app, &ids, "Expenses:Food", "0.01", "2026-08-11", true).await;
    let uneven = app
        .get(&format!(
            "/api/budgets/suggestion?account_id={food}&on=2026-09-20"
        ))
        .await
        .body;
    assert_eq!(uneven["suggested"], "60.0000");

    let salary = ids["Income:Salary"];
    app.get(&format!("/api/budgets/suggestion?account_id={salary}"))
        .await
        .assert_status(StatusCode::BAD_REQUEST);
}
