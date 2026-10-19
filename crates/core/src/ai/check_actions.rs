//! Checking that a change the chat is about to make is one the user asked for, with the model
//! the "check_actions" task uses.
//!
//! Chat changes data without asking first, so before each change a second model reads the
//! recent conversation and the proposed action and says how likely it is that the user asked
//! for it. Unlikely changes are held back and the chat is told to ask the user instead.

use std::time::Instant;

use serde_json::{Map, Value, json};
use sqlx::SqlitePool;

use super::config::{self, Kind, Task};
use super::http::Transport;
use super::log_call;
use super::providers::{self, JsonRequest, ProviderError, ResolveError, Resolved, Turn};

/// Below this probability that the user asked for the action, it's held back.
pub const MIN_PROBABILITY: f64 = 0.5;
/// How many of the latest user and assistant messages the check reads.
const CONTEXT_MESSAGES: usize = 6;

const QUESTION: &str = "Did the user ask for `proposed_action`, or clearly agree to it, in \
                        `conversation`? The action, and the details in its arguments, should be \
                        what the user wanted.";
const YES: &str = "The user asked for this change, or agreed to it when the assistant offered \
                   it, and its details match what they said or what the assistant looked up for \
                   them";
const NO: &str = "The user only asked a question, asked for something different, or the action \
                  changes more, or other things, than they asked for";

const SYSTEM: &str = "You check actions an AI assistant in a personal finance app is about to \
                      take on the user's behalf. You answer only whether the user asked for the \
                      action.";

/// Whether checking is on and has a model.
pub async fn checking_enabled(db: &SqlitePool) -> bool {
    matches!(
        config::task_model(db, Task::CheckActions).await,
        Ok((Some(_), Some(_)))
    )
}

/// The action to check, as the actions layer describes it.
pub struct Proposed<'a> {
    pub name: &'a str,
    pub description: &'a str,
    pub arguments: &'a Map<String, Value>,
}

fn state(conversation: &[Turn], action: &Proposed<'_>) -> Value {
    let messages: Vec<Value> = conversation
        .iter()
        .filter_map(|turn| match turn {
            Turn::User { content } if !content.is_empty() => {
                Some(json!({"role": "user", "content": content}))
            }
            Turn::Assistant { content, .. } if !content.is_empty() => {
                Some(json!({"role": "assistant", "content": content}))
            }
            _ => None,
        })
        .collect();
    let recent = &messages[messages.len().saturating_sub(CONTEXT_MESSAGES)..];
    json!({
        "conversation": recent,
        "proposed_action": {
            "name": action.name,
            "description": action.description,
            "arguments": action.arguments,
        },
    })
}

fn unexpected(model: &Resolved) -> ProviderError {
    ProviderError::new(format!(
        "{} returned an unexpected response.",
        model.provider.label()
    ))
}

async fn with_decision_model(
    http: &dyn Transport,
    model: &Resolved,
    state: &Value,
) -> Result<(f64, Value, Value), ProviderError> {
    let mut questions = Map::new();
    questions.insert(
        "asked".into(),
        json!({
            "type": "noul",
            "instructions": QUESTION,
            "criteria": {"true": YES, "false": NO},
        }),
    );
    let call = model
        .provider
        .choose(http, &model.cfg, &model.model, state, &questions)
        .await?;
    let probability = call
        .data
        .get("asked")
        .and_then(|a| a.get("noul"))
        .and_then(Value::as_f64)
        .ok_or_else(|| unexpected(model))?;
    Ok((probability, call.request, call.response))
}

async fn with_language_model(
    http: &dyn Transport,
    model: &Resolved,
    state: &Value,
) -> Result<(f64, Value, Value), ProviderError> {
    let prompt = format!(
        "{}\n\n{QUESTION}\n\nYes: {YES}.\nNo: {NO}.\n\nGive the probability, from 0 to 1, that \
         the answer is yes.",
        serde_json::to_string_pretty(state).expect("JSON serialises")
    );
    let schema = json!({
        "type": "object",
        "properties": {"probability": {"type": "number"}},
        "required": ["probability"],
        "additionalProperties": false,
    });
    let call = model
        .provider
        .complete_json(
            http,
            &model.cfg,
            JsonRequest {
                model: &model.model,
                system: SYSTEM,
                prompt: &prompt,
                schema: &schema,
                name: "check_action",
            },
        )
        .await?;
    let probability = call
        .data
        .get("probability")
        .and_then(Value::as_f64)
        .ok_or_else(|| unexpected(model))?;
    Ok((probability.clamp(0.0, 1.0), call.request, call.response))
}

/// How likely it is that the user asked for this action, given the conversation so far. Fails
/// if the model can't be reached or isn't set up; a call to it is logged either way.
pub async fn check_action(
    db: &SqlitePool,
    http: &dyn Transport,
    conversation: &[Turn],
    action: &Proposed<'_>,
) -> Result<f64, ProviderError> {
    let model = match providers::resolve_task(db, Task::CheckActions).await {
        Ok(model) => model,
        Err(ResolveError::Provider(e)) => return Err(e),
        Err(ResolveError::Setting(e)) => return Err(ProviderError::new(e.to_string())),
    };
    let state = state(conversation, action);
    let started = Instant::now();
    let checked = match model.provider.kind() {
        Kind::Decision => with_decision_model(http, &model, &state).await,
        Kind::Language => with_language_model(http, &model, &state).await,
    };
    let label = model.label();
    match &checked {
        Ok((_, request, response)) => {
            log_call(
                db,
                &label,
                "check_action",
                started,
                Some(request),
                Some(response),
                None,
            )
            .await;
        }
        Err(e) => {
            log_call(
                db,
                &label,
                "check_action",
                started,
                None,
                None,
                Some(&e.message),
            )
            .await;
        }
    }
    checked.map(|(probability, ..)| probability)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_check_reads_only_the_latest_messages_with_text() {
        let mut conversation = vec![];
        for i in 0..5 {
            conversation.push(Turn::User {
                content: format!("question {i}"),
            });
            conversation.push(Turn::Assistant {
                content: String::new(),
                tool_calls: vec![],
                raw: None,
                raw_provider: None,
            });
            conversation.push(Turn::Assistant {
                content: format!("answer {i}"),
                tool_calls: vec![],
                raw: None,
                raw_provider: None,
            });
        }
        let arguments = Map::new();
        let action = Proposed {
            name: "create_category",
            description: "Creates a category.",
            arguments: &arguments,
        };
        let state = state(&conversation, &action);
        let read: Vec<&str> = state["conversation"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["content"].as_str().unwrap())
            .collect();
        assert_eq!(
            read,
            [
                "question 2",
                "answer 2",
                "question 3",
                "answer 3",
                "question 4",
                "answer 4"
            ]
        );
        assert_eq!(state["proposed_action"]["name"], "create_category");
    }
}
