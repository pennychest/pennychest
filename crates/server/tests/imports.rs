//! Importing statements: uploads, detecting the importer, mapping CSV columns, import history
//! and categorising what's imported.

mod common;

use std::sync::Arc;

use axum::http::StatusCode;
use chrono::NaiveDate;
use common::TestApp;
use pennychest_core::Money;
use pennychest_core::imports::csv::CsvImporter;
use pennychest_core::imports::{ImportedRow, Importer, Importers, StatementInfo};
use serde_json::{Value, json};

const ODD: &str = "Posted,Ref,Who,Out,In,Running total\n\
                   20/09/2026,A1,TESCO STORES,12.50,,987.50\n\
                   21/09/2026,A2,SALARY,,1500.00,2487.50\n";
const ODD_COLUMNS: &str = r#"{"date": 0, "description": 2, "money_out": 3, "money_in": 4}"#;
const STATEMENT: &[u8] = b"Date,Description,Amount\n03/07/2026,TESCO,-4.00\n";

struct FakeStatementImporter;

impl Importer for FakeStatementImporter {
    fn name(&self) -> &str {
        "fakebank"
    }
    fn label(&self) -> &str {
        "Fake Bank statements"
    }
    fn description(&self) -> &str {
        ""
    }
    fn file_types(&self) -> Vec<String> {
        vec!["fake".into()]
    }
    fn detect(&self, content: &[u8], _filename: &str) -> bool {
        content.starts_with(b"FAKEBANK")
    }
    fn statement_info(&self, _content: &[u8]) -> Option<StatementInfo> {
        Some(StatementInfo {
            statement_type: "credit_card".into(),
            bank_identifier: "****4242".into(),
            label: "Fake Bank Credit Card Statement".into(),
            name_hint: Some("Fake Card".into()),
            period_start: NaiveDate::from_ymd_opt(2026, 7, 1),
            period_end: NaiveDate::from_ymd_opt(2026, 7, 31),
            opening_balance: Some("-10.00".parse().unwrap()),
            closing_balance: Some("-14.00".parse().unwrap()),
        })
    }
    fn parse(&self, _content: &[u8]) -> Result<Vec<ImportedRow>, String> {
        Ok(vec![ImportedRow {
            date: NaiveDate::from_ymd_opt(2026, 7, 3).unwrap(),
            description: "TESCO".into(),
            amount: "-4.00".parse().unwrap(),
            raw_content: "TESCO".into(),
            line_number: None,
            metadata: serde_json::Map::new(),
        }])
    }
}

async fn account(app: &mut TestApp, path: &str, kind: &str, extra: Value) -> i64 {
    let mut body =
        json!({"name": path.rsplit(':').next().unwrap(), "full_path": path, "type": kind});
    if let (Value::Object(body), Value::Object(extra)) = (&mut body, extra) {
        body.extend(extra);
    }
    let response = app.post("/api/accounts", body).await;
    response.assert_status(StatusCode::CREATED);
    response.body["id"].as_i64().unwrap()
}

/// Assets:Current and Expenses:Uncategorised, returning the current account's id.
async fn current(app: &mut TestApp) -> i64 {
    account(app, "Expenses:Uncategorised", "expense", json!({})).await;
    account(app, "Assets:Current", "asset", json!({})).await
}

async fn upload(
    app: &mut TestApp,
    filename: &str,
    content: &[u8],
    fields: &[(&str, &str)],
) -> common::TestResponse {
    app.post_form("/api/imports/upload", Some((filename, content)), fields)
        .await
}

#[tokio::test]
async fn import_with_a_mapping() {
    let mut app = TestApp::signed_in().await;
    let current = current(&mut app).await.to_string();
    let response = upload(
        &mut app,
        "statement.csv",
        ODD.as_bytes(),
        &[
            ("account_id", &current),
            ("importer_name", "csv"),
            ("csv_columns", ODD_COLUMNS),
        ],
    )
    .await;
    response.assert_status(StatusCode::OK);
    let batch = app
        .get(&format!(
            "/api/imports/batches/{}",
            response.body["batch_id"]
        ))
        .await
        .body;
    let amounts: Vec<(String, String)> = batch["transactions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| {
            (
                t["description"].as_str().unwrap().into(),
                t["amount"].as_str().unwrap().into(),
            )
        })
        .collect();
    assert!(amounts.contains(&("TESCO STORES".into(), "-12.5000".into())));
    assert!(amounts.contains(&("SALARY".into(), "1500.0000".into())));
}

