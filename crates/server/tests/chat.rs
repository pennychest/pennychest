//! Chat, with the model scripted through a faked OpenAI API, and checking the actions it
//! proposes. Its provider tests are with the providers.

mod common;

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use axum::http::StatusCode;
use common::TestApp;
use pennychest_core::ai::http::FakeApi;
use pennychest_core::chat::{LAST_ROUND, MAX_ROUNDS, TITLE_LENGTH};
use serde_json::{Value, json};

const OPENAI: &str = "https://api.openai.com/v1/chat/completions";
const JEV: &str = "https://api.typesafe.ai/v1/systemone";

/// A streamed reply that writes `text` in two parts, then asks for `calls`.
fn reply(text: &str, calls: &[(&str, Value)]) -> (u16, String) {
    let chunk = |delta: Value, finish: Value| json!({"choices": [{"index": 0, "delta": delta, "finish_reason": finish}]});
    let half = text
        .char_indices()
        .nth(text.chars().count() / 2)
        .map_or(text.len(), |(i, _)| i);
    let mut chunks = vec![];
    for part in [&text[..half], &text[half..]] {
        if !part.is_empty() {
            chunks.push(chunk(json!({"content": part}), Value::Null));
        }
    }
    for (i, (name, arguments)) in calls.iter().enumerate() {
        chunks.push(chunk(
            json!({"tool_calls": [{
                "index": i,
                "id": format!("id{i}"),
                "type": "function",
                "function": {"name": name, "arguments": arguments.to_string()},
            }]}),
            Value::Null,
        ));
    }
    let finish = if calls.is_empty() {
        "stop"
    } else {
        "tool_calls"
    };
    chunks.push(chunk(json!({}), json!(finish)));
    let mut body: String = chunks.iter().map(|c| format!("data: {c}\n\n")).collect();
    body.push_str("data: [DONE]\n\n");
    (200, body)
}

/// Another lookup, with nothing written.
fn lookup() -> (u16, String) {
    reply("", &[("list_categories", json!({}))])
}

/// The chat model's replies in order, then `default` for any more turns. Requests that aren't
/// streamed are the action check's, answered with `probability`.
struct Script {
    replies: Mutex<VecDeque<(u16, String)>>,
    default: Mutex<(u16, String)>,
    probability: Mutex<f64>,
}

impl Script {
    fn install(api: &FakeApi, replies: Vec<(u16, String)>) -> Arc<Self> {
        let script = Arc::new(Self {
            replies: Mutex::new(replies.into()),
            default: Mutex::new(lookup()),
            probability: Mutex::new(1.0),
        });
        let answers = script.clone();
        api.respond_raw("POST", OPENAI, move |body| {
            if body["stream"] == true {
                let next = answers.replies.lock().unwrap().pop_front();
                return next.unwrap_or_else(|| answers.default.lock().unwrap().clone());
            }
            let probability = *answers.probability.lock().unwrap();
            let content = json!({ "probability": probability }).to_string();
            let response =
                json!({"choices": [{"message": {"role": "assistant", "content": content}}]});
            (200, response.to_string())
        });
        script
    }
}

/// The chat model's requests.
fn chats(api: &FakeApi) -> Vec<Value> {
    api.bodies(OPENAI)
        .into_iter()
        .filter(|b| b["stream"] == true)
        .collect()
}

/// A request's conversation, without the system prompt.
fn history(request: &Value) -> Vec<Value> {
    request["messages"].as_array().unwrap()[1..].to_vec()
}

