//! OAuth 2.1 for the MCP server, so web connectors (claude.ai, ChatGPT) can sign in.
//!
//! Follows the MCP authorization spec: protected resource metadata (RFC 9728) points connectors
//! at this server's authorization server metadata (RFC 8414); they register themselves (RFC
//! 7591), send the user to PennyChest's consent page, and exchange the code for tokens with
//! PKCE (S256). Access tokens last an hour and are renewed with a refresh token that rotates on
//! every use.

use std::collections::HashMap;

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use subtle::ConstantTimeEq;
use url::Url;

use super::tokens::{WRITE_SCOPES, new_access_token, prefix, write_scopes};
use crate::auth::{now, random_token, token_hash};

pub const ACCESS_TOKEN_SECONDS: i64 = 60 * 60;
pub const CODE_SECONDS: i64 = 10 * 60;
pub const MAX_CLIENTS: i64 = 200;
const AUTH_METHODS: [&str; 3] = ["none", "client_secret_post", "client_secret_basic"];

fn scopes_supported() -> Vec<&'static str> {
    let mut scopes = vec!["read"];
    scopes.extend(WRITE_SCOPES.iter().map(|s| s.name()));
    scopes
}

/// RFC 9728, for the MCP server at `{base}/mcp`.
pub fn protected_resource_metadata(base: &str) -> Value {
    json!({
        "resource": format!("{base}/mcp"),
        "authorization_servers": [base],
        "scopes_supported": scopes_supported(),
        "bearer_methods_supported": ["header"],
        "resource_name": "PennyChest",
    })
}

/// RFC 8414.
pub fn authorization_server_metadata(base: &str) -> Value {
    json!({
        "issuer": base,
        "authorization_endpoint": format!("{base}/oauth/authorize"),
        "token_endpoint": format!("{base}/oauth/token"),
        "registration_endpoint": format!("{base}/oauth/register"),
        "revocation_endpoint": format!("{base}/oauth/revoke"),
        "scopes_supported": scopes_supported(),
        "response_types_supported": ["code"],
        "grant_types_supported": ["authorization_code", "refresh_token"],
        "code_challenge_methods_supported": ["S256"],
        "token_endpoint_auth_methods_supported": AUTH_METHODS,
    })
}

/// An OAuth error, as the spec has endpoints report it: `{"error", "error_description"}`.
#[derive(Debug, thiserror::Error)]
pub enum OAuthError {
    #[error("{error}: {description}")]
    Refused {
        error: &'static str,
        description: String,
        status: u16,
    },
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

impl OAuthError {
    fn new(error: &'static str, description: impl Into<String>) -> Self {
        Self::Refused {
            error,
            description: description.into(),
            status: 400,
        }
    }

    fn with_status(error: &'static str, description: impl Into<String>, status: u16) -> Self {
        Self::Refused {
            error,
            description: description.into(),
            status,
        }
    }

    fn invalid_client() -> Self {
        Self::with_status("invalid_client", "Unknown client or wrong secret.", 401)
    }

