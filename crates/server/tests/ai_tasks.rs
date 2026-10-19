//! The AI tasks, with providers' APIs faked: categorising, rules, insights and CSV column
//! mapping.

mod common;

use std::collections::HashMap;
use std::sync::Arc;

use axum::http::StatusCode;
use common::TestApp;
use pennychest_core::ai::http::FakeApi;
use serde_json::{Value, json};

const JEV: &str = "https://api.typesafe.ai/v1/systemone";
const OPENAI: &str = "https://api.openai.com/v1/chat/completions";
const IMPORT_DEFAULT: i64 = 3;

/// Has `task` use this provider's model, running `mode` where it has the choice.
async fn use_model(app: &mut TestApp, task: &str, provider: &str, model: &str, mode: Option<&str>) {
    let kind = if ["typesafe", "openai_decisions"].contains(&provider) {
        "decision"
    } else {
        "llm"
    };
    app.put(
        &format!("/api/ai/models/{kind}"),
        json!({"provider": provider, "model": model}),
    )
    .await
    .assert_status(StatusCode::NO_CONTENT);
    let mut settings = json!({"enabled": true, "kind": kind});
    if let Some(mode) = mode {
        settings["mode"] = json!(mode);
    }
    app.put(&format!("/api/ai/tasks/{task}"), settings)
        .await
        .assert_status(StatusCode::NO_CONTENT);
}

async fn set_key(app: &mut TestApp, provider: &str, values: Value) {
    app.put(
        &format!("/api/ai/providers/{provider}"),
        json!({ "values": values }),
    )
    .await
    .assert_status(StatusCode::NO_CONTENT);
}

/// Answers every choice question with `choice` at `confidence`, and every score question with
/// `score(context)`.
fn fake_jev(
    api: &FakeApi,
    choice: impl Fn(&Value) -> (Value, f64) + Send + Sync + 'static,
    score: impl Fn(&Value) -> f64 + Send + Sync + 'static,
) {
    api.respond("POST", JEV, move |body| {
        let mut answers = serde_json::Map::new();
        for (key, question) in body["questions"].as_object().unwrap() {
            let item = &body["state"][key];
            let answer = if question["type"] == "choice" {
                let (choice, confidence) = choice(item);
                json!({"type": "choice", "choice": choice, "confidence": confidence})
            } else {
                assert_eq!(question["criteria"].as_array().unwrap().len(), 4);
                json!({"type": "score", "score": score(item), "confidence": 0.8})
            };
            answers.insert(key.clone(), answer);
        }
        (200, json!({"model": "jev-1", "answers": answers}))
    });
}

/// Jev requests asking this type of question, about something other than a transaction's
/// category (whose questions are keyed "t" and the transaction's id).
fn asked(api: &FakeApi, kind: &str) -> Vec<Value> {
    api.bodies(JEV)
        .into_iter()
        .filter(|b| {
            let (key, question) = b["questions"].as_object().unwrap().iter().next().unwrap();
            question["type"] == kind && !key.starts_with('t')
        })
        .collect()
}

/// Jev requests asking for transactions' categories.
fn categorising(api: &FakeApi) -> Vec<Value> {
    api.bodies(JEV)
        .into_iter()
        .filter(|b| {
            b["questions"]
                .as_object()
                .unwrap()
                .keys()
                .all(|k| k.starts_with('t'))
        })
        .collect()
}

/// Transactions as an import leaves them: the expense side on Expenses:Uncategorised.
async fn imported_transactions(
    app: &mut TestApp,
    descriptions: &[&str],
) -> (Vec<i64>, HashMap<String, i64>) {
    let card = app
        .post(
            "/api/accounts",
            json!({"name": "Card", "full_path": "Liabilities:Card", "type": "liability"}),
        )
        .await
        .body["id"]
        .as_i64()
        .unwrap();
    for path in [
        "Expenses:Uncategorised",
        "Expenses:Groceries",
        "Expenses:Transport",
    ] {
        let name = path.rsplit(':').next().unwrap();
        app.post(
            "/api/accounts",
            json!({"name": name, "full_path": path, "type": "expense"}),
        )
        .await;
    }
    let accounts: HashMap<String, i64> = app
        .get("/api/accounts")
        .await
        .body
        .as_array()
        .unwrap()
        .iter()
        .map(|a| {
            (
                a["full_path"].as_str().unwrap().to_owned(),
                a["id"].as_i64().unwrap(),
            )
        })
        .collect();
    let mut ids = Vec::new();
    for description in descriptions {
        let response = app
            .post(
                "/api/transactions",
                json!({
                    "date": "2026-09-20",
                    "description": description,
                    "postings": [
                        {"account_id": card, "amount": "-5.00", "categorised_by_id": IMPORT_DEFAULT},
                        {"account_id": accounts["Expenses:Uncategorised"], "amount": "5.00",
                         "categorised_by_id": IMPORT_DEFAULT},
                    ],
                }),
            )
            .await;
        ids.push(response.body["id"].as_i64().unwrap());
    }
    (ids, accounts)
}

