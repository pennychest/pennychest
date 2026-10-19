//! Duplicate and transfer matching, with Jev faked. Card tap matching is tested with taps.

mod common;

use std::collections::HashMap;
use std::sync::Arc;

use axum::http::StatusCode;
use common::TestApp;
use pennychest_core::ai::http::FakeApi;
use serde_json::{Value, json};

const JEV: &str = "https://api.typesafe.ai/v1/systemone";

/// Answers each yes/no question with `judge(a, b)`, and anything else (other tasks' questions)
/// with no answer.
fn fake_jev(api: &FakeApi, judge: impl Fn(&Value, &Value) -> f64 + Send + Sync + 'static) {
    api.respond("POST", JEV, move |body| {
        let mut answers = serde_json::Map::new();
        for (key, question) in body["questions"].as_object().unwrap() {
            if question["type"] != "noul" {
                continue;
            }
            let instructions = question["instructions"].as_str().unwrap();
            assert!(
                instructions.contains(&format!("`{key}.a`")),
                "{instructions}"
            );
            let pair = &body["state"][key];
            answers.insert(
                key.clone(),
                json!({"type": "noul", "noul": judge(&pair["a"], &pair["b"])}),
            );
        }
        (200, json!({"model": "jev-1", "answers": answers}))
    });
}

/// The Jev requests matching sent.
fn matching(api: &FakeApi) -> Vec<Value> {
    api.bodies(JEV)
        .into_iter()
        .filter(|b| {
            b["questions"]
                .as_object()
                .unwrap()
                .values()
                .all(|q| q["type"] == "noul")
        })
        .collect()
}

struct Setup {
    app: TestApp,
    api: Arc<FakeApi>,
    accounts: HashMap<&'static str, i64>,
}

/// The accounts, with Jev (as `fake` sets it up) matching in `mode` if `jev`; without, it
/// would answer yes to everything.
async fn setup(jev: bool, mode: &str, fake: impl FnOnce(&FakeApi)) -> Setup {
    let api = FakeApi::new();
    let mut app = TestApp::with_fake_api(api.clone()).await;
    if jev {
        fake(&api);
        app.put(
            "/api/ai/providers/typesafe",
            json!({"values": {"api_key": "ts-1"}}),
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);
        app.put(
            "/api/ai/models/decision",
            json!({"provider": "typesafe", "model": "jev-latest"}),
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);
        app.put(
            "/api/ai/tasks/matching",
            json!({"enabled": true, "kind": "decision", "mode": mode}),
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);
    } else {
        fake_jev(&api, |_, _| 1.0);
    }
    let mut accounts = HashMap::new();
    for (path, kind) in [
        ("Assets:Current", "asset"),
        ("Liabilities:Card", "liability"),
        ("Assets:Transfers:Pending", "asset"),
        ("Expenses:Uncategorised", "expense"),
    ] {
        let name = path.rsplit(':').next().unwrap();
        let response = app
            .post(
                "/api/accounts",
                json!({"name": name, "full_path": path, "type": kind}),
            )
            .await;
        accounts.insert(path, response.body["id"].as_i64().unwrap());
    }
    Setup { app, api, accounts }
}

async fn with_jev(judge: impl Fn(&Value, &Value) -> f64 + Send + Sync + 'static) -> Setup {
    setup(true, "automatic", |api| fake_jev(api, judge)).await
}

async fn without_jev() -> Setup {
    setup(false, "automatic", |_| {}).await
}

impl Setup {
    /// Imports these `(date, description, amount)` rows; the batch's transactions.
    async fn import(
        &mut self,
        account: &str,
        rows: &[(&str, &str, &str)],
        name: &str,
    ) -> Vec<Value> {
        let mut content = "Date,Description,Amount\n".to_owned();
        for (date, description, amount) in rows {
            content.push_str(&format!("{date},{description},{amount}\n"));
        }
        let response = self
            .app
            .post_form(
                "/api/imports/upload",
                Some((name, content.as_bytes())),
                &[
                    ("account_id", &self.accounts[account].to_string()),
                    ("importer_name", "csv"),
                ],
            )
            .await;
        assert_eq!(response.status, StatusCode::OK, "{}", response.body);
        self.app.settle().await;
        let batch = &response.body["batch_id"];
        self.transactions(batch.as_i64().unwrap()).await
    }

    async fn transactions(&mut self, batch: i64) -> Vec<Value> {
        self.app
            .get(&format!("/api/imports/batches/{batch}"))
            .await
            .body["transactions"]
            .as_array()
            .unwrap()
            .clone()
    }

