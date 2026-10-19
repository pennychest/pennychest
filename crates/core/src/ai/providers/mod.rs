//! The AI providers the app can use, what each needs to connect, and what kind of model each
//! offers.
//!
//! Language model providers implement `LanguageModel`: answers as JSON matching a schema, and
//! chat with tools. Decision model providers, TypeSafe's Jev and OpenAI's Decisions API,
//! implement `DecisionModel`, which answers typed questions with a confidence. They can do
//! every task except writing rules and chat; `Task::kinds` says which kinds each task can use.

mod anthropic;
mod decisions;
mod ollama;
mod openai;
mod vertex;

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sqlx::SqlitePool;

use super::config::{self, Kind, ModelInfo, Task};
use super::http::{Request, Response, Transport, TransportError};
use crate::settings::SettingError;

pub use anthropic::Anthropic;
pub use decisions::{OpenAiDecisions, TypeSafe};
pub use ollama::Ollama;
pub use openai::OpenAiCompatible;
pub use vertex::Vertex;

/// A provider problem whose message is safe and useful to show the user.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct ProviderError {
    pub message: String,
    /// The provider rejected the saved credentials, as opposed to being busy or unreachable.
    pub credentials: bool,
}

impl ProviderError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            credentials: false,
        }
    }

    pub fn credentials(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            credentials: true,
        }
    }
}

/// Something a provider needs to connect, as Settings > AI asks for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    pub key: &'static str,
    pub label: &'static str,
    pub secret: bool,
    pub required: bool,
    pub default: Option<&'static str>,
    pub placeholder: &'static str,
}

impl Field {
    const fn api_key(placeholder: &'static str) -> Self {
        Self {
            key: "api_key",
            label: "API key",
            secret: true,
            required: true,
            default: None,
            placeholder,
        }
    }
}

/// A provider's saved fields, with defaults filled in.
pub type ProviderConfig = HashMap<String, String>;

fn setting<'a>(cfg: &'a ProviderConfig, key: &str) -> &'a str {
    cfg.get(key).map_or("", String::as_str)
}

/// A model's answer, with the request and response for the AI logs.
#[derive(Debug, Clone, PartialEq)]
pub struct Call {
    pub data: Map<String, Value>,
    pub request: Value,
    pub response: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Map<String, Value>,
}

/// One model turn: its text, any tools it wants run, and the message exactly as the provider
/// returned it. `raw` is replayed verbatim when the conversation continues with the same
/// provider, which keeps provider-specific data such as Claude's thinking blocks or Gemini's
/// thought signatures intact.
#[derive(Debug, Clone, PartialEq)]
pub struct ChatReply {
    pub text: String,
    pub tool_calls: Vec<ToolCall>,
    pub raw: Value,
    pub request: Value,
    pub response: Value,
}

/// A conversation as providers are given it, whichever provider wrote its earlier turns.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "lowercase")]
pub enum Turn {
    User {
        content: String,
    },
    Assistant {
        #[serde(default)]
        content: String,
        #[serde(default)]
        tool_calls: Vec<ToolCall>,
        /// The message as its provider returned it, replayed to the same provider.
        #[serde(default)]
        raw: Option<Value>,
        #[serde(default)]
        raw_provider: Option<String>,
    },
    Tool {
        tool_call_id: String,
        name: String,
        content: String,
        #[serde(default)]
        is_error: bool,
    },
}

impl Turn {
    /// The raw message, if `provider` wrote it.
    fn raw_from(&self, provider: &str) -> Option<&Value> {
        match self {
            Self::Assistant {
                raw: Some(raw),
                raw_provider: Some(by),
                ..
            } if by == provider => Some(raw),
            _ => None,
        }
    }
}

/// A tool a model may call: an action, as the actions layer describes it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Tool {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

/// What a chat turn needs.
pub struct ChatRequest<'a> {
    pub model: &'a str,
    pub system: &'a str,
    pub history: &'a [Turn],
    pub tools: &'a [Tool],
    /// The tools stay listed (providers reject a history of tool calls without them) but the
    /// model is stopped from calling any, where the provider allows.
    pub answer_only: bool,
}

/// What `complete_json` asks: an answer as JSON matching `schema`, which is called `name`.
#[derive(Debug, Clone, Copy)]
pub struct JsonRequest<'a> {
    pub model: &'a str,
    pub system: &'a str,
    pub prompt: &'a str,
    pub schema: &'a Value,
    pub name: &'a str,
}

