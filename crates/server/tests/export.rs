//! Exporting the ledger and downloading the database.

mod common;

use std::sync::{Arc, Mutex};

use axum::http::StatusCode;
use chrono::Local;
use common::TestApp;
use pennychest_core::export::{Exporter, Exporters};
use pennychest_plugin::{Decimal, Ledger};
use serde_json::json;

/// Stands in for an exporter plugin, and keeps the ledger it was given.
#[derive(Default)]
struct CsvExporter {
    received: Mutex<Option<Ledger>>,
}

impl Exporter for CsvExporter {
    fn name(&self) -> &str {
        "csv"
    }

    fn label(&self) -> &str {
        "Spreadsheet"
    }

    fn description(&self) -> &str {
        ""
    }

    fn file_extension(&self) -> &str {
        "csv"
    }

    fn media_type(&self) -> &str {
        "text/csv"
    }

    fn export(&self, ledger: &Ledger) -> Result<Vec<u8>, String> {
        *self.received.lock().unwrap() = Some(ledger.clone());
        Ok(format!("exported on {}\n", ledger.today).into_bytes())
    }
}

/// An exporter whose plugin reports a problem.
struct Broken;

impl Exporter for Broken {
    fn name(&self) -> &str {
        "broken"
    }

    fn label(&self) -> &str {
        ""
    }

    fn description(&self) -> &str {
        "Always fails"
    }

    fn file_extension(&self) -> &str {
        "txt"
    }

    fn media_type(&self) -> &str {
        "text/plain"
    }

    fn export(&self, _ledger: &Ledger) -> Result<Vec<u8>, String> {
        Err("The ledger has an account Beancount can't name.".into())
    }
}

/// Only the stand-in plugins are installed.
async fn installed() -> (TestApp, Arc<CsvExporter>) {
    let csv = Arc::new(CsvExporter::default());
    let exporters = Exporters::new(vec![csv.clone(), Arc::new(Broken)]);
    (TestApp::with_exporters(exporters).await, csv)
}

#[tokio::test]
async fn downloads_a_working_copy_of_the_database() {
    let (mut app, _) = installed().await;
    app.post(
        "/api/accounts",
        json!({"name": "Food", "full_path": "Expenses:Food", "type": "expense"}),
    )
    .await;
    let info = app.get("/api/export").await.body;
    assert_eq!(info["engine"], "sqlite");
    assert_eq!(info["database_backup"], true);
    assert!(info["size_bytes"].as_i64().unwrap() > 0);

    let dir = tempfile::tempdir().unwrap();
    let response = app.get("/api/export/database").await;
    response.assert_status(StatusCode::OK);
    assert_eq!(response.headers["content-type"], "application/vnd.sqlite3");
    let disposition = response.headers["content-disposition"].to_str().unwrap();
    let today = Local::now().date_naive();
    assert_eq!(
        disposition,
        format!("attachment; filename=\"pennychest-backup-{today}.db\"")
    );

    // It opens as a PennyChest database as it is.
    let copy = dir.path().join("backup.db");
    let bytes = app.get_bytes("/api/export/database").await;
    std::fs::write(&copy, bytes).unwrap();
    let restored = pennychest_core::db::open(&copy).await.unwrap();
    let paths: Vec<String> = sqlx::query_scalar("SELECT full_path FROM accounts")
        .fetch_all(&restored)
        .await
        .unwrap();
    assert_eq!(paths, ["Expenses:Food"]);
}

#[tokio::test]
async fn export_needs_a_session() {
    let (app, _) = installed().await;
    let mut anon = app.other_device();
    for path in ["/api/export", "/api/export/database", "/api/export/csv"] {
        anon.get(path).await.assert_status(StatusCode::UNAUTHORIZED);
    }
}

#[tokio::test]
async fn lists_installed_exporters() {
    let (mut app, _) = installed().await;
    assert_eq!(
        app.get("/api/export").await.body["exporters"],
        json!([
            {"name": "csv", "label": "Spreadsheet", "description": "", "file_extension": "csv"},
            {"name": "broken", "label": "broken", "description": "Always fails",
             "file_extension": "txt"},
        ])
    );
}

