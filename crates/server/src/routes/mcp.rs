//! The MCP server at `/mcp`, the OAuth endpoints connectors use to sign in, and the signed-in
//! user's side of both: access tokens (`/api/mcp/tokens`) and the consent page's
//! `/api/oauth`.

use std::collections::HashMap;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use pennychest_core::mcp::oauth::{self, ConsentError, Decision, OAuthError, Registration};
use pennychest_core::mcp::tokens::{self, Token, TokenError};
use pennychest_core::mcp::{self, Reply};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::AppState;
use crate::error::{ApiError, ApiResult, JsonBody, PathParam, QueryParams};

/// Outside `/api`: agents and connectors sign in with tokens, not a session.
pub fn public_router() -> Router<AppState> {
    Router::new()
        .route("/mcp", post(endpoint).get(no_stream).delete(no_stream))
        .route(
            "/.well-known/oauth-protected-resource",
            get(resource_metadata),
        )
        .route(
            "/.well-known/oauth-protected-resource/mcp",
            get(resource_metadata),
        )
        .route(
            "/.well-known/oauth-authorization-server",
            get(server_metadata),
        )
        .route("/oauth/register", post(register))
        .route("/oauth/token", post(token))
        .route("/oauth/revoke", post(revoke))
}

/// `/api/mcp/tokens`, managed from Settings.
pub fn tokens_router() -> Router<AppState> {
    Router::new()
        .route("/", get(list_tokens).post(add_token))
        .route("/{id}", delete(delete_token))
}

/// `/api/oauth`, for the consent page.
pub fn consent_router() -> Router<AppState> {
    Router::new()
        .route("/authorize-info", get(authorize_info))
        .route("/approve", post(approve))
        .route("/deny", post(deny))
}

/// This server's public address, e.g. `https://pennychest.example`. Behind a proxy (Fly) the
/// app itself is reached over http, so the scheme comes from `X-Forwarded-Proto`. Only the
/// scheme is taken from it: forwarded client addresses aren't trusted.
fn public_base(headers: &HeaderMap) -> String {
    let host = headers
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("localhost");
    let proto = headers
        .get("x-forwarded-proto")
        .and_then(|p| p.to_str().ok())
        .and_then(|p| p.split(',').next())
        .map(str::trim)
        .filter(|p| ["http", "https"].contains(p))
        .unwrap_or("http");
    format!("{proto}://{host}")
}

fn metadata_url(headers: &HeaderMap) -> String {
    format!(
        "{}/.well-known/oauth-protected-resource/mcp",
        public_base(headers)
    )
}

fn no_store(status: StatusCode, body: Value) -> Response {
    let mut response = (status, Json(body)).into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

fn oauth_response(result: Result<Value, OAuthError>, ok: StatusCode) -> Response {
    match result {
        Ok(body) => no_store(ok, body),
        Err(OAuthError::Refused {
            error,
            description,
            status,
        }) => no_store(
            StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_REQUEST),
            json!({"error": error, "error_description": description}),
        ),
        Err(OAuthError::Database(e)) => ApiError::internal(e).into_response(),
    }
}

// The MCP server

/// Points OAuth connectors at the discovery document (RFC 9728) so they can sign in.
fn unauthorised(headers: &HeaderMap) -> Response {
    let mut response = (
        StatusCode::UNAUTHORIZED,
        Json(json!({
            "error": "invalid_token",
            "error_description": "Use a PennyChest access token.",
        })),
    )
        .into_response();
    let challenge = format!(
        r#"Bearer realm="PennyChest", resource_metadata="{}""#,
        metadata_url(headers)
    );
    if let Ok(value) = HeaderValue::from_str(&challenge) {
        response
            .headers_mut()
            .insert(header::WWW_AUTHENTICATE, value);
    }
    response
}

async fn endpoint(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    let bearer = headers
        .get(header::AUTHORIZATION)
        .and_then(|a| a.to_str().ok())
        .and_then(|a| {
            a.get(..7)
                .filter(|scheme| scheme.eq_ignore_ascii_case("bearer "))
                .map(|_| a[7..].trim())
        });
    let token = match bearer {
        Some(bearer) => match tokens::find(&state.db, bearer).await {
            Ok(token) => token,
            Err(e) => return ApiError::internal(e).into_response(),
        },
        None => None,
    };
    let Some(token) = token else {
        return unauthorised(&headers);
    };
    match mcp::handle(&state.db, &token.scopes, &body, &public_base(&headers)).await {
        Ok(Reply { status, body }) => {
            let status = StatusCode::from_u16(status).unwrap_or(StatusCode::OK);
            match body {
                Some(body) => (status, Json(body)).into_response(),
                None => status.into_response(),
            }
        }
        Err(e) => ApiError::internal(e).into_response(),
    }
}