    async fn latest_batch(&mut self) -> i64 {
        let batches = self.app.get("/api/imports/batches").await.body;
        batches
            .as_array()
            .unwrap()
            .iter()
            .map(|b| b["id"].as_i64().unwrap())
            .max()
            .unwrap()
    }
}

fn by_description(transactions: &[Value]) -> HashMap<String, Value> {
    transactions
        .iter()
        .map(|t| (t["description"].as_str().unwrap().to_owned(), t.clone()))
        .collect()
}

fn mentions(record: &Value, text: &str) -> bool {
    record["description"]
        .as_str()
        .unwrap()
        .to_uppercase()
        .contains(text)
}

// Duplicates

#[tokio::test]
async fn exact_duplicates_are_flagged_without_a_model() {
    let mut s = without_jev().await;
    let first = s
        .import(
            "Assets:Current",
            &[("20/09/2026", "TESCO STORES", "-12.50")],
            "a.csv",
        )
        .await;
    let second = s
        .import(
            "Assets:Current",
            &[
                ("20/09/2026", "Tesco Stores", "-12.50"),
                ("21/09/2026", "TFL", "-3.10"),
            ],
            "b.csv",
        )
        .await;
    let flagged = by_description(&second);
    assert_eq!(
        flagged["Tesco Stores"]["duplicate_of"],
        json!({
            "transaction_id": first[0]["id"],
            "date": "2026-09-20",
            "description": "TESCO STORES",
            "source": "imported from a.csv",
        })
    );
    assert_eq!(flagged["TFL"]["duplicate_of"], Value::Null);
    assert_eq!(first[0]["duplicate_of"], Value::Null);
    assert!(s.api.bodies(JEV).is_empty());
}

#[tokio::test]
async fn the_model_spots_duplicates_described_differently() {
    let mut s = with_jev(|a, _| if mentions(a, "TESCO") { 0.95 } else { 0.1 }).await;
    let first = s
        .import(
            "Assets:Current",
            &[
                ("20/09/2026", "TESCO STORES 2231", "-12.50"),
                ("20/09/2026", "PRET", "-4.00"),
            ],
            "a.csv",
        )
        .await;
    let second = s
        .import(
            "Assets:Current",
            &[
                ("22/09/2026", "Tesco Stores London", "-12.50"),
                ("21/09/2026", "Pret A Manger", "-4.00"),
            ],
            "b.pdf.csv",
        )
        .await;
    let found = by_description(&second);
    let tesco = &by_description(&first)["TESCO STORES 2231"];
    assert_eq!(
        found["Tesco Stores London"]["duplicate_of"]["transaction_id"],
        tesco["id"]
    );
    assert_eq!(found["Pret A Manger"]["duplicate_of"], Value::Null);
    let requests = matching(&s.api);
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0]["state"]["p0"]["b"]["source"],
        "imported from a.csv"
    );
}

#[tokio::test]
async fn same_day_repeats_within_one_statement_are_not_duplicates() {
    let mut s = without_jev().await;
    let rows = [
        ("20/09/2026", "COFFEE", "-3.00"),
        ("20/09/2026", "COFFEE", "-3.00"),
    ];
    let imported = s.import("Assets:Current", &rows, "a.csv").await;
    let flags: Vec<&Value> = imported.iter().map(|t| &t["duplicate_of"]).collect();
    assert_eq!(flags, [&Value::Null, &Value::Null]);
}

#[tokio::test]
async fn flagged_duplicates_can_be_kept_or_deleted() {
    let mut s = without_jev().await;
    s.import(
        "Assets:Current",
        &[("20/09/2026", "TESCO", "-12.50")],
        "a.csv",
    )
    .await;
    let rows = [
        ("20/09/2026", "TESCO", "-12.50"),
        ("21/09/2026", "TFL", "-3.10"),
    ];
    s.import("Assets:Current", &rows, "b.csv").await;
    let mut more = rows.to_vec();
    more.push(("22/09/2026", "BOOTS", "-6.00"));
    let found = by_description(&s.import("Assets:Current", &more, "c.csv").await);
    let (tesco, tfl) = (&found["TESCO"], &found["TFL"]);
    assert!(tesco["duplicate_of"].is_object() && tfl["duplicate_of"].is_object());
    assert_eq!(found["BOOTS"]["duplicate_of"], Value::Null);

    s.app
        .post(
            &format!("/api/imports/transactions/{}/not-duplicate", tesco["id"]),
            json!(null),
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);
    s.app
        .delete(&format!("/api/transactions/{}", tfl["id"]))
        .await
        .assert_status(StatusCode::NO_CONTENT);
    let remaining = s
        .transactions(tesco["import_batch_id"].as_i64().unwrap())
        .await;
    let flags: HashMap<String, Value> = remaining
        .iter()
        .map(|t| {
            (
                t["description"].as_str().unwrap().to_owned(),
                t["duplicate_of"].clone(),
            )
        })
        .collect();
    assert_eq!(
        flags,
        HashMap::from([("TESCO".into(), Value::Null), ("BOOTS".into(), Value::Null)])
    );
}

