//! Stored AI settings: provider credentials, the one language model and one decision model
//! the tasks share, how each task is set up, and each provider's last-fetched model list.
//! Everything lives in `app_settings`.

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::SqlitePool;

use crate::settings::{Json, Setting, SettingError};

/// The two kinds of model. A language model writes text and can do anything; a decision model
/// (such as Jev) only picks from fixed answers, with a confidence, but is fast and cheap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Kind {
    #[serde(rename = "llm")]
    Language,
    #[serde(rename = "decision")]
    Decision,
}

impl Kind {
    pub const ALL: [Self; 2] = [Self::Language, Self::Decision];

    pub fn id(self) -> &'static str {
        match self {
            Self::Language => "llm",
            Self::Decision => "decision",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Language => "language model",
            Self::Decision => "decision model",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.id() == id)
    }
}

/// What the AI is used for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Task {
    Taps,
    Categorise,
    Rules,
    Chat,
    CheckActions,
    Matching,
    Insights,
    CsvColumns,
}

impl Task {
    pub const ALL: [Self; 8] = [
        Self::Taps,
        Self::Categorise,
        Self::Rules,
        Self::Chat,
        Self::CheckActions,
        Self::Matching,
        Self::Insights,
        Self::CsvColumns,
    ];

    pub fn id(self) -> &'static str {
        match self {
            Self::Taps => "taps",
            Self::Categorise => "categorise",
            Self::Rules => "rules",
            Self::Chat => "chat",
            Self::CheckActions => "check_actions",
            Self::Matching => "matching",
            Self::Insights => "insights",
            Self::CsvColumns => "csv_columns",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Taps => "Categorising card taps",
            Self::Categorise => "Categorising transactions",
            Self::Rules => "Suggesting rules",
            Self::Chat => "Chat",
            Self::CheckActions => "Checking chat actions",
            Self::Matching => "Matching taps, duplicates and transfers",
            Self::Insights => "Flagging subscriptions and unusual charges",
            Self::CsvColumns => "Reading CSV columns",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|t| t.id() == id)
    }

    /// Which kinds of model can do it, preferred first. Rules and chat need to write text.
    pub fn kinds(self) -> &'static [Kind] {
        match self {
            Self::Rules | Self::Chat => &[Kind::Language],
            _ => &[Kind::Decision, Kind::Language],
        }
    }

    /// Whether it can run by itself as new data arrives, as well as when asked. Rules only
    /// run when asked; chat and checking chat's actions run whenever the chat does.
    pub fn has_mode(self) -> bool {
        matches!(
            self,
            Self::Taps | Self::Categorise | Self::Matching | Self::Insights | Self::CsvColumns
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Automatic,
    OnDemand,
}

impl Mode {
    pub fn id(self) -> &'static str {
        match self {
            Self::Automatic => "automatic",
            Self::OnDemand => "on_demand",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        [Self::Automatic, Self::OnDemand]
            .into_iter()
            .find(|m| m.id() == id)
    }
}

fn text(key: String) -> Setting<String> {
    Setting::named(key)
}

async fn put(db: &SqlitePool, key: String, value: Option<&str>) -> Result<(), SettingError> {
    let setting = text(key);
    match value {
        Some(value) => setting.set(db, &value.to_owned()).await,
        None => setting.clear(db).await,
    }
}

pub async fn provider_value(
    db: &SqlitePool,
    provider: &str,
    field: &str,
) -> Result<Option<String>, SettingError> {
    text(format!("ai.provider.{provider}.{field}"))
        .get(db)
        .await
}

/// Saves a provider's fields: a blank or missing value clears one. New credentials haven't
/// been tried yet, so any problem with the old ones is forgotten.
pub async fn set_provider_values(
    db: &SqlitePool,
    provider: &str,
    values: &[(String, Option<String>)],
) -> Result<(), SettingError> {
    for (field, value) in values {
        let cleaned = value.as_deref().map(str::trim).filter(|v| !v.is_empty());
        put(db, format!("ai.provider.{provider}.{field}"), cleaned).await?;
    }
    put(db, format!("ai.provider_status.{provider}"), None).await
}

/// Why the provider's credentials failed the last time they were tried, if they did.
pub async fn provider_problem(
    db: &SqlitePool,
    provider: &str,
) -> Result<Option<String>, SettingError> {
    text(format!("ai.provider_status.{provider}")).get(db).await
}

pub async fn set_provider_problem(
    db: &SqlitePool,
    provider: &str,
    problem: Option<&str>,
) -> Result<(), SettingError> {
    put(db, format!("ai.provider_status.{provider}"), problem).await
}

/// The provider and model chosen for a kind of model.
pub async fn model(
    db: &SqlitePool,
    kind: Kind,
) -> Result<(Option<String>, Option<String>), SettingError> {
    let id = kind.id();
    Ok((
        text(format!("ai.model.{id}.provider")).get(db).await?,
        text(format!("ai.model.{id}.model")).get(db).await?,
    ))
}

pub async fn set_model(
    db: &SqlitePool,
    kind: Kind,
    provider: Option<&str>,
    model: Option<&str>,
) -> Result<(), SettingError> {
    let id = kind.id();
    put(db, format!("ai.model.{id}.provider"), provider).await?;
    put(db, format!("ai.model.{id}.model"), provider.and(model)).await
}

async fn model_chosen(db: &SqlitePool, kind: Kind) -> Result<bool, SettingError> {
    let (provider, model) = model(db, kind).await?;
    Ok(provider.is_some() && model.is_some())
}

/// On unless the user turns it off. It only runs once a model of its kind is chosen.
pub async fn task_enabled(db: &SqlitePool, task: Task) -> Result<bool, SettingError> {
    let stored = text(format!("ai.task.{}.enabled", task.id()))
        .get(db)
        .await?;
    Ok(stored.as_deref() != Some("off"))
}

pub async fn set_task_enabled(
    db: &SqlitePool,
    task: Task,
    enabled: bool,
) -> Result<(), SettingError> {
    let value = if enabled { "on" } else { "off" };
    put(db, format!("ai.task.{}.enabled", task.id()), Some(value)).await
}

/// The kind of model a task uses: the user's choice, or else the first it can use that's
/// been chosen (a decision model, being faster and cheaper), or else its first.
pub async fn task_kind(db: &SqlitePool, task: Task) -> Result<Kind, SettingError> {
    let kinds = task.kinds();
    let stored = text(format!("ai.task.{}.kind", task.id())).get(db).await?;
    if let Some(kind) = stored.as_deref().and_then(Kind::from_id)
        && kinds.contains(&kind)
    {
        return Ok(kind);
    }
    for kind in kinds {
        if model_chosen(db, *kind).await? {
            return Ok(*kind);
        }
    }
    Ok(kinds[0])
}

pub async fn set_task_kind(db: &SqlitePool, task: Task, kind: Kind) -> Result<(), SettingError> {
    put(db, format!("ai.task.{}.kind", task.id()), Some(kind.id())).await
}

/// Whether a task runs by itself on new data or only when asked; `None` for tasks without
/// the choice. Unless the user chooses, decision models run automatically, being fast and
/// cheap, and language models only when asked.
pub async fn task_mode(db: &SqlitePool, task: Task) -> Result<Option<Mode>, SettingError> {
    if !task.has_mode() {
        return Ok(None);
    }
    let stored = text(format!("ai.task.{}.mode", task.id())).get(db).await?;
    if let Some(mode) = stored.as_deref().and_then(Mode::from_id) {
        return Ok(Some(mode));
    }
    Ok(Some(if task_kind(db, task).await? == Kind::Decision {
        Mode::Automatic
    } else {
        Mode::OnDemand
    }))
}

pub async fn set_task_mode(db: &SqlitePool, task: Task, mode: Mode) -> Result<(), SettingError> {
    put(db, format!("ai.task.{}.mode", task.id()), Some(mode.id())).await
}

/// The provider and model a task would use, or nothing if it's turned off.
pub async fn task_model(
    db: &SqlitePool,
    task: Task,
) -> Result<(Option<String>, Option<String>), SettingError> {
    if !task_enabled(db, task).await? {
        return Ok((None, None));
    }
    model(db, task_kind(db, task).await?).await
}

/// Whether the task should run by itself as new data arrives: it's on, has a model, and
/// isn't set to run only when asked.
pub async fn runs_automatically(db: &SqlitePool, task: Task) -> Result<bool, SettingError> {
    let (provider, model) = task_model(db, task).await?;
    Ok(provider.is_some() && model.is_some() && task_mode(db, task).await? != Some(Mode::OnDemand))
}

/// A model a provider offers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub label: String,
}