/// No server-initiated messages or sessions, so there's no stream to open or close.
async fn no_stream() -> Response {
    (StatusCode::METHOD_NOT_ALLOWED, [(header::ALLOW, "POST")]).into_response()
}

// Discovery and the OAuth endpoints

async fn resource_metadata(headers: HeaderMap) -> Json<Value> {
    Json(oauth::protected_resource_metadata(&public_base(&headers)))
}

async fn server_metadata(headers: HeaderMap) -> Json<Value> {
    Json(oauth::authorization_server_metadata(&public_base(&headers)))
}

async fn register(
    State(state): State<AppState>,
    JsonBody(registration): JsonBody<Registration>,
) -> Response {
    oauth_response(
        oauth::register(&state.db, registration).await,
        StatusCode::CREATED,
    )
}

fn form(body: &[u8]) -> HashMap<String, String> {
    url::form_urlencoded::parse(body).into_owned().collect()
}

fn authorization(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|a| a.to_str().ok())
}

async fn token(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    let result = oauth::token(&state.db, &form(&body), authorization(&headers)).await;
    oauth_response(result, StatusCode::OK)
}

async fn revoke(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    let result = oauth::revoke(&state.db, &form(&body), authorization(&headers)).await;
    oauth_response(result.map(|()| json!({})), StatusCode::OK)
}

// The consent page

impl From<ConsentError> for ApiError {
    fn from(err: ConsentError) -> Self {
        match err {
            ConsentError::Database(e) => Self::internal(e),
            other => Self::bad_request(other.to_string()),
        }
    }
}

#[derive(Deserialize)]
struct Asking {
    client_id: String,
    redirect_uri: String,
}

async fn authorize_info(
    State(state): State<AppState>,
    QueryParams(asking): QueryParams<Asking>,
) -> ApiResult<Json<Value>> {
    Ok(Json(
        oauth::authorize_info(&state.db, &asking.client_id, &asking.redirect_uri).await?,
    ))
}

#[derive(Serialize)]
struct SendBack {
    redirect: String,
}

async fn approve(
    State(state): State<AppState>,
    headers: HeaderMap,
    JsonBody(decision): JsonBody<Decision>,
) -> ApiResult<Json<SendBack>> {
    let redirect = oauth::approve(&state.db, &public_base(&headers), decision).await?;
    Ok(Json(SendBack { redirect }))
}

async fn deny(
    State(state): State<AppState>,
    headers: HeaderMap,
    JsonBody(decision): JsonBody<Decision>,
) -> ApiResult<Json<SendBack>> {
    let redirect = oauth::deny(&state.db, &public_base(&headers), decision).await?;
    Ok(Json(SendBack { redirect }))
}

// Access tokens

impl From<TokenError> for ApiError {
    fn from(err: TokenError) -> Self {
        match err {
            TokenError::Database(e) => Self::internal(e),
            other => Self::bad_request(other.to_string()),
        }
    }
}

#[derive(Serialize)]
struct Tokens {
    tokens: Vec<Token>,
}

async fn list_tokens(State(state): State<AppState>) -> ApiResult<Json<Tokens>> {
    Ok(Json(Tokens {
        tokens: tokens::list(&state.db).await?,
    }))
}

#[derive(Deserialize)]
struct NewToken {
    name: String,
    #[serde(default)]
    scopes: Vec<String>,
}

async fn add_token(
    State(state): State<AppState>,
    JsonBody(body): JsonBody<NewToken>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let (token, raw) = tokens::create(&state.db, &body.name, &body.scopes).await?;
    let mut created = serde_json::to_value(token).expect("JSON serialises");
    // The only time the token itself is returned.
    created["token"] = json!(raw);
    Ok((StatusCode::CREATED, Json(created)))
}

async fn delete_token(
    State(state): State<AppState>,
    PathParam(id): PathParam<i64>,
) -> ApiResult<StatusCode> {
    if tokens::delete(&state.db, id).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::not_found("Token not found"))
    }
}
