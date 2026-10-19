//! PennyChest's actions as an MCP server, over Streamable HTTP at `/mcp`.
//!
//! Stateless and JSON-only: each POST carries one JSON-RPC message and gets one JSON reply,
//! which the transport allows. Agents authenticate with an access token, a personal one from
//! Settings or one issued to an OAuth connector, and can only run the actions its scopes allow.

pub mod oauth;
pub mod tokens;

use serde_json::{Value, json};
use sqlx::SqlitePool;

use crate::actions::{self, ActionError, Caller, Scope, Source};

pub const SUPPORTED_VERSIONS: [&str; 3] = ["2025-11-25", "2025-06-18", "2025-03-26"];
const DESTRUCTIVE: [&str; 2] = ["delete_transactions", "delete_budget"];

fn instructions(scopes: &[Scope]) -> String {
    let allowed: Vec<String> = scopes
        .iter()
        .filter(|s| **s != Scope::Read)
        .map(|s| s.label().to_lowercase())
        .collect();
    let can = if allowed.is_empty() {
        "This token is read-only.".to_owned()
    } else {
        format!(
            "This token can also {}; every change is listed in PennyChest's Settings > Changes \
             made by AI, where the user can undo it.",
            allowed.join(" and ")
        )
    };
    format!(
        "PennyChest is the user's personal finance ledger (double-entry, amounts in GBP unless \
         stated). Use these tools to look up their accounts, spending, budgets and card taps. \
         {can}"
    )
}

fn tool(action: &Value) -> Value {
    let name = action["name"].as_str().unwrap_or_default();
    json!({
        "name": name,
        "description": action["description"],
        "inputSchema": action["input_schema"],
        "annotations": {
            "readOnlyHint": action["scope"] == "read",
            "destructiveHint": DESTRUCTIVE.contains(&name),
            "openWorldHint": false,
        },
    })
}

/// What to answer: an HTTP status, and a JSON body unless there's nothing to say.
#[derive(Debug, Clone, PartialEq)]
pub struct Reply {
    pub status: u16,
    pub body: Option<Value>,
}

fn result(id: &Value, result: Value) -> Reply {
    Reply {
        status: 200,
        body: Some(json!({"jsonrpc": "2.0", "id": id, "result": result})),
    }
}

fn error(id: &Value, code: i64, message: &str, status: u16) -> Reply {
    Reply {
        status,
        body: Some(json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {"code": code, "message": message},
        })),
    }
}

fn initialize(id: &Value, params: &Value, scopes: &[Scope], base: &str) -> Reply {
    let requested = params["protocolVersion"].as_str();
    let version = requested
        .filter(|v| SUPPORTED_VERSIONS.contains(v))
        .unwrap_or(SUPPORTED_VERSIONS[0]);
    result(
        id,
        json!({
            "protocolVersion": version,
            "capabilities": {"tools": {"listChanged": false}},
            "serverInfo": {
                "name": "PennyChest",
                "version": env!("CARGO_PKG_VERSION"),
                "title": "PennyChest",
                "websiteUrl": base,
                // Shown by clients that display server icons (MCP 2025-11-25).
                "icons": [
                    {
                        "src": format!("{base}/pwa-512x512.png"),
                        "mimeType": "image/png",
                        "sizes": ["512x512"],
                    },
                    {
                        "src": format!("{base}/pwa-192x192.png"),
                        "mimeType": "image/png",
                        "sizes": ["192x192"],
                    },
                    {
                        "src": format!("{base}/favicon.svg"),
                        "mimeType": "image/svg+xml",
                        "sizes": ["any"],
                    },
                ],
            },
            "instructions": instructions(scopes),
        }),
    )
}

async fn call_tool(
    db: &SqlitePool,
    id: &Value,
    params: &Value,
    scopes: &[Scope],
) -> Result<Reply, sqlx::Error> {
    let name = params["name"].as_str().unwrap_or_default();
    if actions::find(name).is_none() {
        let shown = params["name"].as_str().unwrap_or("None");
        return Ok(error(id, -32602, &format!("Unknown tool: {shown}"), 200));
    }
    let arguments = match &params["arguments"] {
        Value::Null => json!({}),
        other => other.clone(),
    };
    let caller = Caller {
        scopes,
        source: Source::Mcp,
        conversation_id: None,
    };
    match actions::run(db, name, arguments, caller).await {
        Ok(output) => Ok(result(
            id,
            json!({
                "content": [{"type": "text", "text": Value::Object(output.clone()).to_string()}],
                "structuredContent": output,
                "isError": false,
            }),
        )),
        Err(ActionError::Database(e)) => Err(e),
        Err(e) => Ok(result(
            id,
            json!({"content": [{"type": "text", "text": e.to_string()}], "isError": true}),
        )),
    }
}

/// Answers one JSON-RPC message from an agent holding a token with these scopes. `base` is
/// this server's public address, e.g. `https://pennychest.example`.
pub async fn handle(
    db: &SqlitePool,
    scopes: &[Scope],
    body: &[u8],
    base: &str,
) -> Result<Reply, sqlx::Error> {
    let Ok(message) = serde_json::from_slice::<Value>(body) else {
        return Ok(error(&Value::Null, -32700, "Parse error", 400));
    };
    if !message.is_object() || message["jsonrpc"] != "2.0" {
        let problem = "Expected a single JSON-RPC 2.0 message";
        return Ok(error(&Value::Null, -32600, problem, 400));
    }
    let method = &message["method"];
    let Some(id) = message.get("id").filter(|_| !method.is_null()) else {
        // Notifications and responses need no reply.
        return Ok(Reply {
            status: 202,
            body: None,
        });
    };
    let params = match &message["params"] {
        Value::Null => json!({}),
        other => other.clone(),
    };
    let scopes: Vec<Scope> = std::iter::once(Scope::Read)
        .chain(scopes.iter().copied())
        .collect();
    Ok(match method.as_str().unwrap_or_default() {
        "initialize" => initialize(id, &params, &scopes, base),
        "ping" => result(id, json!({})),
        "tools/list" => {
            let tools: Vec<Value> = actions::describe(&scopes).iter().map(tool).collect();
            result(id, json!({ "tools": tools }))
        }
        "tools/call" => call_tool(db, id, &params, &scopes).await?,
        other => error(id, -32601, &format!("Method not found: {other}"), 200),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instructions_say_what_the_token_may_change() {
        assert!(instructions(&[Scope::Read]).ends_with("This token is read-only."));
        let organise = instructions(&[Scope::Read, Scope::Organise]);
        assert!(organise.contains("This token can also change categories, rules and budgets;"));
    }
}
