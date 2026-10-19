//! Chat: send the conversation to the chat task's model, run the actions it asks for, feed the
//! results back, and repeat until it answers.

use std::collections::HashMap;
use std::time::Instant;

use chrono::{NaiveDate, NaiveDateTime};
use serde::Serialize;
use serde_json::{Map, Value, json};
use sqlx::SqlitePool;
use tokio::sync::mpsc::UnboundedSender;

use crate::actions::{self, ActionError, Caller, Scope, Source};
use crate::ai::check_actions::{self, MIN_PROBABILITY, Proposed};
use crate::ai::config::{self, Task};
use crate::ai::http::Transport;
use crate::ai::log_call;
use crate::ai::providers::{
    self, ChatReply, ChatRequest, ResolveError, Resolved, Tool, ToolCall, Turn,
};

pub const MAX_ROUNDS: usize = 8;
pub const TITLE_LENGTH: usize = 60;

pub const LAST_ROUND: &str = "\n\nYou have used up your lookups for this message. Answer now \
                              from the results you already have, without calling any more \
                              tools, and say briefly if anything is missing.";

const GAVE_UP: &str = "I couldn't finish working that out in a reasonable number of steps. \
                       Could you ask something more specific?";

const HELD_BACK: &str = "Not done: a check found this probably isn't what the user asked for. \
                         Don't try it again unless they confirm. Tell them exactly what it \
                         would change and ask whether they want it.";

fn permissions(scopes: &[Scope]) -> String {
    let mut can = vec![];
    if scopes.contains(&Scope::Organise) {
        can.push("create and rename categories, create rules and set budgets");
    }
    if scopes.contains(&Scope::Transactions) {
        can.push("add, edit, recategorise, review and delete transactions");
    }
    if can.is_empty() {
        return "You can look things up but can't change anything. If the user asks for a \
                change, explain that they can let you make changes in Settings > AI Integration \
                > Chat."
            .into();
    }
    let cannot: Vec<&str> = [
        (Scope::Organise, "change categories, rules or budgets"),
        (Scope::Transactions, "change transactions"),
    ]
    .into_iter()
    .filter(|(scope, _)| !scopes.contains(scope))
    .map(|(_, text)| text)
    .collect();
    let mut text = format!(
        "When the user asks, you can {}. Do what they ask without asking for confirmation, then \
         say exactly what you changed; they can undo any change from the chat. Look up the \
         details you need (IDs, category paths) first rather than guessing, and don't make \
         changes they didn't ask for.",
        can.join(" and ")
    );
    if !cannot.is_empty() {
        text.push_str(&format!(
            " You can't {}; if they ask, explain that they can allow it in Settings > AI \
             Integration > Chat.",
            cannot.join(" or ")
        ));
    }
    text
}

pub fn system_prompt(today: NaiveDate, scopes: &[Scope]) -> String {
    format!(
        "You are the assistant inside PennyChest, a personal finance app. You help the user \
understand and manage their own money: spending, income, categories, budgets, rules and card \
payments.

Use the tools to look things up. When you need several lookups that don't depend on each \
other, ask for them all at once rather than one at a time. Prefer the broadest tool that \
answers the question: savings_opportunities for where they could save or cut back, \
recurring_payments for subscriptions and bills, compare_periods for what has changed. \
For open-ended questions where you need to see the detail, transactions_digest gives you a \
whole period in one call.

Every figure you give must come from a tool result; never estimate or do your own sums over \
transactions when a tool can total them. If a tool reports \
an error, fix the call and try again, or explain what's missing. Amounts are in pounds (GBP) \
unless a result says otherwise; spending is positive and refunds reduce it. Mention when \
totals include unreviewed transactions.

Answer concisely in plain English. Use short lists or small Markdown tables when they make \
figures easier to read. If the question is ambiguous (for example which period they mean), \
make a sensible assumption and say what you assumed.

{}

Today is {}.",
        permissions(scopes),
        today.format("%A %-d %B %Y")
    )
}

/// What the chat may change, as Settings > AI Integration > Chat grants it.
pub async fn granted_scopes(db: &SqlitePool) -> Vec<Scope> {
    let names = config::chat_scopes(db).await.unwrap_or_default();
    names.iter().filter_map(|n| n.parse().ok()).collect()
}

