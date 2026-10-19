//! Card taps from the phone's wallet shortcut: parsing, AI reading and matching them to
//! statement lines.

mod common;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::http::{Method, StatusCode};
use common::{TestApp, TestResponse};
use pennychest_core::ai::http::FakeApi;
use serde_json::{Value, json};

const TOKEN: &str = "test-token";
const JEV: &str = "https://api.typesafe.ai/v1/systemone";
const AI: i64 = 4;
const IMPORT_DEFAULT: i64 = 3;
const MANUAL: i64 = 2;

/// How sure the model is that a tap and a statement line are the same payment.
type Judge = Box<dyn Fn(&Value, &Value) -> f64 + Send>;

/// How the fake Jev answers: choice questions with `choice` at `confidence`, yes/no questions
/// with `judge(a, b)`, or everything with `status` if it isn't 200.
struct Jev {
    choice: Option<String>,
    confidence: f64,
    judge: Judge,
    status: u16,
}

type Answers = Arc<Mutex<Jev>>;

fn fake_jev(api: &FakeApi) -> Answers {
    let answers = Arc::new(Mutex::new(Jev {
        choice: None,
        confidence: 0.0,
        judge: Box::new(|_, _| 0.0),
        status: 200,
    }));
    let jev = answers.clone();
    api.respond("POST", JEV, move |body| {
        let jev = jev.lock().unwrap();
        if jev.status != 200 {
            return (jev.status, json!({"error": "overloaded"}));
        }
        let mut answered = serde_json::Map::new();
        for (key, question) in body["questions"].as_object().unwrap() {
            let answer = if question["type"] == "noul" {
                let pair = &body["state"][key];
                json!({"type": "noul", "noul": (jev.judge)(&pair["a"], &pair["b"])})
            } else {
                json!({"type": "choice", "choice": jev.choice, "confidence": jev.confidence})
            };
            answered.insert(key.clone(), answer);
        }
        (200, json!({"model": "jev-1", "answers": answered}))
    });
    answers
}

struct Setup {
    app: TestApp,
    api: Arc<FakeApi>,
    jev: Answers,
}

/// A signed-in app with the tap token set, and Jev faked but not chosen for anything.
async fn setup() -> Setup {
    let api = FakeApi::new();
    let jev = fake_jev(&api);
    let app = TestApp::with_fake_api(api.clone()).await;
    sqlx::query("INSERT INTO app_settings (key, value) VALUES ('tap_token', ?)")
        .bind(TOKEN)
        .execute(&app.db)
        .await
        .unwrap();
    Setup { app, api, jev }
}

/// With Jev chosen for `task`, run `mode`.
async fn use_jev(s: &mut Setup, task: &str, mode: &str) {
    for (path, body) in [
        (
            "/api/ai/providers/typesafe".to_owned(),
            json!({"values": {"api_key": "ts-1"}}),
        ),
        (
            "/api/ai/models/decision".to_owned(),
            json!({"provider": "typesafe", "model": "jev-latest"}),
        ),
        (
            format!("/api/ai/tasks/{task}"),
            json!({"enabled": true, "kind": "decision", "mode": mode}),
        ),
    ] {
        s.app
            .put(&path, body)
            .await
            .assert_status(StatusCode::NO_CONTENT);
    }
}

impl Setup {
    async fn tap(&mut self, overrides: Value) -> TestResponse {
        let mut body = json!({"merchant": "Tesco", "amount": "£12.50", "card": "My Visa", "date": "2026-09-20"});
        for (key, value) in overrides.as_object().unwrap() {
            body[key] = value.clone();
        }
        let bearer = format!("Bearer {TOKEN}");
        let response = self
            .app
            .send_with_headers(
                Method::POST,
                "/api/taps",
                Some(body),
                &[("authorization", &bearer)],
            )
            .await;
        self.app.settle().await;
        response
    }

    async fn account(&mut self, path: &str, kind: &str) -> i64 {
        let name = path.rsplit(':').next().unwrap();
        let response = self
            .app
            .post(
                "/api/accounts",
                json!({"name": name, "full_path": path, "type": kind}),
            )
            .await;
        response.assert_status(StatusCode::CREATED);
        response.body["id"].as_i64().unwrap()
    }