async fn expense_account(app: &mut TestApp, id: i64) -> String {
    let txn = app.get(&format!("/api/transactions/{id}")).await.body;
    txn["postings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["account_full_path"].as_str().unwrap().to_owned())
        .find(|p| p.starts_with("Expenses"))
        .unwrap()
}

async fn logs(app: &mut TestApp) -> Vec<Value> {
    app.get("/api/ai/logs").await.body["logs"]
        .as_array()
        .unwrap()
        .clone()
}

// Categorising on demand

#[tokio::test]
async fn categorising_needs_a_configured_task() {
    let mut app = TestApp::signed_in().await;
    imported_transactions(&mut app, &["TESCO STORES"]).await;
    let response = app.post("/api/ai/categorise/all", json!({})).await;
    response.assert_status(StatusCode::BAD_REQUEST);
    assert!(
        response.body["detail"]
            .as_str()
            .unwrap()
            .contains("Settings > AI")
    );
    assert_eq!(logs(&mut app).await[0]["provider"], "not configured");
}

#[tokio::test]
async fn categorising_with_jev_uses_choices_and_confidence() {
    let api = FakeApi::new();
    let mut app = TestApp::with_fake_api(api.clone()).await;
    let (ids, _) = imported_transactions(&mut app, &["TESCO STORES 2041", "SQ *MYSTERY"]).await;
    let (tesco, mystery) = (ids[0], ids[1]);
    set_key(&mut app, "typesafe", json!({"api_key": "ts-1"})).await;
    use_model(
        &mut app,
        "categorise",
        "typesafe",
        "jev-latest",
        Some("automatic"),
    )
    .await;
    api.on(
        "POST",
        JEV,
        200,
        json!({
            "model": "jev-1.13.0",
            "answers": {
                format!("t{tesco}"): {"type": "choice", "choice": "Expenses:Groceries", "confidence": 0.9},
                format!("t{mystery}"): {"type": "choice", "choice": "Expenses:Transport", "confidence": 0.85},
            },
        }),
    );

    let response = app.post("/api/ai/categorise/all", json!({})).await;
    response.assert_status(StatusCode::OK);
    assert_eq!(response.body, json!({"updated": 1}));
    assert_eq!(expense_account(&mut app, tesco).await, "Expenses:Groceries");
    assert_eq!(
        expense_account(&mut app, mystery).await,
        "Expenses:Uncategorised"
    );

    let sent = api.last_body();
    assert_eq!(sent["model"], "jev-latest");
    assert_eq!(
        sent["state"],
        json!({format!("t{tesco}"): "TESCO STORES 2041", format!("t{mystery}"): "SQ *MYSTERY"})
    );
    let question = &sent["questions"][format!("t{tesco}")];
    assert_eq!(question["type"], "choice");
    assert!(question["criteria"].get("Expenses:Groceries").is_some());
    assert!(question["criteria"].get("Expenses:Uncategorised").is_none());
    let log = &logs(&mut app).await[0];
    assert_eq!(log["provider"], "typesafe:jev-latest");
    assert_eq!(log["error"], Value::Null);
}

#[tokio::test]
async fn categorising_with_openai_structured_output() {
    let api = FakeApi::new();
    let mut app = TestApp::with_fake_api(api.clone()).await;
    let (ids, _) = imported_transactions(&mut app, &["TESCO STORES"]).await;
    set_key(&mut app, "openai", json!({"api_key": "sk-1"})).await;
    use_model(&mut app, "categorise", "openai", "gpt-5-mini", None).await;
    let content = json!({"categorisations": [
        {"transaction_id": ids[0], "account_full_path": "Expenses:Groceries"},
        {"transaction_id": 99999, "account_full_path": "Expenses:Groceries"},
    ]});
    api.on(
        "POST",
        OPENAI,
        200,
        json!({"choices": [{"message": {"content": content.to_string()}, "finish_reason": "stop"}]}),
    );
    let response = app.post("/api/ai/categorise/all", json!({})).await;
    assert_eq!(response.body, json!({"updated": 1}));
    assert_eq!(
        expense_account(&mut app, ids[0]).await,
        "Expenses:Groceries"
    );
    let sent = api.last_body();
    assert_eq!(sent["model"], "gpt-5-mini");
    let schema = &sent["response_format"]["json_schema"];
    assert_eq!(schema["strict"], true);
    let item = &schema["schema"]["properties"]["categorisations"]["items"];
    assert!(
        item["properties"]["account_full_path"]["enum"]
            .as_array()
            .unwrap()
            .contains(&json!("Expenses:Groceries"))
    );
}

#[tokio::test]
async fn categorising_with_anthropic_structured_output() {
    let api = FakeApi::new();
    let mut app = TestApp::with_fake_api(api.clone()).await;
    let (ids, _) = imported_transactions(&mut app, &["TESCO STORES"]).await;
    set_key(&mut app, "anthropic", json!({"api_key": "sk-ant-1"})).await;
    use_model(
        &mut app,
        "categorise",
        "anthropic",
        "claude-haiku-5-5",
        None,
    )
    .await;
    let text = json!({"categorisations": [
        {"transaction_id": ids[0], "account_full_path": "Expenses:Groceries"},
    ]});
    api.on(
        "POST",
        "https://api.anthropic.com/v1/messages",
        200,
        json!({"id": "msg_1", "stop_reason": "end_turn", "content": [{"type": "text", "text": text.to_string()}]}),
    );
    assert_eq!(
        app.post("/api/ai/categorise/all", json!({})).await.body,
        json!({"updated": 1})
    );
    let sent = api.last_body();
    assert_eq!(sent["model"], "claude-haiku-5-5");
    assert_eq!(sent["output_config"]["format"]["type"], "json_schema");
    assert!(sent.get("tool_choice").is_none());
}

#[tokio::test]
async fn an_anthropic_refusal_is_reported() {
    let api = FakeApi::new();
    let mut app = TestApp::with_fake_api(api.clone()).await;
    imported_transactions(&mut app, &["TESCO STORES"]).await;
    set_key(&mut app, "anthropic", json!({"api_key": "sk-ant-1"})).await;
    use_model(&mut app, "categorise", "anthropic", "claude-opus-5-5", None).await;
    api.on(
        "POST",
        "https://api.anthropic.com/v1/messages",
        200,
        json!({"stop_reason": "refusal", "content": []}),
    );
    let response = app.post("/api/ai/categorise/all", json!({})).await;
    response.assert_status(StatusCode::BAD_REQUEST);
    assert!(
        response.body["detail"]
            .as_str()
            .unwrap()
            .contains("declined")
    );
    assert!(logs(&mut app).await[0]["error"].is_string());
}

// Rule suggestions

#[tokio::test]
async fn suggesting_rules_with_ollama() {
    let api = FakeApi::new();
    let mut app = TestApp::with_fake_api(api.clone()).await;
    imported_transactions(&mut app, &["TESCO STORES"]).await;
    set_key(&mut app, "ollama", json!({"url": "http://ollama:11434"})).await;
    use_model(&mut app, "rules", "ollama", "qwen3:4b", None).await;
    let content = json!({"rules": [
        {"pattern": "TESCO", "match_type": "substring",
         "target_account_full_path": "Expenses:Groceries", "priority": 500,
         "description": "Supermarket"},
        {"pattern": "X", "match_type": "fuzzy", "target_account_full_path": "Expenses:Groceries",
         "priority": 10, "description": "bad match type"},
        {"pattern": "Y", "match_type": "prefix", "target_account_full_path": "Expenses:Invented",
         "priority": 10, "description": "unknown account"},
    ]});
    api.on(
        "POST",
        "http://ollama:11434/api/chat",
        200,
        json!({"message": {"content": content.to_string()}}),
    );
    let response = app.post("/api/ai/suggest-rules", json!({})).await;
    response.assert_status(StatusCode::OK);
    let suggestions = response.body["suggestions"].as_array().unwrap();
    assert_eq!(suggestions.len(), 1);
    assert_eq!(suggestions[0]["pattern"], "TESCO");
    assert_eq!(suggestions[0]["priority"], 100);
    assert_eq!(suggestions[0]["match_type_id"], 1);
    assert!(suggestions[0]["target_account_id"].is_i64());
    let sent = api.last_body();
    assert_eq!(
        (&sent["model"], &sent["stream"]),
        (&json!("qwen3:4b"), &json!(false))
    );
    assert!(
        sent["format"]["properties"]["rules"]["items"]["properties"]["match_type"]["enum"]
            .is_array()
    );
}

#[tokio::test]
async fn rules_need_a_language_model() {
    let mut app = TestApp::signed_in().await;
    set_key(&mut app, "typesafe", json!({"api_key": "ts-1"})).await;
    app.put(
        "/api/ai/models/decision",
        json!({"provider": "typesafe", "model": "jev-latest"}),
    )
    .await;
    let response = app.post("/api/ai/suggest-rules", json!({})).await;
    response.assert_status(StatusCode::BAD_REQUEST);
    assert!(
        response.body["detail"]
            .as_str()
            .unwrap()
            .contains("Choose a language model")
    );
}

// Categorising as imports arrive

const STATEMENT: &str =
    "Date,Description,Amount\n20/09/2026,OAKFIELD FARM SHOP,-8.40\n21/09/2026,TFL TRAVEL,-3.10\n";

/// A card and the categories, with Jev answering everything as groceries at 0.9.
async fn with_jev() -> (TestApp, Arc<FakeApi>, i64) {
    let api = FakeApi::new();
    let mut app = TestApp::with_fake_api(api.clone()).await;
    fake_jev(&api, |_| (json!("Expenses:Groceries"), 0.9), |_| 0.0);
    set_key(&mut app, "typesafe", json!({"api_key": "ts-1"})).await;
    use_model(
        &mut app,
        "categorise",
        "typesafe",
        "jev-latest",
        Some("automatic"),
    )
    .await;
    let (_, accounts) = imported_transactions(&mut app, &[]).await;
    let card = accounts["Liabilities:Card"];
    (app, api, card)
}

async fn upload(app: &mut TestApp, account: i64, name: &str, content: &str) -> Value {
    let response = app
        .post_form(
            "/api/imports/upload",
            Some((name, content.as_bytes())),
            &[
                ("account_id", &account.to_string()),
                ("importer_name", "csv"),
            ],
        )
        .await;
    assert_eq!(response.status, StatusCode::OK, "{}", response.body);
    app.settle().await;
    response.body
}

async fn batch_transactions(app: &mut TestApp, batch: &Value) -> Vec<Value> {
    app.get(&format!("/api/imports/batches/{batch}")).await.body["transactions"]
        .as_array()
        .unwrap()
        .clone()
}

#[tokio::test]
async fn imports_are_categorised_straight_away() {
    let (mut app, _, card) = with_jev().await;
    let result = upload(&mut app, card, "september.csv", STATEMENT).await;
    assert_eq!(result["ai_categorised_count"], 2);
    assert_eq!(result["uncategorised_count"], 0);
    assert_eq!(result["ai_error"], Value::Null);
    for txn in batch_transactions(&mut app, &result["batch_id"]).await {
        let id = txn["id"].as_i64().unwrap();
        assert_eq!(expense_account(&mut app, id).await, "Expenses:Groceries");
    }
    let changes = app.get("/api/changes").await.body["changes"].clone();
    assert_eq!(
        changes[0]["summary"],
        "Categorised 2 transactions from september.csv"
    );
    assert_eq!(changes[0]["source"], "import");
    assert_eq!(changes.as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn the_import_can_be_undone_in_one_go() {
    let (mut app, _, card) = with_jev().await;
    let result = upload(&mut app, card, "september.csv", STATEMENT).await;
    let change = app.get("/api/changes").await.body["changes"][0]["id"].clone();
    app.post(&format!("/api/changes/{change}/undo"), json!({}))
        .await
        .assert_status(StatusCode::OK);
    for txn in batch_transactions(&mut app, &result["batch_id"]).await {
        let id = txn["id"].as_i64().unwrap();
        let detail = app.get(&format!("/api/transactions/{id}")).await.body;
        let expense = detail["postings"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["account_full_path"].as_str().unwrap().starts_with("Exp"))
            .unwrap()
            .clone();
        assert_eq!(expense["account_full_path"], "Expenses:Uncategorised");
        assert_eq!(expense["categorised_by_id"], IMPORT_DEFAULT);
    }
}

#[tokio::test]
async fn categorising_can_run_on_demand_instead() {
    let (mut app, api, card) = with_jev().await;
    app.put("/api/ai/tasks/categorise", json!({"mode": "on_demand"}))
        .await
        .assert_status(StatusCode::NO_CONTENT);
    let result = upload(&mut app, card, "september.csv", STATEMENT).await;
    assert_eq!(result["ai_categorised_count"], 0);
    assert_eq!(result["uncategorised_count"], 2);
    assert!(categorising(&api).is_empty());

    // The button is undoable too.
    let response = app
        .post(
            &format!("/api/ai/categorise/batch/{}", result["batch_id"]),
            json!({}),
        )
        .await;
    assert_eq!(response.body, json!({"updated": 2}));
    assert_eq!(
        app.get("/api/changes").await.body["changes"][0]["source"],
        "api"
    );
    app.post("/api/ai/categorise/batch/9999", json!({}))
        .await
        .assert_status(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn nothing_runs_without_a_model() {
    let (mut app, api, card) = with_jev().await;
    app.put("/api/ai/tasks/categorise", json!({"enabled": false}))
        .await;
    let result = upload(&mut app, card, "september.csv", STATEMENT).await;
    assert_eq!(result["ai_categorised_count"], 0);
    assert_eq!(result["ai_error"], Value::Null);
    assert!(categorising(&api).is_empty());
}

#[tokio::test]
async fn a_failing_model_still_imports() {
    let api = FakeApi::new();
    let mut app = TestApp::with_fake_api(api.clone()).await;
    api.respond("POST", JEV, |_| (503, json!({"error": "overloaded"})));
    set_key(&mut app, "typesafe", json!({"api_key": "ts-1"})).await;
    use_model(
        &mut app,
        "categorise",
        "typesafe",
        "jev-latest",
        Some("automatic"),
    )
    .await;
    let (_, accounts) = imported_transactions(&mut app, &[]).await;
    let result = upload(
        &mut app,
        accounts["Liabilities:Card"],
        "september.csv",
        STATEMENT,
    )
    .await;
    assert_eq!(result["transaction_count"], 2);
    assert_eq!(result["uncategorised_count"], 2);
    assert!(result["ai_error"].is_string());
    assert_eq!(app.get("/api/changes").await.body["changes"], json!([]));
    assert!(logs(&mut app).await[0]["error"].is_string());
}

#[tokio::test]
async fn only_what_rules_left_is_sent() {
    let (mut app, api, card) = with_jev().await;
    let accounts = app.get("/api/accounts").await.body;
    let transport = accounts
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["full_path"] == "Expenses:Transport")
        .unwrap()["id"]
        .clone();
    app.post(
        "/api/rules",
        json!({"pattern": "TFL", "match_type_id": 1, "target_account_id": transport,
               "priority": 10, "source": "manual", "description": ""}),
    )
    .await;
    let result = upload(&mut app, card, "september.csv", STATEMENT).await;
    assert_eq!(result["categorised_count"], 1);
    assert_eq!(result["ai_categorised_count"], 1);
    let sent = categorising(&api).pop().unwrap();
    let described: Vec<&Value> = sent["state"].as_object().unwrap().values().collect();
    assert_eq!(described, [&json!("OAKFIELD FARM SHOP")]);
}

// Insights

const HISTORY: [(&str, &str, &str); 7] = [
    ("15/05/2026", "NETFLIX.COM 0588", "-10.99"),
    ("15/06/2026", "NETFLIX.COM 0688", "-10.99"),
    ("15/07/2026", "NETFLIX.COM 0788", "-10.99"),
    ("15/08/2026", "NETFLIX.COM 0888", "-10.99"),
    ("27/08/2026", "TESCO STORES", "-40.00"),
    ("03/09/2026", "TESCO STORES", "-41.00"),
    ("10/09/2026", "TESCO STORES", "-39.00"),
];
const SEPTEMBER: [(&str, &str, &str); 4] = [
    ("15/09/2026", "NETFLIX.COM 0988", "-12.99"),
    ("17/09/2026", "TESCO STORES", "-40.50"),
    ("21/09/2026", "ELECTRONICS WORLD", "-899.00"),
    ("22/09/2026", "REFUND ACME", "15.00"),
];

async fn current(app: &mut TestApp) -> i64 {
    for (path, kind) in [
        ("Assets:Current", "asset"),
        ("Expenses:Uncategorised", "expense"),
    ] {
        let name = path.rsplit(':').next().unwrap();
        app.post(
            "/api/accounts",
            json!({"name": name, "full_path": path, "type": kind}),
        )
        .await;
    }
    let accounts = app.get("/api/accounts").await.body;
    accounts
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["full_path"] == "Assets:Current")
        .unwrap()["id"]
        .as_i64()
        .unwrap()
}

/// Imports the rows; the batch id and its transactions by description.
async fn import_rows(
    app: &mut TestApp,
    account: i64,
    rows: &[(&str, &str, &str)],
    name: &str,
) -> (i64, HashMap<String, Value>) {
    let content: String = std::iter::once("Date,Description,Amount\n".to_owned())
        .chain(
            rows.iter()
                .map(|(d, desc, amt)| format!("{d},{desc},{amt}\n")),
        )
        .collect();
    let result = upload(app, account, name, &content).await;
    let batch = result["batch_id"].as_i64().unwrap();
    let by_description = batch_transactions(app, &result["batch_id"])
        .await
        .into_iter()
        .map(|t| (t["description"].as_str().unwrap().to_owned(), t))
        .collect();
    (batch, by_description)
}

async fn insights_jev() -> (TestApp, Arc<FakeApi>, i64) {
    let api = FakeApi::new();
    let mut app = TestApp::with_fake_api(api.clone()).await;
    fake_jev(
        &api,
        |m| {
            // Categorising asks too, about a transaction's description; leave those be.
            let Some(merchant) = m["merchant"].as_str() else {
                return (Value::Null, 0.0);
            };
            if merchant.contains("NETFLIX") {
                (json!("subscription"), 0.95)
            } else {
                (json!("other"), 0.9)
            }
        },
        |c| {
            let amount: f64 = c["charge"]["amount"].as_str().unwrap().parse().unwrap();
            if amount > 500.0 { 3.0 } else { 0.2 }
        },
    );
    set_key(&mut app, "typesafe", json!({"api_key": "ts-1"})).await;
    use_model(
        &mut app,
        "insights",
        "typesafe",
        "jev-latest",
        Some("automatic"),
    )
    .await;
    let account = current(&mut app).await;
    (app, api, account)
}

async fn action(app: &mut TestApp, name: &str, arguments: Value) -> Value {
    let response = app.post(&format!("/api/actions/{name}"), arguments).await;
    assert_eq!(response.status, StatusCode::OK, "{}", response.body);
    response.body
}

#[tokio::test]
async fn imports_are_scored_against_each_merchants_history() {
    let (mut app, api, account) = insights_jev().await;
    import_rows(&mut app, account, &HISTORY, "history.csv").await;
    let before = api.bodies(JEV).len();
    let (_, september) = import_rows(&mut app, account, &SEPTEMBER, "september.csv").await;

    assert_eq!(september["ELECTRONICS WORLD"]["unusual_score"], 1.0);
    assert_eq!(september["TESCO STORES"]["unusual_score"], 0.067);
    // Refunds aren't charges.
    assert_eq!(september["REFUND ACME"]["unusual_score"], Value::Null);

    let scored: Vec<Value> = asked(&api, "score").into_iter().skip(1).collect();
    assert_eq!(scored.len(), 1);
    assert!(api.bodies(JEV).len() > before);
    let contexts: HashMap<String, Value> = scored[0]["state"]
        .as_object()
        .unwrap()
        .values()
        .map(|c| {
            (
                c["charge"]["description"].as_str().unwrap().to_owned(),
                c.clone(),
            )
        })
        .collect();
    let mut described: Vec<&String> = contexts.keys().collect();
    described.sort();
    assert_eq!(
        described,
        ["ELECTRONICS WORLD", "NETFLIX.COM 0988", "TESCO STORES"]
    );
    let tesco: Vec<&Value> = contexts["TESCO STORES"]["merchant_history"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| &h["amount"])
        .collect();
    assert_eq!(tesco, [&json!("40.00"), &json!("41.00"), &json!("39.00")]);
    assert_eq!(contexts["ELECTRONICS WORLD"]["merchant_history"], json!([]));
    // Uncategorised, so there's no category to compare with.
    assert!(contexts["TESCO STORES"].get("category_last_year").is_none());
}

#[tokio::test]
async fn recurring_merchants_are_labelled_once() {
    let (mut app, api, account) = insights_jev().await;
    import_rows(&mut app, account, &HISTORY, "history.csv").await;
    import_rows(&mut app, account, &SEPTEMBER, "september.csv").await;

    // Tesco is weekly, so it only still counts as active within ten days or so of its last.
    let payments = action(&mut app, "recurring_payments", json!({"on": "2026-09-20"})).await;
    let mut kinds: Vec<(String, String)> = payments["payments"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| {
            (
                p["merchant"].as_str().unwrap().into(),
                p["kind"].as_str().unwrap().into(),
            )
        })
        .collect();
    kinds.sort();
    assert_eq!(
        kinds,
        [
            ("NETFLIX.COM 0988".to_owned(), "subscription".to_owned()),
            ("TESCO STORES".to_owned(), "other".to_owned()),
        ]
    );
    let only = action(
        &mut app,
        "recurring_payments",
        json!({"on": "2026-09-20", "kind": "subscription"}),
    )
    .await;
    assert_eq!(only["payments"][0]["merchant"], "NETFLIX.COM 0988");
    assert_eq!(only["yearly_total"], "131.88");

    let labelled = asked(&api, "choice").len();
    import_rows(
        &mut app,
        account,
        &[("24/09/2026", "TESCO STORES", "-38.00")],
        "more.csv",
    )
    .await;
    // Nothing new to label.
    assert_eq!(asked(&api, "choice").len(), labelled);
}

#[tokio::test]
async fn unusual_charges_and_price_rises() {
    let (mut app, _, account) = insights_jev().await;
    import_rows(&mut app, account, &HISTORY, "history.csv").await;
    import_rows(&mut app, account, &SEPTEMBER, "september.csv").await;
    let result = action(
        &mut app,
        "unusual_charges",
        json!({"start_date": "2026-09-01", "end_date": "2026-09-30"}),
    )
    .await;
    let found: Vec<(&Value, &Value, &Value)> = result["unusual_charges"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| (&c["description"], &c["amount"], &c["unusual_score"]))
        .collect();
    assert_eq!(
        found,
        [(&json!("ELECTRONICS WORLD"), &json!("899.00"), &json!(1.0))]
    );
    assert_eq!(
        result["price_rises"],
        json!([{
            "merchant": "NETFLIX.COM 0988",
            "kind": "subscription",
            "cadence": "monthly",
            "usual_amount": "10.99",
            "latest_amount": "12.99",
            "latest_date": "2026-09-15",
            "increase": "2.00",
        }])
    );
    // Tesco's charges come from the history import, which was scored when it arrived.
    assert_eq!(result["unscored_charges"], 0);
}

#[tokio::test]
async fn nothing_is_judged_unless_turned_on() {
    let api = FakeApi::new();
    let mut app = TestApp::with_fake_api(api.clone()).await;
    let account = current(&mut app).await;
    let (_, september) = import_rows(&mut app, account, &SEPTEMBER, "september.csv").await;
    assert!(api.requests().is_empty());
    assert_eq!(september["ELECTRONICS WORLD"]["unusual_score"], Value::Null);
    let result = action(
        &mut app,
        "unusual_charges",
        json!({"start_date": "2026-09-01", "end_date": "2026-09-30"}),
    )
    .await;
    assert_eq!(result["unusual_charges"], json!([]));
    assert_eq!(result["unscored_charges"], 3);
}

#[tokio::test]
async fn a_failing_model_never_breaks_the_import() {
    let api = FakeApi::new();
    let mut app = TestApp::with_fake_api(api.clone()).await;
    api.respond("POST", JEV, |_| (503, json!({"error": "overloaded"})));
    set_key(&mut app, "typesafe", json!({"api_key": "ts-1"})).await;
    use_model(
        &mut app,
        "insights",
        "typesafe",
        "jev-latest",
        Some("automatic"),
    )
    .await;
    let account = current(&mut app).await;
    let (_, september) = import_rows(&mut app, account, &SEPTEMBER, "september.csv").await;
    assert_eq!(september["ELECTRONICS WORLD"]["unusual_score"], Value::Null);
    assert!(
        logs(&mut app)
            .await
            .iter()
            .any(|l| l["operation"] == "score_charges" && l["error"].is_string())
    );
}

#[tokio::test]
async fn language_models_score_and_label_with_json() {
    let api = FakeApi::new();
    let mut app = TestApp::with_fake_api(api.clone()).await;
    let account = current(&mut app).await;
    let (batch, _) = import_rows(&mut app, account, &HISTORY, "history.csv").await;
    api.respond("POST", OPENAI, |body| {
        let prompt = body["messages"][1]["content"].as_str().unwrap();
        let name = body["response_format"]["json_schema"]["name"].as_str().unwrap();
        let content = if name == "score_charges" {
            let listing: serde_json::Map<String, Value> =
                serde_json::from_str(&prompt[prompt.find('{').unwrap()..]).unwrap();
            let scores: Vec<Value> = listing
                .keys()
                .map(|id| json!({"id": id.parse::<i64>().unwrap(), "level": 2}))
                .collect();
            json!({ "scores": scores })
        } else {
            let listing: Vec<Value> =
                serde_json::from_str(&prompt[prompt.find("[\n").unwrap()..]).unwrap();
            let labels: Vec<Value> = listing
                .iter()
                .map(|pair| json!({"index": pair[0], "kind": "bill"}))
                .collect();
            json!({ "labels": labels })
        };
        (
            200,
            json!({"choices": [{"message": {"content": content.to_string()}, "finish_reason": "stop"}]}),
        )
    });
    set_key(&mut app, "openai", json!({"api_key": "sk-1"})).await;
    use_model(&mut app, "insights", "openai", "gpt", Some("on_demand")).await;
    app.post(&format!("/api/ai/insights/batch/{batch}"), json!({}))
        .await
        .assert_status(StatusCode::OK);
    let payments = action(&mut app, "recurring_payments", json!({"on": "2026-09-10"})).await;
    let kinds: Vec<&Value> = payments["payments"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| &p["kind"])
        .collect();
    assert!(!kinds.is_empty() && kinds.iter().all(|k| **k == "bill"));
    let unusual = action(
        &mut app,
        "unusual_charges",
        json!({"start_date": "2026-05-01", "end_date": "2026-09-30"}),
    )
    .await;
    assert!(
        unusual["unusual_charges"]
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c["unusual_score"] == 0.667)
    );
}

#[tokio::test]
async fn on_demand_insights_run_from_the_review_page() {
    let (mut app, api, account) = insights_jev().await;
    app.put("/api/ai/tasks/insights", json!({"mode": "on_demand"}))
        .await;
    let (batch, _) = import_rows(&mut app, account, &HISTORY, "history.csv").await;
    assert!(asked(&api, "score").is_empty());
    app.post(&format!("/api/ai/insights/batch/{batch}"), json!({}))
        .await
        .assert_status(StatusCode::OK);
    assert!(!asked(&api, "score").is_empty());
    app.post("/api/ai/insights/batch/9999", json!({}))
        .await
        .assert_status(StatusCode::NOT_FOUND);
}

// CSV columns

/// A layout the importer doesn't know by its column names.
const ODD: &str = "Posted,Ref,Who,Out,In,Running total\n\
                   20/09/2026,A1,TESCO STORES,12.50,,987.50\n\
                   21/09/2026,A2,SALARY,,1500.00,2487.50\n";

fn odd_columns() -> Value {
    json!({"date": 0, "description": 2, "money_out": 3, "money_in": 4})
}

/// Jev's reply choosing these columns, and "none" for the other fields.
fn jev_columns(columns: &Value, confidence: f64) -> Value {
    let answers: serde_json::Map<String, Value> =
        ["date", "description", "amount", "money_out", "money_in"]
            .iter()
            .map(|f| {
                let choice = columns.get(*f).map_or("none".to_owned(), |c| c.to_string());
                (
                    (*f).to_owned(),
                    json!({"type": "choice", "choice": choice, "confidence": confidence}),
                )
            })
            .collect();
    json!({ "answers": answers })
}

async fn preview(app: &mut TestApp, content: &str, ai: bool) -> Value {
    let fields: &[(&str, &str)] = if ai { &[("ai", "true")] } else { &[] };
    let response = app
        .post_form(
            "/api/imports/csv/preview",
            Some(("statement.csv", content.as_bytes())),
            fields,
        )
        .await;
    assert_eq!(response.status, StatusCode::OK, "{}", response.body);
    response.body
}

fn chosen(preview: &Value) -> Value {
    let columns: serde_json::Map<String, Value> = preview["columns"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(f, c)| (f.clone(), c["column"].clone()))
        .collect();
    Value::Object(columns)
}

#[tokio::test]
async fn without_a_model_the_gaps_stay() {
    let mut app = TestApp::signed_in().await;
    let shown = preview(&mut app, ODD, false).await;
    assert_eq!(shown["columns"], json!({}));
    assert_eq!(shown["ai_available"], false);
    let asked = preview(&mut app, ODD, true).await;
    assert!(
        asked["ai_error"]
            .as_str()
            .unwrap()
            .contains("Settings > AI")
    );
}

#[tokio::test]
async fn a_decision_model_fills_the_gaps_with_its_confidence() {
    let api = FakeApi::new();
    let mut app = TestApp::with_fake_api(api.clone()).await;
    set_key(&mut app, "typesafe", json!({"api_key": "ts-1"})).await;
    use_model(
        &mut app,
        "csv_columns",
        "typesafe",
        "jev-latest",
        Some("automatic"),
    )
    .await;
    api.on("POST", JEV, 200, jev_columns(&odd_columns(), 0.88));
    let shown = preview(&mut app, ODD, false).await;
    assert_eq!(chosen(&shown), odd_columns());
    assert_eq!(
        shown["columns"]["date"],
        json!({"column": 0, "source": "ai", "confidence": 0.88})
    );
    assert_eq!(shown["ai_available"], true);
    let sent = api.last_body();
    assert_eq!(sent["state"]["csv"]["headers"][0], "Posted");
    assert_eq!(
        sent["questions"]["date"]["criteria"]["5"],
        "Column 6, headed 'Running total'"
    );
    assert!(
        sent["questions"]["amount"]["criteria"]
            .get("none")
            .is_some()
    );
    let log = &logs(&mut app).await[0];
    assert_eq!(
        (&log["operation"], &log["error"]),
        (&json!("csv_columns"), &Value::Null)
    );
}

#[tokio::test]
async fn on_demand_the_model_waits_to_be_asked() {
    let api = FakeApi::new();
    let mut app = TestApp::with_fake_api(api.clone()).await;
    set_key(&mut app, "typesafe", json!({"api_key": "ts-1"})).await;
    use_model(
        &mut app,
        "csv_columns",
        "typesafe",
        "jev-latest",
        Some("on_demand"),
    )
    .await;
    assert_eq!(preview(&mut app, ODD, false).await["columns"], json!({}));
    assert!(api.requests().is_empty());
    api.on("POST", JEV, 200, jev_columns(&odd_columns(), 0.9));
    assert_eq!(chosen(&preview(&mut app, ODD, true).await), odd_columns());
}

#[tokio::test]
async fn a_language_model_fills_the_gaps() {
    let api = FakeApi::new();
    let mut app = TestApp::with_fake_api(api.clone()).await;
    set_key(&mut app, "openai", json!({"api_key": "sk-1"})).await;
    use_model(
        &mut app,
        "csv_columns",
        "openai",
        "gpt-5-mini",
        Some("automatic"),
    )
    .await;
    let answer =
        json!({"date": 0, "description": 2, "amount": null, "money_out": 3, "money_in": 4});
    api.on(
        "POST",
        OPENAI,
        200,
        json!({"choices": [{"message": {"content": answer.to_string()}, "finish_reason": "stop"}]}),
    );
    let shown = preview(&mut app, ODD, false).await;
    assert_eq!(chosen(&shown), odd_columns());
    assert_eq!(shown["columns"]["date"]["confidence"], Value::Null);
}

#[tokio::test]
async fn column_names_win_over_the_model() {
    let api = FakeApi::new();
    let mut app = TestApp::with_fake_api(api.clone()).await;
    set_key(&mut app, "typesafe", json!({"api_key": "ts-1"})).await;
    use_model(
        &mut app,
        "csv_columns",
        "typesafe",
        "jev-latest",
        Some("automatic"),
    )
    .await;
    // The names give the date and amount but not the description.
    api.on(
        "POST",
        JEV,
        200,
        jev_columns(&json!({"date": 2, "description": 1, "money_out": 2}), 0.9),
    );
    let columns = preview(
        &mut app,
        "Date,Who,Amount\n20/09/2026,TESCO,-12.50\n",
        false,
    )
    .await["columns"]
        .clone();
    assert_eq!(
        (&columns["date"]["source"], &columns["date"]["column"]),
        (&json!("name"), &json!(0))
    );
    assert_eq!(
        columns["description"],
        json!({"column": 1, "source": "ai", "confidence": 0.9})
    );
    assert!(columns.get("money_out").is_none());
}

#[tokio::test]
async fn a_failing_column_model_is_reported() {
    let api = FakeApi::new();
    let mut app = TestApp::with_fake_api(api.clone()).await;
    set_key(&mut app, "typesafe", json!({"api_key": "ts-1"})).await;
    use_model(
        &mut app,
        "csv_columns",
        "typesafe",
        "jev-latest",
        Some("automatic"),
    )
    .await;
    api.on("POST", JEV, 503, json!({"error": "overloaded"}));
    let shown = preview(&mut app, ODD, false).await;
    assert_eq!(shown["columns"], json!({}));
    assert!(shown["ai_error"].is_string());
}