// Transfers

#[tokio::test]
async fn confident_transfers_are_linked() {
    let mut s = with_jev(|a, b| {
        if mentions(a, "AMEX") || mentions(b, "AMEX") {
            0.9
        } else {
            0.2
        }
    })
    .await;
    let card = s
        .import(
            "Liabilities:Card",
            &[("21/09/2026", "PAYMENT RECEIVED - THANK YOU", "250.00")],
            "card.csv",
        )
        .await;
    let current = s
        .import(
            "Assets:Current",
            &[
                ("20/09/2026", "AMEX DD 0042", "-250.00"),
                ("20/09/2026", "RENT", "-250.00"),
            ],
            "current.csv",
        )
        .await;
    let found = by_description(&current);
    let amex = &found["AMEX DD 0042"];
    assert_eq!(amex["transfer_peer_id"], card[0]["id"]);
    assert_eq!(amex["target_account_full_path"], "Assets:Transfers:Pending");
    assert_eq!(amex["categorised_by"], "transfer");
    assert_eq!(found["RENT"]["transfer_peer_id"], Value::Null);
    assert_eq!(
        found["RENT"]["target_account_full_path"],
        "Expenses:Uncategorised"
    );
    let card_side = s
        .transactions(card[0]["import_batch_id"].as_i64().unwrap())
        .await;
    assert_eq!(card_side[0]["transfer_peer_id"], amex["id"]);
    assert_eq!(
        card_side[0]["target_account_full_path"],
        "Assets:Transfers:Pending"
    );

    let requests = matching(&s.api);
    let sent = &requests.last().unwrap()["state"]["p0"];
    assert_eq!(sent["a"]["account"], "Assets:Current");
    assert_eq!(sent["b"]["account"], "Liabilities:Card");
    assert!(sent["a"]["amount"].as_str().unwrap().starts_with("-250"));
    assert!(sent["b"]["amount"].as_str().unwrap().starts_with("250"));
}

#[tokio::test]
async fn unsure_transfers_are_left_alone() {
    let mut s = with_jev(|_, _| 0.6).await;
    s.import(
        "Liabilities:Card",
        &[("21/09/2026", "PAYMENT", "250.00")],
        "c.csv",
    )
    .await;
    let current = s
        .import(
            "Assets:Current",
            &[("20/09/2026", "AMEX", "-250.00")],
            "d.csv",
        )
        .await;
    assert_eq!(current[0]["transfer_peer_id"], Value::Null);
    assert_eq!(
        current[0]["target_account_full_path"],
        "Expenses:Uncategorised"
    );
}

#[tokio::test]
async fn transfers_need_the_model_turned_on() {
    let mut s = without_jev().await;
    s.import(
        "Liabilities:Card",
        &[("21/09/2026", "PAYMENT", "250.00")],
        "c.csv",
    )
    .await;
    let current = s
        .import(
            "Assets:Current",
            &[("20/09/2026", "AMEX", "-250.00")],
            "d.csv",
        )
        .await;
    assert_eq!(current[0]["transfer_peer_id"], Value::Null);
    assert!(s.api.bodies(JEV).is_empty());
}

#[tokio::test]
async fn a_failing_model_never_breaks_the_import() {
    let mut s = setup(true, "automatic", |api| {
        api.respond("POST", JEV, |_| (503, json!({"error": "overloaded"})));
    })
    .await;
    s.import(
        "Liabilities:Card",
        &[("21/09/2026", "PAYMENT", "250.00")],
        "c.csv",
    )
    .await;
    let current = s
        .import(
            "Assets:Current",
            &[("20/09/2026", "AMEX", "-250.00")],
            "d.csv",
        )
        .await;
    assert_eq!(current[0]["transfer_peer_id"], Value::Null);
    let logs = s.app.get("/api/ai/logs").await.body["logs"].clone();
    assert!(
        logs.as_array()
            .unwrap()
            .iter()
            .any(|l| l["operation"] == "transfer_match" && l["error"].is_string()),
        "{logs}"
    );
}