fn offered(request: &Value) -> Vec<String> {
    request["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["function"]["name"].as_str().unwrap().to_owned())
        .collect()
}

async fn put(app: &mut TestApp, path: &str, body: Value) {
    app.put(path, body)
        .await
        .assert_status(StatusCode::NO_CONTENT);
}

/// A signed-in app chatting with a scripted GPT.
async fn with_model(replies: Vec<(u16, String)>) -> (TestApp, Arc<FakeApi>, Arc<Script>) {
    let api = FakeApi::new();
    let script = Script::install(&api, replies);
    let mut app = TestApp::with_fake_api(api.clone()).await;
    put(
        &mut app,
        "/api/ai/providers/openai",
        json!({"values": {"api_key": "sk"}}),
    )
    .await;
    put(
        &mut app,
        "/api/ai/models/llm",
        json!({"provider": "openai", "model": "gpt-5"}),
    )
    .await;
    put(
        &mut app,
        "/api/ai/tasks/chat",
        json!({"enabled": true, "kind": "llm"}),
    )
    .await;
    (app, api, script)
}

fn parse_sse(text: &str) -> Vec<(String, Value)> {
    text.trim()
        .split("\n\n")
        .map(|block| {
            let (mut event, mut data) = (String::new(), Value::Null);
            for line in block.lines() {
                if let Some(name) = line.strip_prefix("event: ") {
                    event = name.to_owned();
                } else if let Some(json) = line.strip_prefix("data: ") {
                    data = serde_json::from_str(json).unwrap();
                }
            }
            (event, data)
        })
        .collect()
}

async fn send(app: &mut TestApp, message: &str, conversation: Option<i64>) -> Vec<(String, Value)> {
    let mut body = json!({ "message": message });
    if let Some(id) = conversation {
        body["conversation_id"] = json!(id);
    }
    let response = app.post("/api/chat/messages", body).await;
    assert_eq!(response.status, StatusCode::OK, "{}", response.body);
    assert!(
        response.headers["content-type"]
            .to_str()
            .unwrap()
            .starts_with("text/event-stream")
    );
    parse_sse(response.body.as_str().unwrap())
}

fn names(events: &[(String, Value)]) -> Vec<&str> {
    events.iter().map(|(e, _)| e.as_str()).collect()
}

fn has(events: &[(String, Value)], event: &str, data: Value) -> bool {
    events.iter().any(|(e, d)| e == event && *d == data)
}

async fn account(app: &mut TestApp, path: &str) {
    let name = path.rsplit(':').next().unwrap();
    app.post(
        "/api/accounts",
        json!({"name": name, "full_path": path, "type": "expense"}),
    )
    .await
    .assert_status(StatusCode::CREATED);
}

async fn turns(app: &mut TestApp, conversation: &Value) -> Vec<Value> {
    let response = app
        .get(&format!("/api/chat/conversations/{}", conversation["id"]))
        .await;
    response.body["turns"].as_array().unwrap().clone()
}

async fn logs(app: &mut TestApp) -> Vec<Value> {
    app.get("/api/ai/logs").await.body["logs"]
        .as_array()
        .unwrap()
        .clone()
}

// The chat loop

#[tokio::test]
async fn chat_runs_read_actions_and_answers() {
    let (mut app, api, _) = with_model(vec![
        reply(
            "Let me look.",
            &[("list_categories", json!({"kind": "expense"}))],
        ),
        reply("You have one spending category: Food.", &[]),
    ])
    .await;
    account(&mut app, "Expenses:Food").await;
    let events = send(&mut app, "What categories do I have?", None).await;
    assert_eq!(
        names(&events),
        [
            "conversation",
            "text",
            "text",
            "tool",
            "tool_result",
            "text",
            "text",
            "message",
            "done"
        ]
    );
    let conversation = &events[0].1;
    assert_eq!(conversation["title"], "What categories do I have?");
    let written = |range: std::ops::Range<usize>| -> String {
        events[range]
            .iter()
            .map(|(_, d)| d["delta"].as_str().unwrap())
            .collect()
    };
    assert_eq!(written(1..3), "Let me look.");
    assert_eq!(
        events[3].1,
        json!({"name": "list_categories", "arguments": {"kind": "expense"}})
    );
    assert_eq!(events[4].1, json!({"name": "list_categories", "ok": true}));
    assert_eq!(written(5..7), "You have one spending category: Food.");
    assert_eq!(
        events[7].1,
        json!({"content": "You have one spending category: Food."})
    );

    let requests = chats(&api);
    let tools = offered(&requests[0]);
    assert!(tools.contains(&"spending_summary".into()));
    assert!(!tools.contains(&"create_category".into()));
    let system = requests[0]["messages"][0]["content"].as_str().unwrap();
    assert!(system.contains("Today is"));
    let second = history(&requests[1]);
    let roles: Vec<&str> = second.iter().map(|m| m["role"].as_str().unwrap()).collect();
    assert_eq!(roles, ["user", "assistant", "tool"]);
    let result: Value = serde_json::from_str(second[2]["content"].as_str().unwrap()).unwrap();
    assert_eq!(result["categories"][0]["path"], "Expenses:Food");
    // The model's own message is replayed as it sent it.
    assert_eq!(second[1]["content"], "Let me look.");
    assert_eq!(second[1]["tool_calls"][0]["id"], "id0");

    assert_eq!(
        turns(&mut app, conversation).await,
        [
            json!({"role": "user", "content": "What categories do I have?"}),
            json!({
                "role": "assistant",
                "content": "You have one spending category: Food.",
                "tools": [{"name": "list_categories", "ok": true}],
                "changes": [],
            }),
        ]
    );
    let logged: Vec<(Value, Value)> = logs(&mut app)
        .await
        .iter()
        .map(|l| (l["operation"].clone(), l["provider"].clone()))
        .collect();
    assert_eq!(
        logged,
        [
            (json!("chat"), json!("openai:gpt-5")),
            (json!("chat"), json!("openai:gpt-5"))
        ]
    );
}

#[tokio::test]
async fn follow_up_messages_continue_the_conversation() {
    let (mut app, api, _) = with_model(vec![
        reply("First answer.", &[]),
        reply("Second answer.", &[]),
    ])
    .await;
    let id = send(&mut app, "Hello", None).await[0].1["id"]
        .as_i64()
        .unwrap();
    let events = send(&mut app, "And another thing", Some(id)).await;
    assert_eq!(events[0].1["id"], id);
    let contents: Vec<Value> = history(&chats(&api)[1])
        .iter()
        .map(|m| m["content"].clone())
        .collect();
    assert_eq!(
        contents,
        [
            json!("Hello"),
            json!("First answer."),
            json!("And another thing")
        ]
    );
    assert_eq!(
        app.get("/api/chat/conversations")
            .await
            .body
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn write_actions_are_refused_in_read_only_chat() {
    let (mut app, api, _) = with_model(vec![
        reply(
            "",
            &[
                (
                    "create_category",
                    json!({"name": "Gym", "parent": "Expenses"}),
                ),
                ("no_such_tool", json!({})),
            ],
        ),
        reply("I can't make changes yet.", &[]),
    ])
    .await;
    account(&mut app, "Expenses").await;
    let events = send(&mut app, "Create a Gym category", None).await;
    assert!(has(
        &events,
        "tool_result",
        json!({"name": "create_category", "ok": false})
    ));
    assert!(has(
        &events,
        "tool_result",
        json!({"name": "no_such_tool", "ok": false})
    ));
    let results = history(&chats(&api)[1]);
    let refused = results[2]["content"].as_str().unwrap();
    assert!(refused.contains("'organise' permission"), "{refused}");
    assert!(
        results[3]["content"]
            .as_str()
            .unwrap()
            .contains("no action called")
    );
    let categories = app
        .post("/api/actions/list_categories", json!({}))
        .await
        .body;
    assert!(!categories.to_string().contains("Expenses:Gym"));
}

#[tokio::test]
async fn the_last_round_must_answer_with_what_it_has() {
    let mut replies = vec![lookup(); MAX_ROUNDS - 1];
    replies.push(reply("Cancel the gym: £22.99 a month.", &[]));
    let (mut app, api, _) = with_model(replies).await;
    let events = send(&mut app, "What's my quickest win for saving money?", None).await;
    let requests = chats(&api);
    assert_eq!(requests.len(), MAX_ROUNDS);
    let system = |r: &Value| r["messages"][0]["content"].as_str().unwrap().to_owned();
    let before = &requests[MAX_ROUNDS - 2];
    assert!(before.get("tool_choice").is_none());
    assert!(!system(before).contains(LAST_ROUND));
    let last = &requests[MAX_ROUNDS - 1];
    assert_eq!(last["tool_choice"], "none");
    assert!(!offered(last).is_empty());
    assert!(system(last).ends_with(LAST_ROUND));
    assert_eq!(
        events[events.len() - 2],
        (
            "message".into(),
            json!({"content": "Cancel the gym: £22.99 a month."})
        )
    );
}

#[tokio::test]
async fn tool_calls_on_the_last_round_are_dropped_but_the_answer_kept() {
    // As from Ollama, which can't be stopped from calling tools.
    let (mut app, _, script) = with_model(vec![lookup(); MAX_ROUNDS - 1]).await;
    *script.default.lock().unwrap() = reply(
        "Here's what I found so far.",
        &[("list_categories", json!({}))],
    );
    let events = send(&mut app, "Loop forever", None).await;
    assert_eq!(
        events[events.len() - 2],
        (
            "message".into(),
            json!({"content": "Here's what I found so far."})
        )
    );
    let conversation = events[0].1.clone();
    let shown = turns(&mut app, &conversation).await;
    assert_eq!(
        shown.last().unwrap()["tools"].as_array().unwrap().len(),
        MAX_ROUNDS - 1
    );

    // The stored history has no unanswered tool calls, so the conversation can carry on.
    script
        .replies
        .lock()
        .unwrap()
        .push_back(reply("Sure.", &[]));
    let id = conversation["id"].as_i64();
    let events = send(&mut app, "Thanks", id).await;
    assert_eq!(
        events[events.len() - 2],
        ("message".into(), json!({"content": "Sure."}))
    );
}

#[tokio::test]
async fn chat_gives_up_if_the_last_round_writes_nothing() {
    let (mut app, api, _) = with_model(vec![]).await;
    let events = send(&mut app, "Loop forever", None).await;
    assert_eq!(chats(&api).len(), MAX_ROUNDS);
    let (event, data) = &events[events.len() - 2];
    assert_eq!(event, "message");
    assert!(
        data["content"]
            .as_str()
            .unwrap()
            .contains("reasonable number of steps")
    );
    let shown = turns(&mut app, &events[0].1).await;
    let last = shown.last().unwrap();
    assert_eq!(last["role"], "assistant");
    assert_eq!(last["tools"].as_array().unwrap().len(), MAX_ROUNDS - 1);
}

#[tokio::test]
async fn provider_errors_are_reported_and_logged() {
    let failure = (401, json!({"error": {"message": "bad key"}}).to_string());
    let (mut app, _, _) = with_model(vec![failure]).await;
    let events = send(&mut app, "Hi", None).await;
    let (event, data) = &events[events.len() - 2];
    assert_eq!(event, "error");
    let detail = data["detail"].as_str().unwrap();
    assert!(detail.contains("rejected the credentials"), "{detail}");
    assert_eq!(events.last().unwrap().0, "done");
    assert_eq!(logs(&mut app).await[0]["error"], detail);
}

#[tokio::test]
async fn chat_needs_a_configured_model() {
    let mut app = TestApp::with_fake_api(FakeApi::new()).await;
    let events = send(&mut app, "Hi", None).await;
    assert_eq!(events[1].0, "error");
    let detail = events[1].1["detail"].as_str().unwrap();
    assert!(detail.contains("Settings > AI"), "{detail}");
}

#[tokio::test]
async fn conversations_list_titles_and_delete() {
    let (mut app, _, _) = with_model(vec![reply("ok", &[]), reply("ok", &[])]).await;
    let long = format!("Please summarise {}", "all of my spending ".repeat(10));
    let first = send(&mut app, &long, None).await[0].1.clone();
    let title = first["title"].as_str().unwrap();
    assert!(title.chars().count() <= TITLE_LENGTH && title.ends_with('…'));
    let second = send(&mut app, "Budgets?", None).await[0].1.clone();
    let listed: Vec<Value> = app
        .get("/api/chat/conversations")
        .await
        .body
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["id"].clone())
        .collect();
    assert_eq!(listed, [second["id"].clone(), first["id"].clone()]);

    let path = format!("/api/chat/conversations/{}", first["id"]);
    app.delete(&path)
        .await
        .assert_status(StatusCode::NO_CONTENT);
    app.get(&path).await.assert_status(StatusCode::NOT_FOUND);
    app.post(
        "/api/chat/messages",
        json!({"message": "x", "conversation_id": first["id"]}),
    )
    .await
    .assert_status(StatusCode::NOT_FOUND);
    app.post("/api/chat/messages", json!({"message": ""}))
        .await
        .assert_status(StatusCode::UNPROCESSABLE_ENTITY);
    app.post("/api/chat/messages", json!({"message": "   "}))
        .await
        .assert_status(StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn chat_requires_a_session() {
    let mut app = TestApp::new().await;
    app.post(
        "/api/auth/setup",
        json!({"password": "correct horse battery"}),
    )
    .await;
    app.post("/api/auth/logout", json!({})).await;
    app.get("/api/chat/conversations")
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
    app.post("/api/chat/messages", json!({"message": "hi"}))
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}

// Checking actions

/// Chat allowed to organise, with a model that creates a Gym category unless told otherwise,
/// and Jev checking actions (answering `probability`, or failing with `status`).
async fn organising(
    replies: Vec<(u16, String)>,
    jev: Option<(u16, f64)>,
) -> (TestApp, Arc<FakeApi>) {
    let replies = if replies.is_empty() {
        vec![
            reply(
                "",
                &[(
                    "create_category",
                    json!({"name": "Gym", "parent": "Expenses"}),
                )],
            ),
            reply("Done.", &[]),
        ]
    } else {
        replies
    };
    let (mut app, api, _) = with_model(replies).await;
    account(&mut app, "Expenses").await;
    put(
        &mut app,
        "/api/ai/chat/scopes",
        json!({"scopes": ["organise"]}),
    )
    .await;
    let (status, probability) = jev.unwrap_or((200, 1.0));
    api.respond("POST", JEV, move |body| {
        if status != 200 {
            return (status, json!({"error": "overloaded"}));
        }
        let mut answers = serde_json::Map::new();
        for key in body["questions"].as_object().unwrap().keys() {
            answers.insert(key.clone(), json!({"type": "noul", "noul": probability}));
        }
        (200, json!({"model": "jev-1", "answers": answers}))
    });
    if jev.is_some() {
        put(
            &mut app,
            "/api/ai/providers/typesafe",
            json!({"values": {"api_key": "ts-1"}}),
        )
        .await;
        put(
            &mut app,
            "/api/ai/models/decision",
            json!({"provider": "typesafe", "model": "jev-latest"}),
        )
        .await;
        put(
            &mut app,
            "/api/ai/tasks/check_actions",
            json!({"enabled": true, "kind": "decision"}),
        )
        .await;
    }
    (app, api)
}

async fn category_paths(app: &mut TestApp) -> Vec<String> {
    app.post("/api/actions/list_categories", json!({}))
        .await
        .body["categories"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["path"].as_str().unwrap().to_owned())
        .collect()
}

#[tokio::test]
async fn changes_the_user_asked_for_go_ahead() {
    let (mut app, api) = organising(vec![], Some((200, 0.97))).await;
    let events = send(&mut app, "Add a Gym category", None).await;
    assert!(has(
        &events,
        "tool_result",
        json!({"name": "create_category", "ok": true})
    ));
    assert!(
        category_paths(&mut app)
            .await
            .contains(&"Expenses:Gym".into())
    );
    let change = events.iter().find(|(e, _)| e == "change").unwrap();
    let shown = turns(&mut app, &events[0].1).await;
    assert_eq!(
        shown.last().unwrap()["changes"],
        json!([{"id": change.1["id"], "summary": change.1["summary"], "undone": false}])
    );

    let requests = api.bodies(JEV);
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert_eq!(request["model"], "jev-latest");
    assert_eq!(request["questions"]["asked"]["type"], "noul");
    assert_eq!(
        request["state"]["conversation"],
        json!([{"role": "user", "content": "Add a Gym category"}])
    );
    let proposed = &request["state"]["proposed_action"];
    assert_eq!(proposed["name"], "create_category");
    assert_eq!(
        proposed["arguments"],
        json!({"name": "Gym", "parent": "Expenses"})
    );
    assert!(
        logs(&mut app)
            .await
            .iter()
            .any(|l| l["operation"] == "check_action")
    );
}

#[tokio::test]
async fn unlikely_changes_are_held_back_and_the_model_asks() {
    let (mut app, api) = organising(vec![], Some((200, 0.1))).await;
    let events = send(&mut app, "How much did I spend on food?", None).await;
    assert!(has(
        &events,
        "tool_result",
        json!({"name": "create_category", "ok": false, "held": true})
    ));
    assert!(!names(&events).contains(&"change"));
    assert!(
        !category_paths(&mut app)
            .await
            .contains(&"Expenses:Gym".into())
    );
    assert_eq!(app.get("/api/changes").await.body["changes"], json!([]));

    let result = history(&chats(&api)[1]).last().unwrap().clone();
    let content = result["content"].as_str().unwrap();
    assert!(content.contains("Don't try it again unless they confirm"));
    let shown = turns(&mut app, &events[0].1).await;
    assert_eq!(
        shown.last().unwrap()["tools"],
        json!([{"name": "create_category", "ok": false, "held": true}])
    );
}

#[tokio::test]
async fn read_actions_are_never_checked() {
    let replies = vec![lookup(), reply("You have some.", &[])];
    let (mut app, api) = organising(replies, Some((200, 0.0))).await;
    send(&mut app, "What categories do I have?", None).await;
    assert!(api.bodies(JEV).is_empty());
}

#[tokio::test]
async fn actions_the_chat_cannot_run_are_not_checked() {
    let replies = vec![
        reply("", &[("add_transaction", json!({"description": "x"}))]),
        reply("I can't.", &[]),
    ];
    let (mut app, api) = organising(replies, Some((200, 0.0))).await;
    send(&mut app, "Add a transaction", None).await;
    assert!(api.bodies(JEV).is_empty());
}

#[tokio::test]
async fn nothing_is_checked_unless_turned_on() {
    let (mut app, api) = organising(vec![], None).await;
    send(&mut app, "Add a Gym category", None).await;
    assert!(api.bodies(JEV).is_empty());
    assert!(
        category_paths(&mut app)
            .await
            .contains(&"Expenses:Gym".into())
    );
}

#[tokio::test]
async fn a_failing_check_lets_the_change_through() {
    let (mut app, _) = organising(vec![], Some((503, 0.0))).await;
    let events = send(&mut app, "Add a Gym category", None).await;
    assert!(has(
        &events,
        "tool_result",
        json!({"name": "create_category", "ok": true})
    ));
    assert!(
        logs(&mut app)
            .await
            .iter()
            .any(|l| l["operation"] == "check_action" && l["error"].is_string())
    );
}

#[tokio::test]
async fn language_models_give_a_probability() {
    let replies = vec![
        reply(
            "",
            &[(
                "create_rule",
                json!({"pattern": "DELIVEROO", "category": "Expenses"}),
            )],
        ),
        reply("Done.", &[]),
    ];
    let (mut app, api) = organising(replies, None).await;
    put(
        &mut app,
        "/api/ai/tasks/check_actions",
        json!({"enabled": true, "kind": "llm"}),
    )
    .await;
    let events = send(&mut app, "Make a rule for Deliveroo", None).await;
    assert!(
        has(
            &events,
            "tool_result",
            json!({"name": "create_rule", "ok": true})
        ),
        "{events:?}"
    );
    let checks: Vec<Value> = api
        .bodies(OPENAI)
        .into_iter()
        .filter(|b| b["stream"] != true)
        .collect();
    assert_eq!(checks.len(), 1);
    let prompt = checks[0]["messages"].to_string();
    assert!(prompt.contains("Make a rule for Deliveroo"));
    assert!(prompt.contains("DELIVEROO"));
    assert!(!prompt.contains(r#"\"role\": \"tool\""#));
}