#[derive(Debug, Clone, PartialEq, Serialize, sqlx::FromRow)]
pub struct Conversation {
    pub id: i64,
    pub title: String,
    pub updated_at: NaiveDateTime,
}

/// The most recently used first.
pub async fn conversations(db: &SqlitePool) -> Result<Vec<Conversation>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, title, updated_at FROM chat_conversations ORDER BY updated_at DESC, id DESC",
    )
    .fetch_all(db)
    .await
}

pub async fn conversation(db: &SqlitePool, id: i64) -> Result<Option<Conversation>, sqlx::Error> {
    sqlx::query_as("SELECT id, title, updated_at FROM chat_conversations WHERE id = ?")
        .bind(id)
        .fetch_optional(db)
        .await
}

/// Whether there was one to delete.
pub async fn delete(db: &SqlitePool, id: i64) -> Result<bool, sqlx::Error> {
    let deleted = sqlx::query("DELETE FROM chat_conversations WHERE id = ?")
        .bind(id)
        .execute(db)
        .await?;
    Ok(deleted.rows_affected() > 0)
}

/// The first message with its spacing tidied, shortened to fit.
fn title(first_message: &str) -> String {
    let title = first_message
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if title.chars().count() <= TITLE_LENGTH {
        return title;
    }
    let short: String = title.chars().take(TITLE_LENGTH - 1).collect();
    format!("{}…", short.trim_end())
}

/// A new conversation, titled after its first message.
pub async fn start(db: &SqlitePool, first_message: &str) -> Result<Conversation, sqlx::Error> {
    sqlx::query_as(
        "INSERT INTO chat_conversations (title, updated_at)
         VALUES (?, strftime('%Y-%m-%d %H:%M:%f', 'now'))
         RETURNING id, title, updated_at",
    )
    .bind(title(first_message))
    .fetch_one(db)
    .await
}

#[derive(sqlx::FromRow)]
struct MessageRow {
    id: i64,
    role: String,
    content: Option<String>,
    tool_calls: Option<String>,
    tool_call_id: Option<String>,
    tool_name: Option<String>,
    is_error: bool,
    raw: Option<String>,
    raw_provider: Option<String>,
}

impl MessageRow {
    fn tool_calls(&self) -> Vec<ToolCall> {
        self.tool_calls
            .as_deref()
            .and_then(|t| serde_json::from_str(t).ok())
            .unwrap_or_default()
    }

    fn turn(&self) -> Turn {
        let content = self.content.clone().unwrap_or_default();
        match self.role.as_str() {
            "user" => Turn::User { content },
            "assistant" => Turn::Assistant {
                content,
                tool_calls: self.tool_calls(),
                raw: self
                    .raw
                    .as_deref()
                    .and_then(|r| serde_json::from_str(r).ok()),
                raw_provider: self.raw_provider.clone(),
            },
            _ => Turn::Tool {
                tool_call_id: self.tool_call_id.clone().unwrap_or_default(),
                name: self.tool_name.clone().unwrap_or_default(),
                content,
                is_error: self.is_error,
            },
        }
    }
}

async fn messages(db: &SqlitePool, conversation_id: i64) -> Result<Vec<MessageRow>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, role, content, tool_calls, tool_call_id, tool_name, is_error, raw,
                raw_provider
         FROM chat_messages WHERE conversation_id = ? ORDER BY id",
    )
    .bind(conversation_id)
    .fetch_all(db)
    .await
}

/// The conversation as providers are given it.
pub async fn history(db: &SqlitePool, conversation_id: i64) -> Result<Vec<Turn>, sqlx::Error> {
    Ok(messages(db, conversation_id)
        .await?
        .iter()
        .map(MessageRow::turn)
        .collect())
}