#[tokio::test]
async fn money_out_is_outgoing_whichever_sign_the_bank_uses() {
    let mut app = TestApp::signed_in().await;
    let current = current(&mut app).await.to_string();
    let columns = r#"{"date": 0, "description": 1, "money_out": 2, "money_in": 3}"#;
    let response = upload(
        &mut app,
        "statement.csv",
        b"Posted,Who,Out,In\n20/09/2026,TESCO,-12.50,\n",
        &[("account_id", &current), ("csv_columns", columns)],
    )
    .await;
    let batch = app
        .get(&format!(
            "/api/imports/batches/{}",
            response.body["batch_id"]
        ))
        .await
        .body;
    assert_eq!(batch["transactions"][0]["amount"], "-12.5000");
}

#[tokio::test]
async fn import_needs_a_complete_mapping() {
    let mut app = TestApp::signed_in().await;
    let current = current(&mut app).await.to_string();
    let response = upload(
        &mut app,
        "statement.csv",
        ODD.as_bytes(),
        &[("account_id", &current), ("csv_columns", r#"{"date": 0}"#)],
    )
    .await;
    response.assert_status(StatusCode::BAD_REQUEST);
    assert!(
        response.body["detail"]
            .as_str()
            .unwrap()
            .contains("description and amount")
    );
}

#[tokio::test]
async fn upload_records_the_statement_summary() {
    let importers = Importers::new(vec![Arc::new(CsvImporter), Arc::new(FakeStatementImporter)]);
    let mut app = TestApp::with_importers(importers).await;
    app.post(
        "/api/auth/setup",
        json!({"password": common::TEST_PASSWORD}),
    )
    .await;
    let card = account(
        &mut app,
        "Liabilities:CreditCard:Fake",
        "liability",
        json!({}),
    )
    .await;
    account(&mut app, "Expenses:Uncategorised", "expense", json!({})).await;
    let response = upload(
        &mut app,
        "july.fake",
        b"FAKEBANK statement",
        &[
            ("account_id", &card.to_string()),
            ("importer_name", "fakebank"),
        ],
    )
    .await;
    response.assert_status(StatusCode::OK);
    let (start, end, closing): (Option<NaiveDate>, Option<NaiveDate>, Option<Money>) =
        sqlx::query_as(
            "SELECT period_start, period_end, closing_balance FROM import_batches WHERE id = ?",
        )
        .bind(response.body["batch_id"].as_i64())
        .fetch_one(&app.db)
        .await
        .unwrap();
    assert_eq!(
        (start, end),
        (
            NaiveDate::from_ymd_opt(2026, 7, 1),
            NaiveDate::from_ymd_opt(2026, 7, 31)
        )
    );
    assert_eq!(closing, Some("-14".parse().unwrap()));
}

#[tokio::test]
async fn upload_with_an_importer_that_isnt_installed() {
    let mut app = TestApp::signed_in().await;
    let card = account(
        &mut app,
        "Liabilities:CreditCard:Card",
        "liability",
        json!({}),
    )
    .await;
    let response = upload(
        &mut app,
        "july.pdf",
        b"%PDF",
        &[("account_id", &card.to_string()), ("importer_name", "hsbc")],
    )
    .await;
    response.assert_status(StatusCode::BAD_REQUEST);
    assert!(
        response.body["detail"]
            .as_str()
            .unwrap()
            .contains("isn't installed")
    );
}

#[tokio::test]
async fn batches_name_their_account_and_period() {
    let mut app = TestApp::signed_in().await;
    let id = account(
        &mut app,
        "Assets:Bank:Current:654321_87654321",
        "asset",
        json!({"name": "HSBC Current", "bank_identifier": "654321 87654321"}),
    )
    .await;
    account(&mut app, "Expenses:Uncategorised", "expense", json!({})).await;
    let response = upload(
        &mut app,
        "july.csv",
        STATEMENT,
        &[("account_id", &id.to_string()), ("importer_name", "csv")],
    )
    .await;
    response.assert_status(StatusCode::OK);

    let batches = app.get("/api/imports/batches").await.body;
    let batch = &batches[0];
    assert_eq!(batch["account_name"], "HSBC Current");
    assert_eq!(batch["account_number_hint"], "4321");
    assert_eq!(batch["period_start"], Value::Null);
    assert_eq!(batch["file_name"], "july.csv");

    // Statements that state their period (HSBC PDFs) report it
    sqlx::query("UPDATE import_batches SET period_start = '2026-07-01', period_end = '2026-07-31'")
        .execute(&app.db)
        .await
        .unwrap();
    let batches = app.get("/api/imports/batches").await.body;
    assert_eq!(batches[0]["period_start"], "2026-07-01");
    assert_eq!(batches[0]["period_end"], "2026-07-31");
}

#[tokio::test]
async fn categorise_changes_the_category_side_of_a_hand_made_transaction() {
    let mut app = TestApp::signed_in().await;
    let current = account(&mut app, "Assets:Current", "asset", json!({})).await;
    let food = account(&mut app, "Expenses:Food", "expense", json!({})).await;
    let transport = account(&mut app, "Expenses:Transport", "expense", json!({})).await;
    // Category posting first, so "the posting that isn't first" would pick the bank side
    let txn = app
        .post(
            "/api/transactions",
            json!({"date": "2026-07-21", "description": "Bus", "postings": [
                {"account_id": food, "amount": "3.30", "categorised_by_id": 2},
                {"account_id": current, "amount": "-3.30", "categorised_by_id": 2}
            ]}),
        )
        .await
        .body;
    let response = app
        .send(
            axum::http::Method::PUT,
            &format!(
                "/api/imports/transactions/{}/categorise?target_account_id={transport}",
                txn["id"]
            ),
            None,
        )
        .await;
    response.assert_status(StatusCode::OK);
    assert_eq!(
        response.body,
        json!({"status": "ok", "transaction_id": txn["id"]})
    );
    let postings = app
        .get(&format!("/api/transactions/{}", txn["id"]))
        .await
        .body["postings"]
        .clone();
    let mut paths: Vec<&str> = postings
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["account_full_path"].as_str().unwrap())
        .collect();
    paths.sort_unstable();
    assert_eq!(paths, ["Assets:Current", "Expenses:Transport"]);
}

#[tokio::test]
async fn the_same_file_twice_is_refused() {
    let mut app = TestApp::signed_in().await;
    let current = current(&mut app).await.to_string();
    let first = upload(&mut app, "a.csv", STATEMENT, &[("account_id", &current)]).await;
    let again = upload(&mut app, "b.csv", STATEMENT, &[("account_id", &current)]).await;
    again.assert_status(StatusCode::CONFLICT);
    assert_eq!(
        again.body["detail"],
        format!(
            "This file has already been imported (batch #{})",
            first.body["batch_id"]
        )
    );
}

#[tokio::test]
async fn review_confirm_reapply_download_and_delete() {
    let mut app = TestApp::signed_in().await;
    let current = current(&mut app).await;
    let uploaded = upload(
        &mut app,
        "july.csv",
        STATEMENT,
        &[("account_id", &current.to_string())],
    )
    .await;
    assert_eq!(
        uploaded.body,
        json!({"batch_id": uploaded.body["batch_id"], "file_name": "july.csv", "transaction_count": 1,
               "categorised_count": 0, "uncategorised_count": 1, "ai_categorised_count": 0,
               "learned_count": 0, "ai_error": null})
    );
    let batch = uploaded.body["batch_id"].as_i64().unwrap();

    let file = app.get(&format!("/api/imports/batches/{batch}/file")).await;
    file.assert_status(StatusCode::OK);
    assert_eq!(file.headers["content-type"], "text/csv");
    assert_eq!(
        file.headers["content-disposition"],
        "attachment; filename=\"july.csv\""
    );

    let by_account = app
        .get(&format!("/api/imports/account/{current}"))
        .await
        .body;
    assert_eq!(by_account["batches"][0]["pending_count"], 1);
    assert_eq!(by_account["transactions"][0]["description"], "TESCO");
    app.get("/api/imports/account/999")
        .await
        .assert_status(StatusCode::NOT_FOUND);

    let groceries = account(&mut app, "Expenses:Groceries", "expense", json!({})).await;
    app.post(
        "/api/rules",
        json!({"pattern": "TESCO", "match_type_id": 1, "target_account_id": groceries}),
    )
    .await;
    let reapplied = app
        .post(
            &format!("/api/imports/batches/{batch}/reapply-rules"),
            json!({}),
        )
        .await;
    assert_eq!(reapplied.body, json!({"updated": 1}));

    let confirmed = app
        .post(
            &format!("/api/imports/batches/{batch}/confirm-all"),
            json!({}),
        )
        .await;
    assert_eq!(confirmed.body, json!({"confirmed": 1}));
    let again = app
        .post(
            &format!("/api/imports/account/{current}/confirm-all"),
            json!({}),
        )
        .await;
    assert_eq!(again.body, json!({"confirmed": 0}));

    let deleted = app.delete(&format!("/api/imports/batches/{batch}")).await;
    assert_eq!(deleted.body, json!({"deleted": true}));
    app.get(&format!("/api/imports/batches/{batch}"))
        .await
        .assert_status(StatusCode::NOT_FOUND);
}