/// Receives a reply's text as it's written.
pub type OnText<'a> = &'a mut (dyn FnMut(&str) + Send);

/// A model that writes text: JSON answers and chat with tools.
pub trait LanguageModel {
    /// An answer as JSON matching the request's schema.
    fn complete_json(
        &self,
        http: &dyn Transport,
        cfg: &ProviderConfig,
        ask: JsonRequest<'_>,
    ) -> impl Future<Output = Result<Call, ProviderError>> + Send;

    /// One chat turn, passing its text to `on_text` as it arrives.
    fn chat_turn(
        &self,
        http: &dyn Transport,
        cfg: &ProviderConfig,
        request: ChatRequest<'_>,
        on_text: OnText<'_>,
    ) -> impl Future<Output = Result<ChatReply, ProviderError>> + Send;
}

/// A decision model. Tasks ask it questions in Jev's form, which other decision providers
/// translate:
///
/// ```text
/// state: {key: anything}, which the questions refer to as `key`
/// questions: {name: {"type": "choice", "instructions": str, "criteria": {value: str | null}}
///                 | {"type": "noul", "instructions": str, "criteria"?: {"true": str,
///                    "false": str}}
///                 | {"type": "score", "instructions": str, "criteria": [str, ...]}}
/// ```
///
/// and get answers keyed by question name: `{"choice", "confidence"}`, `{"noul": probability}`
/// or `{"score": level, "confidence"}`, where level counts from 0. A question the model won't
/// answer is missing or empty.
pub trait DecisionModel {
    fn choose(
        &self,
        http: &dyn Transport,
        cfg: &ProviderConfig,
        model: &str,
        state: &Value,
        questions: &Map<String, Value>,
    ) -> impl Future<Output = Result<Call, ProviderError>> + Send;
}

use std::future::Future;

/// Every provider the app knows.
pub enum Provider {
    Anthropic(Anthropic),
    OpenAi(OpenAiCompatible),
    Google(OpenAiCompatible),
    Vertex(Vertex),
    TypeSafe(TypeSafe),
    OpenAiDecisions(OpenAiDecisions),
    Ollama(Ollama),
}

/// In the order Settings > AI lists them.
pub static PROVIDERS: std::sync::LazyLock<Vec<Provider>> = std::sync::LazyLock::new(|| {
    vec![
        Provider::Anthropic(Anthropic),
        Provider::OpenAi(OpenAiCompatible::openai()),
        Provider::Google(OpenAiCompatible::google()),
        Provider::Vertex(Vertex),
        Provider::TypeSafe(TypeSafe),
        Provider::OpenAiDecisions(OpenAiDecisions),
        Provider::Ollama(Ollama),
    ]
});

pub fn find(id: &str) -> Option<&'static Provider> {
    PROVIDERS.iter().find(|p| p.id() == id)
}