    /// A card and a groceries category.
    async fn card_and_groceries(&mut self) -> (i64, i64) {
        let card = self
            .account("Liabilities:CreditCard:Visa", "liability")
            .await;
        let groceries = self.account("Expenses:Groceries", "expense").await;
        (card, groceries)
    }

    /// A statement line, as an import would make it: the card side negative for a purchase.
    async fn statement_line(
        &mut self,
        card: i64,
        category: i64,
        source: i64,
        day: &str,
        description: &str,
        amount: &str,
    ) -> i64 {
        let response = self
            .app
            .post(
                "/api/transactions",
                json!({
                    "date": day,
                    "description": description,
                    "postings": [
                        {"account_id": card, "amount": format!("-{amount}"), "categorised_by_id": source},
                        {"account_id": category, "amount": amount, "categorised_by_id": source},
                    ],
                }),
            )
            .await;
        response.assert_status(StatusCode::CREATED);
        response.body["id"].as_i64().unwrap()
    }

    async fn pair(&mut self, card: Option<i64>) -> TestResponse {
        self.app
            .put(
                "/api/taps/cards",
                json!({"card_name": "My Visa", "account_id": card}),
            )
            .await
    }

    async fn taps(&mut self, status: &str) -> Vec<Value> {
        let response = self.app.get(&format!("/api/taps?status={status}")).await;
        response.assert_status(StatusCode::OK);
        response.body.as_array().unwrap().clone()
    }

    async fn expense_path(&mut self, transaction: i64) -> String {
        let txn = self
            .app
            .get(&format!("/api/transactions/{transaction}"))
            .await
            .body;
        let posting = txn["postings"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| {
                p["account_full_path"]
                    .as_str()
                    .unwrap()
                    .starts_with("Expenses")
            })
            .unwrap()
            .clone();
        posting["account_full_path"].as_str().unwrap().to_owned()
    }
}

// Ingesting taps