/// Adds a message to the conversation, which becomes its latest; its id.
async fn add(db: &SqlitePool, conversation_id: i64, turn: &Turn) -> Result<i64, sqlx::Error> {
    let mut tx = db.begin().await?;
    let query = sqlx::query_scalar(
        "INSERT INTO chat_messages
            (conversation_id, role, content, tool_calls, tool_call_id, tool_name, is_error, raw,
             raw_provider)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(conversation_id);
    let query = match turn {
        Turn::User { content } => query
            .bind("user")
            .bind(content)
            .bind(None::<String>)
            .bind(None::<String>)
            .bind(None::<String>)
            .bind(false)
            .bind(None::<String>)
            .bind(None::<String>),
        Turn::Assistant {
            content,
            tool_calls,
            raw,
            raw_provider,
        } => query
            .bind("assistant")
            .bind(content)
            .bind(serde_json::to_string(tool_calls).expect("JSON serialises"))
            .bind(None::<String>)
            .bind(None::<String>)
            .bind(false)
            .bind(raw.as_ref().map(Value::to_string))
            .bind(raw_provider),
        Turn::Tool {
            tool_call_id,
            name,
            content,
            is_error,
        } => query
            .bind("tool")
            .bind(content)
            .bind(None::<String>)
            .bind(tool_call_id)
            .bind(name)
            .bind(is_error)
            .bind(None::<String>)
            .bind(None::<String>),
    };
    let id: i64 = query.fetch_one(&mut *tx).await?;
    sqlx::query(
        "UPDATE chat_conversations SET updated_at = strftime('%Y-%m-%d %H:%M:%f', 'now')
         WHERE id = ?",
    )
    .bind(conversation_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(id)
}

/// Progress through a turn, as the Chat page shows it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    Conversation {
        id: i64,
        title: String,
    },
    /// The model's text as it writes it; text before a `Tool` is a preamble to that step.
    Text {
        delta: String,
    },
    Tool {
        name: String,
        arguments: Map<String, Value>,
    },
    ToolResult {
        name: String,
        ok: bool,
        /// Held back because the user probably didn't ask for it.
        #[serde(skip_serializing_if = "std::ops::Not::not")]
        held: bool,
    },
    Change {
        id: i64,
        summary: String,
    },
    /// The whole answer.
    Message {
        content: String,
    },
    Error {
        detail: String,
    },
    Done,
}

impl Event {
    /// Its name, and its data without the name.
    pub fn parts(&self) -> (String, Value) {
        let mut data = serde_json::to_value(self).expect("JSON serialises");
        let name = data
            .as_object_mut()
            .and_then(|d| d.shift_remove("event"))
            .and_then(|n| n.as_str().map(str::to_owned))
            .unwrap_or_default();
        (name, data)
    }
}

/// Where a turn sends its events. A reader that has gone away doesn't stop the turn.
pub type Events = UnboundedSender<Event>;

fn emit(events: &Events, event: Event) {
    let _ = events.send(event);
}

/// Whether a change the model asked for should be held back because the check model thinks the
/// user didn't ask for it. Reading, actions the chat isn't allowed to run anyway, and turns
/// where checking is off or the check fails all go ahead.
async fn held_back(
    db: &SqlitePool,
    http: &dyn Transport,
    conversation_id: i64,
    call: &ToolCall,
    scopes: &[Scope],
) -> Result<bool, sqlx::Error> {
    let Some(action) = actions::find(&call.name) else {
        return Ok(false);
    };
    if action.scope == Scope::Read
        || !scopes.contains(&action.scope)
        || !check_actions::checking_enabled(db).await
    {
        return Ok(false);
    }
    let conversation = history(db, conversation_id).await?;
    let proposed = Proposed {
        name: action.name,
        description: action.description,
        arguments: &call.arguments,
    };
    Ok(
        match check_actions::check_action(db, http, &conversation, &proposed).await {
            Ok(probability) => probability < MIN_PROBABILITY,
            // Already in the AI logs.
            Err(_) => false,
        },
    )
}

