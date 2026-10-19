//! `/api/chat`: conversations with the chat task's model, which looks things up and makes
//! changes through the actions.

use std::convert::Infallible;

use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::response::Response;
use axum::routing::{get, post};
use axum::{Json, Router};
use pennychest_core::chat::{self, Conversation, DisplayTurn, Event};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::AppState;
use crate::error::{ApiError, ApiResult, JsonBody, PathParam};

const MAX_MESSAGE: usize = 4000;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/conversations", get(list))
        .route("/conversations/{id}", get(show).delete(delete_conversation))
        .route("/messages", post(send))
}

async fn list(State(state): State<AppState>) -> ApiResult<Json<Vec<Conversation>>> {
    Ok(Json(chat::conversations(&state.db).await?))
}

async fn find(state: &AppState, id: i64) -> ApiResult<Conversation> {
    chat::conversation(&state.db, id)
        .await?
        .ok_or_else(|| ApiError::not_found("Conversation not found"))
}

#[derive(Serialize)]
struct Shown {
    id: i64,
    title: String,
    turns: Vec<DisplayTurn>,
}

async fn show(
    State(state): State<AppState>,
    PathParam(id): PathParam<i64>,
) -> ApiResult<Json<Shown>> {
    let conversation = find(&state, id).await?;
    Ok(Json(Shown {
        id,
        title: conversation.title,
        turns: chat::display_turns(&state.db, id).await?,
    }))
}

async fn delete_conversation(
    State(state): State<AppState>,
    PathParam(id): PathParam<i64>,
) -> ApiResult<StatusCode> {
    if chat::delete(&state.db, id).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::not_found("Conversation not found"))
    }
}

#[derive(Deserialize)]
struct SendMessage {
    message: String,
    #[serde(default)]
    conversation_id: Option<i64>,
}

/// Sends a message and streams the turn back as server-sent events: `conversation`, then the
/// model's `text` and any `tool`, `tool_result` and `change` steps, then `message` (or
/// `error`), then `done`. The turn finishes even if the reader goes away.
async fn send(
    State(state): State<AppState>,
    JsonBody(body): JsonBody<SendMessage>,
) -> ApiResult<Response> {
    let length = body.message.chars().count();
    if length == 0 {
        return Err(ApiError::invalid("message: must have at least 1 character"));
    }
    if length > MAX_MESSAGE {
        return Err(ApiError::invalid(format!(
            "message: must have at most {MAX_MESSAGE} characters"
        )));
    }
    let text = body.message.trim().to_owned();
    if text.is_empty() {
        return Err(ApiError::bad_request("Message is empty"));
    }
    let conversation = match body.conversation_id {
        Some(id) => find(&state, id).await?,
        None => chat::start(&state.db, &text).await?,
    };

    let (events, mut received) = mpsc::unbounded_channel();
    let (db, http) = (state.db.clone(), state.http.clone());
    tokio::spawn(async move {
        let _ = events.send(Event::Conversation {
            id: conversation.id,
            title: conversation.title,
        });
        let scopes = chat::granted_scopes(&db).await;
        let today = chrono::Local::now().date_naive();
        let turn = chat::run_turn(
            &db,
            http.as_ref(),
            conversation.id,
            &text,
            &scopes,
            today,
            &events,
        );
        if let Err(e) = turn.await {
            tracing::error!("A chat turn failed: {e}");
            let _ = events.send(Event::Error {
                detail: "Something went wrong on the server. The details are in its log.".into(),
            });
        }
        let _ = events.send(Event::Done);
    });

    let stream = futures_util::stream::poll_fn(move |cx| {
        received.poll_recv(cx).map(|event| {
            event.map(|event| {
                let (name, data) = event.parts();
                Ok::<_, Infallible>(Bytes::from(format!("event: {name}\ndata: {data}\n\n")))
            })
        })
    });
    Response::builder()
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache")
        .header("X-Accel-Buffering", "no")
        .body(Body::from_stream(stream))
        .map_err(ApiError::internal)
}
