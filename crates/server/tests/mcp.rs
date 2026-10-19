//! The MCP server, its access tokens, and OAuth for connectors.

mod common;

use std::collections::HashMap;

use axum::http::{Method, StatusCode};
use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use common::{TestApp, TestResponse};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const REDIRECT: &str = "https://claude.ai/api/mcp/auth_callback";
const HOST: (&str, &str) = ("host", "testserver");

/// A signed-in app with the accounts the actions need.
async fn ledger() -> TestApp {
    let mut app = TestApp::signed_in().await;
    for path in ["Expenses", "Expenses:Food"] {
        let name = path.rsplit(':').next().unwrap();
        app.post(
            "/api/accounts",
            json!({"name": name, "full_path": path, "type": "expense"}),
        )
        .await
        .assert_status(StatusCode::CREATED);
    }
    app
}

async fn new_token(app: &mut TestApp, scopes: &[&str]) -> Value {
    let response = app
        .post(
            "/api/mcp/tokens",
            json!({"name": "Claude Code", "scopes": scopes}),
        )
        .await;
    response.assert_status(StatusCode::CREATED);
    response.body
}

async fn rpc(app: &mut TestApp, token: &str, method: &str, params: Option<Value>) -> TestResponse {
    let mut body = json!({"jsonrpc": "2.0", "id": 1, "method": method});
    if let Some(params) = params {
        body["params"] = params;
    }
    let bearer = format!("Bearer {token}");
    app.send_with_headers(
        Method::POST,
        "/mcp",
        Some(body),
        &[
            ("authorization", &bearer),
            ("accept", "application/json, text/event-stream"),
            HOST,
        ],
    )
    .await
}

async fn call(app: &mut TestApp, token: &str, tool: &str, arguments: Value) -> Value {
    let response = rpc(
        app,
        token,
        "tools/call",
        Some(json!({"name": tool, "arguments": arguments})),
    )
    .await;
    response.assert_status(StatusCode::OK);
    response.body["result"].clone()
}

