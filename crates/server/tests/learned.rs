//! The learned categoriser.

mod common;

use std::collections::HashMap;

use axum::http::StatusCode;
use common::TestApp;
use pennychest_core::learned::Learned;
use serde_json::{Value, json};

fn history() -> Vec<(String, &'static str)> {
    (0..12)
        .map(|n| (format!("TESCO STORES {n} ST HELIER"), "Expenses:Groceries"))
        .chain((0..12).map(|n| (format!("SOJ PARKING JERSEY {n}"), "Expenses:Transport")))
        .chain(std::iter::repeat_n(
            ("NETFLIX.COM LONDON".to_owned(), "Expenses:Entertainment"),
            8,
        ))
        .collect()
}

async fn setup_accounts(app: &mut TestApp) -> HashMap<&'static str, i64> {
    let mut accounts = HashMap::new();
    for (path, kind) in [
        ("Liabilities:Card", "liability"),
        ("Expenses:Uncategorised", "expense"),
        ("Expenses:Groceries", "expense"),
        ("Expenses:Transport", "expense"),
        ("Expenses:Entertainment", "expense"),
    ] {
        let response = app
            .post(
                "/api/accounts",
                json!({"name": path.rsplit(':').next().unwrap(), "full_path": path, "type": kind}),
            )
            .await;
        accounts.insert(path, response.body["id"].as_i64().unwrap());
    }
    accounts
}

/// A card payment filed under `category`, categorised by `source` (1 rule, 2 manual, 4 ai).
async fn add(
    app: &mut TestApp,
    accounts: &HashMap<&str, i64>,
    description: &str,
    category: &str,
    source: i64,
) {
    app.post(
        "/api/transactions",
        json!({"date": "2026-06-01", "description": description, "postings": [
            {"account_id": accounts["Liabilities:Card"], "amount": "-5.00", "categorised_by_id": source},
            {"account_id": accounts[category], "amount": "5.00", "categorised_by_id": source}
        ]}),
    )
    .await
    .assert_status(StatusCode::CREATED);
}

async fn with_history() -> (TestApp, HashMap<&'static str, i64>) {
    let mut app = TestApp::signed_in().await;
    let accounts = setup_accounts(&mut app).await;
    for (description, category) in history() {
        add(&mut app, &accounts, &description, category, 2).await;
    }
    (app, accounts)
}

async fn predict(app: &TestApp, descriptions: &[&str]) -> Vec<Option<(i64, f64)>> {
    let descriptions: Vec<String> = descriptions.iter().map(|d| (*d).to_owned()).collect();
    Learned::default()
        .suggest(&app.db, &descriptions)
        .await
        .unwrap()
}

#[tokio::test]
async fn needs_enough_history() {
    let mut app = TestApp::signed_in().await;
    let accounts = setup_accounts(&mut app).await;
    for (description, category) in history().into_iter().take(10) {
        add(&mut app, &accounts, &description, category, 2).await;
    }
    assert_eq!(predict(&app, &["TESCO STORES 99"]).await, [None]);
    let status = app.get("/api/ai/learned").await.body;
    assert_eq!(status["ready"], false);
    assert_eq!(status["examples"], 10);
}

#[tokio::test]
async fn learns_merchants_and_stays_quiet_when_unsure() {
    let (app, accounts) = with_history().await;
    let found = predict(&app, &["TESCO STORES 4471 ST BRELADE", "PAYPAL PAYMENT"]).await;
    let (category, p) = found[0].unwrap();
    assert_eq!(category, accounts["Expenses:Groceries"]);
    assert!(p >= 0.7);
    assert_eq!(found[1], None);
}

#[tokio::test]
async fn learns_only_what_the_user_stands_behind() {
    let mut app = TestApp::signed_in().await;
    let accounts = setup_accounts(&mut app).await;
    for (description, category) in history() {
        add(&mut app, &accounts, &description, category, 4).await; // pending AI guesses
    }
    assert_eq!(app.get("/api/ai/learned").await.body["examples"], 0);
    assert_eq!(predict(&app, &["TESCO STORES 1"]).await, [None]);
}