pub async fn cached_models(
    db: &SqlitePool,
    provider: &str,
) -> Result<(Vec<ModelInfo>, Option<String>), SettingError> {
    let models: Setting<Json<Vec<ModelInfo>>> =
        Setting::named(format!("ai.provider.{provider}.models"));
    let updated = text(format!("ai.provider.{provider}.models_updated_at"))
        .get(db)
        .await?;
    Ok((
        models.get(db).await?.map(|m| m.0).unwrap_or_default(),
        updated,
    ))
}

/// Remembers a provider's model list, returning when (e.g. "2026-10-14T09:00:00+00:00").
pub async fn cache_models(
    db: &SqlitePool,
    provider: &str,
    models: &[ModelInfo],
) -> Result<String, SettingError> {
    let updated = Utc::now().format("%Y-%m-%dT%H:%M:%S+00:00").to_string();
    let setting: Setting<Json<Vec<ModelInfo>>> =
        Setting::named(format!("ai.provider.{provider}.models"));
    setting.set(db, &Json(models.to_vec())).await?;
    put(
        db,
        format!("ai.provider.{provider}.models_updated_at"),
        Some(&updated),
    )
    .await?;
    Ok(updated)
}

/// What the chat may change beyond looking things up: "organise" and "transactions".
pub const CHAT_SCOPES: [&str; 2] = ["organise", "transactions"];
const CHAT_SCOPE_KEY: &str = "ai.chat.scopes";

/// What the chat may change. Nothing unless the user allows it.
pub async fn chat_scopes(db: &SqlitePool) -> Result<Vec<String>, SettingError> {
    // Anything unreadable counts as nothing allowed.
    let setting: Setting<Json<Value>> = Setting::new(CHAT_SCOPE_KEY);
    let stored: Vec<String> = setting
        .get(db)
        .await
        .ok()
        .flatten()
        .and_then(|j| serde_json::from_value(j.0).ok())
        .unwrap_or_default();
    Ok(CHAT_SCOPES
        .iter()
        .filter(|s| stored.iter().any(|t| t == *s))
        .map(|s| (*s).to_owned())
        .collect())
}

pub async fn set_chat_scopes(db: &SqlitePool, scopes: &[String]) -> Result<(), SettingError> {
    let kept: Vec<String> = CHAT_SCOPES
        .iter()
        .filter(|s| scopes.iter().any(|t| t == *s))
        .map(|s| (*s).to_owned())
        .collect();
    let setting: Setting<Json<Vec<String>>> = Setting::new(CHAT_SCOPE_KEY);
    setting.set(db, &Json(kept)).await
}
