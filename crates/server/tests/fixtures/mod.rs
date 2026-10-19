//! Fixtures shared by the action and change tests: a small chart of accounts, a few months of
//! spending, and an imported transaction.

#![allow(dead_code)]

use std::collections::HashMap;

use axum::http::StatusCode;
use serde_json::{Value, json};

use crate::common::{TestApp, TestResponse};

pub type Ids = HashMap<&'static str, i64>;

const MANUAL: i64 = 2;

pub async fn ledger(app: &mut TestApp) -> Ids {
    let mut ids = HashMap::new();
    for (name, path, kind) in [
        ("Current", "Assets:Bank:Current", "asset"),
        ("Savings", "Assets:Bank:Savings", "asset"),
        ("Visa", "Liabilities:Visa", "liability"),
        ("Expenses", "Expenses", "expense"),
        ("Food", "Expenses:Food", "expense"),
        ("Groceries", "Expenses:Food:Groceries", "expense"),
        ("Restaurants", "Expenses:Food:Restaurants", "expense"),
        ("Transport", "Expenses:Transport", "expense"),
        ("Other", "Expenses:Other", "expense"),
        ("Uncategorised", "Expenses:Uncategorised", "expense"),
        ("Income", "Income", "income"),
        ("Salary", "Income:Salary", "income"),
        ("Other", "Income:Other", "income"),
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

pub async fn txn(
    app: &mut TestApp,
    ids: &Ids,
    day: &str,
    description: &str,
    postings: &[(&str, &str)],
    confirmed: bool,
) -> i64 {
    let postings: Vec<Value> = postings
        .iter()
        .map(|(path, amount)| {
            json!({"account_id": ids[path], "amount": amount, "categorised_by_id": MANUAL})
        })
        .collect();
    let status = if confirmed { "confirmed" } else { "pending" };
    let response = app
        .post(
            "/api/transactions",
            json!({"date": day, "description": description, "status": status, "postings": postings}),
        )
        .await;
    response.assert_status(StatusCode::CREATED);
    response.body["id"].as_i64().unwrap()
}

#[allow(clippy::too_many_arguments)]
pub async fn spend(
    app: &mut TestApp,
    ids: &Ids,
    day: &str,
    description: &str,
    category: &str,
    amount: &str,
    paid_with: &str,
    confirmed: bool,
) -> i64 {
    let negated = amount
        .strip_prefix('-')
        .map(str::to_owned)
        .unwrap_or_else(|| format!("-{amount}"));
    txn(
        app,
        ids,
        day,
        description,
        &[(paid_with, &negated), (category, amount)],
        confirmed,
    )
    .await
}

const CURRENT: &str = "Assets:Bank:Current";

/// A few months of activity.
pub async fn spending(app: &mut TestApp, ids: &Ids) -> Ids {
    let groceries = "Expenses:Food:Groceries";
    let mut s = HashMap::new();
    s.insert(
        "tesco_jul",
        spend(
            app,
            ids,
            "2026-07-03",
            "TESCO STORES 1",
            groceries,
            "40.00",
            CURRENT,
            true,
        )
        .await,
    );
    s.insert(
        "tesco_aug",
        spend(
            app,
            ids,
            "2026-08-03",
            "TESCO STORES 1",
            groceries,
            "55.50",
            CURRENT,
            true,
        )
        .await,
    );
    s.insert(
        "pizza",
        spend(
            app,
            ids,
            "2026-08-10",
            "PIZZA 100% REAL",
            "Expenses:Food:Restaurants",
            "30.00",
            "Liabilities:Visa",
            true,
        )
        .await,
    );
    s.insert(
        "train",
        spend(
            app,
            ids,
            "2026-08-12",
            "TRAINLINE",
            "Expenses:Transport",
            "25.00",
            CURRENT,
            true,
        )
        .await,
    );
    s.insert(
        "refund",
        spend(
            app,
            ids,
            "2026-08-20",
            "TESCO REFUND",
            groceries,
            "-5.50",
            CURRENT,
            true,
        )
        .await,
    );
    s.insert(
        "pending",
        spend(
            app,
            ids,
            "2026-08-25",
            "ALDI",
            groceries,
            "10.00",
            CURRENT,
            false,
        )
        .await,
    );
    s.insert(
        "salary",
        txn(
            app,
            ids,
            "2026-08-28",
            "ACME PAYROLL",
            &[(CURRENT, "2000.00"), ("Income:Salary", "-2000.00")],
            true,
        )
        .await,
    );
    s.insert(
        "transfer",
        txn(
            app,
            ids,
            "2026-08-29",
            "TO SAVINGS",
            &[(CURRENT, "-100.00"), ("Assets:Bank:Savings", "100.00")],
            true,
        )
        .await,
    );
    s.insert(
        "split",
        txn(
            app,
            ids,
            "2026-08-30",
            "SUPERMARKET + TRAIN",
            &[
                (CURRENT, "-20.00"),
                (groceries, "12.00"),
                ("Expenses:Transport", "8.00"),
            ],
            true,
        )
        .await,
    );
    s
}

/// A transaction as a statement import leaves it, with its statement row.
pub async fn imported(app: &mut TestApp, ids: &Ids) -> i64 {
    let batch: i64 = sqlx::query_scalar(
        "INSERT INTO import_batches (importer_name, source_type_id) VALUES ('csv', 1) RETURNING id",
    )
    .fetch_one(&app.db)
    .await
    .unwrap();
    let id = spend(
        app,
        ids,
        "2026-08-05",
        "TESCO 123",
        "Expenses:Food:Groceries",
        "20.00",
        CURRENT,
        false,
    )
    .await;
    sqlx::query("UPDATE transactions SET import_batch_id = ? WHERE id = ?")
        .bind(batch)
        .bind(id)
        .execute(&app.db)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO raw_import_rows (batch_id, raw_content, line_number, transaction_id)
         VALUES (?, '05/08/2026,TESCO 123,-20.00', 1, ?)",
    )
    .bind(batch)
    .bind(id)
    .execute(&app.db)
    .await
    .unwrap();
    id
}

/// The transaction the imported transaction's statement row points at, and its line.
pub async fn statement_row(app: &TestApp) -> (Option<i64>, Option<i64>) {
    sqlx::query_as(
        "SELECT transaction_id, line_number FROM raw_import_rows
         WHERE raw_content = '05/08/2026,TESCO 123,-20.00'",
    )
    .fetch_one(&app.db)
    .await
    .unwrap()
}

pub async fn act(app: &mut TestApp, name: &str, arguments: Value) -> TestResponse {
    app.post(&format!("/api/actions/{name}"), arguments).await
}

pub async fn ok(app: &mut TestApp, name: &str, arguments: Value) -> Value {
    let response = act(app, name, arguments).await;
    assert_eq!(response.status, StatusCode::OK, "{name}: {}", response.body);
    response.body
}

/// Each posting's amount by account path.
pub async fn postings(app: &mut TestApp, id: i64) -> HashMap<String, String> {
    let txn = app.get(&format!("/api/transactions/{id}")).await.body;
    txn["postings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| {
            (
                p["account_full_path"].as_str().unwrap().to_owned(),
                p["amount"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}