impl Provider {
    pub fn id(&self) -> &'static str {
        match self {
            Self::Anthropic(_) => "anthropic",
            Self::OpenAi(_) => "openai",
            Self::Google(_) => "google",
            Self::Vertex(_) => "vertex",
            Self::TypeSafe(_) => "typesafe",
            Self::OpenAiDecisions(_) => "openai_decisions",
            Self::Ollama(_) => "ollama",
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Anthropic(_) => anthropic::LABEL,
            Self::OpenAi(p) | Self::Google(p) => p.label,
            Self::Vertex(_) => vertex::LABEL,
            Self::TypeSafe(_) => decisions::TYPESAFE,
            Self::OpenAiDecisions(_) => decisions::OPENAI,
            Self::Ollama(_) => ollama::LABEL,
        }
    }

    pub fn kind(&self) -> Kind {
        match self {
            Self::TypeSafe(_) | Self::OpenAiDecisions(_) => Kind::Decision,
            _ => Kind::Language,
        }
    }

    pub fn fields(&self) -> Vec<Field> {
        match self {
            Self::Anthropic(_) => vec![Field::api_key("sk-ant-...")],
            Self::OpenAi(p) | Self::Google(p) => p.fields(),
            Self::Vertex(_) => vec![Field {
                label: "Express mode API key",
                ..Field::api_key("AQ....")
            }],
            Self::TypeSafe(_) => vec![Field::api_key("")],
            Self::OpenAiDecisions(_) => vec![Field::api_key("sk-...")],
            Self::Ollama(_) => vec![Field {
                key: "url",
                label: "Server URL",
                secret: false,
                required: true,
                default: None,
                placeholder: "http://ollama:11434",
            }],
        }
    }

    /// The models it offers, checking the credentials work.
    pub async fn list_models(
        &self,
        http: &dyn Transport,
        cfg: &ProviderConfig,
    ) -> Result<Vec<ModelInfo>, ProviderError> {
        match self {
            Self::Anthropic(p) => p.list_models(http, cfg).await,
            Self::OpenAi(p) | Self::Google(p) => p.list_models(http, cfg).await,
            Self::Vertex(p) => p.list_models(http, cfg).await,
            Self::TypeSafe(p) => p.list_models(http, cfg).await,
            Self::OpenAiDecisions(p) => p.list_models(http, cfg).await,
            Self::Ollama(p) => p.list_models(http, cfg).await,
        }
    }

    pub async fn complete_json(
        &self,
        http: &dyn Transport,
        cfg: &ProviderConfig,
        ask: JsonRequest<'_>,
    ) -> Result<Call, ProviderError> {
        match self {
            Self::Anthropic(p) => p.complete_json(http, cfg, ask).await,
            Self::OpenAi(p) | Self::Google(p) => p.complete_json(http, cfg, ask).await,
            Self::Vertex(p) => p.complete_json(http, cfg, ask).await,
            Self::Ollama(p) => p.complete_json(http, cfg, ask).await,
            Self::TypeSafe(_) | Self::OpenAiDecisions(_) => Err(ProviderError::new(format!(
                "{} can't do this task.",
                self.label()
            ))),
        }
    }

    pub async fn chat_turn(
        &self,
        http: &dyn Transport,
        cfg: &ProviderConfig,
        request: ChatRequest<'_>,
        on_text: OnText<'_>,
    ) -> Result<ChatReply, ProviderError> {
        match self {
            Self::Anthropic(p) => p.chat_turn(http, cfg, request, on_text).await,
            Self::OpenAi(p) | Self::Google(p) => p.chat_turn(http, cfg, request, on_text).await,
            Self::Vertex(p) => p.chat_turn(http, cfg, request, on_text).await,
            Self::Ollama(p) => p.chat_turn(http, cfg, request, on_text).await,
            Self::TypeSafe(_) | Self::OpenAiDecisions(_) => Err(ProviderError::new(format!(
                "{} can't be used for chat.",
                self.label()
            ))),
        }
    }

    pub async fn choose(
        &self,
        http: &dyn Transport,
        cfg: &ProviderConfig,
        model: &str,
        state: &Value,
        questions: &Map<String, Value>,
    ) -> Result<Call, ProviderError> {
        match self {
            Self::TypeSafe(p) => p.choose(http, cfg, model, state, questions).await,
            Self::OpenAiDecisions(p) => p.choose(http, cfg, model, state, questions).await,
            _ => Err(ProviderError::new(format!(
                "{} can't do this task.",
                self.label()
            ))),
        }
    }
}

/// A provider's saved fields, with defaults for those not set.
pub async fn provider_config(
    db: &SqlitePool,
    provider: &Provider,
) -> Result<ProviderConfig, SettingError> {
    let mut cfg = HashMap::new();
    for field in provider.fields() {
        let value = config::provider_value(db, provider.id(), field.key)
            .await?
            .or_else(|| field.default.map(str::to_owned));
        if let Some(value) = value {
            cfg.insert(field.key.to_owned(), value);
        }
    }
    Ok(cfg)
}

/// The labels of the required fields not yet filled in.
pub fn missing_fields(provider: &Provider, cfg: &ProviderConfig) -> Vec<&'static str> {
    provider
        .fields()
        .into_iter()
        .filter(|f| f.required && setting(cfg, f.key).is_empty())
        .map(|f| f.label)
        .collect()
}

/// What a task runs on.
pub struct Resolved {
    pub provider: &'static Provider,
    pub cfg: ProviderConfig,
    pub model: String,
}

