//! Choosing an importer for a file and mapping CSV columns.

mod common;

use std::sync::Arc;

use axum::http::StatusCode;
use chrono::NaiveDate;
use common::TestApp;
use pennychest_core::imports::csv::CsvImporter;
use pennychest_core::imports::{ImportedRow, Importer, Importers, StatementInfo};
use serde_json::{Value, json};

const ODD: &str = "Posted,Ref,Who,Out,In,Running total\n\
                   20/09/2026,A1,TESCO STORES,12.50,,987.50\n\
                   21/09/2026,A2,SALARY,,1500.00,2487.50\n";
const CSV: &[u8] = b"Date,Description,Amount\n03/07/2026,TESCO,-4.00\n";

/// Stands in for a plugin that reads statements identifying their account.
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
        Ok(vec![])
    }
}

/// Only the CSV importer and the fake plugin are installed.
async fn installed() -> TestApp {
    let importers = Importers::new(vec![Arc::new(CsvImporter), Arc::new(FakeStatementImporter)]);
    let mut app = TestApp::with_importers(importers).await;
    app.post(
        "/api/auth/setup",
        json!({"password": common::TEST_PASSWORD}),
    )
    .await
    .assert_status(StatusCode::NO_CONTENT);
    app
}

async fn detect(app: &mut TestApp, filename: &str, content: &[u8]) -> common::TestResponse {
    app.post_form("/api/imports/detect", Some((filename, content)), &[])
        .await
}

async fn preview(app: &mut TestApp, content: &str, fields: &[(&str, &str)]) -> Value {
    let response = app
        .post_form(
            "/api/imports/csv/preview",
            Some(("statement.csv", content.as_bytes())),
            fields,
        )
        .await;
    response.assert_status(StatusCode::OK);
    response.body
}

#[tokio::test]
async fn lists_installed_importers() {
    let mut app = installed().await;
    let importers = app.get("/api/imports/importers").await.body;
    let by_name = |name: &str| {
        importers
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["name"] == name)
            .unwrap()
            .clone()
    };
    assert_eq!(by_name("fakebank")["label"], "Fake Bank statements");
    assert_eq!(by_name("csv")["file_types"], json!(["csv", "txt"]));
}

#[tokio::test]
async fn csv_needs_no_statement_details() {
    let mut app = installed().await;
    let response = detect(&mut app, "july.csv", CSV).await;
    response.assert_status(StatusCode::OK);
    assert_eq!(response.body, json!({"importer": "csv", "statement": null}));
}

#[tokio::test]
async fn plugin_statement_suggests_its_account() {
    let mut app = installed().await;
    let card = app
        .post(
            "/api/accounts",
            json!({"name": "Fake Card", "full_path": "Liabilities:CreditCard:Fake",
                   "type": "liability", "bank_identifier": "****4242"}),
        )
        .await
        .body;
    let response = detect(&mut app, "july.fake", b"FAKEBANK statement").await;
    response.assert_status(StatusCode::OK);
    assert_eq!(response.body["importer"], "fakebank");
    let statement = &response.body["statement"];
    assert_eq!(statement["suggested_account_id"], card["id"]);
    assert_eq!(
        statement["suggested_account_name"],
        "Liabilities:CreditCard:Fake"
    );
    assert_eq!(statement["period_start"], "2026-07-01");
    assert_eq!(statement["opening_balance"], "-10.00");
}

#[tokio::test]
async fn unrecognised_file_goes_to_the_importer_for_its_extension() {
    let mut app = installed().await;
    let response = detect(&mut app, "other.fake", b"not a statement").await;
    assert_eq!(response.body["importer"], "fakebank");
}

#[tokio::test]
async fn unsupported_file_type() {
    let mut app = installed().await;
    let response = detect(&mut app, "photo.jpg", b"\xff\xd8").await;
    response.assert_status(StatusCode::UNPROCESSABLE_ENTITY);
    let detail = response.body["detail"].as_str().unwrap();
    assert!(
        detail.contains(".csv") && detail.contains(".fake"),
        "{detail}"
    );
}

#[tokio::test]
async fn detect_offers_unrecognised_csvs_for_mapping() {
    let mut app = TestApp::signed_in().await;
    let response = detect(&mut app, "statement.csv", ODD.as_bytes()).await;
    assert_eq!(response.body, json!({"importer": "csv", "statement": null}));
}

