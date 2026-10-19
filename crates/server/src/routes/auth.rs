//! `/api/auth`: setting the password, signing in and out, and the session check that guards
//! the rest of the API.

use std::net::SocketAddr;

use axum::extract::{ConnectInfo, FromRequestParts, Request, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use pennychest_core::auth;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::AppState;
use crate::error::{ApiError, ApiResult, ErrorBody, JsonBody};

pub const COOKIE_NAME: &str = "pennychest_session";

/// Reachable without signing in. Everything else under /api/ needs a session.
const PUBLIC_PATHS: &[&str] = &[
    "/api/health",
    "/api/openapi.json",
    "/api/auth/status",
    "/api/auth/setup",
    "/api/auth/login",
    "/api/auth/logout",
];
/// Called by the phone wallet shortcut, which authenticates with the tap token from Settings.
const TOKEN_AUTH_ROUTES: &[(&str, &str)] = &[("POST", "/api/taps")];

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/status", get(status))
        .route("/setup", post(setup))
        .route("/login", post(login))
        .route("/logout", post(logout))
        .route("/password", post(change_password))
}

/// Refuses requests to the API without a valid session. Runs only for routes that exist, so
/// an unknown path is still a 404.
pub async fn require_session(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    let path = request.uri().path();
    let method = request.method().as_str();
    let open = !path.starts_with("/api/")
        || PUBLIC_PATHS.contains(&path)
        || TOKEN_AUTH_ROUTES.contains(&(method, path));
    if open {
        return next.run(request).await;
    }
    match auth::is_valid_session(&state.db, session_cookie(request.headers())).await {
        Ok(true) => next.run(request).await,
        Ok(false) => ApiError::new(StatusCode::UNAUTHORIZED, "Not signed in").into_response(),
        Err(err) => ApiError::from(err).into_response(),
    }
}

#[derive(Serialize, ToSchema)]
pub(crate) struct AuthStatus {
    /// No password has been set yet, so the app shows the setup screen.
    setup_required: bool,
    /// The request came with a valid session.
    authenticated: bool,
}

#[derive(Deserialize, ToSchema)]
pub(crate) struct PasswordRequest {
    password: String,
}

#[derive(Deserialize, ToSchema)]
pub(crate) struct ChangePasswordRequest {
    current_password: String,
    /// At least 8 characters.
    new_password: String,
}

/// Whether a password has been set, and whether this request is signed in.
#[utoipa::path(get, path = "/api/auth/status", tag = "auth", responses(
    (status = 200, body = AuthStatus),
))]
pub(crate) async fn status(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> ApiResult<Json<AuthStatus>> {
    Ok(Json(AuthStatus {
        setup_required: !auth::password_is_set(&state.db).await?,
        authenticated: auth::is_valid_session(&state.db, session_cookie(&headers)).await?,
    }))
}

/// Sets the first password, and signs in.
#[utoipa::path(post, path = "/api/auth/setup", tag = "auth", request_body = PasswordRequest, responses(
        (status = 204, description = "Signed in", headers(
            ("Set-Cookie" = String, description = "The session cookie, `pennychest_session`"),
        )),
        (status = 400, description = "The password is too weak", body = ErrorBody),
        (status = 409, description = "A password has already been set", body = ErrorBody),
        (status = 422, description = "The body isn't the JSON described", body = ErrorBody),
))]
pub(crate) async fn setup(
    State(state): State<AppState>,
    headers: HeaderMap,
    JsonBody(body): JsonBody<PasswordRequest>,
) -> ApiResult<Response> {
    if auth::password_is_set(&state.db).await? {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "A password has already been set",
        ));
    }
    auth::check_strength(&body.password).map_err(ApiError::bad_request)?;
    let hash = blocking(move || auth::hash_password(&body.password)).await?;
    auth::PASSWORD.set(&state.db, &hash).await?;
    sign_in(&state, &headers).await
}

/// Signs in with the password.
#[utoipa::path(post, path = "/api/auth/login", tag = "auth", request_body = PasswordRequest, responses(
        (status = 204, description = "Signed in", headers(
            ("Set-Cookie" = String, description = "The session cookie, `pennychest_session`"),
        )),
        (status = 401, description = "Incorrect password", body = ErrorBody),
        (status = 409, description = "No password has been set yet", body = ErrorBody),
        (status = 422, description = "The body isn't the JSON described", body = ErrorBody),
        (status = 429, description = "Too many failed attempts", body = ErrorBody),
))]
pub(crate) async fn login(
    State(state): State<AppState>,
    headers: HeaderMap,
    ClientAddr(address): ClientAddr,
    JsonBody(body): JsonBody<PasswordRequest>,
) -> ApiResult<Response> {
    // Behind a reverse proxy this is the proxy's address, so the limit becomes global.
    let client = address.map_or_else(|| "unknown".to_owned(), |a| a.ip().to_string());
    if state.login_throttle.is_blocked(&client) {
        return Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "Too many failed attempts. Try again later.",
        ));
    }
    let Some(stored) = auth::PASSWORD.get(&state.db).await? else {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "No password has been set yet",
        ));
    };
    let matches = blocking(move || auth::verify_password(&body.password, &stored)).await?;
    if !matches {
        state.login_throttle.record_failure(&client);
        return Err(ApiError::new(
            StatusCode::UNAUTHORIZED,
            "Incorrect password",
        ));
    }
    state.login_throttle.reset(&client);
    sign_in(&state, &headers).await
}

