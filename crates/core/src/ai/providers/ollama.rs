//! Ollama, self-hosted.

use serde_json::{Value, json};

use super::{
    Call, ChatReply, ChatRequest, JsonRequest, LanguageModel, OnText, ProviderConfig,
    ProviderError, ToolCall, Turn, function_tools, open_stream, parse_arguments, parse_json, send,
    setting,
};
use crate::ai::config::ModelInfo;
use crate::ai::http::{Request, Transport};

pub const LABEL: &str = "Ollama (self-hosted)";

pub struct Ollama;

fn url(cfg: &ProviderConfig) -> &str {
    setting(cfg, "url").trim_end_matches('/')
}

impl Ollama {
    pub async fn list_models(
        &self,
        http: &dyn Transport,
        cfg: &ProviderConfig,
    ) -> Result<Vec<ModelInfo>, ProviderError> {
        let request = Request::get(format!("{}/api/tags", url(cfg))).timeout(30);
        let body = send(http, LABEL, request).await?;
        let mut names: Vec<String> = body["models"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .filter_map(|m| m["name"].as_str())
            .filter(|n| !n.is_empty())
            .map(str::to_owned)
            .collect();
        names.sort();
        names.dedup();
        Ok(names
            .into_iter()
            .map(|n| ModelInfo {
                label: n.clone(),
                id: n,
            })
            .collect())
    }

    fn messages(system: &str, history: &[Turn]) -> Vec<Value> {
        let mut messages = vec![json!({"role": "system", "content": system})];
        for turn in history {
            if let Some(raw) = turn.raw_from("ollama") {
                messages.push(raw.clone());
                continue;
            }
            match turn {
                Turn::User { content } => {
                    messages.push(json!({"role": "user", "content": content}))
                }
                Turn::Assistant {
                    content,
                    tool_calls,
                    ..
                } => {
                    let mut message = json!({"role": "assistant", "content": content});
                    if !tool_calls.is_empty() {
                        message["tool_calls"] = tool_calls
                            .iter()
                            .map(
                                |c| json!({"function": {"name": c.name, "arguments": c.arguments}}),
                            )
                            .collect();
                    }
                    messages.push(message);
                }
                Turn::Tool { name, content, .. } => {
                    messages.push(json!({"role": "tool", "tool_name": name, "content": content}));
                }
            }
        }
        messages
    }
}

impl LanguageModel for Ollama {
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
            "messages": [
                {"role": "system", "content": system},
                // Stops Qwen3-style models spending their budget on visible reasoning.
                {"role": "user", "content": format!("/no_think\n{prompt}")},
            ],
            "format": schema,
            "stream": false,
        });
        let http_request =
            Request::post(format!("{}/api/chat", url(cfg)), request.clone()).timeout(300);
        let body = send(http, LABEL, http_request).await?;
        let data = parse_json(LABEL, body["message"]["content"].as_str())?;
        Ok(Call {
            data,
            request,
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
        // Ollama has no way to switch tools off for one request, so answer_only relies on the
        // prompt and the chat loop discarding any calls it makes anyway.
        let request = json!({
            "model": chat.model,
            "messages": Self::messages(chat.system, chat.history),
            "tools": function_tools(chat.tools),
            "stream": true,
        });
        let http_request =
            Request::post(format!("{}/api/chat", url(cfg)), request.clone()).timeout(300);
        let (mut lines, url) = open_stream(http, LABEL, http_request).await?;
        let (mut text, mut tool_calls, mut chunks) = (String::new(), Vec::new(), Vec::new());
        while let Some(line) = lines
            .next()
            .await
            .map_err(|e| super::transport_problem(LABEL, &url, e))?
        {
            if line.trim().is_empty() {
                continue;
            }
            let chunk: Value = serde_json::from_str(&line).map_err(|_| {
                ProviderError::new(format!("{LABEL} sent a malformed streaming response."))
            })?;
            if let Some(error) = chunk.get("error").filter(|e| super::truthy(e)) {
                return Err(ProviderError::new(format!(
                    "{LABEL} returned an error: {}",
                    super::text_of(error)
                )));
            }
            let message = &chunk["message"];
            if let Some(content) = message["content"].as_str().filter(|c| !c.is_empty()) {
                text.push_str(content);
                on_text(content);
            }
            if let Some(calls) = message["tool_calls"].as_array() {
                tool_calls.extend(calls.iter().cloned());
            }
            chunks.push(chunk);
        }
        let mut raw = json!({"role": "assistant", "content": text});
        if !tool_calls.is_empty() {
            raw["tool_calls"] = json!(tool_calls);
        }
        // Ollama doesn't give tool calls ids; number them so results can be matched up.
        let calls = tool_calls
            .iter()
            .enumerate()
            .map(|(i, c)| {
                Ok(ToolCall {
                    id: format!("call_{i}"),
                    name: c["function"]["name"]
                        .as_str()
                        .unwrap_or_default()
                        .to_owned(),
                    arguments: parse_arguments(LABEL, &c["function"]["arguments"])?,
                })
            })
            .collect::<Result<_, ProviderError>>()?;
        Ok(ChatReply {
            text,
            tool_calls: calls,
            raw,
            request,
            response: json!({ "chunks": chunks }),
        })
    }
}