impl Resolved {
    /// How the AI logs record it, e.g. "openai:gpt-5-mini".
    pub fn label(&self) -> String {
        format!("{}:{}", self.provider.id(), self.model)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ResolveError {
    /// What to set up, for the user.
    #[error(transparent)]
    Provider(#[from] ProviderError),
    #[error(transparent)]
    Setting(#[from] SettingError),
}

/// The provider, its settings and the model `task` uses, or what the user needs to set up.
pub async fn resolve_task(db: &SqlitePool, task: Task) -> Result<Resolved, ResolveError> {
    let label = task.label();
    if !config::task_enabled(db, task).await? {
        return Err(ProviderError::new(format!("{label} is turned off in Settings > AI.")).into());
    }
    let kind = config::task_kind(db, task).await?;
    let (provider_id, model) = config::model(db, kind).await?;
    let (Some(provider), Some(model)) = (provider_id.as_deref().and_then(find), model) else {
        return Err(ProviderError::new(format!(
            "Choose a {} for {} in Settings > AI.",
            kind.label(),
            label.to_lowercase()
        ))
        .into());
    };
    if provider.kind() != kind {
        return Err(
            ProviderError::new(format!("{} isn't a {}.", provider.label(), kind.label())).into(),
        );
    }
    let cfg = provider_config(db, provider).await?;
    if let Some(problem) = config::provider_problem(db, provider.id()).await? {
        return Err(ProviderError::new(problem).into());
    }
    let missing = missing_fields(provider, &cfg);
    if !missing.is_empty() {
        return Err(ProviderError::new(format!(
            "{} isn't set up yet: add its {} in Settings > AI.",
            provider.label(),
            missing.join(", ")
        ))
        .into());
    }
    Ok(Resolved {
        provider,
        cfg,
        model,
    })
}

// Plain HTTP, shared by the providers.

/// What went wrong, from an error response's body.
fn error_detail(body: &[u8]) -> String {
    let short = |s: &str| s.chars().take(200).collect::<String>();
    let Ok(parsed) = serde_json::from_slice::<Value>(body) else {
        return short(&String::from_utf8_lossy(body));
    };
    if let Some(map) = parsed.as_object() {
        match map.get("error") {
            Some(Value::Object(error)) => {
                if let Some(message) = error.get("message").filter(|m| !m.is_null()) {
                    return short(&text_of(message));
                }
            }
            Some(Value::String(error)) => return short(error),
            _ => {}
        }
        if let Some(detail) = map.get("detail").filter(|d| truthy(d)) {
            return short(&text_of(detail));
        }
    }
    short(&parsed.to_string())
}

fn truthy(value: &Value) -> bool {
    match value {
        Value::Null | Value::Bool(false) => false,
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
        _ => true,
    }
}

fn text_of(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// The user-facing problem with an error response, if it is one.
fn status_problem(label: &str, status: u16, body: &[u8]) -> Option<ProviderError> {
    let detail = error_detail(body);
    // Google answers an invalid key with 400 ("Please pass a valid API key") rather than 401.
    let invalid_key = status == 400 && detail.to_lowercase().contains("api key");
    Some(if status == 401 || status == 403 || invalid_key {
        ProviderError::credentials(format!(
            "{label} rejected the credentials ({detail}). Check them in Settings > AI."
        ))
    } else if status == 404 {
        ProviderError::new(format!("{label} couldn't find that model: {detail}"))
    } else if status == 429 || status == 529 {
        ProviderError::new(format!(
            "{label} is busy or rate limiting requests. Try again shortly."
        ))
    } else if status >= 400 {
        ProviderError::new(format!("{label} returned an error ({status}): {detail}"))
    } else {
        return None;
    })
}

fn transport_problem(label: &str, url: &str, error: TransportError) -> ProviderError {
    match error {
        TransportError::Timeout => ProviderError::new(format!("{label} took too long to respond.")),
        TransportError::Connect => {
            ProviderError::new(format!("Couldn't connect to {label} at {url}."))
        }
    }
}

/// Sends a request and returns its JSON answer, or what went wrong in the user's terms.
async fn send(http: &dyn Transport, label: &str, request: Request) -> Result<Value, ProviderError> {
    let url = request.url.clone();
    let response = http
        .send(request)
        .await
        .map_err(|e| transport_problem(label, &url, e))?;
    let status = response.status;
    let body = response
        .bytes()
        .await
        .map_err(|e| transport_problem(label, &url, e))?;
    if let Some(problem) = status_problem(label, status, &body) {
        return Err(problem);
    }
    serde_json::from_slice(&body)
        .map_err(|_| ProviderError::new(format!("{label} returned a response that wasn't JSON.")))
}

/// Opens a streamed response, checking its status first.
async fn open_stream(
    http: &dyn Transport,
    label: &str,
    request: Request,
) -> Result<(super::http::Lines, String), ProviderError> {
    let url = request.url.clone();
    let response: Response = http
        .send(request)
        .await
        .map_err(|e| transport_problem(label, &url, e))?;
    if response.status >= 400 {
        let status = response.status;
        let body = response
            .bytes()
            .await
            .map_err(|e| transport_problem(label, &url, e))?;
        return Err(status_problem(label, status, &body)
            .unwrap_or_else(|| ProviderError::new(format!("{label} returned an error."))));
    }
    Ok((response.lines(), url))
}

/// JSON text a model returned, which must be an object.
fn parse_json(label: &str, text: Option<&str>) -> Result<Map<String, Value>, ProviderError> {
    let text = text.unwrap_or_default();
    let short: String = text.chars().take(200).collect();
    match serde_json::from_str::<Value>(text) {
        Ok(Value::Object(map)) => Ok(map),
        Ok(other) => Err(ProviderError::new(format!(
            "{label} returned unexpected output: {}",
            other.to_string().chars().take(200).collect::<String>()
        ))),
        Err(_) => Err(ProviderError::new(format!(
            "{label} returned output that wasn't valid JSON: {short}"
        ))),
    }
}

/// A tool call's arguments, which some providers send as JSON text.
fn parse_arguments(label: &str, value: &Value) -> Result<Map<String, Value>, ProviderError> {
    match value {
        Value::Object(map) => Ok(map.clone()),
        Value::Null => Ok(Map::new()),
        Value::String(text) => {
            let text = if text.is_empty() { "{}" } else { text };
            match serde_json::from_str::<Value>(text) {
                Ok(Value::Object(map)) => Ok(map),
                Ok(_) => Ok(Map::new()),
                Err(_) => Err(ProviderError::new(format!(
                    "{label} asked to run a tool with arguments that weren't valid JSON."
                ))),
            }
        }
        _ => Ok(Map::new()),
    }
}

/// Tools in the function-calling form OpenAI and Ollama take.
fn function_tools(tools: &[Tool]) -> Vec<Value> {
    tools
        .iter()
        .map(|t| {
            json!({
                "type": "function",
                "function": {
                    "name": t.name,
                    "description": t.description,
                    "parameters": t.input_schema,
                },
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_details_come_from_the_body() {
        assert_eq!(
            error_detail(br#"{"error": {"message": "Incorrect key"}}"#),
            "Incorrect key"
        );
        assert_eq!(error_detail(br#"{"error": "nope"}"#), "nope");
        assert_eq!(error_detail(br#"{"detail": "Not here"}"#), "Not here");
        assert_eq!(error_detail(b"plain text"), "plain text");
        assert_eq!(error_detail(br#"{"other": 1}"#), r#"{"other":1}"#);
    }

    #[test]
    fn statuses_become_readable_problems() {
        let key = status_problem("OpenAI", 401, br#"{"error": {"message": "bad"}}"#).unwrap();
        assert!(key.credentials);
        assert_eq!(
            key.message,
            "OpenAI rejected the credentials (bad). Check them in Settings > AI."
        );
        let google = status_problem(
            "Google",
            400,
            br#"{"error": {"message": "Please pass a valid API key"}}"#,
        );
        assert!(google.unwrap().credentials);
        assert!(!status_problem("X", 429, b"").unwrap().credentials);
        assert!(status_problem("X", 200, b"").is_none());
    }

    #[test]
    fn every_provider_is_listed_once_with_its_kind() {
        let ids: Vec<&str> = PROVIDERS.iter().map(Provider::id).collect();
        assert_eq!(
            ids,
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
        let decisions: Vec<&str> = PROVIDERS
            .iter()
            .filter(|p| p.kind() == Kind::Decision)
            .map(Provider::id)
            .collect();
        assert_eq!(decisions, ["typesafe", "openai_decisions"]);
    }
}
