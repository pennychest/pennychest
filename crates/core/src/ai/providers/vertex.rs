//! Gemini on Google Cloud Vertex AI, with an express mode API key. Express mode keys only work
//! against Vertex's global endpoint, in Google's own request format (not the OpenAI-compatible
//! one the Gemini API provider uses).

use serde_json::{Map, Value, json};

use super::{
    Call, ChatReply, ChatRequest, JsonRequest, LanguageModel, OnText, ProviderConfig,
    ProviderError, ToolCall, Turn, open_stream, parse_arguments, parse_json, send, setting,
};
use crate::ai::config::ModelInfo;
use crate::ai::http::{Request, Transport};

pub const LABEL: &str = "Google Vertex AI";
const BASE_URL: &str = "https://aiplatform.googleapis.com/v1/publishers/google/models";
/// Express mode has no model listing; these are Google's current Gemini models.
const MODELS: [(&str, &str); 7] = [
    ("gemini-3.8-flash", "Gemini 3.8 Flash"),
    ("gemini-3.5-flash", "Gemini 3.5 Flash"),
    ("gemini-3.5-flash-lite", "Gemini 3.5 Flash-Lite"),
    ("gemini-3-pro-preview", "Gemini 3 Pro (preview)"),
    ("gemini-2.5-pro", "Gemini 2.5 Pro"),
    ("gemini-2.5-flash", "Gemini 2.5 Flash"),
    ("gemini-2.5-flash-lite", "Gemini 2.5 Flash-Lite"),
];

pub struct Vertex;

fn url(model: &str, method: &str) -> String {
    format!("{BASE_URL}/{model}:{method}")
}

fn authorised(request: Request, cfg: &ProviderConfig) -> Request {
    request.header("x-goog-api-key", setting(cfg, "api_key"))
}

/// The first candidate answer, unless the request was blocked.
fn check(body: &Value) -> Result<&Value, ProviderError> {
    if let Some(blocked) = body["promptFeedback"]["blockReason"].as_str() {
        return Err(ProviderError::new(format!(
            "{LABEL} declined this request ({blocked})."
        )));
    }
    body["candidates"]
        .as_array()
        .and_then(|c| c.first())
        .ok_or_else(|| ProviderError::new(format!("{LABEL} returned no answer.")))
}

fn finish(reason: Option<&str>) -> Result<(), ProviderError> {
    match reason {
        Some("MAX_TOKENS") => Err(ProviderError::new(format!(
            "{LABEL}'s answer was cut off. Try fewer transactions."
        ))),
        Some(reason @ ("SAFETY" | "RECITATION" | "PROHIBITED_CONTENT" | "BLOCKLIST" | "SPII")) => {
            Err(ProviderError::new(format!(
                "{LABEL} declined this request ({reason})."
            )))
        }
        _ => Ok(()),
    }
}

fn is_thought(part: &Value) -> bool {
    super::truthy(&part["thought"])
}

/// With the model added, as the AI logs record the request.
fn logged(model: &str, request: &Value) -> Value {
    let mut logged = Map::new();
    logged.insert("model".into(), json!(model));
    if let Some(fields) = request.as_object() {
        logged.extend(fields.clone());
    }
    Value::Object(logged)
}

impl Vertex {
    pub async fn list_models(
        &self,
        http: &dyn Transport,
        cfg: &ProviderConfig,
    ) -> Result<Vec<ModelInfo>, ProviderError> {
        // Nothing to list, so check the key with a free token count instead.
        let body = json!({"contents": [{"role": "user", "parts": [{"text": "hi"}]}]});
        let request = authorised(
            Request::post(url("gemini-2.5-flash", "countTokens"), body),
            cfg,
        )
        .timeout(30);
        send(http, LABEL, request).await?;
        Ok(MODELS
            .iter()
            .map(|(id, label)| ModelInfo {
                id: (*id).to_owned(),
                label: (*label).to_owned(),
            })
            .collect())
    }