// Matching on demand

#[tokio::test]
async fn on_demand_matching_waits_for_the_button() {
    let mut s = setup(true, "on_demand", |api| {
        fake_jev(api, |a, _| if mentions(a, "TESCO") { 0.95 } else { 0.1 });
    })
    .await;
    let first = s
        .import(
            "Assets:Current",
            &[("20/09/2026", "TESCO STORES 2231", "-12.50")],
            "a.csv",
        )
        .await;
    let second = s
        .import(
            "Assets:Current",
            &[("22/09/2026", "Tesco Stores London", "-12.50")],
            "b.csv",
        )
        .await;
    assert_eq!(second[0]["duplicate_of"], Value::Null);
    assert!(matching(&s.api).is_empty());

    let batch = s.latest_batch().await;
    let review = s.app.get(&format!("/api/imports/batches/{batch}")).await;
    assert_eq!(review.body["batch"]["matched_at"], Value::Null);
    let response = s
        .app
        .post(&format!("/api/imports/batches/{batch}/match"), json!(null))
        .await;
    assert_eq!(
        response.body,
        json!({"duplicates": 1, "transfers": 0, "taps": 0})
    );

    let review = s.app.get(&format!("/api/imports/batches/{batch}")).await;
    assert_eq!(
        review.body["transactions"][0]["duplicate_of"]["transaction_id"],
        first[0]["id"]
    );
    assert!(review.body["batch"]["matched_at"].is_string());
}

#[tokio::test]
async fn matching_on_demand_needs_the_task_on() {
    let mut s = without_jev().await;
    s.import("Assets:Current", &[("20/09/2026", "TFL", "-3.10")], "a.csv")
        .await;
    let batch = s.latest_batch().await;
    s.app
        .post(&format!("/api/imports/batches/{batch}/match"), json!(null))
        .await
        .assert_status(StatusCode::BAD_REQUEST);
    assert!(s.api.bodies(JEV).is_empty());
}

// Marking transfers by hand

#[tokio::test]
async fn transfers_can_be_marked_by_hand() {
    let mut s = without_jev().await;
    let card = s
        .import(
            "Liabilities:Card",
            &[("21/09/2026", "PAYMENT", "250.00")],
            "c.csv",
        )
        .await;
    let current = s
        .import(
            "Assets:Current",
            &[
                ("20/09/2026", "AMEX", "-250.00"),
                ("20/09/2026", "SAVINGS", "-40.00"),
            ],
            "d.csv",
        )
        .await;
    let found = by_description(&current);
    let (amex, savings) = (&found["AMEX"], &found["SAVINGS"]);

    let suggestions = s
        .app
        .get(&format!(
            "/api/imports/transactions/{}/transfer-suggestions",
            amex["id"]
        ))
        .await
        .body;
    assert_eq!(
        suggestions,
        json!([{
            "transaction_id": card[0]["id"],
            "date": "2026-09-21",
            "description": "PAYMENT",
            "amount": "250.0000",
            "account_full_path": "Liabilities:Card",
            "days_apart": 1,
        }])
    );

    let marked = s
        .app
        .post(
            &format!("/api/imports/transactions/{}/mark-as-transfer", amex["id"]),
            json!({"peer_transaction_id": card[0]["id"]}),
        )
        .await
        .body;
    assert_eq!(marked["parked"], false);
    let again = s
        .app
        .post(
            &format!(
                "/api/imports/transactions/{}/mark-as-transfer",
                savings["id"]
            ),
            json!({"peer_transaction_id": card[0]["id"]}),
        )
        .await;
    again.assert_status(StatusCode::CONFLICT);

    // Without a peer, it waits in Pending for the other side.
    let parked = s
        .app
        .post(
            &format!(
                "/api/imports/transactions/{}/mark-as-transfer",
                savings["id"]
            ),
            json!({}),
        )
        .await
        .body;
    assert_eq!(parked["parked"], true);
    let balance = s
        .app
        .get("/api/imports/transfers/pending-balance")
        .await
        .body;
    assert_eq!(balance["unlinked_count"], 1);
    assert_eq!(balance["balance"], "40.0000");
}