/// Runs one call the model asked for, recording its result in the conversation.
async fn run_call(
    db: &SqlitePool,
    http: &dyn Transport,
    conversation_id: i64,
    call: &ToolCall,
    scopes: &[Scope],
    events: &Events,
) -> Result<(), sqlx::Error> {
    emit(
        events,
        Event::Tool {
            name: call.name.clone(),
            arguments: call.arguments.clone(),
        },
    );
    if held_back(db, http, conversation_id, call, scopes).await? {
        let turn = Turn::Tool {
            tool_call_id: call.id.clone(),
            name: call.name.clone(),
            content: json!({ "error": HELD_BACK }).to_string(),
            is_error: true,
        };
        add(db, conversation_id, &turn).await?;
        emit(
            events,
            Event::ToolResult {
                name: call.name.clone(),
                ok: false,
                held: true,
            },
        );
        return Ok(());
    }
    let caller = Caller {
        scopes,
        source: Source::Chat,
        conversation_id: Some(conversation_id),
    };
    let ran = actions::run(
        db,
        &call.name,
        Value::Object(call.arguments.clone()),
        caller,
    )
    .await;
    let (result, is_error) = match ran {
        Ok(result) => (result, false),
        Err(ActionError::Database(e)) => return Err(e),
        Err(e) => {
            let mut result = Map::new();
            result.insert("error".into(), json!(e.to_string()));
            (result, true)
        }
    };
    let turn = Turn::Tool {
        tool_call_id: call.id.clone(),
        name: call.name.clone(),
        content: Value::Object(result.clone()).to_string(),
        is_error,
    };
    let message_id = add(db, conversation_id, &turn).await?;
    emit(
        events,
        Event::ToolResult {
            name: call.name.clone(),
            ok: !is_error,
            held: false,
        },
    );
    if let Some(change) = result.get("change")
        && let (Some(id), Some(summary)) = (change["id"].as_i64(), change["summary"].as_str())
    {
        sqlx::query("UPDATE action_changes SET chat_message_id = ? WHERE id = ?")
            .bind(message_id)
            .bind(id)
            .execute(db)
            .await?;
        emit(
            events,
            Event::Change {
                id,
                summary: summary.to_owned(),
            },
        );
    }
    Ok(())
}

/// One round: the model's reply to the conversation so far, its text streamed as it comes.
async fn ask(
    db: &SqlitePool,
    http: &dyn Transport,
    model: &Resolved,
    request: ChatRequest<'_>,
    events: &Events,
) -> Result<ChatReply, String> {
    let started = Instant::now();
    let mut on_text = |delta: &str| {
        if !delta.is_empty() {
            emit(
                events,
                Event::Text {
                    delta: delta.to_owned(),
                },
            );
        }
    };
    let label = model.label();
    match model
        .provider
        .chat_turn(http, &model.cfg, request, &mut on_text)
        .await
    {
        Ok(reply) => {
            log_call(
                db,
                &label,
                "chat",
                started,
                Some(&reply.request),
                Some(&reply.response),
                None,
            )
            .await;
            Ok(reply)
        }
        Err(e) => {
            log_call(db, &label, "chat", started, None, None, Some(&e.message)).await;
            Err(e.message)
        }
    }
}