#[tokio::test]
async fn retrains_when_history_changes() {
    let (mut app, accounts) = with_history().await;
    let learned = Learned::default();
    assert_eq!(
        learned
            .suggest(&app.db, &["ODEON CINEMA".to_owned()])
            .await
            .unwrap()[0],
        None
    );
    for n in 0..10 {
        add(
            &mut app,
            &accounts,
            &format!("ODEON CINEMA {n}"),
            "Expenses:Entertainment",
            2,
        )
        .await;
    }
    let (category, _) = learned
        .suggest(&app.db, &["ODEON CINEMA".to_owned()])
        .await
        .unwrap()[0]
        .unwrap();
    assert_eq!(category, accounts["Expenses:Entertainment"]);
}

#[tokio::test]
async fn status_reports_a_check_on_past_data() {
    let (mut app, _) = with_history().await;
    let status = app.get("/api/ai/learned").await.body;
    assert_eq!(status["ready"], true);
    assert_eq!(status["examples"], history().len());
    assert_eq!(status["categories"], 3);
    assert_eq!(status["min_examples"], 30);
    assert_eq!(status["confidence"], 0.7);
    let coverage = status["check"]["coverage"].as_f64().unwrap();
    assert!(coverage > 0.0 && coverage <= 1.0);
    assert!(status["check"]["accuracy"].as_f64().unwrap() > 0.9);
}

#[tokio::test]
async fn can_be_turned_off() {
    let (mut app, _) = with_history().await;
    app.put("/api/ai/learned", json!({"enabled": false}))
        .await
        .assert_status(StatusCode::NO_CONTENT);
    assert_eq!(app.get("/api/ai/learned").await.body["enabled"], false);
    assert_eq!(predict(&app, &["TESCO STORES 1"]).await, [None]);
}

const STATEMENT: &[u8] = b"Date,Description,Amount\n\
    01/07/2026,TESCO STORES 9001 ST HELIER,-12.00\n\
    02/07/2026,SOJ PARKING JERSEY JEY,-3.30\n\
    03/07/2026,OAKFIELD FARM SHOP,-8.40\n";

#[tokio::test]
async fn imports_use_history_without_any_ai() {
    let (mut app, accounts) = with_history().await;
    let response = app
        .post_form(
            "/api/imports/upload",
            Some(("july.csv", STATEMENT)),
            &[("account_id", &accounts["Liabilities:Card"].to_string())],
        )
        .await;
    response.assert_status(StatusCode::OK);
    assert_eq!(response.body["learned_count"], 2);
    assert_eq!(response.body["ai_error"], Value::Null);
    assert_eq!(response.body["uncategorised_count"], 1);

    let batch = app
        .get(&format!(
            "/api/imports/batches/{}",
            response.body["batch_id"]
        ))
        .await
        .body;
    let tesco = batch["transactions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["description"] == "TESCO STORES 9001 ST HELIER")
        .unwrap();
    assert_eq!(tesco["target_account_full_path"], "Expenses:Groceries");
    assert_eq!(tesco["categorised_by"], "learned");
}

#[tokio::test]
async fn an_imports_categorising_is_one_change_that_can_be_undone() {
    let (mut app, accounts) = with_history().await;
    let response = app
        .post_form(
            "/api/imports/upload",
            Some(("july.csv", STATEMENT)),
            &[("account_id", &accounts["Liabilities:Card"].to_string())],
        )
        .await;
    let batch_path = format!("/api/imports/batches/{}", response.body["batch_id"]);
    let changes = app.get("/api/changes").await.body;
    let change = &changes["changes"][0];
    assert_eq!(changes["total"], 1);
    assert_eq!(
        change["summary"],
        "Categorised 2 transactions from july.csv (2 from your history)"
    );
    assert_eq!(change["source"], "import");

    app.post(&format!("/api/changes/{}/undo", change["id"]), json!({}))
        .await
        .assert_status(StatusCode::OK);
    let batch = app.get(&batch_path).await.body;
    for txn in batch["transactions"].as_array().unwrap() {
        assert_eq!(txn["target_account_full_path"], "Expenses:Uncategorised");
        assert_eq!(txn["categorised_by"], "import_default");
    }
}