    fn invalid_grant(description: &str) -> Self {
        Self::new("invalid_grant", description)
    }
}

// Dynamic client registration

fn valid_redirect(uri: &str) -> bool {
    let Ok(url) = Url::parse(uri) else {
        return false;
    };
    if url.fragment().is_some() || url.host_str().is_none_or(str::is_empty) {
        return false;
    }
    match url.scheme() {
        "https" => true,
        // Plain http only back to the user's own machine (desktop and CLI clients).
        "http" => matches!(
            url.host_str(),
            Some("localhost" | "127.0.0.1" | "[::1]" | "::1")
        ),
        _ => false,
    }
}

#[derive(Debug, Deserialize)]
pub struct Registration {
    pub redirect_uris: Vec<String>,
    #[serde(default)]
    pub client_name: Option<String>,
    #[serde(default)]
    pub token_endpoint_auth_method: Option<String>,
}

/// Registers a connector; what it needs to know, including its secret if it asked for one.
pub async fn register(db: &SqlitePool, registration: Registration) -> Result<Value, OAuthError> {
    let uris = &registration.redirect_uris;
    if uris.is_empty() || !uris.iter().all(|u| valid_redirect(u)) {
        return Err(OAuthError::new(
            "invalid_redirect_uri",
            "Redirect URIs must be https, or http to localhost.",
        ));
    }
    let method = registration
        .token_endpoint_auth_method
        .as_deref()
        .unwrap_or("none");
    if !AUTH_METHODS.contains(&method) {
        return Err(OAuthError::new(
            "invalid_client_metadata",
            format!("Unsupported auth method '{method}'."),
        ));
    }

    let mut tx = db.begin().await?;
    // Registration is open, so keep it from growing without bound: clients that registered
    // but never connected are dropped after a day.
    sqlx::query(
        "DELETE FROM oauth_clients
         WHERE created_at < datetime('now', '-1 day')
           AND client_id NOT IN
               (SELECT client_id FROM access_tokens WHERE client_id IS NOT NULL)",
    )
    .execute(&mut *tx)
    .await?;
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM oauth_clients")
        .fetch_one(&mut *tx)
        .await?;
    if count >= MAX_CLIENTS {
        return Err(OAuthError::with_status(
            "temporarily_unavailable",
            "Too many registered clients.",
            503,
        ));
    }

    let client_id = format!("pcc_{}", random_token(16));
    let secret = (method != "none").then(|| random_token(32));
    let name = registration
        .client_name
        .as_deref()
        .unwrap_or("An MCP client")
        .trim();
    let name: String = name.chars().take(80).collect();
    sqlx::query(
        "INSERT INTO oauth_clients (client_id, client_secret_hash, client_name, redirect_uris)
         VALUES (?, ?, ?, ?)",
    )
    .bind(&client_id)
    .bind(secret.as_deref().map(token_hash))
    .bind(&name)
    .bind(serde_json::to_string(uris).expect("JSON serialises"))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    let mut response = json!({
        "client_id": client_id,
        "client_id_issued_at": now(),
        "client_name": name,
        "redirect_uris": uris,
        "token_endpoint_auth_method": method,
        "grant_types": ["authorization_code", "refresh_token"],
        "response_types": ["code"],
    });
    if let Some(secret) = secret {
        response["client_secret"] = json!(secret);
        response["client_secret_expires_at"] = json!(0);
    }
    Ok(response)
}

// The consent page (the frontend's /oauth/authorize) uses these, signed in as the user.

#[derive(Debug, thiserror::Error)]
pub enum ConsentError {
    #[error("This app isn't registered with PennyChest.")]
    UnknownClient,
    #[error("This app's return address doesn't match.")]
    WrongRedirect,
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

struct Client {
    client_id: String,
    client_name: String,
    secret_hash: Option<String>,
    redirect_uris: Vec<String>,
}

async fn find_client(db: &SqlitePool, client_id: &str) -> Result<Option<Client>, sqlx::Error> {
    let row: Option<(String, String, Option<String>, String)> = sqlx::query_as(
        "SELECT client_id, client_name, client_secret_hash, redirect_uris FROM oauth_clients
         WHERE client_id = ?",
    )
    .bind(client_id)
    .fetch_optional(db)
    .await?;
    Ok(
        row.map(|(client_id, client_name, secret_hash, uris)| Client {
            client_id,
            client_name,
            secret_hash,
            redirect_uris: serde_json::from_str(&uris).unwrap_or_default(),
        }),
    )
}

async fn client_for(
    db: &SqlitePool,
    client_id: &str,
    redirect_uri: &str,
) -> Result<Client, ConsentError> {
    let client = find_client(db, client_id)
        .await?
        .ok_or(ConsentError::UnknownClient)?;
    if !client.redirect_uris.iter().any(|u| u == redirect_uri) {
        return Err(ConsentError::WrongRedirect);
    }
    Ok(client)
}

/// Who's asking, for the consent page: the app's name and where it will send the user back.
pub async fn authorize_info(
    db: &SqlitePool,
    client_id: &str,
    redirect_uri: &str,
) -> Result<Value, ConsentError> {
    let client = client_for(db, client_id, redirect_uri).await?;
    let host = Url::parse(redirect_uri)
        .ok()
        .map_or_else(String::new, |url| {
            let host = url.host_str().unwrap_or_default().to_owned();
            url.port()
                .map_or(host.clone(), |port| format!("{host}:{port}"))
        });
    Ok(json!({"client_name": client.client_name, "redirect_host": host}))
}

/// The user's answer on the consent page.
#[derive(Debug, Deserialize)]
pub struct Decision {
    pub client_id: String,
    pub redirect_uri: String,
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub code_challenge: Option<String>,
    #[serde(default)]
    pub code_challenge_method: Option<String>,
    #[serde(default = "code")]
    pub response_type: String,
    #[serde(default)]
    pub scopes: Vec<String>,
}

fn code() -> String {
    "code".into()
}

/// Where to send the user back: `uri` with these parameters and the issuer, `base`.
fn redirect(base: &str, uri: &str, params: &[(&str, Option<&str>)]) -> String {
    let mut query = url::form_urlencoded::Serializer::new(String::new());
    for (key, value) in params {
        if let Some(value) = value {
            query.append_pair(key, value);
        }
    }
    query.append_pair("iss", base);
    let separator = if Url::parse(uri).is_ok_and(|u| u.query().is_some()) {
        '&'
    } else {
        '?'
    };
    format!("{uri}{separator}{}", query.finish())
}

/// The user allowed the connector: a code for it to exchange, in the address to send them back
/// to (or the problem with the request, there instead).
pub async fn approve(
    db: &SqlitePool,
    base: &str,
    decision: Decision,
) -> Result<String, ConsentError> {
    let client = client_for(db, &decision.client_id, &decision.redirect_uri).await?;
    let state = decision.state.as_deref();
    let uri = &decision.redirect_uri;
    if decision.response_type != "code" {
        let params = [
            ("error", Some("unsupported_response_type")),
            ("state", state),
        ];
        return Ok(redirect(base, uri, &params));
    }
    let challenge = match (
        &decision.code_challenge,
        decision.code_challenge_method.as_deref(),
    ) {
        (Some(challenge), Some("S256")) if !challenge.is_empty() => challenge,
        _ => {
            let params = [
                ("error", Some("invalid_request")),
                ("error_description", Some("PKCE (S256) is required.")),
                ("state", state),
            ];
            return Ok(redirect(base, uri, &params));
        }
    };
    let code = random_token(32);
    sqlx::query(
        "INSERT INTO oauth_codes (code_hash, client_id, redirect_uri, code_challenge, scopes,
                                  expires_at)
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(token_hash(&code))
    .bind(&client.client_id)
    .bind(uri)
    .bind(challenge)
    .bind(serde_json::to_string(&write_scopes(&decision.scopes)).expect("JSON serialises"))
    .bind(now() + CODE_SECONDS)
    .execute(db)
    .await?;
    Ok(redirect(
        base,
        uri,
        &[("code", Some(&code)), ("state", state)],
    ))
}

/// The user said no: the address to send them back to, saying so.
pub async fn deny(db: &SqlitePool, base: &str, decision: Decision) -> Result<String, ConsentError> {
    client_for(db, &decision.client_id, &decision.redirect_uri).await?;
    let params = [
        ("error", Some("access_denied")),
        ("state", decision.state.as_deref()),
    ];
    Ok(redirect(base, &decision.redirect_uri, &params))
}

// Tokens

/// The client a token or revocation request comes from, by `client_id` and `client_secret`
/// in the form, or HTTP Basic authentication.
async fn authenticate_client(
    db: &SqlitePool,
    form: &HashMap<String, String>,
    authorization: Option<&str>,
) -> Result<Client, OAuthError> {
    let (mut client_id, mut secret) = (
        form.get("client_id").cloned(),
        form.get("client_secret").cloned(),
    );
    if let Some(basic) = authorization.and_then(|a| {
        a.get(..6)
            .filter(|scheme| scheme.eq_ignore_ascii_case("basic "))
            .map(|_| &a[6..])
    }) {
        let decoded = STANDARD
            .decode(basic.trim())
            .ok()
            .and_then(|d| String::from_utf8(d).ok())
            .ok_or_else(OAuthError::invalid_client)?;
        let (id, pass) = decoded
            .split_once(':')
            .ok_or_else(OAuthError::invalid_client)?;
        (client_id, secret) = (Some(id.to_owned()), Some(pass.to_owned()));
    }
    let client_id = client_id
        .filter(|id| !id.is_empty())
        .ok_or_else(OAuthError::invalid_client)?;
    let client = find_client(db, &client_id)
        .await?
        .ok_or_else(OAuthError::invalid_client)?;
    if let Some(expected) = &client.secret_hash {
        let matches = secret
            .is_some_and(|s| bool::from(token_hash(&s).as_bytes().ct_eq(expected.as_bytes())));
        if !matches {
            return Err(OAuthError::invalid_client());
        }
    }
    Ok(client)
}

/// New access and refresh tokens for the connector's token row, replacing any it had.
async fn issue(db: &SqlitePool, token_id: i64, scopes: &[String]) -> Result<Value, sqlx::Error> {
    let access = new_access_token();
    let refresh = format!("pcr_{}", random_token(32));
    sqlx::query(
        "UPDATE access_tokens SET token_hash = ?, prefix = ?, refresh_hash = ?, expires_at = ?
         WHERE id = ?",
    )
    .bind(token_hash(&access))
    .bind(prefix(&access))
    .bind(token_hash(&refresh))
    .bind(now() + ACCESS_TOKEN_SECONDS)
    .bind(token_id)
    .execute(db)
    .await?;
    let mut scope = vec!["read".to_owned()];
    scope.extend(scopes.iter().cloned());
    Ok(json!({
        "access_token": access,
        "token_type": "Bearer",
        "expires_in": ACCESS_TOKEN_SECONDS,
        "refresh_token": refresh,
        "scope": scope.join(" "),
    }))
}

fn pkce_matches(verifier: &str, challenge: &str) -> bool {
    let expected = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    bool::from(expected.as_bytes().ct_eq(challenge.as_bytes()))
}

/// The token endpoint: exchanges a code, or a refresh token, for tokens.
pub async fn token(
    db: &SqlitePool,
    form: &HashMap<String, String>,
    authorization: Option<&str>,
) -> Result<Value, OAuthError> {
    let client = authenticate_client(db, form, authorization).await?;
    let field = |name: &str| form.get(name).map(String::as_str).filter(|v| !v.is_empty());
    match form.get("grant_type").map(String::as_str) {
        Some("authorization_code") => {
            let (Some(code), Some(verifier)) = (field("code"), field("code_verifier")) else {
                return Err(OAuthError::new(
                    "invalid_request",
                    "code and code_verifier are required.",
                ));
            };
            let mut tx = db.begin().await?;
            let grant: Option<(i64, String, String, String, String, i64, bool)> = sqlx::query_as(
                "SELECT id, client_id, redirect_uri, code_challenge, scopes, expires_at, used
                 FROM oauth_codes WHERE code_hash = ?",
            )
            .bind(token_hash(code))
            .fetch_optional(&mut *tx)
            .await?;
            let refused = "The code is invalid, expired or already used.";
            let Some((id, grant_client, grant_redirect, challenge, scopes, expires_at, used)) =
                grant
            else {
                return Err(OAuthError::invalid_grant(refused));
            };
            if used
                || grant_client != client.client_id
                || expires_at < now()
                || field("redirect_uri").is_some_and(|r| r != grant_redirect)
                || !pkce_matches(verifier, &challenge)
            {
                return Err(OAuthError::invalid_grant(refused));
            }
            sqlx::query("UPDATE oauth_codes SET used = 1 WHERE id = ?")
                .bind(id)
                .execute(&mut *tx)
                .await?;
            // Codes are single use and short-lived; tidy up the spent ones.
            sqlx::query("DELETE FROM oauth_codes WHERE expires_at < ?")
                .bind(now())
                .execute(&mut *tx)
                .await?;
            let token_id: i64 = sqlx::query_scalar(
                "INSERT INTO access_tokens (name, token_hash, prefix, scopes, client_id)
                 VALUES (?, ?, '', ?, ?) RETURNING id",
            )
            .bind(&client.client_name)
            // Unique until it's issued, just below.
            .bind(format!("pending-{}", random_token(16)))
            .bind(&scopes)
            .bind(&client.client_id)
            .fetch_one(&mut *tx)
            .await?;
            tx.commit().await?;
            let scopes: Vec<String> = serde_json::from_str(&scopes).unwrap_or_default();
            Ok(issue(db, token_id, &scopes).await?)
        }
        Some("refresh_token") => {
            let Some(refresh) = field("refresh_token") else {
                return Err(OAuthError::new(
                    "invalid_request",
                    "refresh_token is required.",
                ));
            };
            let found: Option<(i64, Option<String>, String)> = sqlx::query_as(
                "SELECT id, client_id, scopes FROM access_tokens WHERE refresh_hash = ?",
            )
            .bind(token_hash(refresh))
            .fetch_optional(db)
            .await?;
            match found {
                Some((id, Some(owner), scopes)) if owner == client.client_id => {
                    let scopes: Vec<String> = serde_json::from_str(&scopes).unwrap_or_default();
                    Ok(issue(db, id, &scopes).await?)
                }
                _ => Err(OAuthError::invalid_grant(
                    "The refresh token is invalid or was revoked.",
                )),
            }
        }
        other => Err(OAuthError::new(
            "unsupported_grant_type",
            format!("Unsupported grant type '{}'.", other.unwrap_or_default()),
        )),
    }
}

/// Revokes one of the client's tokens, access or refresh, and so its connection. The answer is
/// the same whether or not the token existed (RFC 7009).
pub async fn revoke(
    db: &SqlitePool,
    form: &HashMap<String, String>,
    authorization: Option<&str>,
) -> Result<(), OAuthError> {
    let client = authenticate_client(db, form, authorization).await?;
    let Some(token) = form.get("token") else {
        return Err(OAuthError::new("invalid_request", "token is required."));
    };
    let hashed = token_hash(token);
    sqlx::query(
        "DELETE FROM access_tokens
         WHERE client_id = ? AND (token_hash = ?2 OR refresh_hash = ?2)",
    )
    .bind(&client.client_id)
    .bind(&hashed)
    .execute(db)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redirects_are_https_or_back_to_this_machine() {
        assert!(valid_redirect("https://claude.ai/api/mcp/auth_callback"));
        assert!(valid_redirect("http://127.0.0.1:33418/callback"));
        assert!(valid_redirect("http://localhost/cb"));
        assert!(valid_redirect("http://[::1]:8000/cb"));
        assert!(!valid_redirect("http://evil.example/cb"));
        assert!(!valid_redirect("https://claude.ai/cb#frag"));
        assert!(!valid_redirect("not a url"));
        assert!(!valid_redirect("ftp://claude.ai/cb"));
    }

    #[test]
    fn redirects_add_to_the_address_and_name_the_issuer() {
        assert_eq!(
            redirect(
                "https://pc.example",
                "https://a.example/cb",
                &[("code", Some("c 1")), ("state", None)]
            ),
            "https://a.example/cb?code=c+1&iss=https%3A%2F%2Fpc.example"
        );
        assert_eq!(
            redirect(
                "http://x",
                "https://a.example/cb?v=1",
                &[("error", Some("access_denied"))]
            ),
            "https://a.example/cb?v=1&error=access_denied&iss=http%3A%2F%2Fx"
        );
    }

    #[test]
    fn pkce_checks_the_verifier_against_the_challenge() {
        // RFC 7636's example.
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        assert!(pkce_matches(
            verifier,
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        ));
        assert!(!pkce_matches(
            "wrong",
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        ));
    }
}
