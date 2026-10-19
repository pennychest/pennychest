//! `/api/ai`: AI providers and their credentials, the language and decision models the tasks
//! share, how each task is set up, running the tasks on demand, the request logs, and the
//! model learnt from the user's own history.

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post, put};
use axum::{Json, Router};
use pennychest_core::actions::Source;
use pennychest_core::ai::config::{self, CHAT_SCOPES, Kind, Mode, Task};
use pennychest_core::ai::providers::{self, Provider, ResolveError};
use pennychest_core::ai::rules::{self as ai_rules, SuggestError};
use pennychest_core::ai::{self as core_ai, LogEntry, categorise, insights as ai_insights};
use pennychest_core::learned::{self, Status};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::AppState;
use crate::error::{ApiError, ApiResult, JsonBody, PathParam, QueryParams};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/config", get(get_config))
        .route("/providers/{id}", put(save_provider))
        .route("/providers/{id}/models", post(refresh_models))
        .route("/models/{kind}", put(choose_model))
        .route("/tasks/{task}", put(update_task))
        .route("/chat/scopes", put(choose_chat_scopes))
        .route("/logs", get(list_logs))
        .route("/categorise/batch/{id}", post(categorise_batch))
        .route("/categorise/all", post(categorise_all))
        .route("/insights/batch/{id}", post(insights_batch))
        .route("/suggest-rules", post(suggest_rules))
        .route("/learned", get(learned_status).put(set_learned))
}

fn provider_or_404(id: &str) -> ApiResult<&'static Provider> {
    providers::find(id).ok_or_else(|| ApiError::not_found("Unknown AI provider"))
}

async fn get_config(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let db = &state.db;
    let mut listed = Vec::new();
    for provider in providers::PROVIDERS.iter() {
        let cfg = providers::provider_config(db, provider).await?;
        let (models, updated_at) = config::cached_models(db, provider.id()).await?;
        let mut fields = Vec::new();
        for field in provider.fields() {
            let saved = config::provider_value(db, provider.id(), field.key).await?;
            fields.push(json!({
                "key": field.key,
                "label": field.label,
                "secret": field.secret,
                "required": field.required,
                "placeholder": field.placeholder,
                "is_set": saved.is_some(),
                // Secrets never leave the server.
                "value": if field.secret { None } else { cfg.get(field.key) },
            }));
        }
        listed.push(json!({
            "id": provider.id(),
            "label": provider.label(),
            "kind": provider.kind(),
            "configured": providers::missing_fields(provider, &cfg).is_empty(),
            // Set when the saved credentials were rejected the last time they were tried
            "problem": config::provider_problem(db, provider.id()).await?,
            "fields": fields,
            "models": models,
            "models_updated_at": updated_at,
        }));
    }

    let mut models = Map::new();
    for kind in Kind::ALL {
        let (provider_id, model) = config::model(db, kind).await?;
        let provider = provider_id.as_deref().and_then(providers::find);
        let mut problem = None;
        if let Some(provider) = provider {
            let missing = providers::missing_fields(
                provider,
                &providers::provider_config(db, provider).await?,
            );
            problem = config::provider_problem(db, provider.id()).await?;
            if problem.is_none() && !missing.is_empty() {
                problem = Some(format!(
                    "Add {}'s {} below.",
                    provider.label(),
                    missing.join(", ")
                ));
            }
        }
        models.insert(
            kind.id().into(),
            json!({
                "provider": provider_id,
                "model": model,
                "ready": provider.is_some() && model.is_some() && problem.is_none(),
                "problem": problem,
            }),
        );
    }

    let mut tasks = Map::new();
    for task in Task::ALL {
        let (provider, model) = config::task_model(db, task).await?;
        let problem = match providers::resolve_task(db, task).await {
            Ok(_) => None,
            Err(ResolveError::Provider(e)) => Some(e.message),
            Err(ResolveError::Setting(e)) => return Err(e.into()),
        };
        let kinds: Vec<&str> = task.kinds().iter().map(|k| k.id()).collect();
        let mut entry = json!({
            "label": task.label(),
            // The kinds of model it can use, and the one it does
            "kinds": kinds,
            "kind": config::task_kind(db, task).await?,
            "enabled": config::task_enabled(db, task).await?,
            // "automatic" or "on_demand", or null if it doesn't have the choice
            "mode": config::task_mode(db, task).await?,
            "provider": provider,
            "model": model,
            "ready": problem.is_none(),
            "problem": problem,
        });
        if task == Task::Chat {
            entry["scopes"] = json!(config::chat_scopes(db).await?);
        }
        tasks.insert(task.id().into(), entry);
    }
    Ok(Json(
        json!({ "providers": listed, "models": models, "tasks": tasks }),
    ))
}

#[derive(Deserialize)]
struct ProviderValues {
    /// Leave a field out to keep it; send null or "" to clear it.
    values: Map<String, Value>,
}