    fn contents(history: &[Turn]) -> Vec<Value> {
        let mut contents: Vec<Value> = Vec::new();
        // Whether the last content holds tool results, so the next joins it.
        let mut results_open = false;
        for turn in history {
            if let Some(raw) = turn.raw_from("vertex") {
                // Replayed as returned, so Gemini's thought signatures survive.
                contents.push(raw.clone());
                results_open = false;
                continue;
            }
            match turn {
                Turn::User { content } => {
                    contents.push(json!({"role": "user", "parts": [{"text": content}]}));
                    results_open = false;
                }
                Turn::Assistant {
                    content,
                    tool_calls,
                    ..
                } => {
                    let mut parts = Vec::new();
                    if !content.is_empty() {
                        parts.push(json!({"text": content}));
                    }
                    parts.extend(
                        tool_calls.iter().map(
                            |c| json!({"functionCall": {"name": c.name, "args": c.arguments}}),
                        ),
                    );
                    if !parts.is_empty() {
                        contents.push(json!({"role": "model", "parts": parts}));
                        results_open = false;
                    }
                }
                Turn::Tool { name, content, .. } => {
                    let result =
                        serde_json::from_str::<Value>(content).unwrap_or_else(|_| json!(content));
                    let response = if result.is_object() {
                        result
                    } else {
                        json!({"result": result})
                    };
                    let part = json!({"functionResponse": {"name": name, "response": response}});
                    // Results for one turn's calls go back together in a single user message.
                    match contents.last_mut() {
                        Some(last) if results_open => {
                            last["parts"].as_array_mut().expect("parts").push(part);
                        }
                        _ => {
                            contents.push(json!({"role": "user", "parts": [part]}));
                            results_open = true;
                        }
                    }
                }
            }
        }
        contents
    }
}

