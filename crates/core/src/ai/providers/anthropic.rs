//! Claude, through Anthropic's Messages API over plain HTTP (there's no official Rust SDK).

use serde_json::{Map, Value, json};

use super::{
    Call, ChatReply, ChatRequest, JsonRequest, LanguageModel, OnText, ProviderConfig,
    ProviderError, ToolCall, Turn, parse_json, setting,
};
use crate::ai::config::ModelInfo;
use crate::ai::http::{Request, Transport, TransportError};

pub const LABEL: &str = "Anthropic (Claude)";
const BASE_URL: &str = "https://api.anthropic.com/v1";
const VERSION: &str = "2023-06-01";

pub struct Anthropic;

fn authorised(request: Request, cfg: &ProviderConfig) -> Request {
    request
        .header("x-api-key", setting(cfg, "api_key"))
        .header("anthropic-version", VERSION)
}

/// The API's own message for an error, from its body.
fn message(body: &[u8]) -> String {
    serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|b| b["error"]["message"].as_str().map(str::to_owned))
        .unwrap_or_else(|| String::from_utf8_lossy(body).chars().take(200).collect())
}

/// An error response in the user's terms.
fn status_error(status: u16, body: &[u8]) -> ProviderError {
    match status {
        401 => {
            ProviderError::credentials("Anthropic rejected the API key. Check it in Settings > AI.")
        }
        403 => {
            ProviderError::credentials("That Anthropic API key isn't allowed to use this model.")
        }
        404 => ProviderError::new("Anthropic couldn't find that model. Refresh the model list."),
        429 => ProviderError::new("Anthropic is rate limiting requests. Try again shortly."),
        400 | 413 | 422 => {
            ProviderError::new(format!("Anthropic rejected the request: {}", message(body)))
        }
        status => ProviderError::new(format!(
            "Anthropic returned an error ({status}). Try again shortly."
        )),
    }
}

fn unreachable(_: TransportError) -> ProviderError {
    ProviderError::new("Couldn't connect to Anthropic.")
}

async fn send(http: &dyn Transport, request: Request) -> Result<Value, ProviderError> {
    let response = http.send(request).await.map_err(unreachable)?;
    let status = response.status;
    let body = response.bytes().await.map_err(unreachable)?;
    if status >= 400 {
        return Err(status_error(status, &body));
    }
    serde_json::from_slice(&body).map_err(|_| unreachable(TransportError::Connect))
}

/// Problems the stop reason reports, with advice that fits the task.
fn check_stop(reason: Option<&str>, advice: &str) -> Result<(), ProviderError> {
    match reason {
        Some("refusal") => Err(ProviderError::new("Claude declined this request.")),
        Some("max_tokens") => Err(ProviderError::new(format!(
            "Claude's answer was cut off. {advice}"
        ))),
        _ => Ok(()),
    }
}

/// The conversation as the Messages API takes it.
fn messages(history: &[Turn]) -> Vec<Value> {
    let mut messages: Vec<Value> = Vec::new();
    for turn in history {
        if let Some(raw) = turn.raw_from("anthropic") {
            messages.push(raw.clone());
            continue;
        }
        match turn {
            Turn::User { content } => messages.push(json!({"role": "user", "content": content})),
            Turn::Assistant {
                content,
                tool_calls,
                ..
            } => {
                let mut blocks = Vec::new();
                if !content.is_empty() {
                    blocks.push(json!({"type": "text", "text": content}));
                }
                blocks.extend(tool_calls.iter().map(|c| {
                    json!({"type": "tool_use", "id": c.id, "name": c.name, "input": c.arguments})
                }));
                if !blocks.is_empty() {
                    messages.push(json!({"role": "assistant", "content": blocks}));
                }
            }
            Turn::Tool {
                tool_call_id,
                content,
                is_error,
                ..
            } => {
                let block = json!({
                    "type": "tool_result",
                    "tool_use_id": tool_call_id,
                    "content": content,
                    "is_error": is_error,
                });
                // All results for one assistant turn go back in a single user message.
                let joins = messages.last().is_some_and(|last| {
                    last["role"] == "user"
                        && last["content"]
                            .as_array()
                            .and_then(|c| c.last())
                            .is_some_and(|b| b["type"] == "tool_result")
                });
                if joins {
                    let last = messages.last_mut().expect("checked above");
                    last["content"]
                        .as_array_mut()
                        .expect("checked above")
                        .push(block);
                } else {
                    messages.push(json!({"role": "user", "content": [block]}));
                }
            }
        }
    }
    messages
}

