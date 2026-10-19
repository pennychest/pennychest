//! OpenAI's chat completions API, which Google also serves for Gemini.

use std::collections::BTreeMap;

use regex::Regex;
use serde_json::{Map, Value, json};

use super::{
    Call, ChatReply, ChatRequest, Field, JsonRequest, LanguageModel, OnText, ProviderConfig,
    ProviderError, ToolCall, Turn, function_tools, open_stream, parse_arguments, parse_json, send,
    setting,
};
use crate::ai::config::ModelInfo;
use crate::ai::http::{Request, Transport};

pub struct OpenAiCompatible {
    pub id: &'static str,
    pub label: &'static str,
    default_base_url: &'static str,
    /// Which of the provider's own catalogue to offer.
    model_filter: fn(&str) -> bool,
    custom_base_url: bool,
    key_hint: &'static str,
}

fn is_openai_chat_model(id: &str) -> bool {
    let m = id.to_lowercase();
    let reasoning = Regex::new(r"^o\d").expect("a valid pattern");
    if !(m.starts_with("gpt-") || m.starts_with("chatgpt-") || reasoning.is_match(&m)) {
        return false;
    }
    let excluded = [
        "audio",
        "realtime",
        "tts",
        "transcribe",
        "image",
        "search",
        "embedding",
        "moderation",
        "instruct",
    ];
    !excluded.iter().any(|word| m.contains(word))
}

fn is_gemini_text_model(id: &str) -> bool {
    let m = id.to_lowercase();
    m.starts_with("gemini")
        && !["embedding", "image", "tts", "live", "audio"]
            .iter()
            .any(|word| m.contains(word))
}

impl OpenAiCompatible {
    pub fn openai() -> Self {
        Self {
            id: "openai",
            label: "OpenAI",
            default_base_url: "https://api.openai.com/v1",
            model_filter: is_openai_chat_model,
            custom_base_url: true,
            key_hint: "sk-...",
        }
    }

    pub fn google() -> Self {
        Self {
            id: "google",
            label: "Google (Gemini)",
            default_base_url: "https://generativelanguage.googleapis.com/v1beta/openai",
            model_filter: is_gemini_text_model,
            custom_base_url: false,
            key_hint: "",
        }
    }

    pub fn fields(&self) -> Vec<Field> {
        let mut fields = vec![Field::api_key(self.key_hint)];
        if self.custom_base_url {
            fields.push(Field {
                key: "base_url",
                label: "Base URL",
                secret: false,
                required: false,
                default: Some(self.default_base_url),
                placeholder: "https://api.openai.com/v1 (or any OpenAI-compatible API)",
            });
        }
        fields
    }

    fn base_url<'a>(&'a self, cfg: &'a ProviderConfig) -> &'a str {
        let custom = setting(cfg, "base_url");
        let url = if custom.is_empty() {
            self.default_base_url
        } else {
            custom
        };
        url.trim_end_matches('/')
    }

    fn authorised(&self, request: Request, cfg: &ProviderConfig) -> Request {
        request.header(
            "Authorization",
            format!("Bearer {}", setting(cfg, "api_key")),
        )
    }

    pub async fn list_models(
        &self,
        http: &dyn Transport,
        cfg: &ProviderConfig,
    ) -> Result<Vec<ModelInfo>, ProviderError> {
        let base_url = self.base_url(cfg);
        let request = self.authorised(Request::get(format!("{base_url}/models")), cfg);
        let body = send(http, self.label, request).await?;
        let mut ids: Vec<String> = body["data"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .map(|m| {
                let id = match &m["id"] {
                    Value::String(s) => s.clone(),
                    Value::Null => String::new(),
                    other => other.to_string(),
                };
                id.strip_prefix("models/").map(str::to_owned).unwrap_or(id)
            })
            .filter(|id| !id.is_empty())
            .collect();
        // Only filter the provider's own catalogue; other compatible APIs name models freely.
        if base_url == self.default_base_url.trim_end_matches('/') {
            ids.retain(|id| (self.model_filter)(id));
        }
        ids.sort();
        ids.dedup();
        Ok(ids
            .into_iter()
            .map(|id| ModelInfo {
                label: id.clone(),
                id,
            })
            .collect())
    }

    fn messages(&self, system: &str, history: &[Turn]) -> Vec<Value> {
        let mut messages = vec![json!({"role": "system", "content": system})];
        for turn in history {
            if let Some(raw) = turn.raw_from(self.id) {
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
                    let mut message = json!({
                        "role": "assistant",
                        "content": if content.is_empty() { Value::Null } else { json!(content) },
                    });
                    if !tool_calls.is_empty() {
                        message["tool_calls"] = tool_calls
                            .iter()
                            .map(|c| {
                                json!({
                                    "id": c.id,
                                    "type": "function",
                                    "function": {
                                        "name": c.name,
                                        "arguments": Value::Object(c.arguments.clone()).to_string(),
                                    },
                                })
                            })
                            .collect();
                    }
                    messages.push(message);
                }
                Turn::Tool {
                    tool_call_id,
                    content,
                    ..
                } => messages.push(
                    json!({"role": "tool", "tool_call_id": tool_call_id, "content": content}),
                ),
            }
        }
        messages
    }
}