impl LanguageModel for Vertex {
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
            "systemInstruction": {"parts": [{"text": system}]},
            "contents": [{"role": "user", "parts": [{"text": prompt}]}],
            "generationConfig": {
                "responseMimeType": "application/json",
                "responseJsonSchema": schema,
            },
        });
        let http_request = authorised(
            Request::post(url(model, "generateContent"), request.clone()),
            cfg,
        )
        .timeout(300);
        let body = send(http, LABEL, http_request).await?;
        let candidate = check(&body)?;
        finish(candidate["finishReason"].as_str())?;
        let text: String = candidate["content"]["parts"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .filter(|p| !is_thought(p))
            .filter_map(|p| p["text"].as_str())
            .collect();
        let data = parse_json(LABEL, Some(&text))?;
        Ok(Call {
            data,
            request: logged(model, &request),
            response: body,
        })
    }

    async fn chat_turn(
        &self,
        http: &dyn Transport,
        cfg: &ProviderConfig,
        chat: ChatRequest<'_>,
        on_text: OnText<'_>,
    ) -> Result<ChatReply, ProviderError> {
        let mut request = json!({
            "systemInstruction": {"parts": [{"text": chat.system}]},
            "contents": Self::contents(chat.history),
        });
        if !chat.tools.is_empty() {
            let declarations: Vec<Value> = chat
                .tools
                .iter()
                .map(|t| {
                    json!({
                        "name": t.name,
                        "description": t.description,
                        "parametersJsonSchema": t.input_schema,
                    })
                })
                .collect();
            request["tools"] = json!([{"functionDeclarations": declarations}]);
            if chat.answer_only {
                request["toolConfig"] = json!({"functionCallingConfig": {"mode": "NONE"}});
            }
        }
        let address = format!("{}?alt=sse", url(chat.model, "streamGenerateContent"));
        let http_request = authorised(Request::post(address, request.clone()), cfg).timeout(300);
        let (mut lines, address) = open_stream(http, LABEL, http_request).await?;
        let (mut parts, mut chunks, mut reason) = (Vec::<Value>::new(), Vec::new(), None);
        while let Some(line) = lines
            .next()
            .await
            .map_err(|e| super::transport_problem(LABEL, &address, e))?
        {
            let Some(data) = line.strip_prefix("data:") else {
                continue;
            };
            let chunk: Value = serde_json::from_str(data.trim()).map_err(|_| {
                ProviderError::new(format!("{LABEL} sent a malformed streaming response."))
            })?;
            let candidate = check(&chunk)?;
            if let Some(r) = candidate["finishReason"].as_str() {
                reason = Some(r.to_owned());
            }
            for part in candidate["content"]["parts"]
                .as_array()
                .map(Vec::as_slice)
                .unwrap_or_default()
            {
                parts.push(part.clone());
                if let Some(text) = part["text"].as_str().filter(|t| !t.is_empty())
                    && !is_thought(part)
                {
                    on_text(text);
                }
            }
            chunks.push(chunk);
        }
        finish(reason.as_deref())?;
        let text: String = parts
            .iter()
            .filter(|p| !is_thought(p))
            .filter_map(|p| p["text"].as_str())
            .collect();
        let tool_calls = parts
            .iter()
            .filter(|p| p.get("functionCall").is_some())
            .enumerate()
            .map(|(i, p)| {
                let call = &p["functionCall"];
                Ok(ToolCall {
                    id: call["id"]
                        .as_str()
                        .filter(|id| !id.is_empty())
                        .map_or_else(|| format!("call_{i}"), str::to_owned),
                    name: call["name"].as_str().unwrap_or_default().to_owned(),
                    arguments: parse_arguments(LABEL, &call["args"])?,
                })
            })
            .collect::<Result<_, ProviderError>>()?;
        Ok(ChatReply {
            text,
            tool_calls,
            raw: json!({"role": "model", "parts": parts}),
            request: logged(chat.model, &request),
            response: json!({ "chunks": chunks }),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::http::FakeApi;
    use crate::ai::providers::Tool;

    fn cfg() -> ProviderConfig {
        ProviderConfig::from([("api_key".into(), "AQ.key".into())])
    }

    #[tokio::test]
    async fn the_key_is_checked_with_a_token_count() {
        let api = FakeApi::new();
        api.on(
            "POST",
            &url("gemini-2.5-flash", "countTokens"),
            200,
            json!({"totalTokens": 1}),
        );
        let models = Vertex.list_models(api.as_ref(), &cfg()).await.unwrap();
        assert_eq!(models[0].id, "gemini-3.8-flash");
        let request = &api.requests()[0];
        assert!(
            request
                .headers
                .contains(&("x-goog-api-key".into(), "AQ.key".into()))
        );

        api.on(
            "POST",
            &url("gemini-2.5-flash", "countTokens"),
            400,
            json!({"error": {"message": "API key not valid. Please pass a valid API key."}}),
        );
        let error = Vertex.list_models(api.as_ref(), &cfg()).await.unwrap_err();
        assert!(error.credentials);
    }

    #[tokio::test]
    async fn json_answers_skip_thoughts_and_report_blocks() {
        let api = FakeApi::new();
        let address = url("gemini-3.8-flash", "generateContent");
        api.on(
            "POST",
            &address,
            200,
            json!({"candidates": [{"content": {"parts": [
                {"text": "thinking...", "thought": true},
                {"text": "{\"answer\": 1}"},
            ]}, "finishReason": "STOP"}]}),
        );
        let call = Vertex
            .complete_json(
                api.as_ref(),
                &cfg(),
                JsonRequest {
                    model: "gemini-3.8-flash",
                    system: "sys",
                    prompt: "hi",
                    schema: &json!({}),
                    name: "x",
                },
            )
            .await
            .unwrap();
        assert_eq!(Value::Object(call.data), json!({"answer": 1}));
        assert_eq!(call.request["model"], "gemini-3.8-flash");
        assert_eq!(
            api.last_body()["generationConfig"]["responseMimeType"],
            "application/json"
        );

        api.on(
            "POST",
            &address,
            200,
            json!({"promptFeedback": {"blockReason": "SAFETY"}}),
        );
        let error = Vertex
            .complete_json(
                api.as_ref(),
                &cfg(),
                JsonRequest {
                    model: "gemini-3.8-flash",
                    system: "sys",
                    prompt: "hi",
                    schema: &json!({}),
                    name: "x",
                },
            )
            .await
            .unwrap_err();
        assert_eq!(
            error.message,
            "Google Vertex AI declined this request (SAFETY)."
        );
        api.on(
            "POST",
            &address,
            200,
            json!({"candidates": [{"content": {"parts": [{"text": "{"}]}, "finishReason": "MAX_TOKENS"}]}),
        );
        let error = Vertex
            .complete_json(
                api.as_ref(),
                &cfg(),
                JsonRequest {
                    model: "gemini-3.8-flash",
                    system: "sys",
                    prompt: "hi",
                    schema: &json!({}),
                    name: "x",
                },
            )
            .await
            .unwrap_err();
        assert!(error.message.contains("cut off"));
    }

    #[tokio::test]
    async fn chat_streams_text_and_tool_calls_and_replays_them() {
        let api = FakeApi::new();
        let address = format!(
            "{}?alt=sse",
            url("gemini-3.8-flash", "streamGenerateContent")
        );
        api.on_raw(
            "POST",
            &address,
            200,
            concat!(
                "data: {\"candidates\": [{\"content\": {\"parts\": [{\"text\": \"Let me \"}]}}]}\n\n",
                "data: {\"candidates\": [{\"content\": {\"parts\": [{\"text\": \"look.\"}, ",
                "{\"functionCall\": {\"name\": \"list_accounts\", \"args\": {}}, \"thoughtSignature\": \"sig\"}]}, ",
                "\"finishReason\": \"STOP\"}]}\n\n"
            ),
        );
        let history = vec![
            Turn::User {
                content: "What accounts?".into(),
            },
            Turn::Assistant {
                content: String::new(),
                tool_calls: vec![ToolCall {
                    id: "a".into(),
                    name: "list_rules".into(),
                    arguments: Map::new(),
                }],
                raw: None,
                raw_provider: None,
            },
            Turn::Tool {
                tool_call_id: "a".into(),
                name: "list_rules".into(),
                content: "[1]".into(),
                is_error: false,
            },
        ];
        let tools = vec![Tool {
            name: "list_accounts".into(),
            description: "List accounts".into(),
            input_schema: json!({"type": "object"}),
        }];
        let mut streamed = String::new();
        let reply = Vertex
            .chat_turn(
                api.as_ref(),
                &cfg(),
                ChatRequest {
                    model: "gemini-3.8-flash",
                    system: "Be brief",
                    history: &history,
                    tools: &tools,
                    answer_only: true,
                },
                &mut |t: &str| streamed.push_str(t),
            )
            .await
            .unwrap();
        assert_eq!(streamed, "Let me look.");
        assert_eq!(reply.text, "Let me look.");
        assert_eq!(reply.tool_calls[0].name, "list_accounts");
        assert_eq!(reply.tool_calls[0].id, "call_0");
        assert_eq!(reply.raw["parts"][2]["thoughtSignature"], "sig");

        let sent = api.last_body();
        assert_eq!(sent["toolConfig"]["functionCallingConfig"]["mode"], "NONE");
        assert_eq!(
            sent["contents"][2],
            json!({"role": "user", "parts": [{"functionResponse": {"name": "list_rules", "response": {"result": [1]}}}]})
        );
        assert_eq!(
            sent["tools"][0]["functionDeclarations"][0]["parametersJsonSchema"]["type"],
            "object"
        );
    }
}