/// A content block as it builds up from streamed events.
struct Block {
    value: Map<String, Value>,
    /// A tool call's input so far, as JSON text.
    partial_json: String,
}

impl Anthropic {
    pub async fn list_models(
        &self,
        http: &dyn Transport,
        cfg: &ProviderConfig,
    ) -> Result<Vec<ModelInfo>, ProviderError> {
        let mut models = Vec::new();
        let mut after: Option<String> = None;
        loop {
            let mut url = format!("{BASE_URL}/models?limit=1000");
            if let Some(id) = &after {
                url.push_str(&format!("&after_id={id}"));
            }
            let page = send(http, authorised(Request::get(url), cfg)).await?;
            for model in page["data"]
                .as_array()
                .map(Vec::as_slice)
                .unwrap_or_default()
            {
                // Categorisation and rule suggestions rely on structured outputs.
                if model["capabilities"]["structured_outputs"]["supported"] == json!(false) {
                    continue;
                }
                let Some(id) = model["id"].as_str() else {
                    continue;
                };
                let label = model["display_name"].as_str().filter(|l| !l.is_empty());
                models.push(ModelInfo {
                    id: id.to_owned(),
                    label: label.unwrap_or(id).to_owned(),
                });
            }
            match (page["has_more"].as_bool(), page["last_id"].as_str()) {
                (Some(true), Some(last)) => after = Some(last.to_owned()),
                _ => return Ok(models),
            }
        }
    }
}

impl LanguageModel for Anthropic {
    async fn complete_json(
        &self,
        http: &dyn Transport,
        cfg: &ProviderConfig,
        ask: JsonRequest<'_>,
    ) -> Result<Call, ProviderError> {
        let JsonRequest {
            model,
            system,
            prompt,
            schema,
            name: _,
        } = ask;
        let request = json!({
            "model": model,
            "max_tokens": 16000,
            "system": system,
            "messages": [{"role": "user", "content": prompt}],
            "output_config": {"format": {"type": "json_schema", "schema": schema}},
        });
        let http_request = authorised(
            Request::post(format!("{BASE_URL}/messages"), request.clone()),
            cfg,
        )
        .timeout(600);
        let response = send(http, http_request).await?;
        check_stop(
            response["stop_reason"].as_str(),
            "Try categorising fewer transactions.",
        )?;
        let text = response["content"]
            .as_array()
            .and_then(|blocks| blocks.iter().find(|b| b["type"] == "text"))
            .and_then(|b| b["text"].as_str());
        let data = parse_json(LABEL, text)?;
        Ok(Call {
            data,
            request,
            response,
        })
    }