/// Signs out, ending this session.
#[utoipa::path(post, path = "/api/auth/logout", tag = "auth", responses(
    (status = 204, description = "Signed out, and the cookie cleared"),
))]
pub(crate) async fn logout(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> ApiResult<Response> {
    if let Some(token) = session_cookie(&headers) {
        auth::end_session(&state.db, token).await?;
    }
    let mut response = StatusCode::NO_CONTENT.into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_static(
            "pennychest_session=\"\"; expires=Thu, 01 Jan 1970 00:00:00 GMT; Max-Age=0; Path=/; \
             SameSite=lax",
        ),
    );
    Ok(response)
}

/// Changes the password, and signs out every other device.
#[utoipa::path(post, path = "/api/auth/password", tag = "auth", request_body = ChangePasswordRequest, responses(
    (status = 204, description = "Changed"),
    (status = 400, description = "The current password is wrong, or the new one too weak", body = ErrorBody),
    (status = 422, description = "The body isn't the JSON described", body = ErrorBody),
))]
pub(crate) async fn change_password(
    State(state): State<AppState>,
    headers: HeaderMap,
    JsonBody(body): JsonBody<ChangePasswordRequest>,
) -> ApiResult<StatusCode> {
    let stored = auth::PASSWORD.get(&state.db).await?;
    let current = body.current_password;
    let correct = match stored {
        Some(stored) => blocking(move || auth::verify_password(&current, &stored)).await?,
        None => false,
    };
    if !correct {
        return Err(ApiError::bad_request("Current password is incorrect"));
    }
    auth::check_strength(&body.new_password).map_err(ApiError::bad_request)?;
    let new_password = body.new_password;
    let hash = blocking(move || auth::hash_password(&new_password)).await?;

    let mut tx = state.db.begin().await?;
    auth::PASSWORD.set(&mut *tx, &hash).await?;
    // Sign out every other device; this one keeps its session.
    let token = session_cookie(&headers).unwrap_or_default();
    auth::end_other_sessions(&mut *tx, token).await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

/// The client's address, when the server was started with it (not in tests).
pub(crate) struct ClientAddr(Option<SocketAddr>);

impl<S: Send + Sync> FromRequestParts<S> for ClientAddr {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
        Ok(Self(
            parts
                .extensions
                .get::<ConnectInfo<SocketAddr>>()
                .map(|info| info.0),
        ))
    }
}

/// Starts a session and answers 204 with its cookie.
async fn sign_in(state: &AppState, headers: &HeaderMap) -> ApiResult<Response> {
    let token = auth::create_session(&state.db).await?;
    let secure = if forwarded_https(headers) {
        "; Secure"
    } else {
        ""
    };
    let cookie = format!(
        "{COOKIE_NAME}={token}; HttpOnly; Max-Age={}; Path=/; SameSite=lax{secure}",
        auth::SESSION_TTL.as_secs()
    );
    let mut response = StatusCode::NO_CONTENT.into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&cookie).map_err(ApiError::internal)?,
    );
    Ok(response)
}

/// Whether the browser reached us over HTTPS through a proxy (Fly, Render, Caddy...), so the
/// cookie should only ever be sent over HTTPS.
fn forwarded_https(headers: &HeaderMap) -> bool {
    headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .is_some_and(|proto| proto.trim().eq_ignore_ascii_case("https"))
}

/// The session token from the request's cookies.
pub fn session_cookie(headers: &HeaderMap) -> Option<&str> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(name, _)| *name == COOKIE_NAME)
        .map(|(_, value)| value)
}

/// Runs slow, CPU-bound work (password hashing) without holding up other requests.
async fn blocking<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> ApiResult<T> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(ApiError::internal)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_session_cookie_among_others() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_static("theme=dark; pennychest_session=abc; other=1"),
        );
        assert_eq!(session_cookie(&headers), Some("abc"));
        assert_eq!(session_cookie(&HeaderMap::new()), None);
    }

    #[test]
    fn notices_https_through_a_proxy() {
        let mut headers = HeaderMap::new();
        assert!(!forwarded_https(&headers));
        headers.insert("x-forwarded-proto", HeaderValue::from_static("https, http"));
        assert!(forwarded_https(&headers));
        headers.insert("x-forwarded-proto", HeaderValue::from_static("http"));
        assert!(!forwarded_https(&headers));
    }
}