#[tokio::test]
async fn ingest_requires_the_token() {
    let mut s = setup().await;
    let body = json!({"merchant": "Tesco", "amount": "1"});
    s.app
        .post("/api/taps", body.clone())
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
    s.app
        .send_with_headers(
            Method::POST,
            "/api/taps",
            Some(body),
            &[("authorization", "Bearer wrong")],
        )
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn ingest_is_off_without_a_token() {
    let mut s = setup().await;
    s.app
        .delete("/api/taps/token")
        .await
        .assert_status(StatusCode::NO_CONTENT);
    s.tap(json!({}))
        .await
        .assert_status(StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn token_endpoints_require_a_session() {
    let s = setup().await;
    let mut anon = s.app.other_device();
    anon.get("/api/taps/token")
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
    anon.post("/api/taps/token", json!(null))
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
    anon.delete("/api/taps/token")
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn the_token_can_be_read_replaced_and_cleared() {
    let mut s = setup().await;
    assert_eq!(
        s.app.get("/api/taps/token").await.body,
        json!({"token": TOKEN})
    );
    let new_token = s.app.post("/api/taps/token", json!(null)).await.body["token"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_ne!(new_token, TOKEN);
    assert_eq!(
        s.app.get("/api/taps/token").await.body,
        json!({"token": new_token})
    );
    s.tap(json!({}))
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
    let bearer = format!("Bearer {new_token}");
    let body = json!({"merchant": "Tesco", "amount": "£1.00"});
    s.app
        .send_with_headers(
            Method::POST,
            "/api/taps",
            Some(body.clone()),
            &[("authorization", &bearer)],
        )
        .await
        .assert_status(StatusCode::CREATED);

    s.app
        .delete("/api/taps/token")
        .await
        .assert_status(StatusCode::NO_CONTENT);
    assert_eq!(
        s.app.get("/api/taps/token").await.body,
        json!({"token": null})
    );
    s.app
        .send_with_headers(
            Method::POST,
            "/api/taps",
            Some(body),
            &[("authorization", &bearer)],
        )
        .await
        .assert_status(StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn ingest_validates_the_payload() {
    let mut s = setup().await;
    for (overrides, detail) in [
        (json!({"merchant": ""}), "merchant is required"),
        (json!({"amount": "abc"}), "could not parse amount 'abc'"),
        (json!({"amount": "£0.00"}), "amount must not be zero"),
        (
            json!({"date": "not a date"}),
            "could not parse date 'not a date'",
        ),
    ] {
        let response = s.tap(overrides).await;
        response.assert_status(StatusCode::BAD_REQUEST);
        assert_eq!(response.body["detail"], detail);
    }
    s.tap(json!({"amount": [1]}))
        .await
        .assert_status(StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn ingest_and_list() {
    let mut s = setup().await;
    let response = s.tap(json!({})).await;
    response.assert_status(StatusCode::CREATED);
    let tap = response.body;
    assert_eq!(tap["status"], "unmatched");
    assert_eq!(tap["currency"], "GBP");
    assert_eq!(tap["amount"], "12.5000");
    assert_eq!(tap["tapped_on"], "2026-09-20");
    assert_eq!(tap["account_id"], Value::Null);

    let listed = s.taps("unmatched").await;
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0]["id"], tap["id"]);
    assert_eq!(
        s.app.get("/api/taps/cards").await.body,
        json!([{"card_name": "My Visa", "account_id": null, "account_full_path": null, "tap_count": 1}])
    );
}

#[tokio::test]
async fn foreign_amounts_take_their_currency() {
    let mut s = setup().await;
    let tap = s.tap(json!({"amount": "12,50 €", "card": null})).await.body;
    assert_eq!(tap["currency"], "EUR");
    assert_eq!(tap["amount"], "12.5000");
    assert_eq!(tap["card_name"], Value::Null);
    let told = s.tap(json!({"amount": 3, "currency": " usd "})).await.body;
    assert_eq!(told["currency"], "USD");
    assert_eq!(told["amount"], "3.0000");
}

// Reconciling

#[tokio::test]
async fn pairing_reconciles_existing_statement_lines() {
    let mut s = setup().await;
    let (card, groceries) = s.card_and_groceries().await;
    let txn = s
        .statement_line(
            card,
            groceries,
            MANUAL,
            "2026-09-22",
            "TESCO STORES 1234",
            "12.50",
        )
        .await;
    assert_eq!(s.tap(json!({})).await.body["status"], "unmatched");

    let paired = s.pair(Some(card)).await;
    paired.assert_status(StatusCode::OK);
    assert_eq!(
        paired.body,
        json!({
            "card_name": "My Visa",
            "account_id": card,
            "account_full_path": "Liabilities:CreditCard:Visa",
            "tap_count": 1,
        })
    );
    let reconciled = s.taps("reconciled").await;
    assert_eq!(reconciled.len(), 1);
    assert_eq!(reconciled[0]["transaction_id"], txn);
    assert_eq!(
        reconciled[0]["transaction_description"],
        "TESCO STORES 1234"
    );
    assert_eq!(reconciled[0]["transaction_date"], "2026-09-22");
    assert_eq!(reconciled[0]["account_id"], card);
}

#[tokio::test]
async fn a_tap_after_the_statement_reconciles_on_ingest() {
    let mut s = setup().await;
    let (card, groceries) = s.card_and_groceries().await;
    s.pair(Some(card)).await;
    s.statement_line(card, groceries, MANUAL, "2026-09-21", "TESCO", "12.50")
        .await;
    assert_eq!(s.tap(json!({})).await.body["status"], "reconciled");
}

#[tokio::test]
async fn no_match_outside_the_window_or_for_another_amount() {
    let mut s = setup().await;
    let (card, groceries) = s.card_and_groceries().await;
    s.pair(Some(card)).await;
    s.statement_line(card, groceries, MANUAL, "2026-10-05", "TESCO", "12.50")
        .await;
    s.statement_line(card, groceries, MANUAL, "2026-09-21", "TESCO", "12.51")
        .await;
    assert_eq!(s.tap(json!({})).await.body["status"], "unmatched");
}

#[tokio::test]
async fn the_merchant_wins_and_each_transaction_is_linked_once() {
    let mut s = setup().await;
    let (card, groceries) = s.card_and_groceries().await;
    s.pair(Some(card)).await;
    let other = s
        .statement_line(
            card,
            groceries,
            MANUAL,
            "2026-09-20",
            "PRET A MANGER",
            "12.50",
        )
        .await;
    let tesco = s
        .statement_line(
            card,
            groceries,
            MANUAL,
            "2026-09-23",
            "TESCO STORES",
            "12.50",
        )
        .await;
    assert_eq!(s.tap(json!({})).await.body["transaction_id"], tesco);
    // A second tap can only take the remaining transaction.
    let second = s.tap(json!({"merchant": "Pret"})).await.body;
    assert_eq!(second["transaction_id"], other);
}

#[tokio::test]
async fn foreign_currency_taps_are_not_matched() {
    let mut s = setup().await;
    let (card, groceries) = s.card_and_groceries().await;
    s.pair(Some(card)).await;
    s.statement_line(card, groceries, MANUAL, "2026-09-21", "TESCO", "12.50")
        .await;
    let tap = s.tap(json!({"amount": "12,50 €"})).await.body;
    assert_eq!(tap["status"], "unmatched");
}

#[tokio::test]
async fn dismiss_restore_and_delete() {
    let mut s = setup().await;
    let id = s.tap(json!({})).await.body["id"].as_i64().unwrap();
    let dismissed = s
        .app
        .post(&format!("/api/taps/{id}/dismiss"), json!(null))
        .await
        .body;
    assert_eq!(dismissed["status"], "dismissed");
    assert!(s.taps("unmatched").await.is_empty());
    assert_eq!(s.taps("dismissed").await.len(), 1);
    assert_eq!(s.taps("all").await.len(), 1);

    let restored = s
        .app
        .post(&format!("/api/taps/{id}/restore"), json!(null))
        .await
        .body;
    assert_eq!(restored["status"], "unmatched");

    s.app
        .delete(&format!("/api/taps/{id}"))
        .await
        .assert_status(StatusCode::NO_CONTENT);
    s.app
        .delete(&format!("/api/taps/{id}"))
        .await
        .assert_status(StatusCode::NOT_FOUND);
    s.app
        .post(&format!("/api/taps/{id}/dismiss"), json!(null))
        .await
        .assert_status(StatusCode::NOT_FOUND);
    s.app
        .get("/api/taps?status=sideways")
        .await
        .assert_status(StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn deleting_the_statement_transaction_unlinks_the_tap() {
    let mut s = setup().await;
    let (card, groceries) = s.card_and_groceries().await;
    s.pair(Some(card)).await;
    let txn = s
        .statement_line(card, groceries, MANUAL, "2026-09-21", "TESCO", "12.50")
        .await;
    assert_eq!(s.tap(json!({})).await.body["status"], "reconciled");
    s.app
        .delete(&format!("/api/transactions/{txn}"))
        .await
        .assert_status(StatusCode::NO_CONTENT);
    assert_eq!(s.taps("unmatched").await[0]["status"], "unmatched");
}

#[tokio::test]
async fn pairing_refuses_expense_accounts_and_can_unpair() {
    let mut s = setup().await;
    let (card, groceries) = s.card_and_groceries().await;
    s.tap(json!({})).await;
    s.pair(Some(groceries))
        .await
        .assert_status(StatusCode::BAD_REQUEST);
    s.pair(Some(999))
        .await
        .assert_status(StatusCode::BAD_REQUEST);
    s.app
        .put(
            "/api/taps/cards",
            json!({"card_name": " ", "account_id": card}),
        )
        .await
        .assert_status(StatusCode::BAD_REQUEST);

    s.pair(Some(card)).await.assert_status(StatusCode::OK);
    let unpaired = s.pair(None).await.body;
    assert_eq!(unpaired["account_id"], Value::Null);
    assert_eq!(s.taps("unmatched").await[0]["account_id"], Value::Null);
}

#[tokio::test]
async fn rules_suggest_a_category() {
    let mut s = setup().await;
    let (_, groceries) = s.card_and_groceries().await;
    s.app
        .post(
            "/api/rules",
            json!({"pattern": "tesco", "match_type_id": 1, "target_account_id": groceries,
                   "priority": 0, "source": "manual", "description": ""}),
        )
        .await
        .assert_status(StatusCode::CREATED);
    let tap = s.tap(json!({})).await.body;
    assert_eq!(tap["suggested_account_full_path"], "Expenses:Groceries");
    assert_eq!(tap["suggestion_source"], "rule");
}

// Categorising taps with a model

async fn categories(s: &mut Setup) -> HashMap<String, i64> {
    let mut ids = HashMap::new();
    for path in [
        "Expenses:Uncategorised",
        "Expenses:Groceries",
        "Expenses:Transport",
    ] {
        ids.insert(path.to_owned(), s.account(path, "expense").await);
    }
    ids
}

fn answer(s: &Setup, choice: &str, confidence: f64) {
    let mut jev = s.jev.lock().unwrap();
    jev.choice = Some(choice.to_owned());
    jev.confidence = confidence;
}

async fn logs(s: &mut Setup) -> Vec<Value> {
    s.app.get("/api/ai/logs").await.body["logs"]
        .as_array()
        .unwrap()
        .clone()
}

#[tokio::test]
async fn a_tap_is_categorised_after_the_reply() {
    let mut s = setup().await;
    use_jev(&mut s, "taps", "automatic").await;
    categories(&mut s).await;
    answer(&s, "Expenses:Groceries", 0.92);
    let reply = s.tap(json!({})).await;
    reply.assert_status(StatusCode::CREATED);
    // The reply to the phone doesn't wait for the model.
    assert_eq!(reply.body["suggested_account_full_path"], Value::Null);

    let tap = &s.taps("unmatched").await[0];
    assert_eq!(tap["suggested_account_full_path"], "Expenses:Groceries");
    assert_eq!(tap["suggestion_source"], "ai");
    assert_eq!(tap["suggestion_confidence"], 0.92);

    let sent = s.api.last_body();
    assert_eq!(sent["model"], "jev-latest");
    let state: Vec<&Value> = sent["state"].as_object().unwrap().values().collect();
    assert_eq!(state, [&json!("Tesco (GBP 12.50)")]);
    let question = sent["questions"]
        .as_object()
        .unwrap()
        .values()
        .next()
        .unwrap();
    let criteria = question["criteria"].as_object().unwrap();
    assert!(!criteria.contains_key("Expenses:Uncategorised"));
    assert!(criteria.contains_key("Expenses:Groceries"));

    let log = &logs(&mut s).await[0];
    assert_eq!(log["operation"], "tap_categorise");
    assert_eq!(log["provider"], "typesafe:jev-latest");
    assert_eq!(log["error"], Value::Null);
}

#[tokio::test]
async fn refunds_say_so() {
    let mut s = setup().await;
    use_jev(&mut s, "taps", "automatic").await;
    categories(&mut s).await;
    s.tap(json!({"amount": "-3.10"})).await;
    let sent = s.api.last_body();
    let state: Vec<&Value> = sent["state"].as_object().unwrap().values().collect();
    assert_eq!(state, [&json!("Tesco (GBP 3.10, refund)")]);
}

#[tokio::test]
async fn unsure_answers_are_left_for_the_user() {
    let mut s = setup().await;
    use_jev(&mut s, "taps", "automatic").await;
    categories(&mut s).await;
    answer(&s, "Expenses:Transport", 0.2);
    s.tap(json!({})).await;
    assert_eq!(
        s.taps("unmatched").await[0]["suggested_account_full_path"],
        Value::Null
    );
}

#[tokio::test]
async fn nothing_runs_unless_turned_on() {
    let mut s = setup().await;
    categories(&mut s).await;
    s.tap(json!({})).await;
    assert!(s.api.bodies(JEV).is_empty());
}

#[tokio::test]
async fn rules_win_and_skip_the_model() {
    let mut s = setup().await;
    use_jev(&mut s, "taps", "automatic").await;
    let ids = categories(&mut s).await;
    s.app
        .post(
            "/api/rules",
            json!({"pattern": "tesco", "match_type_id": 1,
                   "target_account_id": ids["Expenses:Groceries"],
                   "priority": 0, "source": "manual", "description": ""}),
        )
        .await;
    assert_eq!(s.tap(json!({})).await.body["suggestion_source"], "rule");
    assert!(s.api.bodies(JEV).is_empty());
}

#[tokio::test]
async fn a_failing_model_never_breaks_the_tap() {
    let mut s = setup().await;
    use_jev(&mut s, "taps", "automatic").await;
    categories(&mut s).await;
    s.jev.lock().unwrap().status = 503;
    s.tap(json!({})).await.assert_status(StatusCode::CREATED);
    assert_eq!(
        s.taps("unmatched").await[0]["suggested_account_full_path"],
        Value::Null
    );
    assert!(logs(&mut s).await[0]["error"].is_string());
}

/// A card statement line the import left uncategorised.
async fn imported_line(s: &mut Setup, card: i64, uncategorised: i64, source: i64) -> i64 {
    s.statement_line(
        card,
        uncategorised,
        source,
        "2026-09-20",
        "TESCO STORES 2041",
        "5.00",
    )
    .await
}

#[tokio::test]
async fn a_statement_line_takes_the_taps_category() {
    let mut s = setup().await;
    use_jev(&mut s, "taps", "automatic").await;
    let ids = categories(&mut s).await;
    let card = s.account("Liabilities:Card", "liability").await;
    answer(&s, "Expenses:Groceries", 0.9);
    let txn = imported_line(&mut s, card, ids["Expenses:Uncategorised"], IMPORT_DEFAULT).await;
    s.tap(json!({"amount": "£5.00"})).await;
    s.pair(Some(card)).await;
    assert_eq!(s.taps("reconciled").await[0]["transaction_id"], txn);
    assert_eq!(s.expense_path(txn).await, "Expenses:Groceries");
    let postings = s.app.get(&format!("/api/transactions/{txn}")).await.body["postings"].clone();
    let expense = postings
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["account_full_path"] == "Expenses:Groceries")
        .unwrap()
        .clone();
    assert_eq!(expense["categorised_by_id"], AI);
}

#[tokio::test]
async fn statement_categories_are_never_overwritten() {
    let mut s = setup().await;
    use_jev(&mut s, "taps", "automatic").await;
    let ids = categories(&mut s).await;
    let card = s.account("Liabilities:Card", "liability").await;
    answer(&s, "Expenses:Transport", 0.9);
    let txn = s
        .statement_line(
            card,
            ids["Expenses:Groceries"],
            MANUAL,
            "2026-09-20",
            "TESCO STORES 2041",
            "5.00",
        )
        .await;
    s.tap(json!({"amount": "£5.00"})).await;
    s.pair(Some(card)).await;
    assert_eq!(s.taps("reconciled").await[0]["transaction_id"], txn);
    assert_eq!(s.expense_path(txn).await, "Expenses:Groceries");
}

#[tokio::test]
async fn on_demand_taps_wait_to_be_asked() {
    let mut s = setup().await;
    use_jev(&mut s, "taps", "on_demand").await;
    categories(&mut s).await;
    answer(&s, "Expenses:Groceries", 0.92);
    let id = s.tap(json!({})).await.body["id"].as_i64().unwrap();
    assert!(s.api.bodies(JEV).is_empty());
    let response = s
        .app
        .post(&format!("/api/taps/{id}/categorise"), json!(null))
        .await;
    response.assert_status(StatusCode::OK);
    assert_eq!(
        response.body["suggested_account_full_path"],
        "Expenses:Groceries"
    );
}

#[tokio::test]
async fn asking_for_a_category_needs_the_task_on() {
    let mut s = setup().await;
    categories(&mut s).await;
    let id = s.tap(json!({})).await.body["id"].as_i64().unwrap();
    let path = format!("/api/taps/{id}/categorise");
    // On by default, so a new install says what's missing.
    let response = s.app.post(&path, json!(null)).await;
    response.assert_status(StatusCode::BAD_REQUEST);
    assert_eq!(
        response.body["detail"],
        "Choose a decision model for categorising card taps in Settings > AI."
    );
    s.app
        .put("/api/ai/tasks/taps", json!({"enabled": false}))
        .await
        .assert_status(StatusCode::NO_CONTENT);
    let response = s.app.post(&path, json!(null)).await;
    response.assert_status(StatusCode::BAD_REQUEST);
    let detail = response.body["detail"].as_str().unwrap();
    assert!(detail.contains("turned off"), "{detail}");
    s.app
        .post("/api/taps/999/categorise", json!(null))
        .await
        .assert_status(StatusCode::NOT_FOUND);
}

// Matching taps with a model

/// Two statement lines a "Bean There Coffee" tap could be, neither sharing a word with it,
/// then the card paired (which reconciles). The lines' ids.
async fn two_lines(s: &mut Setup) -> (i64, i64) {
    let (card, groceries) = s.card_and_groceries().await;
    let bean = s
        .statement_line(
            card,
            groceries,
            MANUAL,
            "2026-09-20",
            "SQ *BTC LTD 0231",
            "12.50",
        )
        .await;
    let other = s
        .statement_line(card, groceries, MANUAL, "2026-09-21", "PAYPAL *ZX", "12.50")
        .await;
    s.pair(Some(card)).await;
    s.app.settle().await;
    (bean, other)
}

fn judge_btc(s: &Setup, sure: f64) {
    s.jev.lock().unwrap().judge = Box::new(move |_, line| {
        if line["description"].as_str().unwrap().contains("BTC") {
            sure
        } else {
            0.05
        }
    });
}

#[tokio::test]
async fn the_model_picks_between_candidates() {
    let mut s = setup().await;
    use_jev(&mut s, "matching", "automatic").await;
    judge_btc(&s, 0.92);
    s.tap(json!({"merchant": "Bean There Coffee"})).await;
    let (bean, _) = two_lines(&mut s).await;
    let reconciled = s.taps("reconciled").await;
    assert_eq!(reconciled.len(), 1);
    assert_eq!(reconciled[0]["transaction_id"], bean);
    let sent = &s.api.bodies(JEV)[0]["state"];
    assert_eq!(sent["p0"]["a"]["merchant"], "Bean There Coffee");
    let mut lines = [
        sent["p0"]["b"]["description"].as_str().unwrap(),
        sent["p1"]["b"]["description"].as_str().unwrap(),
    ];
    lines.sort_unstable();
    assert_eq!(lines, ["PAYPAL *ZX", "SQ *BTC LTD 0231"]);
    assert!(
        logs(&mut s)
            .await
            .iter()
            .any(|l| l["operation"] == "tap_match")
    );
}

#[tokio::test]
async fn a_late_tap_is_matched_after_the_reply() {
    let mut s = setup().await;
    use_jev(&mut s, "matching", "automatic").await;
    judge_btc(&s, 0.92);
    let (bean, _) = two_lines(&mut s).await;
    // The reply doesn't wait for the model; reconciling in the background links it.
    let reply = s.tap(json!({"merchant": "Bean There Coffee"})).await.body;
    assert_eq!(reply["transaction_id"], Value::Null);
    let reconciled = s.taps("reconciled").await;
    assert_eq!(reconciled[0]["transaction_id"], bean);
}

#[tokio::test]
async fn taps_stay_unmatched_when_the_model_is_unsure() {
    let mut s = setup().await;
    use_jev(&mut s, "matching", "automatic").await;
    judge_btc(&s, 0.3);
    s.tap(json!({"merchant": "Bean There Coffee"})).await;
    two_lines(&mut s).await;
    let reconciled = s.app.post("/api/taps/reconcile", json!(null)).await;
    assert_eq!(reconciled.body, json!({"reconciled": 0}));
    assert!(s.taps("reconciled").await.is_empty());
}

#[tokio::test]
async fn without_the_model_the_closest_date_wins() {
    let mut s = setup().await;
    s.tap(json!({"merchant": "Bean There Coffee"})).await;
    let (bean, _) = two_lines(&mut s).await;
    s.app.post("/api/taps/reconcile", json!(null)).await;
    assert_eq!(s.taps("reconciled").await[0]["transaction_id"], bean);
    assert!(s.api.bodies(JEV).is_empty());
}