#[tokio::test]
async fn downloads_from_an_exporter_plugin() {
    let (mut app, _) = installed().await;
    let response = app.get("/api/export/csv").await;
    response.assert_status(StatusCode::OK);
    assert!(
        response.headers["content-type"]
            .to_str()
            .unwrap()
            .starts_with("text/csv")
    );
    assert!(response.body.as_str().unwrap().starts_with("exported on "));
    let today = Local::now().date_naive();
    assert_eq!(
        response.headers["content-disposition"],
        format!("attachment; filename=\"pennychest-{today}.csv\"").as_str()
    );
}

#[tokio::test]
async fn unknown_and_failing_exporters() {
    let (mut app, _) = installed().await;
    let unknown = app.get("/api/export/nonesuch").await;
    unknown.assert_status(StatusCode::NOT_FOUND);
    assert_eq!(
        unknown.body["detail"],
        "Exporter 'nonesuch' isn't installed"
    );
    let broken = app.get("/api/export/broken").await;
    broken.assert_status(StatusCode::BAD_REQUEST);
    assert_eq!(
        broken.body["detail"],
        "The ledger has an account Beancount can't name."
    );
}

#[tokio::test]
async fn exporters_get_the_ledger_as_plain_data() {
    let (mut app, csv) = installed().await;
    let mut ids = std::collections::HashMap::new();
    for (path, kind, ident) in [
        (
            "Assets:Bank:Current:123456_12345678",
            "asset",
            Some("12-34-56 12345678"),
        ),
        ("Expenses:Food:Groceries", "expense", None),
    ] {
        let response = app
            .post(
                "/api/accounts",
                json!({"name": path.rsplit(':').next().unwrap(), "full_path": path,
                       "type": kind, "bank_identifier": ident}),
            )
            .await;
        ids.insert(path, response.body["id"].as_i64().unwrap());
    }
    let (bank, food) = (
        ids["Assets:Bank:Current:123456_12345678"],
        ids["Expenses:Food:Groceries"],
    );
    for (day, description, amount, status) in [
        ("2026-07-02", "COOP", "1.2345", "pending"),
        ("2026-07-01", "TESCO", "4.15", "confirmed"),
    ] {
        app.post(
            "/api/transactions",
            json!({
                "date": day,
                "description": description,
                "status": status,
                "postings": [
                    {"account_id": food, "amount": amount, "categorised_by_id": 2},
                    {"account_id": bank, "amount": format!("-{amount}"), "categorised_by_id": 2},
                ],
            }),
        )
        .await
        .assert_status(StatusCode::CREATED);
    }
    app.post(
        "/api/actions/set_budget",
        json!({"category": "Groceries", "amount": "250"}),
    )
    .await
    .assert_status(StatusCode::OK);

    app.get("/api/export/csv")
        .await
        .assert_status(StatusCode::OK);
    let ledger = csv.received.lock().unwrap().clone().unwrap();
    let today = Local::now().date_naive();
    assert_eq!(ledger.today, today);

    let current = ledger
        .accounts
        .iter()
        .find(|a| a.full_path == "Assets:Bank:Current:123456_12345678")
        .unwrap();
    assert_eq!(
        (
            current.kind.as_str(),
            current.currency.as_str(),
            current.bank_identifier.as_deref()
        ),
        ("asset", "GBP", Some("12-34-56 12345678"))
    );
    // Parents before their children.
    let paths: Vec<&str> = ledger
        .accounts
        .iter()
        .map(|a| a.full_path.as_str())
        .collect();
    let mut sorted = paths.clone();
    sorted.sort_unstable();
    assert_eq!(paths, sorted);

    // Oldest first, with exact amounts that balance.
    let [tesco, coop] = &ledger.transactions[..] else {
        panic!("two transactions: {:?}", ledger.transactions);
    };
    assert_eq!(tesco.description, "TESCO");
    assert_eq!(tesco.status, "confirmed");
    assert_eq!(coop.status, "pending");
    let amounts: Vec<(i64, Decimal)> = coop
        .postings
        .iter()
        .map(|p| (p.account_id, p.amount))
        .collect();
    assert_eq!(
        amounts,
        [
            (food, "1.2345".parse().unwrap()),
            (bank, "-1.2345".parse().unwrap())
        ]
    );

    let [budget] = &ledger.budgets[..] else {
        panic!("one budget: {:?}", ledger.budgets);
    };
    assert_eq!(budget.account_id, food);
    assert_eq!(budget.amount, "250".parse::<Decimal>().unwrap());
    assert_eq!(budget.period, "monthly");
    assert_eq!(budget.created_on, Some(chrono::Utc::now().date_naive()));
}
