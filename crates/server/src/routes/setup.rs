//! `/api/setup` (loading the UK chart of accounts and rules into a new install) and
//! `/api/lookup` (the lookup tables).

use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use pennychest_core::accounts;
use pennychest_core::lookups::{self, LookupItem};
use serde::Serialize;

use crate::AppState;
use crate::error::{ApiError, ApiResult};

pub fn setup_router() -> Router<AppState> {
    Router::new()
        .route("/status", get(status))
        .route("/seed", post(seed))
        .route("/sync-seed-icons", post(sync_seed_icons))
        .route("/seed-rules", post(seed_rules))
}

pub fn lookup_router() -> Router<AppState> {
    Router::new()
        .route("/match-types", get(match_types))
        .route("/categorisation-sources", get(categorisation_sources))
}

#[derive(Serialize)]
struct SetupStatus {
    has_accounts: bool,
    has_rules: bool,
}

async fn status(State(state): State<AppState>) -> ApiResult<Json<SetupStatus>> {
    let mut conn = state.db.acquire().await?;
    Ok(Json(SetupStatus {
        has_accounts: accounts::has_accounts(&mut conn).await?,
        has_rules: accounts::has_rules(&mut conn).await?,
    }))
}

#[derive(Serialize)]
struct Seeded {
    accounts_created: usize,
    rules_created: usize,
}

/// Loads the UK chart of accounts and its rules into an install that has no accounts yet.
async fn seed(State(state): State<AppState>) -> ApiResult<Json<Seeded>> {
    if accounts::has_accounts(&mut *state.db.acquire().await?).await? {
        return Err(ApiError::bad_request("Accounts already exist"));
    }
    Ok(Json(Seeded {
        accounts_created: accounts::load_seed_accounts(&state.db).await?,
        rules_created: accounts::load_seed_rules(&state.db).await?,
    }))
}

#[derive(Serialize)]
struct Done {
    ok: bool,
}

/// Fills in the seed icons on accounts that have none.
async fn sync_seed_icons(State(state): State<AppState>) -> ApiResult<Json<Done>> {
    accounts::load_seed_accounts(&state.db).await?;
    Ok(Json(Done { ok: true }))
}

#[derive(Serialize)]
struct RulesSeeded {
    rules_created: usize,
}

/// Loads just the UK rules, for an install with accounts but no rules.
async fn seed_rules(State(state): State<AppState>) -> ApiResult<Json<RulesSeeded>> {
    let mut conn = state.db.acquire().await?;
    if !accounts::has_accounts(&mut conn).await? {
        return Err(ApiError::bad_request("Load accounts first"));
    }
    if accounts::has_rules(&mut conn).await? {
        return Err(ApiError::bad_request("Rules already exist"));
    }
    drop(conn);
    Ok(Json(RulesSeeded {
        rules_created: accounts::load_seed_rules(&state.db).await?,
    }))
}

async fn match_types(State(state): State<AppState>) -> ApiResult<Json<Vec<LookupItem>>> {
    Ok(Json(lookups::match_types(&state.db).await?))
}

async fn categorisation_sources(State(state): State<AppState>) -> ApiResult<Json<Vec<LookupItem>>> {
    Ok(Json(lookups::categorisation_sources(&state.db).await?))
}