fn tool_names(response: &TestResponse) -> Vec<String> {
    response.body["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_owned())
        .collect()
}

// Tokens

#[tokio::test]
async fn tokens_are_shown_once_and_stored_hashed() {
    let mut app = TestApp::signed_in().await;
    let created = new_token(&mut app, &["organise"]).await;
    let token = created["token"].as_str().unwrap();
    assert!(token.starts_with("pc_"));
    assert_eq!(created["prefix"], token[..10]);
    assert_eq!(created["scopes"], json!(["organise"]));
    assert_eq!(created["kind"], "token");

    let listed = app.get("/api/mcp/tokens").await.body["tokens"].clone();
    assert_eq!(listed.as_array().unwrap().len(), 1);
    assert!(listed[0].get("token").is_none());
    assert_eq!(listed[0]["last_used_at"], Value::Null);
    let stored: String = sqlx::query_scalar("SELECT token_hash FROM access_tokens")
        .fetch_one(&app.db)
        .await
        .unwrap();
    assert!(!stored.contains(token));
}

#[tokio::test]
async fn token_validation() {
    let mut app = TestApp::signed_in().await;
    app.post("/api/mcp/tokens", json!({"name": " ", "scopes": []}))
        .await
        .assert_status(StatusCode::BAD_REQUEST);
    app.post("/api/mcp/tokens", json!({"name": "x", "scopes": ["admin"]}))
        .await
        .assert_status(StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn token_management_needs_a_session() {
    let app = TestApp::signed_in().await;
    let mut anon = app.other_device();
    anon.get("/api/mcp/tokens")
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
    anon.post("/api/mcp/tokens", json!({"name": "x"}))
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn revoked_tokens_stop_working() {
    let mut app = TestApp::signed_in().await;
    let mut anon = app.other_device();
    let created = new_token(&mut app, &[]).await;
    let token = created["token"].as_str().unwrap();
    rpc(&mut anon, token, "ping", None)
        .await
        .assert_status(StatusCode::OK);
    app.delete(&format!("/api/mcp/tokens/{}", created["id"]))
        .await
        .assert_status(StatusCode::NO_CONTENT);
    rpc(&mut anon, token, "ping", None)
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
    app.delete(&format!("/api/mcp/tokens/{}", created["id"]))
        .await
        .assert_status(StatusCode::NOT_FOUND);
}

// The MCP protocol

#[tokio::test]
async fn requests_need_a_valid_token() {
    let app = TestApp::signed_in().await;
    let mut anon = app.other_device();
    for authorization in [
        None,
        Some("Bearer nope"),
        Some("Bearer pc_wrong"),
        Some("Basic abc"),
    ] {
        let mut headers = vec![HOST];
        if let Some(authorization) = authorization {
            headers.push(("authorization", authorization));
        }
        let response = anon
            .send_with_headers(
                Method::POST,
                "/mcp",
                Some(json!({"jsonrpc": "2.0", "id": 1, "method": "ping"})),
                &headers,
            )
            .await;
        response.assert_status(StatusCode::UNAUTHORIZED);
        let challenge = response.headers["www-authenticate"].to_str().unwrap();
        assert!(challenge.starts_with("Bearer"), "{authorization:?}");
        assert!(
            challenge.contains(
                r#"resource_metadata="http://testserver/.well-known/oauth-protected-resource/mcp""#
            ),
            "{challenge}"
        );
    }
}

#[tokio::test]
async fn initialize_and_list_tools() {
    let mut app = TestApp::signed_in().await;
    let mut anon = app.other_device();
    let token = new_token(&mut app, &[]).await["token"]
        .as_str()
        .unwrap()
        .to_owned();
    let params = json!({
        "protocolVersion": "2025-06-18",
        "capabilities": {},
        "clientInfo": {"name": "t", "version": "1"},
    });
    let init = rpc(&mut anon, &token, "initialize", Some(params))
        .await
        .body["result"]
        .clone();
    assert_eq!(init["protocolVersion"], "2025-06-18");
    assert_eq!(init["serverInfo"]["name"], "PennyChest");
    assert!(init["capabilities"].get("tools").is_some());
    assert!(init["instructions"].as_str().unwrap().contains("read-only"));
    assert_eq!(
        init["serverInfo"]["icons"][0],
        json!({
            "src": "http://testserver/pwa-512x512.png",
            "mimeType": "image/png",
            "sizes": ["512x512"],
        })
    );
    assert_eq!(init["serverInfo"]["websiteUrl"], "http://testserver");

    let unknown = rpc(
        &mut anon,
        &token,
        "initialize",
        Some(json!({"protocolVersion": "1999-01-01"})),
    )
    .await;
    assert_eq!(unknown.body["result"]["protocolVersion"], "2025-11-25");

    let bearer = format!("Bearer {token}");
    anon.send_with_headers(
        Method::POST,
        "/mcp",
        Some(json!({"jsonrpc": "2.0", "method": "notifications/initialized"})),
        &[("authorization", &bearer)],
    )
    .await
    .assert_status(StatusCode::ACCEPTED);

    let listed = rpc(&mut anon, &token, "tools/list", None).await;
    let tools: HashMap<String, Value> = listed.body["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| (t["name"].as_str().unwrap().to_owned(), t.clone()))
        .collect();
    assert!(tools.contains_key("list_transactions") && !tools.contains_key("create_category"));
    let listing = &tools["list_transactions"];
    assert_eq!(listing["annotations"]["readOnlyHint"], true);
    assert_eq!(listing["annotations"]["destructiveHint"], false);
    assert_eq!(listing["inputSchema"]["type"], "object");
}

#[tokio::test]
async fn write_tokens_see_and_run_their_tools() {
    let mut app = ledger().await;
    let mut anon = app.other_device();
    let token = new_token(&mut app, &["organise"]).await["token"]
        .as_str()
        .unwrap()
        .to_owned();
    let tools = tool_names(&rpc(&mut anon, &token, "tools/list", None).await);
    assert!(tools.contains(&"create_category".into()));
    assert!(!tools.contains(&"delete_transactions".into()));

    let result = call(
        &mut anon,
        &token,
        "create_category",
        json!({"name": "Gym", "parent": "Expenses"}),
    )
    .await;
    assert_eq!(result["isError"], false);
    assert_eq!(
        result["structuredContent"]["created"]["path"],
        "Expenses:Gym"
    );
    let text: Value = serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(text, result["structuredContent"]);
    let changes = app.get("/api/changes").await.body["changes"].clone();
    assert_eq!(changes.as_array().unwrap().len(), 1);
    assert_eq!(changes[0]["source"], "mcp");
}

#[tokio::test]
async fn destructive_tools_are_marked() {
    let mut app = TestApp::signed_in().await;
    let token = new_token(&mut app, &["transactions", "organise"]).await["token"]
        .as_str()
        .unwrap()
        .to_owned();
    let listed = rpc(&mut app, &token, "tools/list", None).await;
    let destructive: Vec<&str> = listed.body["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|t| t["annotations"]["destructiveHint"] == true)
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(destructive, ["delete_budget", "delete_transactions"]);
}

#[tokio::test]
async fn read_only_tokens_cannot_change_anything() {
    let mut app = ledger().await;
    let token = new_token(&mut app, &[]).await["token"]
        .as_str()
        .unwrap()
        .to_owned();
    let result = call(
        &mut app,
        &token,
        "create_category",
        json!({"name": "Gym", "parent": "Expenses"}),
    )
    .await;
    assert_eq!(result["isError"], true);
    assert!(
        result["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("permission")
    );
    assert_eq!(app.get("/api/changes").await.body["changes"], json!([]));
}

#[tokio::test]
async fn tool_errors_and_protocol_errors() {
    let mut app = ledger().await;
    let token = new_token(&mut app, &[]).await["token"]
        .as_str()
        .unwrap()
        .to_owned();
    let bad_input = call(
        &mut app,
        &token,
        "list_transactions",
        json!({"limit": "lots"}),
    )
    .await;
    assert_eq!(bad_input["isError"], true);
    assert!(
        bad_input["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("Invalid input")
    );

    let unknown_tool = rpc(
        &mut app,
        &token,
        "tools/call",
        Some(json!({"name": "launch_rockets"})),
    )
    .await;
    assert_eq!(unknown_tool.body["error"]["code"], -32602);
    let unknown_method = rpc(&mut app, &token, "resources/list", None).await;
    assert_eq!(unknown_method.body["error"]["code"], -32601);

    let bearer = format!("Bearer {token}");
    let garbage = app
        .send_raw(
            Method::POST,
            "/mcp",
            "application/json",
            "{not json",
            &[("authorization", &bearer)],
        )
        .await;
    garbage.assert_status(StatusCode::BAD_REQUEST);
    assert_eq!(garbage.body["error"]["code"], -32700);
}

#[tokio::test]
async fn reading_data() {
    let mut app = ledger().await;
    let token = new_token(&mut app, &[]).await["token"]
        .as_str()
        .unwrap()
        .to_owned();
    let result = call(&mut app, &token, "list_categories", json!({})).await;
    let categories = result["structuredContent"]["categories"]
        .as_array()
        .unwrap();
    assert!(categories.iter().any(|c| c["path"] == "Expenses:Food"));
    let tokens = app.get("/api/mcp/tokens").await.body["tokens"].clone();
    assert!(tokens[0]["last_used_at"].is_string());
}

#[tokio::test]
async fn no_stream_or_sessions() {
    let mut app = TestApp::signed_in().await;
    let token = new_token(&mut app, &[]).await["token"]
        .as_str()
        .unwrap()
        .to_owned();
    let bearer = format!("Bearer {token}");
    for method in [Method::GET, Method::DELETE] {
        let response = app
            .send_with_headers(method, "/mcp", None, &[("authorization", &bearer)])
            .await;
        response.assert_status(StatusCode::METHOD_NOT_ALLOWED);
        assert_eq!(response.headers["allow"], "POST");
    }
}

#[tokio::test]
async fn a_signed_in_session_is_not_enough() {
    let mut app = TestApp::signed_in().await;
    app.post("/mcp", json!({"jsonrpc": "2.0", "id": 1, "method": "ping"}))
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}

// OAuth: discovery

#[tokio::test]
async fn discovery_documents() {
    let mut anon = TestApp::new().await;
    let get = |path: &'static str| (path, [HOST]);
    let (path, headers) = get("/.well-known/oauth-protected-resource/mcp");
    let resource = anon
        .send_with_headers(Method::GET, path, None, &headers)
        .await
        .body;
    assert_eq!(resource["resource"], "http://testserver/mcp");
    assert_eq!(
        resource["authorization_servers"],
        json!(["http://testserver"])
    );
    assert_eq!(
        resource["scopes_supported"],
        json!(["read", "organise", "transactions"])
    );
    let (path, headers) = get("/.well-known/oauth-protected-resource");
    let plain = anon
        .send_with_headers(Method::GET, path, None, &headers)
        .await;
    assert_eq!(plain.body, resource);

    let (path, headers) = get("/.well-known/oauth-authorization-server");
    let server = anon
        .send_with_headers(Method::GET, path, None, &headers)
        .await
        .body;
    assert_eq!(server["issuer"], "http://testserver");
    assert_eq!(
        server["authorization_endpoint"],
        "http://testserver/oauth/authorize"
    );
    assert_eq!(server["code_challenge_methods_supported"], json!(["S256"]));
    assert_eq!(
        server["registration_endpoint"],
        "http://testserver/oauth/register"
    );
}

#[tokio::test]
async fn the_public_address_follows_the_proxy_scheme() {
    let mut anon = TestApp::new().await;
    let metadata = anon
        .send_with_headers(
            Method::GET,
            "/.well-known/oauth-protected-resource/mcp",
            None,
            &[HOST, ("x-forwarded-proto", "https")],
        )
        .await
        .body;
    assert_eq!(metadata["resource"], "https://testserver/mcp");
}

// OAuth: registration

async fn register(app: &mut TestApp, overrides: Value) -> Value {
    let mut body = json!({"redirect_uris": [REDIRECT], "client_name": "Claude"});
    for (key, value) in overrides.as_object().unwrap() {
        body[key] = value.clone();
    }
    let response = app.post("/oauth/register", body).await;
    response.assert_status(StatusCode::CREATED);
    assert_eq!(response.headers["cache-control"], "no-store");
    response.body
}

#[tokio::test]
async fn registration_rejects_bad_redirects() {
    let mut anon = TestApp::new().await;
    for uris in [
        json!([]),
        json!(["http://evil.example/cb"]),
        json!(["https://claude.ai/cb#frag"]),
        json!(["not a url"]),
    ] {
        let response = anon
            .post("/oauth/register", json!({ "redirect_uris": uris }))
            .await;
        response.assert_status(StatusCode::BAD_REQUEST);
        assert_eq!(response.body["error"], "invalid_redirect_uri", "{uris}");
    }
}

#[tokio::test]
async fn registration_allows_localhost_for_desktop_clients() {
    let mut anon = TestApp::new().await;
    let registered = register(
        &mut anon,
        json!({"redirect_uris": ["http://127.0.0.1:33418/callback"]}),
    )
    .await;
    assert_eq!(registered["token_endpoint_auth_method"], "none");
    assert!(registered.get("client_secret").is_none());
    assert!(
        registered["client_id"]
            .as_str()
            .unwrap()
            .starts_with("pcc_")
    );
}

#[tokio::test]
async fn registration_is_capped_but_drops_clients_that_never_connected() {
    let mut anon = TestApp::new().await;
    for i in 0..200 {
        sqlx::query(
            "INSERT INTO oauth_clients (client_id, client_name, redirect_uris, created_at)
             VALUES (?, 'Old', '[]', datetime('now', '-2 days'))",
        )
        .bind(format!("pcc_old{i}"))
        .execute(&anon.db)
        .await
        .unwrap();
    }
    // They never connected, so they make way.
    register(&mut anon, json!({})).await;
    for i in 0..199 {
        sqlx::query(
            "INSERT INTO oauth_clients (client_id, client_name, redirect_uris)
             VALUES (?, 'New', '[]')",
        )
        .bind(format!("pcc_new{i}"))
        .execute(&anon.db)
        .await
        .unwrap();
    }
    let full = anon
        .post("/oauth/register", json!({"redirect_uris": [REDIRECT]}))
        .await;
    full.assert_status(StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(full.body["error"], "temporarily_unavailable");
}

// OAuth: the consent page

fn pkce() -> (String, String) {
    let verifier = URL_SAFE_NO_PAD.encode(rand_bytes());
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    (verifier, challenge)
}

fn rand_bytes() -> [u8; 48] {
    let mut bytes = [0_u8; 48];
    for (i, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::try_from((i * 37 + 11) % 256).unwrap();
    }
    bytes
}

fn query(redirect: &str) -> HashMap<String, String> {
    let url = url::Url::parse(redirect).unwrap();
    url.query_pairs().into_owned().collect()
}

async fn approve(
    app: &mut TestApp,
    registered: &Value,
    challenge: &str,
    scopes: &[&str],
) -> HashMap<String, String> {
    let response = app
        .send_with_headers(
            Method::POST,
            "/api/oauth/approve",
            Some(json!({
                "client_id": registered["client_id"],
                "redirect_uri": REDIRECT,
                "state": "xyz",
                "code_challenge": challenge,
                "code_challenge_method": "S256",
                "response_type": "code",
                "scopes": scopes,
            })),
            &[HOST],
        )
        .await;
    response.assert_status(StatusCode::OK);
    let redirect = response.body["redirect"].as_str().unwrap();
    assert!(redirect.starts_with(&format!("{REDIRECT}?")), "{redirect}");
    query(redirect)
}

#[tokio::test]
async fn consent_needs_a_signed_in_user() {
    let app = TestApp::signed_in().await;
    let mut anon = app.other_device();
    let registered = register(&mut anon, json!({})).await;
    let path = format!(
        "/api/oauth/authorize-info?client_id={}&redirect_uri={REDIRECT}",
        registered["client_id"].as_str().unwrap()
    );
    anon.get(&path)
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
    anon.post("/api/oauth/approve", json!({}))
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn the_consent_page_info_checks_the_redirect() {
    let mut app = TestApp::signed_in().await;
    let registered = register(&mut app, json!({})).await;
    let id = registered["client_id"].as_str().unwrap();
    let info = app
        .get(&format!(
            "/api/oauth/authorize-info?client_id={id}&redirect_uri={REDIRECT}"
        ))
        .await;
    assert_eq!(
        info.body,
        json!({"client_name": "Claude", "redirect_host": "claude.ai"})
    );
    app.get(&format!(
        "/api/oauth/authorize-info?client_id={id}&redirect_uri=https://evil.example/cb"
    ))
    .await
    .assert_status(StatusCode::BAD_REQUEST);
    app.get(&format!(
        "/api/oauth/authorize-info?client_id=pcc_nope&redirect_uri={REDIRECT}"
    ))
    .await
    .assert_status(StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn denying_sends_the_user_back_with_an_error() {
    let mut app = TestApp::signed_in().await;
    let registered = register(&mut app, json!({})).await;
    let response = app
        .send_with_headers(
            Method::POST,
            "/api/oauth/deny",
            Some(json!({
                "client_id": registered["client_id"],
                "redirect_uri": REDIRECT,
                "state": "s1",
            })),
            &[HOST],
        )
        .await;
    let sent = query(response.body["redirect"].as_str().unwrap());
    assert_eq!(sent["error"], "access_denied");
    assert_eq!(sent["state"], "s1");
    assert_eq!(sent["iss"], "http://testserver");
}

#[tokio::test]
async fn pkce_is_required() {
    let mut app = TestApp::signed_in().await;
    let registered = register(&mut app, json!({})).await;
    let response = app
        .post(
            "/api/oauth/approve",
            json!({
                "client_id": registered["client_id"],
                "redirect_uri": REDIRECT,
                "state": "s",
            }),
        )
        .await;
    let sent = query(response.body["redirect"].as_str().unwrap());
    assert_eq!(sent["error"], "invalid_request");
}

// OAuth: tokens

async fn form(
    app: &mut TestApp,
    path: &str,
    fields: &[(&str, &str)],
    headers: &[(&str, &str)],
) -> TestResponse {
    let body: String = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(fields)
        .finish();
    app.send_raw(
        Method::POST,
        path,
        "application/x-www-form-urlencoded",
        body,
        headers,
    )
    .await
}

async fn exchange(
    app: &mut TestApp,
    registered: &Value,
    code: &str,
    verifier: &str,
    redirect: &str,
) -> TestResponse {
    let fields = [
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", redirect),
        ("code_verifier", verifier),
        ("client_id", registered["client_id"].as_str().unwrap()),
    ];
    form(app, "/oauth/token", &fields, &[]).await
}

async fn connect(app: &mut TestApp, scopes: &[&str]) -> (Value, Value) {
    let registered = register(app, json!({})).await;
    let (verifier, challenge) = pkce();
    let code = approve(app, &registered, &challenge, scopes).await["code"].clone();
    let tokens = exchange(app, &registered, &code, &verifier, REDIRECT).await;
    tokens.assert_status(StatusCode::OK);
    assert_eq!(tokens.headers["cache-control"], "no-store");
    (registered, tokens.body)
}

fn token_of(tokens: &Value, name: &str) -> String {
    tokens[name].as_str().unwrap().to_owned()
}

#[tokio::test]
async fn the_full_flow_gives_a_working_scoped_token() {
    let mut app = ledger().await;
    let (_, tokens) = connect(&mut app, &["organise", "admin"]).await;
    assert_eq!(tokens["token_type"], "Bearer");
    assert_eq!(tokens["expires_in"], 3600);
    assert_eq!(tokens["scope"], "read organise");

    let access = token_of(&tokens, "access_token");
    let tools = tool_names(&rpc(&mut app, &access, "tools/list", None).await);
    assert!(tools.contains(&"create_category".into()));
    assert!(!tools.contains(&"delete_transactions".into()));

    let listed = app.get("/api/mcp/tokens").await.body["tokens"].clone();
    assert_eq!(listed.as_array().unwrap().len(), 1);
    assert_eq!(listed[0]["kind"], "connector");
    assert_eq!(listed[0]["name"], "Claude");
    assert_eq!(listed[0]["scopes"], json!(["organise"]));
}

#[tokio::test]
async fn codes_are_single_use_and_checked() {
    let mut app = TestApp::signed_in().await;
    let registered = register(&mut app, json!({})).await;
    let (verifier, challenge) = pkce();
    let code = approve(&mut app, &registered, &challenge, &[]).await["code"].clone();

    let wrong = exchange(&mut app, &registered, &code, "wrong-verifier", REDIRECT).await;
    assert_eq!(wrong.body["error"], "invalid_grant");
    let elsewhere = exchange(
        &mut app,
        &registered,
        &code,
        &verifier,
        "https://claude.ai/other",
    )
    .await;
    assert_eq!(elsewhere.body["error"], "invalid_grant");
    exchange(&mut app, &registered, &code, &verifier, REDIRECT)
        .await
        .assert_status(StatusCode::OK);
    let again = exchange(&mut app, &registered, &code, &verifier, REDIRECT).await;
    assert_eq!(again.body["error"], "invalid_grant");

    let other = register(&mut app, json!({})).await;
    let code = approve(&mut app, &registered, &challenge, &[]).await["code"].clone();
    let stolen = exchange(&mut app, &other, &code, &verifier, REDIRECT).await;
    assert_eq!(stolen.body["error"], "invalid_grant");
}

#[tokio::test]
async fn expired_codes_are_refused() {
    let mut app = TestApp::signed_in().await;
    let registered = register(&mut app, json!({})).await;
    let (verifier, challenge) = pkce();
    let code = approve(&mut app, &registered, &challenge, &[]).await["code"].clone();
    sqlx::query("UPDATE oauth_codes SET expires_at = unixepoch() - 1")
        .execute(&app.db)
        .await
        .unwrap();
    let expired = exchange(&mut app, &registered, &code, &verifier, REDIRECT).await;
    assert_eq!(expired.body["error"], "invalid_grant");
}

#[tokio::test]
async fn access_tokens_expire_and_refresh_rotates() {
    let mut app = TestApp::signed_in().await;
    let (registered, tokens) = connect(&mut app, &[]).await;
    sqlx::query("UPDATE access_tokens SET expires_at = unixepoch() - 1")
        .execute(&app.db)
        .await
        .unwrap();
    let access = token_of(&tokens, "access_token");
    rpc(&mut app, &access, "ping", None)
        .await
        .assert_status(StatusCode::UNAUTHORIZED);

    let client_id = registered["client_id"].as_str().unwrap();
    let old_refresh = token_of(&tokens, "refresh_token");
    let refresh = [
        ("grant_type", "refresh_token"),
        ("client_id", client_id),
        ("refresh_token", old_refresh.as_str()),
    ];
    let renewed = form(&mut app, "/oauth/token", &refresh, &[]).await;
    renewed.assert_status(StatusCode::OK);
    let access = token_of(&renewed.body, "access_token");
    rpc(&mut app, &access, "ping", None)
        .await
        .assert_status(StatusCode::OK);
    // The old refresh token was replaced.
    let reused = form(&mut app, "/oauth/token", &refresh, &[]).await;
    assert_eq!(reused.body["error"], "invalid_grant");
    let listed = app.get("/api/mcp/tokens").await.body["tokens"].clone();
    assert_eq!(listed.as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn revoking_in_settings_disconnects_the_app() {
    let mut app = TestApp::signed_in().await;
    let (registered, tokens) = connect(&mut app, &[]).await;
    let listed = app.get("/api/mcp/tokens").await.body["tokens"].clone();
    app.delete(&format!("/api/mcp/tokens/{}", listed[0]["id"]))
        .await
        .assert_status(StatusCode::NO_CONTENT);
    let access = token_of(&tokens, "access_token");
    rpc(&mut app, &access, "ping", None)
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
    let refresh = token_of(&tokens, "refresh_token");
    let refreshed = form(
        &mut app,
        "/oauth/token",
        &[
            ("grant_type", "refresh_token"),
            ("client_id", registered["client_id"].as_str().unwrap()),
            ("refresh_token", &refresh),
        ],
        &[],
    )
    .await;
    assert_eq!(refreshed.body["error"], "invalid_grant");
}

#[tokio::test]
async fn clients_can_revoke_their_own_tokens() {
    let mut app = TestApp::signed_in().await;
    let (registered, tokens) = connect(&mut app, &[]).await;
    let refresh = token_of(&tokens, "refresh_token");
    let response = form(
        &mut app,
        "/oauth/revoke",
        &[
            ("token", &refresh),
            ("client_id", registered["client_id"].as_str().unwrap()),
        ],
        &[],
    )
    .await;
    response.assert_status(StatusCode::OK);
    let access = token_of(&tokens, "access_token");
    rpc(&mut app, &access, "ping", None)
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn confidential_clients_must_send_their_secret() {
    let mut app = TestApp::signed_in().await;
    let registered = register(
        &mut app,
        json!({"token_endpoint_auth_method": "client_secret_basic"}),
    )
    .await;
    let secret = registered["client_secret"].as_str().unwrap();
    assert_eq!(registered["client_secret_expires_at"], 0);
    let (verifier, challenge) = pkce();
    let code = approve(&mut app, &registered, &challenge, &[]).await["code"].clone();
    let client_id = registered["client_id"].as_str().unwrap();
    let fields = [
        ("grant_type", "authorization_code"),
        ("code", code.as_str()),
        ("redirect_uri", REDIRECT),
        ("code_verifier", verifier.as_str()),
    ];
    let mut with_id = fields.to_vec();
    with_id.push(("client_id", client_id));
    let no_secret = form(&mut app, "/oauth/token", &with_id, &[]).await;
    no_secret.assert_status(StatusCode::UNAUTHORIZED);
    assert_eq!(no_secret.body["error"], "invalid_client");

    let basic = format!("Basic {}", STANDARD.encode(format!("{client_id}:{secret}")));
    form(
        &mut app,
        "/oauth/token",
        &fields,
        &[("authorization", &basic)],
    )
    .await
    .assert_status(StatusCode::OK);
}

#[tokio::test]
async fn unknown_grant_types_and_clients() {
    let mut app = TestApp::signed_in().await;
    form(&mut app, "/oauth/token", &[("grant_type", "password")], &[])
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
    let registered = register(&mut app, json!({})).await;
    let response = form(
        &mut app,
        "/oauth/token",
        &[
            ("grant_type", "password"),
            ("client_id", registered["client_id"].as_str().unwrap()),
        ],
        &[],
    )
    .await;
    assert_eq!(response.body["error"], "unsupported_grant_type");
    assert_eq!(response.headers["cache-control"], "no-store");
}

#[tokio::test]
async fn pages_cannot_be_framed() {
    let mut anon = TestApp::new().await;
    for path in ["/api/health", "/oauth/authorize"] {
        let response = anon.get(path).await;
        assert_eq!(response.headers["x-frame-options"], "DENY", "{path}");
        assert_eq!(
            response.headers["content-security-policy"],
            "frame-ancestors 'none'"
        );
    }
}