#[tokio::test]
async fn preview_maps_known_column_names() {
    let mut app = TestApp::signed_in().await;
    let preview = preview(
        &mut app,
        "Date,Description,Amount\n20/09/2026,TESCO,-12.50\n",
        &[],
    )
    .await;
    assert_eq!(preview["headers"], json!(["Date", "Description", "Amount"]));
    assert_eq!(preview["rows"], json!([["20/09/2026", "TESCO", "-12.50"]]));
    assert_eq!(
        preview["columns"],
        json!({
            "date": {"column": 0, "source": "name", "confidence": null},
            "description": {"column": 1, "source": "name", "confidence": null},
            "amount": {"column": 2, "source": "name", "confidence": null}
        })
    );
    assert_eq!(preview["template"], Value::Null);
}

#[tokio::test]
async fn preview_without_a_model_leaves_the_gaps() {
    let mut app = TestApp::signed_in().await;
    let preview = preview(&mut app, ODD, &[]).await;
    assert_eq!(preview["columns"], json!({}));
    assert_eq!(preview["ai_available"], false);
    assert_eq!(preview["ai_error"], Value::Null);
    let asked = self::preview(&mut app, ODD, &[("ai", "true")]).await;
    assert!(
        asked["ai_error"]
            .as_str()
            .unwrap()
            .contains("Settings > AI")
    );
}

#[tokio::test]
async fn saved_templates_are_used_for_files_laid_out_the_same() {
    let mut app = TestApp::signed_in().await;
    let headers: Vec<&str> = ODD.lines().next().unwrap().split(',').collect();
    let columns = json!({"date": 0, "description": 2, "money_out": 3, "money_in": 4});
    let saved = app
        .post(
            "/api/imports/csv/templates",
            json!({"name": "Monzo export", "headers": headers, "columns": columns}),
        )
        .await;
    saved.assert_status(StatusCode::CREATED);
    let id = saved.body["id"].clone();

    // Header case and spacing don't matter
    let later = ODD
        .replace("Posted,", " posted ,")
        .replace("21/09/2026", "22/09/2026");
    let preview = preview(&mut app, &later, &[]).await;
    assert_eq!(
        preview["template"],
        json!({"id": id, "name": "Monzo export"})
    );
    assert_eq!(
        preview["columns"]["description"],
        json!({"column": 2, "source": "template", "confidence": null})
    );

    let listed = app.get("/api/imports/csv/templates").await.body;
    assert_eq!(listed[0]["name"], "Monzo export");
    assert_eq!(listed[0]["headers"][0], "posted");
    app.delete(&format!("/api/imports/csv/templates/{id}"))
        .await
        .assert_status(StatusCode::NO_CONTENT);
    assert_eq!(
        self::preview(&mut app, &later, &[]).await["template"],
        Value::Null
    );
}

#[tokio::test]
async fn templates_are_checked() {
    let mut app = TestApp::signed_in().await;
    let headers: Vec<&str> = ODD.lines().next().unwrap().split(',').collect();
    let odd = json!({"date": 0, "description": 2, "money_out": 3, "money_in": 4});
    let mut far = odd.clone();
    far["date"] = json!(9);
    let mut unknown = odd.clone();
    unknown["balance"] = json!(5);
    for (name, columns, problem) in [
        (" ", odd.clone(), "name"),
        ("X", json!({"date": 0}), "description and amount"),
        ("X", far, "outside the file"),
        ("X", unknown, "Unknown fields"),
    ] {
        let response = app
            .post(
                "/api/imports/csv/templates",
                json!({"name": name, "headers": headers, "columns": columns}),
            )
            .await;
        response.assert_status(StatusCode::BAD_REQUEST);
        assert!(
            response.body["detail"].as_str().unwrap().contains(problem),
            "{}",
            response.body
        );
    }
}

#[tokio::test]
async fn template_names_are_unique() {
    let mut app = TestApp::signed_in().await;
    let headers: Vec<&str> = ODD.lines().next().unwrap().split(',').collect();
    let body = json!({"name": "Monzo", "headers": headers,
                      "columns": {"date": 0, "description": 2, "money_out": 3, "money_in": 4}});
    app.post("/api/imports/csv/templates", body.clone())
        .await
        .assert_status(StatusCode::CREATED);
    app.post("/api/imports/csv/templates", body)
        .await
        .assert_status(StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn a_file_that_isnt_csv_cant_be_previewed() {
    let mut app = TestApp::signed_in().await;
    let response = app
        .post_form("/api/imports/csv/preview", Some(("s.csv", b"")), &[])
        .await;
    response.assert_status(StatusCode::BAD_REQUEST);
    assert!(
        response.body["detail"]
            .as_str()
            .unwrap()
            .starts_with("Couldn't read the CSV file")
    );
}
