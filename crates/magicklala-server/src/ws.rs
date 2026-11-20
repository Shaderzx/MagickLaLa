use std::collections::VecDeque;
use std::sync::Arc;

use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use futures::{SinkExt, StreamExt};
use magicklala_runtime::{RuntimeEvent, correlation_id};
use serde::Deserialize;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::{Semaphore, mpsc};
use tokio_util::sync::CancellationToken;
use utoipa::IntoParams;
use uuid::Uuid;

use crate::AppState;
use crate::access::origin_rejected;
use crate::api::{ChatRequest, deliver_chat};
use crate::error::{ApiError, ApiQuery};

pub const MAX_CONCURRENT_SOCKET_CHATS: usize = 8;
const SOCKET_REPLY_BUFFER: usize = 64;
const TRACKED_CORRELATIONS: usize = 512;

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ClientMessage {
    Ping,
    Chat {
        agent_id: Uuid,
        content: String,
        #[serde(default)]
        channel: Option<String>,
        #[serde(default)]
        sender: Option<String>,
    },
}

#[derive(Deserialize, IntoParams)]
pub struct SocketQuery {
    pub agent_id: Option<Uuid>,
}

#[utoipa::path(
    get,
    path = "/ws",
    tag = "system",
    params(SocketQuery, ("ticket" = Option<String>, Query, description = "Single-use ticket from POST /api/v1/ws-ticket for clients that cannot send an Authorization header")),
    responses(
        (status = 101, description = "Upgrades to a WebSocket streaming RuntimeEvent JSON frames, scoped to agent_id when given; accepts ping and chat messages. Authenticate with an Authorization bearer header or a single-use ticket from POST /api/v1/ws-ticket; the API token is never accepted in the URL"),
        (status = 400, description = "Invalid query"),
        (status = 403, description = "The Origin header is not an allowed Studio origin")
    )
)]
pub async fn websocket(
    State(state): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    ApiQuery(query): ApiQuery<SocketQuery>,
    upgrade: WebSocketUpgrade,
) -> Response {
    if origin_rejected(&state.access, &headers, &uri) {
        return ApiError::new(StatusCode::FORBIDDEN, "origin_not_allowed", "this Origin may not open the event socket; add it to MAGICKLALA_CORS_ORIGINS")
            .into_response();
    }
    let scope = EventScope::new(query.agent_id);
    upgrade.on_upgrade(move |socket| session(socket, state, scope))
}

struct EventScope {
    agent: Option<Uuid>,
    correlations: VecDeque<String>,
}

impl EventScope {
    fn new(agent: Option<Uuid>) -> Self {
        Self { agent, correlations: VecDeque::new() }
    }

    fn admits(&mut self, event: &RuntimeEvent) -> bool {
        let Some(agent) = self.agent else { return true };
        match event {
            RuntimeEvent::AgentStatusChanged { agent_id, .. } | RuntimeEvent::RunCompleted { agent_id, .. } => *agent_id == agent,
            RuntimeEvent::Trace { agent_id, .. } => *agent_id == Some(agent),
            RuntimeEvent::Inbound { event } => {
                let admitted = event.agent_id == agent;
                if admitted && let Some(id) = correlation_id(&event.payload) {
                    self.remember(id);
                }
                admitted
            }
            RuntimeEvent::Outbound { action } => correlation_id(&action.payload).is_some_and(|id| self.correlations.iter().any(|known| known == id)),
        }
    }

    fn remember(&mut self, id: &str) {
        if self.correlations.len() == TRACKED_CORRELATIONS {
            self.correlations.pop_front();
        }
        self.correlations.push_back(id.to_owned());
    }
}

fn frame(value: &serde_json::Value) -> Message {
    Message::Text(value.to_string().into())
}

async fn session(socket: WebSocket, state: AppState, mut scope: EventScope) {
    let (mut sink, mut stream) = socket.split();
    let mut events = state.supervisor.bus().subscribe();
    let closing = CancellationToken::new();
    let permits = Arc::new(Semaphore::new(MAX_CONCURRENT_SOCKET_CHATS));
    let (replies, mut outgoing) = mpsc::channel::<serde_json::Value>(SOCKET_REPLY_BUFFER);
    loop {
        let next = tokio::select! {
            event = events.recv() => match event {
                Ok(event) if scope.admits(&event) => serde_json::to_value(&event).ok(),
                Ok(_) => None,
                Err(RecvError::Lagged(skipped)) => Some(serde_json::json!({ "type": "lagged", "skipped": skipped })),
                Err(RecvError::Closed) => break,
            },
            reply = outgoing.recv() => reply,
            inbound = stream.next() => match inbound {
                Some(Ok(Message::Text(text))) => handle(&state, text.as_str(), &permits, &replies, &closing),
                Some(Ok(Message::Close(_)) | Err(_)) | None => break,
                Some(Ok(_)) => None,
            },
        };
        if let Some(value) = next
            && sink.send(frame(&value)).await.is_err()
        {
            break;
        }
    }
    closing.cancel();
}

fn handle(
    state: &AppState,
    text: &str,
    permits: &Arc<Semaphore>,
    replies: &mpsc::Sender<serde_json::Value>,
    closing: &CancellationToken,
) -> Option<serde_json::Value> {
    match serde_json::from_str::<ClientMessage>(text) {
        Ok(ClientMessage::Ping) => Some(serde_json::json!({ "type": "pong" })),
        Ok(ClientMessage::Chat { agent_id, content, channel, sender }) => {
            let Ok(permit) = Arc::clone(permits).try_acquire_owned() else {
                return Some(
                    serde_json::json!({ "type": "error", "code": "too_many_chats", "error": format!("at most {MAX_CONCURRENT_SOCKET_CHATS} chats may be in flight on one socket") }),
                );
            };
            let request =
                ChatRequest { content, channel: channel.unwrap_or_else(|| "websocket".to_owned()), sender: sender.unwrap_or_else(|| "user".to_owned()) };
            let (state, replies, closing) = (state.clone(), replies.clone(), closing.clone());
            tokio::spawn(async move {
                let _permit = permit;
                let answer = tokio::select! {
                    () = closing.cancelled() => return,
                    exchange = deliver_chat(&state, agent_id, request) => match exchange {
                        Ok(exchange) => serde_json::json!({ "type": "chat_accepted", "agent_id": agent_id, "message": exchange.message, "reply": exchange.reply }),
                        Err(error) => serde_json::json!({ "type": "error", "agent_id": agent_id, "error": error.message, "code": error.code }),
                    },
                };
                tokio::select! {
                    () = closing.cancelled() => {}
                    _ = replies.send(answer) => {}
                }
            });
            None
        }
        Err(error) => Some(serde_json::json!({ "type": "error", "error": error.to_string(), "code": "invalid_message" })),
    }
}