async fn save_provider(
    State(state): State<AppState>,
    PathParam(id): PathParam<String>,
    JsonBody(body): JsonBody<ProviderValues>,
) -> ApiResult<StatusCode> {
    let provider = provider_or_404(&id)?;
    let known: Vec<&str> = provider.fields().iter().map(|f| f.key).collect();
    let mut unknown: Vec<&str> = body
        .values
        .keys()
        .map(String::as_str)
        .filter(|k| !known.contains(k))
        .collect();
    if !unknown.is_empty() {
        unknown.sort_unstable();
        return Err(ApiError::bad_request(format!(
            "Unknown fields: {}",
            unknown.join(", ")
        )));
    }
    let mut values = Vec::new();
    for (key, value) in body.values {
        let value = match value {
            Value::String(s) => Some(s),
            Value::Null => None,
            _ => return Err(ApiError::invalid(format!("{key} must be text or null"))),
        };
        values.push((key, value));
    }
    config::set_provider_values(&state.db, provider.id(), &values).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize)]
struct Refreshed {
    models: Vec<config::ModelInfo>,
    models_updated_at: String,
}

/// Fetches the provider's current model list and remembers it.
async fn refresh_models(
    State(state): State<AppState>,
    PathParam(id): PathParam<String>,
) -> ApiResult<Json<Refreshed>> {
    let provider = provider_or_404(&id)?;
    let cfg = providers::provider_config(&state.db, provider).await?;
    let missing = providers::missing_fields(provider, &cfg);
    if !missing.is_empty() {
        return Err(ApiError::bad_request(format!(
            "Add {}'s {} first.",
            provider.label(),
            missing.join(", ")
        )));
    }
    let models = match provider.list_models(state.http.as_ref(), &cfg).await {
        Ok(models) => models,
        Err(e) => {
            // Only a rejected key is remembered; a busy or unreachable provider may work next
            // time.
            if e.credentials {
                config::set_provider_problem(&state.db, provider.id(), Some(&e.message)).await?;
            }
            return Err(ApiError::bad_request(e.message));
        }
    };
    config::set_provider_problem(&state.db, provider.id(), None).await?;
    let updated = config::cache_models(&state.db, provider.id(), &models).await?;
    Ok(Json(Refreshed {
        models,
        models_updated_at: updated,
    }))
}

#[derive(Deserialize)]
struct ModelChoice {
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    model: Option<String>,
}

