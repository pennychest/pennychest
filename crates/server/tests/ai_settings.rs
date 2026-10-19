//! AI settings and model lists. The providers' own tests are in the core crate.

mod common;

use axum::http::StatusCode;
use common::TestApp;
use pennychest_core::ai::config::{self, Task};
use pennychest_core::ai::http::FakeApi;
use serde_json::{Value, json};

async fn provider(app: &mut TestApp, id: &str) -> Value {
    let config = app.get("/api/ai/config").await.body;
    config["providers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == id)
        .unwrap()
        .clone()
}

async fn save(app: &mut TestApp, id: &str, values: Value) -> StatusCode {
    app.put(
        &format!("/api/ai/providers/{id}"),
        json!({ "values": values }),
    )
    .await
    .status
}

fn ids(models: &Value) -> Vec<&str> {
    models
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["id"].as_str().unwrap())
        .collect()
}

// Config endpoints

#[tokio::test]
async fn config_lists_providers_without_revealing_secrets() {
    let mut app = TestApp::signed_in().await;
    save(&mut app, "openai", json!({"api_key": "sk-secret"})).await;
    let config = app.get("/api/ai/config").await;
    let providers = config.body["providers"].as_array().unwrap();
    let listed: Vec<&str> = providers
        .iter()
        .map(|p| p["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        listed,
        [
            "anthropic",
            "openai",
            "google",
            "vertex",
            "typesafe",
            "openai_decisions",
            "ollama"
        ]
    );
    let decisions: Vec<&str> = providers
        .iter()
        .filter(|p| p["kind"] == "decision")
        .map(|p| p["id"].as_str().unwrap())
        .collect();
    assert_eq!(decisions, ["typesafe", "openai_decisions"]);
    let openai = provider(&mut app, "openai").await;
    assert_eq!(openai["configured"], true);
    assert_eq!(provider(&mut app, "anthropic").await["configured"], false);
    let key = &openai["fields"][0];
    assert_eq!(
        (&key["key"], &key["is_set"], &key["value"], &key["secret"]),
        (&json!("api_key"), &json!(true), &Value::Null, &json!(true))
    );
    // The base URL isn't secret, and shows its default.
    assert_eq!(openai["fields"][1]["value"], "https://api.openai.com/v1");
    assert_eq!(openai["fields"][1]["is_set"], false);
    assert!(!config.body.to_string().contains("sk-secret"));
    let ollama = provider(&mut app, "ollama").await;
    assert_eq!(ollama["configured"], false);
    assert_eq!(ollama["fields"][0]["value"], Value::Null);

    let categorise = &config.body["tasks"]["categorise"];
    assert_eq!(categorise["ready"], false);
    assert!(
        categorise["problem"]
            .as_str()
            .unwrap()
            .contains("Settings > AI")
    );
}

#[tokio::test]
async fn save_and_clear_provider_fields() {
    let mut app = TestApp::signed_in().await;
    assert_eq!(
        save(&mut app, "typesafe", json!({"api_key": "ts-1"})).await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        save(&mut app, "typesafe", json!({"nope": "x"})).await,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        save(&mut app, "unknown", json!({})).await,
        StatusCode::NOT_FOUND
    );
    assert_eq!(provider(&mut app, "typesafe").await["configured"], true);
    save(&mut app, "typesafe", json!({"api_key": null})).await;
    assert_eq!(provider(&mut app, "typesafe").await["configured"], false);
    save(&mut app, "typesafe", json!({"api_key": "  "})).await;
    assert_eq!(provider(&mut app, "typesafe").await["configured"], false);
}

#[tokio::test]
async fn choosing_a_model_checks_its_kind() {
    let mut app = TestApp::signed_in().await;
    let choose = async |app: &mut TestApp, kind: &str, provider: Value, model: &str| {
        app.put(
            &format!("/api/ai/models/{kind}"),
            json!({"provider": provider, "model": model}),
        )
        .await
        .status
    };
    assert_eq!(
        choose(&mut app, "llm", json!("typesafe"), "m").await,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        choose(&mut app, "decision", json!("openai"), "m").await,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        choose(&mut app, "decision", json!("typesafe"), "").await,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        choose(&mut app, "nope", json!("openai"), "m").await,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        choose(&mut app, "decision", json!("typesafe"), "jev-latest").await,
        StatusCode::NO_CONTENT
    );

    let decision = app.get("/api/ai/config").await.body["models"]["decision"].clone();
    assert_eq!(decision["provider"], "typesafe");
    assert_eq!(decision["ready"], false);
    assert!(decision["problem"].as_str().unwrap().contains("API key"));

    save(&mut app, "typesafe", json!({"api_key": "ts-1"})).await;
    let config = app.get("/api/ai/config").await.body;
    assert_eq!(config["models"]["decision"]["ready"], true);

    assert_eq!(
        app.put("/api/ai/models/decision", json!({"provider": null}))
            .await
            .status,
        StatusCode::NO_CONTENT
    );
    let config = app.get("/api/ai/config").await.body;
    assert_eq!(config["models"]["decision"]["provider"], Value::Null);
}

#[tokio::test]
async fn tasks_share_the_model_of_their_kind() {
    let mut app = TestApp::signed_in().await;
    save(&mut app, "typesafe", json!({"api_key": "ts-1"})).await;
    save(&mut app, "openai", json!({"api_key": "sk-1"})).await;
    for task in ["categorise", "chat", "rules"] {
        app.put(&format!("/api/ai/tasks/{task}"), json!({"enabled": true}))
            .await;
    }
    app.put(
        "/api/ai/models/decision",
        json!({"provider": "typesafe", "model": "jev-latest"}),
    )
    .await;
    app.put(
        "/api/ai/models/llm",
        json!({"provider": "openai", "model": "gpt-5-mini"}),
    )
    .await;

    let tasks = app.get("/api/ai/config").await.body["tasks"].clone();
    // Tasks that can use either prefer the decision model, and run by themselves with it
    let categorise = &tasks["categorise"];
    assert_eq!(categorise["kinds"], json!(["decision", "llm"]));
    assert_eq!(categorise["kind"], "decision");
    assert_eq!(categorise["provider"], "typesafe");
    assert_eq!(categorise["mode"], "automatic");
    assert_eq!(categorise["ready"], true);
    assert_eq!(tasks["chat"]["provider"], "openai");
    assert_eq!(tasks["chat"]["mode"], Value::Null);
    assert_eq!(tasks["chat"]["scopes"], json!([]));
    assert_eq!(tasks["rules"]["kinds"], json!(["llm"]));

    // With a language model, tasks only run when asked unless told otherwise
    app.put("/api/ai/tasks/categorise", json!({"kind": "llm"}))
        .await;
    let task = app.get("/api/ai/config").await.body["tasks"]["categorise"].clone();
    assert_eq!(
        (&task["provider"], &task["mode"]),
        (&json!("openai"), &json!("on_demand"))
    );
    app.put("/api/ai/tasks/categorise", json!({"mode": "automatic"}))
        .await;
    let task = app.get("/api/ai/config").await.body["tasks"]["categorise"].clone();
    assert_eq!(task["mode"], "automatic");
    assert!(
        config::runs_automatically(&app.db, Task::Categorise)
            .await
            .unwrap()
    );

    app.put("/api/ai/tasks/categorise", json!({"enabled": false}))
        .await;
    let task = app.get("/api/ai/config").await.body["tasks"]["categorise"].clone();
    assert_eq!(
        (&task["enabled"], &task["provider"]),
        (&json!(false), &Value::Null)
    );
    assert!(task["problem"].as_str().unwrap().contains("turned off"));
    assert!(
        !config::runs_automatically(&app.db, Task::Categorise)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn task_settings_are_checked() {
    let mut app = TestApp::signed_in().await;
    for (task, body, status) in [
        (
            "rules",
            json!({"kind": "decision"}),
            StatusCode::BAD_REQUEST,
        ),
        (
            "chat",
            json!({"mode": "automatic"}),
            StatusCode::BAD_REQUEST,
        ),
        (
            "categorise",
            json!({"mode": "sometimes"}),
            StatusCode::BAD_REQUEST,
        ),
        ("nope", json!({"enabled": true}), StatusCode::NOT_FOUND),
        (
            "categorise",
            json!({"kind": "llm", "mode": "on_demand", "enabled": true}),
            StatusCode::NO_CONTENT,
        ),
    ] {
        app.put(&format!("/api/ai/tasks/{task}"), body)
            .await
            .assert_status(status);
    }
    let response = app
        .put("/api/ai/tasks/rules", json!({"kind": "decision"}))
        .await;
    assert_eq!(
        response.body["detail"],
        "Suggesting rules can't use a decision model."
    );
}

#[tokio::test]
async fn new_installs_have_every_task_on() {
    let mut app = TestApp::signed_in().await;
    let tasks = app.get("/api/ai/config").await.body["tasks"].clone();
    assert!(
        tasks
            .as_object()
            .unwrap()
            .values()
            .all(|t| t["enabled"] == true)
    );
}

#[tokio::test]
async fn the_chat_may_only_change_what_it_is_allowed_to() {
    let mut app = TestApp::signed_in().await;
    app.put("/api/ai/chat/scopes", json!({"scopes": ["admin"]}))
        .await
        .assert_status(StatusCode::BAD_REQUEST);
    app.put(
        "/api/ai/chat/scopes",
        json!({"scopes": ["transactions", "organise"]}),
    )
    .await
    .assert_status(StatusCode::NO_CONTENT);
    let tasks = app.get("/api/ai/config").await.body["tasks"].clone();
    assert_eq!(tasks["chat"]["scopes"], json!(["organise", "transactions"]));
}

#[tokio::test]
async fn config_endpoints_require_a_session() {
    let mut app = TestApp::new().await;
    for status in [
        app.get("/api/ai/config").await.status,
        app.put("/api/ai/providers/openai", json!({"values": {}}))
            .await
            .status,
        app.post("/api/ai/providers/openai/models", json!({}))
            .await
            .status,
        app.put("/api/ai/tasks/categorise", json!({"enabled": false}))
            .await
            .status,
        app.put("/api/ai/models/llm", json!({"provider": null}))
            .await
            .status,
        app.get("/api/ai/logs").await.status,
    ] {
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }
}

// Model lists

#[tokio::test]
async fn refreshing_needs_credentials() {
    let mut app = TestApp::signed_in().await;
    let response = app.post("/api/ai/providers/openai/models", json!({})).await;
    response.assert_status(StatusCode::BAD_REQUEST);
    assert!(
        response.body["detail"]
            .as_str()
            .unwrap()
            .contains("API key")
    );
}

#[tokio::test]
async fn refreshing_openai_keeps_chat_models() {
    let api = FakeApi::new();
    let mut app = TestApp::with_fake_api(api.clone()).await;
    save(&mut app, "openai", json!({"api_key": "sk-1"})).await;
    api.on(
        "GET",
        "https://api.openai.com/v1/models",
        200,
        json!({"data": [
            {"id": "gpt-5-mini"}, {"id": "o4-mini"}, {"id": "text-embedding-3-small"},
            {"id": "gpt-realtime"}, {"id": "whisper-1"}, {"id": "gpt-4o-mini-tts"},
        ]}),
    );
    let response = app.post("/api/ai/providers/openai/models", json!({})).await;
    response.assert_status(StatusCode::OK);
    assert_eq!(ids(&response.body["models"]), ["gpt-5-mini", "o4-mini"]);
    assert!(
        api.requests()[0]
            .headers
            .contains(&("Authorization".into(), "Bearer sk-1".into()))
    );
    let openai = provider(&mut app, "openai").await;
    assert_eq!(ids(&openai["models"]), ["gpt-5-mini", "o4-mini"]);
    assert!(
        openai["models_updated_at"]
            .as_str()
            .unwrap()
            .ends_with("+00:00")
    );
}

#[tokio::test]
async fn a_custom_openai_base_url_lists_every_model() {
    let api = FakeApi::new();
    let mut app = TestApp::with_fake_api(api.clone()).await;
    save(
        &mut app,
        "openai",
        json!({"api_key": "k", "base_url": "https://proxy.example/v1/"}),
    )
    .await;
    api.on(
        "GET",
        "https://proxy.example/v1/models",
        200,
        json!({"data": [{"id": "anthropic/claude-sonnet-5-5"}, {"id": "meta/llama"}]}),
    );
    let models = app.post("/api/ai/providers/openai/models", json!({})).await;
    assert_eq!(
        ids(&models.body["models"]),
        ["anthropic/claude-sonnet-5-5", "meta/llama"]
    );
}

#[tokio::test]
async fn refreshing_google_models() {
    let api = FakeApi::new();
    let mut app = TestApp::with_fake_api(api.clone()).await;
    save(&mut app, "google", json!({"api_key": "g-1"})).await;
    api.on(
        "GET",
        "https://generativelanguage.googleapis.com/v1beta/openai/models",
        200,
        json!({"data": [
            {"id": "models/gemini-3.8-flash"}, {"id": "models/gemini-embedding-001"},
            {"id": "models/imagen-4"}, {"id": "gemini-3.1-pro-preview"},
        ]}),
    );
    let models = app.post("/api/ai/providers/google/models", json!({})).await;
    assert_eq!(
        ids(&models.body["models"]),
        ["gemini-3.1-pro-preview", "gemini-3.8-flash"]
    );
}

#[tokio::test]
async fn refreshing_typesafe_and_ollama_models() {
    let api = FakeApi::new();
    let mut app = TestApp::with_fake_api(api.clone()).await;
    save(&mut app, "typesafe", json!({"api_key": "ts-1"})).await;
    api.on(
        "GET",
        "https://api.typesafe.ai/v1/models",
        200,
        json!({"models": [
            {"name": "jev-latest", "description": "Stable", "release_date": "2026-09-01"},
            {"name": "jev-preview", "description": "Preview", "release_date": "2026-09-01"},
        ]}),
    );
    let models = app
        .post("/api/ai/providers/typesafe/models", json!({}))
        .await;
    assert_eq!(ids(&models.body["models"]), ["jev-latest", "jev-preview"]);

    save(&mut app, "ollama", json!({"url": "http://ollama:11434/"})).await;
    api.on(
        "GET",
        "http://ollama:11434/api/tags",
        200,
        json!({"models": [{"name": "qwen3:4b"}, {"name": "llama3.2:3b"}]}),
    );
    let models = app.post("/api/ai/providers/ollama/models", json!({})).await;
    assert_eq!(ids(&models.body["models"]), ["llama3.2:3b", "qwen3:4b"]);
}

#[tokio::test]
async fn provider_errors_are_readable_and_keep_keys_private() {
    let api = FakeApi::new();
    let mut app = TestApp::with_fake_api(api.clone()).await;
    save(&mut app, "openai", json!({"api_key": "sk-very-secret"})).await;
    api.on(
        "GET",
        "https://api.openai.com/v1/models",
        401,
        json!({"error": {"message": "Incorrect key"}}),
    );
    let response = app.post("/api/ai/providers/openai/models", json!({})).await;
    response.assert_status(StatusCode::BAD_REQUEST);
    let detail = response.body["detail"].as_str().unwrap();
    assert!(detail.contains("rejected the credentials"));
    assert!(!response.body.to_string().contains("sk-very-secret"));

    // A rejected key is remembered until the credentials change.
    assert!(
        provider(&mut app, "openai").await["problem"]
            .as_str()
            .unwrap()
            .contains("rejected")
    );
    save(&mut app, "openai", json!({"api_key": "sk-new"})).await;
    assert_eq!(provider(&mut app, "openai").await["problem"], Value::Null);

    // A busy provider isn't remembered as a problem.
    api.on("GET", "https://api.openai.com/v1/models", 429, json!({}));
    app.post("/api/ai/providers/openai/models", json!({}))
        .await
        .assert_status(StatusCode::BAD_REQUEST);
    assert_eq!(provider(&mut app, "openai").await["problem"], Value::Null);
}

#[tokio::test]
async fn refreshing_anthropic_skips_models_without_structured_outputs() {
    let api = FakeApi::new();
    let mut app = TestApp::with_fake_api(api.clone()).await;
    save(&mut app, "anthropic", json!({"api_key": "sk-ant-1"})).await;
    api.on(
        "GET",
        "https://api.anthropic.com/v1/models?limit=1000",
        200,
        json!({"data": [
            {"id": "claude-opus-5-5", "display_name": "Claude Opus 5.5",
             "capabilities": {"structured_outputs": {"supported": true}}},
            {"id": "claude-old", "display_name": "Claude Old",
             "capabilities": {"structured_outputs": {"supported": false}}},
        ], "has_more": false}),
    );
    let models = app
        .post("/api/ai/providers/anthropic/models", json!({}))
        .await;
    assert_eq!(
        models.body["models"],
        json!([{"id": "claude-opus-5-5", "label": "Claude Opus 5.5"}])
    );
    assert!(
        api.requests()[0]
            .headers
            .contains(&("x-api-key".into(), "sk-ant-1".into()))
    );
}

// Logs

#[tokio::test]
async fn request_logs_newest_first() {
    let mut app = TestApp::signed_in().await;
    for (operation, error) in [("categorise", None), ("suggest_rules", Some("Busy"))] {
        pennychest_core::ai::log(
            &app.db,
            pennychest_core::ai::NewLog {
                provider: "openai:gpt-5-mini",
                operation,
                request: Some(&json!({"model": "gpt-5-mini"})),
                response: None,
                duration_ms: 12,
                error,
            },
        )
        .await
        .unwrap();
    }
    let logs = app.get("/api/ai/logs?limit=1").await.body;
    assert_eq!(logs["total"], 2);
    let entry = &logs["logs"][0];
    assert_eq!(entry["operation"], "suggest_rules");
    assert_eq!(entry["error"], "Busy");
    assert_eq!(entry["request_data"], json!({"model": "gpt-5-mini"}));
    assert_eq!(entry["response_data"], Value::Null);
    assert_eq!(entry["duration_ms"], 12);
}