    async fn chat_turn(
        &self,
        http: &dyn Transport,
        cfg: &ProviderConfig,
        chat: ChatRequest<'_>,
        on_text: OnText<'_>,
    ) -> Result<ChatReply, ProviderError> {
        let tools: Vec<Value> = chat
            .tools
            .iter()
            .map(|t| json!({"name": t.name, "description": t.description, "input_schema": t.input_schema}))
            .collect();
        let mut request = json!({
            "model": chat.model,
            "max_tokens": 64000,
            "system": chat.system,
            "messages": messages(chat.history),
            "tools": tools,
            // Caches the tools, system prompt and history so each round only pays for new input.
            "cache_control": {"type": "ephemeral"},
        });
        if chat.answer_only {
            request["tool_choice"] = json!({"type": "none"});
        }
        let mut body = request.clone();
        body["stream"] = json!(true);
        let http_request =
            authorised(Request::post(format!("{BASE_URL}/messages"), body), cfg).timeout(300);
        let response = http.send(http_request).await.map_err(unreachable)?;
        if response.status >= 400 {
            let status = response.status;
            let body = response.bytes().await.map_err(unreachable)?;
            return Err(status_error(status, &body));
        }

        let mut lines = response.lines();
        let mut message = Map::new();
        let mut blocks: Vec<Block> = Vec::new();
        while let Some(line) = lines.next().await.map_err(unreachable)? {
            let Some(data) = line.strip_prefix("data:") else {
                continue;
            };
            let Ok(event) = serde_json::from_str::<Value>(data.trim()) else {
                return Err(ProviderError::new(format!(
                    "{LABEL} sent a malformed streaming response."
                )));
            };
            match event["type"].as_str() {
                Some("message_start") => {
                    if let Some(start) = event["message"].as_object() {
                        message = start.clone();
                    }
                }
                Some("content_block_start") => {
                    let mut value = event["content_block"]
                        .as_object()
                        .cloned()
                        .unwrap_or_default();
                    value.retain(|_, v| !v.is_null());
                    blocks.push(Block {
                        value,
                        partial_json: String::new(),
                    });
                }
                Some("content_block_delta") => {
                    let Some(block) = blocks.last_mut() else {
                        continue;
                    };
                    let delta = &event["delta"];
                    let append = |block: &mut Block, key: &str, more: &str| {
                        let so_far = block
                            .value
                            .get(key)
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        let joined = format!("{so_far}{more}");
                        block.value.insert(key.to_owned(), json!(joined));
                    };
                    match delta["type"].as_str() {
                        Some("text_delta") => {
                            let text = delta["text"].as_str().unwrap_or_default();
                            append(block, "text", text);
                            on_text(text);
                        }
                        Some("input_json_delta") => {
                            block
                                .partial_json
                                .push_str(delta["partial_json"].as_str().unwrap_or_default());
                        }
                        Some("thinking_delta") => {
                            append(
                                block,
                                "thinking",
                                delta["thinking"].as_str().unwrap_or_default(),
                            );
                        }
                        Some("signature_delta") => {
                            block
                                .value
                                .insert("signature".into(), delta["signature"].clone());
                        }
                        Some("citations_delta") => {
                            let citations =
                                block.value.entry("citations").or_insert_with(|| json!([]));
                            if let Some(list) = citations.as_array_mut() {
                                list.push(delta["citation"].clone());
                            }
                        }
                        _ => {}
                    }
                }
                Some("content_block_stop") => {
                    if let Some(block) = blocks.last_mut()
                        && !block.partial_json.is_empty()
                    {
                        let input = serde_json::from_str::<Value>(&block.partial_json).map_err(|_| {
                            ProviderError::new(format!(
                                "{LABEL} asked to run a tool with arguments that weren't valid JSON."
                            ))
                        })?;
                        block.value.insert("input".into(), input);
                    }
                }
                Some("message_delta") => {
                    if let Some(delta) = event["delta"].as_object() {
                        for (key, value) in delta {
                            message.insert(key.clone(), value.clone());
                        }
                    }
                    if let Some(usage) = event["usage"].as_object() {
                        let merged = message.entry("usage").or_insert_with(|| json!({}));
                        if let Some(merged) = merged.as_object_mut() {
                            merged.extend(usage.clone());
                        }
                    }
                }
                Some("error") => {
                    let kind = event["error"]["type"].as_str().unwrap_or_default();
                    return Err(match kind {
                        "overloaded_error" | "rate_limit_error" => ProviderError::new(
                            "Anthropic is rate limiting requests. Try again shortly.",
                        ),
                        _ => ProviderError::new(format!(
                            "Anthropic returned an error ({}). Try again shortly.",
                            event["error"]["message"].as_str().unwrap_or(kind)
                        )),
                    });
                }
                _ => {}
            }
        }

        check_stop(
            message.get("stop_reason").and_then(Value::as_str),
            "Try a narrower question.",
        )?;
        let content: Vec<Value> = blocks.into_iter().map(|b| Value::Object(b.value)).collect();
        let text: String = content
            .iter()
            .filter(|b| b["type"] == "text")
            .filter_map(|b| b["text"].as_str())
            .collect();
        let tool_calls = content
            .iter()
            .filter(|b| b["type"] == "tool_use")
            .map(|b| ToolCall {
                id: b["id"].as_str().unwrap_or_default().to_owned(),
                name: b["name"].as_str().unwrap_or_default().to_owned(),
                arguments: b["input"].as_object().cloned().unwrap_or_default(),
            })
            .collect();
        message.insert("content".into(), json!(content));
        Ok(ChatReply {
            text,
            tool_calls,
            raw: json!({"role": "assistant", "content": content}),
            request,
            response: Value::Object(message),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::http::FakeApi;
    use crate::ai::providers::Tool;

    fn cfg() -> ProviderConfig {
        ProviderConfig::from([("api_key".into(), "sk-ant-1".into())])
    }

    #[tokio::test]
    async fn lists_models_with_structured_outputs_across_pages() {
        let api = FakeApi::new();
        api.on(
            "GET",
            "https://api.anthropic.com/v1/models?limit=1000",
            200,
            json!({"data": [
                {"id": "claude-opus-5-5", "display_name": "Claude Opus 5.5",
                 "capabilities": {"structured_outputs": {"supported": true}}},
                {"id": "claude-old", "display_name": "Claude Old",
                 "capabilities": {"structured_outputs": {"supported": false}}},
            ], "has_more": true, "last_id": "claude-old"}),
        );
        api.on(
            "GET",
            "https://api.anthropic.com/v1/models?limit=1000&after_id=claude-old",
            200,
            json!({"data": [{"id": "claude-haiku-5-5", "display_name": ""}], "has_more": false}),
        );
        let models = Anthropic.list_models(api.as_ref(), &cfg()).await.unwrap();
        let ids: Vec<(&str, &str)> = models
            .iter()
            .map(|m| (m.id.as_str(), m.label.as_str()))
            .collect();
        assert_eq!(
            ids,
            [
                ("claude-opus-5-5", "Claude Opus 5.5"),
                ("claude-haiku-5-5", "claude-haiku-5-5")
            ]
        );
        let request = &api.requests()[0];
        assert!(
            request
                .headers
                .contains(&("x-api-key".into(), "sk-ant-1".into()))
        );
        assert!(
            request
                .headers
                .contains(&("anthropic-version".into(), "2023-06-01".into()))
        );
    }

    #[tokio::test]
    async fn errors_are_explained() {
        let api = FakeApi::new();
        let url = "https://api.anthropic.com/v1/models?limit=1000";
        for (status, credentials, message) in [
            (
                401,
                true,
                "Anthropic rejected the API key. Check it in Settings > AI.",
            ),
            (
                429,
                false,
                "Anthropic is rate limiting requests. Try again shortly.",
            ),
            (
                529,
                false,
                "Anthropic returned an error (529). Try again shortly.",
            ),
        ] {
            api.on(
                "GET",
                url,
                status,
                json!({"type": "error", "error": {"message": "nope"}}),
            );
            let error = Anthropic
                .list_models(api.as_ref(), &cfg())
                .await
                .unwrap_err();
            assert_eq!(
                (error.credentials, error.message.as_str()),
                (credentials, message)
            );
        }
    }

    #[tokio::test]
    async fn structured_output_and_refusals() {
        let api = FakeApi::new();
        let url = "https://api.anthropic.com/v1/messages";
        api.on(
            "POST",
            url,
            200,
            json!({"content": [{"type": "text", "text": "{\"a\": 1}"}], "stop_reason": "end_turn"}),
        );
        let schema = json!({"type": "object"});
        let call = Anthropic
            .complete_json(
                api.as_ref(),
                &cfg(),
                JsonRequest {
                    model: "claude-opus-5-5",
                    system: "sys",
                    prompt: "hi",
                    schema: &schema,
                    name: "x",
                },
            )
            .await
            .unwrap();
        assert_eq!(Value::Object(call.data), json!({"a": 1}));
        assert_eq!(
            api.last_body()["output_config"],
            json!({"format": {"type": "json_schema", "schema": {"type": "object"}}})
        );

        api.on(
            "POST",
            url,
            200,
            json!({"content": [], "stop_reason": "refusal"}),
        );
        let error = Anthropic
            .complete_json(
                api.as_ref(),
                &cfg(),
                JsonRequest {
                    model: "claude-opus-5-5",
                    system: "sys",
                    prompt: "hi",
                    schema: &schema,
                    name: "x",
                },
            )
            .await
            .unwrap_err();
        assert_eq!(error.message, "Claude declined this request.");
    }

    #[tokio::test]
    async fn chat_streams_text_thinking_and_tool_calls() {
        let api = FakeApi::new();
        let events = [
            r#"{"type":"message_start","message":{"id":"msg_1","type":"message","role":"assistant","content":[],"model":"claude-opus-5-5","stop_reason":null,"usage":{"input_tokens":10}}}"#,
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"","signature":""}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"sig"}}"#,
            r#"{"type":"content_block_stop","index":0}"#,
            r#"{"type":"content_block_start","index":1,"content_block":{"type":"text","text":"","citations":null}}"#,
            r#"{"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"Checking "}}"#,
            r#"{"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"now."}}"#,
            r#"{"type":"content_block_stop","index":1}"#,
            r#"{"type":"content_block_start","index":2,"content_block":{"type":"tool_use","id":"toolu_1","name":"list_transactions","input":{}}}"#,
            r#"{"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"{\"search\": "}}"#,
            r#"{"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"\"tesco\"}"}}"#,
            r#"{"type":"content_block_stop","index":2}"#,
            r#"{"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":20}}"#,
            r#"{"type":"message_stop"}"#,
        ];
        let stream: String = events
            .iter()
            .map(|e| format!("event: x\ndata: {e}\n\n"))
            .collect();
        api.on_raw("POST", "https://api.anthropic.com/v1/messages", 200, stream);

        let history = vec![
            Turn::User {
                content: "Tesco?".into(),
            },
            Turn::Assistant {
                content: "Let me look.".into(),
                tool_calls: vec![
                    ToolCall {
                        id: "a".into(),
                        name: "x".into(),
                        arguments: Map::new(),
                    },
                    ToolCall {
                        id: "b".into(),
                        name: "y".into(),
                        arguments: Map::new(),
                    },
                ],
                raw: None,
                raw_provider: None,
            },
            Turn::Tool {
                tool_call_id: "a".into(),
                name: "x".into(),
                content: "1".into(),
                is_error: false,
            },
            Turn::Tool {
                tool_call_id: "b".into(),
                name: "y".into(),
                content: "oops".into(),
                is_error: true,
            },
        ];
        let tools = vec![Tool {
            name: "list_transactions".into(),
            description: "Find".into(),
            input_schema: json!({"type": "object"}),
        }];
        let mut streamed = String::new();
        let reply = Anthropic
            .chat_turn(
                api.as_ref(),
                &cfg(),
                ChatRequest {
                    model: "claude-opus-5-5",
                    system: "Be brief",
                    history: &history,
                    tools: &tools,
                    answer_only: false,
                },
                &mut |t: &str| streamed.push_str(t),
            )
            .await
            .unwrap();
        assert_eq!(streamed, "Checking now.");
        assert_eq!(reply.text, "Checking now.");
        assert_eq!(reply.tool_calls[0].name, "list_transactions");
        assert_eq!(reply.tool_calls[0].arguments["search"], "tesco");
        assert_eq!(
            reply.raw["content"],
            json!([
                {"type": "thinking", "thinking": "", "signature": "sig"},
                {"type": "text", "text": "Checking now."},
                {"type": "tool_use", "id": "toolu_1", "name": "list_transactions", "input": {"search": "tesco"}},
            ])
        );
        assert_eq!(reply.response["stop_reason"], "tool_use");
        assert_eq!(
            reply.response["usage"],
            json!({"input_tokens": 10, "output_tokens": 20})
        );

        let sent = api.last_body();
        assert_eq!(sent["stream"], true);
        assert_eq!(sent["cache_control"], json!({"type": "ephemeral"}));
        // Both results go back in one user message.
        assert_eq!(sent["messages"].as_array().unwrap().len(), 3);
        assert_eq!(sent["messages"][2]["content"][1]["is_error"], true);
        assert!(reply.request.get("stream").is_none());
    }
}