/// Chooses the language model or decision model the tasks share, or clears it.
async fn choose_model(
    State(state): State<AppState>,
    PathParam(kind): PathParam<String>,
    JsonBody(body): JsonBody<ModelChoice>,
) -> ApiResult<StatusCode> {
    let kind = Kind::from_id(&kind).ok_or_else(|| ApiError::not_found("Unknown kind of model"))?;
    let model = body
        .model
        .as_deref()
        .map(str::trim)
        .filter(|m| !m.is_empty());
    if let Some(id) = &body.provider {
        let provider = provider_or_404(id)?;
        if provider.kind() != kind {
            return Err(ApiError::bad_request(format!(
                "{} isn't a {}.",
                provider.label(),
                kind.label()
            )));
        }
        if model.is_none() {
            return Err(ApiError::bad_request("Choose a model"));
        }
    }
    config::set_model(&state.db, kind, body.provider.as_deref(), model).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct TaskSettings {
    // Leave any out to keep it.
    #[serde(default)]
    enabled: Option<bool>,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    mode: Option<String>,
}

/// Turns a task on or off, chooses which kind of model it uses, and whether it runs by itself
/// or only when asked.
async fn update_task(
    State(state): State<AppState>,
    PathParam(task): PathParam<String>,
    JsonBody(body): JsonBody<TaskSettings>,
) -> ApiResult<StatusCode> {
    let task = Task::from_id(&task).ok_or_else(|| ApiError::not_found("Unknown AI task"))?;
    let kind = match &body.kind {
        None => None,
        Some(id) => match Kind::from_id(id).filter(|k| task.kinds().contains(k)) {
            Some(kind) => Some(kind),
            None => {
                let name = Kind::from_id(id).map_or_else(|| id.clone(), |k| k.label().to_owned());
                return Err(ApiError::bad_request(format!(
                    "{} can't use a {name}.",
                    task.label()
                )));
            }
        },
    };
    let mode = match &body.mode {
        None => None,
        Some(id) => match Mode::from_id(id).filter(|_| task.has_mode()) {
            Some(mode) => Some(mode),
            None => {
                return Err(ApiError::bad_request(format!(
                    "{} can't run {id}.",
                    task.label()
                )));
            }
        },
    };
    if let Some(enabled) = body.enabled {
        config::set_task_enabled(&state.db, task, enabled).await?;
    }
    if let Some(kind) = kind {
        config::set_task_kind(&state.db, task, kind).await?;
    }
    if let Some(mode) = mode {
        config::set_task_mode(&state.db, task, mode).await?;
    }
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct ChatScopes {
    scopes: Vec<String>,
}

/// What the chat may change: "organise" (categories, rules and budgets) and/or
/// "transactions". Looking things up is always allowed.
async fn choose_chat_scopes(
    State(state): State<AppState>,
    JsonBody(body): JsonBody<ChatScopes>,
) -> ApiResult<StatusCode> {
    let mut unknown: Vec<&str> = body
        .scopes
        .iter()
        .map(String::as_str)
        .filter(|s| !CHAT_SCOPES.contains(s))
        .collect();
    if !unknown.is_empty() {
        unknown.sort_unstable();
        unknown.dedup();
        return Err(ApiError::bad_request(format!(
            "Unknown scopes: {}",
            unknown.join(", ")
        )));
    }
    config::set_chat_scopes(&state.db, &body.scopes).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct Page {
    #[serde(default = "hundred")]
    limit: i64,
    #[serde(default)]
    offset: i64,
}

fn hundred() -> i64 {
    100
}

#[derive(Serialize)]
struct Logs {
    total: i64,
    logs: Vec<LogEntry>,
}

async fn list_logs(
    State(state): State<AppState>,
    QueryParams(page): QueryParams<Page>,
) -> ApiResult<Json<Logs>> {
    let (total, logs) =
        core_ai::logs(&state.db, page.limit.clamp(1, 1000), page.offset.max(0)).await?;
    Ok(Json(Logs { total, logs }))
}

async fn batch_exists(state: &AppState, id: i64) -> ApiResult<()> {
    let found: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM import_batches WHERE id = ?)")
            .bind(id)
            .fetch_one(&state.db)
            .await?;
    if found {
        Ok(())
    } else {
        Err(ApiError::not_found("Import batch not found"))
    }
}

async fn categorise(state: &AppState, batch: Option<i64>) -> ApiResult<Json<Value>> {
    let outcome = categorise::uncategorised(
        &state.db,
        state.http.as_ref(),
        &state.learned,
        batch,
        Source::Api,
        true,
    )
    .await
    .map_err(ApiError::internal)?;
    if let Some(error) = &outcome.error
        && outcome.total() == 0
    {
        return Err(ApiError::bad_request(error.clone()));
    }
    let mut response = json!({ "updated": outcome.total() });
    if outcome.learned > 0 {
        response["learned"] = json!(outcome.learned);
    }
    if let Some(error) = outcome.error {
        response["error"] = json!(error);
    }
    Ok(Json(response))
}

/// Categorises the uncategorised transactions in one import.
async fn categorise_batch(
    State(state): State<AppState>,
    PathParam(id): PathParam<i64>,
) -> ApiResult<Json<Value>> {
    batch_exists(&state, id).await?;
    categorise(&state, Some(id)).await
}

/// Categorises every uncategorised transaction.
async fn categorise_all(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    categorise(&state, None).await
}

/// Labels the recurring payments and scores the charges in an import, for when insights run
/// on demand rather than straight after each import.
async fn insights_batch(
    State(state): State<AppState>,
    PathParam(id): PathParam<i64>,
) -> ApiResult<Json<Value>> {
    batch_exists(&state, id).await?;
    match providers::resolve_task(&state.db, Task::Insights).await {
        Ok(_) => {}
        Err(ResolveError::Provider(e)) => return Err(ApiError::bad_request(e.message)),
        Err(ResolveError::Setting(e)) => return Err(e.into()),
    }
    if let Some(error) = ai_insights::run_after_import(&state.db, state.http.as_ref(), id).await? {
        return Err(ApiError::bad_request(error));
    }
    Ok(Json(json!({ "ok": true })))
}

/// Rules suggested from the transactions categorised by hand or by the model.
async fn suggest_rules(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    match ai_rules::suggest(&state.db, state.http.as_ref()).await {
        Ok(suggestions) => Ok(Json(json!({ "suggestions": suggestions }))),
        Err(SuggestError::Model(message)) => Err(ApiError::bad_request(message)),
        Err(SuggestError::Database(e)) => Err(e.into()),
    }
}

/// What the model has learned from, and how it does on that history.
async fn learned_status(State(state): State<AppState>) -> ApiResult<Json<Status>> {
    Ok(Json(state.learned.status(&state.db).await?))
}

#[derive(Deserialize)]
struct EnabledBody {
    enabled: bool,
}

async fn set_learned(
    State(state): State<AppState>,
    JsonBody(body): JsonBody<EnabledBody>,
) -> ApiResult<StatusCode> {
    learned::set_enabled(&state.db, body.enabled).await?;
    Ok(StatusCode::NO_CONTENT)
}