impl LanguageModel for OpenAiCompatible {
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
            name,
        } = ask;
        let request = json!({
            "model": model,
            "messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": prompt},
            ],
            "response_format": {
                "type": "json_schema",
                "json_schema": {"name": name, "strict": true, "schema": schema},
            },
        });
        let url = format!("{}/chat/completions", self.base_url(cfg));
        let http_request = self
            .authorised(Request::post(url, request.clone()), cfg)
            .timeout(300);
        let body = send(http, self.label, http_request).await?;
        let choice = &body["choices"][0];
        let Some(message) = choice["message"].as_object() else {
            return Err(ProviderError::new(format!(
                "{} returned an unexpected response.",
                self.label
            )));
        };
        if let Some(refusal) = message.get("refusal").filter(|r| super::truthy(r)) {
            return Err(ProviderError::new(format!(
                "{} declined this request: {}",
                self.label,
                super::text_of(refusal)
            )));
        }
        if choice["finish_reason"] == "length" {
            return Err(ProviderError::new(format!(
                "{}'s answer was cut off. Try fewer transactions.",
                self.label
            )));
        }
        let data = parse_json(self.label, message.get("content").and_then(Value::as_str))?;
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
        let mut request = json!({
            "model": chat.model,
            "messages": self.messages(chat.system, chat.history),
            "tools": function_tools(chat.tools),
            "stream": true,
        });
        if chat.answer_only {
            request["tool_choice"] = json!("none");
        }
        let url = format!("{}/chat/completions", self.base_url(cfg));
        let http_request = self
            .authorised(Request::post(url, request.clone()), cfg)
            .timeout(300);
        let (mut lines, url) = open_stream(http, self.label, http_request).await?;

        let (mut text, mut refusal, mut finish) = (String::new(), String::new(), None);
        let mut calls: BTreeMap<i64, Map<String, Value>> = BTreeMap::new();
        let mut chunks = Vec::new();
        while let Some(line) = lines
            .next()
            .await
            .map_err(|e| super::transport_problem(self.label, &url, e))?
        {
            let Some(data) = line.strip_prefix("data:") else {
                continue;
            };
            let data = data.trim();
            if data == "[DONE]" {
                break;
            }
            let chunk: Value = serde_json::from_str(data).map_err(|_| {
                ProviderError::new(format!(
                    "{} sent a malformed streaming response.",
                    self.label
                ))
            })?;
            for choice in chunk["choices"]
                .as_array()
                .map(Vec::as_slice)
                .unwrap_or_default()
            {
                let delta = &choice["delta"];
                if let Some(content) = delta["content"].as_str().filter(|c| !c.is_empty()) {
                    text.push_str(content);
                    on_text(content);
                }
                if let Some(part) = delta["refusal"].as_str() {
                    refusal.push_str(part);
                }
                for part in delta["tool_calls"]
                    .as_array()
                    .map(Vec::as_slice)
                    .unwrap_or_default()
                {
                    let index = part["index"].as_i64().unwrap_or(calls.len() as i64);
                    let call = calls.entry(index).or_insert_with(|| {
                        let mut call = Map::new();
                        call.insert("id".into(), Value::Null);
                        call.insert("type".into(), json!("function"));
                        call.insert("function".into(), json!({"name": "", "arguments": ""}));
                        call
                    });
                    let function = &part["function"];
                    for key in ["name", "arguments"] {
                        if let Some(more) = function[key].as_str() {
                            let so_far = call["function"][key].as_str().unwrap_or_default();
                            call["function"][key] = json!(format!("{so_far}{more}"));
                        }
                    }
                    if let Some(part) = part.as_object() {
                        for (key, value) in part {
                            // Keep provider extras such as Gemini's thought signatures.
                            if !["index", "function", "type"].contains(&key.as_str())
                                && !value.is_null()
                            {
                                call.insert(key.clone(), value.clone());
                            }
                        }
                    }
                }
                if let Some(reason) = choice["finish_reason"].as_str() {
                    finish = Some(reason.to_owned());
                }
            }
            chunks.push(chunk);
        }
        if !refusal.is_empty() {
            return Err(ProviderError::new(format!(
                "{} declined this request: {refusal}",
                self.label
            )));
        }
        if finish.as_deref() == Some("length") {
            return Err(ProviderError::new(format!(
                "{}'s answer was cut off. Try a narrower question.",
                self.label
            )));
        }
        let mut ordered: Vec<Map<String, Value>> = calls.into_values().collect();
        for (i, call) in ordered.iter_mut().enumerate() {
            if !super::truthy(&call["id"]) {
                call.insert("id".into(), json!(format!("call_{i}")));
            }
        }
        let mut raw = json!({"role": "assistant"});
        if !text.is_empty() {
            raw["content"] = json!(text);
        }
        if !ordered.is_empty() {
            raw["tool_calls"] = json!(ordered);
        }
        let tool_calls = ordered
            .iter()
            .map(|c| {
                Ok(ToolCall {
                    id: super::text_of(&c["id"]),
                    name: c["function"]["name"]
                        .as_str()
                        .unwrap_or_default()
                        .to_owned(),
                    arguments: parse_arguments(self.label, &c["function"]["arguments"])?,
                })
            })
            .collect::<Result<_, ProviderError>>()?;
        Ok(ChatReply {
            text,
            tool_calls,
            raw,
            request,
            response: json!({ "chunks": chunks }),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_chat_models() {
        for id in ["gpt-5-mini", "o4-mini", "chatgpt-4o-latest"] {
            assert!(is_openai_chat_model(id), "{id}");
        }
        for id in [
            "text-embedding-3-small",
            "gpt-realtime",
            "whisper-1",
            "gpt-4o-mini-tts",
        ] {
            assert!(!is_openai_chat_model(id), "{id}");
        }
        assert!(is_gemini_text_model("gemini-3.8-flash"));
        assert!(!is_gemini_text_model("gemini-embedding-001"));
        assert!(!is_gemini_text_model("imagen-4"));
    }
}