/// Handles one message from the user: `Text` as the model writes, `Tool` and `ToolResult` (and
/// `Change`) for each action it runs, then the whole answer as `Message`, or an `Error`.
pub async fn run_turn(
    db: &SqlitePool,
    http: &dyn Transport,
    conversation_id: i64,
    user_text: &str,
    scopes: &[Scope],
    today: NaiveDate,
    events: &Events,
) -> Result<(), sqlx::Error> {
    let user = Turn::User {
        content: user_text.to_owned(),
    };
    add(db, conversation_id, &user).await?;
    let model = match providers::resolve_task(db, Task::Chat).await {
        Ok(model) => model,
        Err(e) => {
            let detail = match e {
                ResolveError::Provider(e) => e.message,
                ResolveError::Setting(e) => e.to_string(),
            };
            emit(events, Event::Error { detail });
            return Ok(());
        }
    };
    let tools: Vec<Tool> = actions::describe(scopes)
        .into_iter()
        .filter_map(|t| serde_json::from_value(t).ok())
        .collect();
    let system = system_prompt(today, scopes);
    let last_system = format!("{system}{LAST_ROUND}");

    for round in 1..=MAX_ROUNDS {
        // On the last round the model must answer with what it has: it's told so, and the
        // provider is asked not to let it call any more tools.
        let last_round = round == MAX_ROUNDS;
        let conversation = history(db, conversation_id).await?;
        let request = ChatRequest {
            model: &model.model,
            system: if last_round { &last_system } else { &system },
            history: &conversation,
            tools: &tools,
            answer_only: last_round,
        };
        let mut reply = match ask(db, http, &model, request, events).await {
            Ok(reply) => reply,
            Err(detail) => {
                emit(events, Event::Error { detail });
                return Ok(());
            }
        };
        let mut raw = Some(reply.raw);
        if last_round && !reply.tool_calls.is_empty() {
            // A provider that can't switch tools off asked for more anyway. Keep any answer it
            // wrote, without the calls (or the raw reply holding them), so the history stays
            // valid.
            if reply.text.trim().is_empty() {
                break;
            }
            reply.tool_calls.clear();
            raw = None;
        }
        let answer = Turn::Assistant {
            content: reply.text.clone(),
            tool_calls: reply.tool_calls.clone(),
            raw,
            raw_provider: Some(model.provider.id().to_owned()),
        };
        add(db, conversation_id, &answer).await?;
        if reply.tool_calls.is_empty() {
            emit(
                events,
                Event::Message {
                    content: reply.text,
                },
            );
            return Ok(());
        }
        for call in &reply.tool_calls {
            run_call(db, http, conversation_id, call, scopes, events).await?;
        }
    }

    // Only reached when the last round still asked for tools and wrote no answer.
    let gave_up = Turn::Assistant {
        content: GAVE_UP.into(),
        tool_calls: vec![],
        raw: None,
        raw_provider: None,
    };
    add(db, conversation_id, &gave_up).await?;
    emit(
        events,
        Event::Message {
            content: GAVE_UP.into(),
        },
    );
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ToolUsed {
    pub name: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub held: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ChangeMade {
    pub id: i64,
    pub summary: String,
    pub undone: bool,
}

/// A conversation as the user saw it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "role", rename_all = "lowercase")]
pub enum DisplayTurn {
    User {
        content: String,
    },
    Assistant {
        content: String,
        tools: Vec<ToolUsed>,
        changes: Vec<ChangeMade>,
    },
}

/// The user's messages and the model's answers, each answer listing the tools used to reach it
/// and any changes made along the way.
pub async fn display_turns(
    db: &SqlitePool,
    conversation_id: i64,
) -> Result<Vec<DisplayTurn>, sqlx::Error> {
    let rows: Vec<(Option<i64>, i64, String, bool)> = sqlx::query_as(
        "SELECT chat_message_id, id, summary, undone_at IS NOT NULL FROM action_changes
         WHERE conversation_id = ?",
    )
    .bind(conversation_id)
    .fetch_all(db)
    .await?;
    let changes: HashMap<i64, ChangeMade> = rows
        .into_iter()
        .filter_map(|(message, id, summary, undone)| {
            Some((
                message?,
                ChangeMade {
                    id,
                    summary,
                    undone,
                },
            ))
        })
        .collect();
    let held_back = json!({ "error": HELD_BACK }).to_string();
    let (mut turns, mut tools, mut made) = (vec![], vec![], vec![]);
    for m in messages(db, conversation_id).await? {
        match m.role.as_str() {
            "user" => {
                turns.push(DisplayTurn::User {
                    content: m.content.unwrap_or_default(),
                });
                tools.clear();
                made.clear();
            }
            "tool" => {
                tools.push(ToolUsed {
                    name: m.tool_name.clone().unwrap_or_default(),
                    ok: !m.is_error,
                    held: m.is_error && m.content.as_deref() == Some(held_back.as_str()),
                });
                if let Some(change) = changes.get(&m.id) {
                    made.push(change.clone());
                }
            }
            _ if m.tool_calls().is_empty() => {
                turns.push(DisplayTurn::Assistant {
                    content: m.content.unwrap_or_default(),
                    tools: std::mem::take(&mut tools),
                    changes: std::mem::take(&mut made),
                });
            }
            _ => {}
        }
    }
    Ok(turns)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_titles_are_shortened() {
        assert_eq!(title("  What   did I\nspend? "), "What did I spend?");
        let long = format!("Please summarise {}", "all of my spending ".repeat(10));
        let short = title(&long);
        assert!(short.chars().count() <= TITLE_LENGTH);
        assert!(short.ends_with('…'));
    }

    #[test]
    fn the_prompt_says_what_the_chat_may_change() {
        let today = NaiveDate::from_ymd_opt(2026, 10, 4).unwrap();
        let read_only = system_prompt(today, &[]);
        assert!(read_only.contains("can't change anything"));
        assert!(read_only.ends_with("Today is Sunday 4 October 2026."));
        let organise = system_prompt(today, &[Scope::Organise]);
        assert!(organise.contains("create rules and set budgets"));
        assert!(organise.contains("You can't change transactions"));
    }

    #[test]
    fn events_are_named_apart_from_their_data() {
        let (name, data) = Event::ToolResult {
            name: "list_categories".into(),
            ok: true,
            held: false,
        }
        .parts();
        assert_eq!(name, "tool_result");
        assert_eq!(data, json!({"name": "list_categories", "ok": true}));
        assert_eq!(Event::Done.parts(), ("done".into(), json!({})));
    }
}
