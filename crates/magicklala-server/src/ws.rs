use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::Response;
use serde::Deserialize;
use tokio::sync::broadcast::error::RecvError;
use uuid::Uuid;

use crate::AppState;
use crate::api::{ChatRequest, deliver_chat};

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

#[utoipa::path(get, path = "/ws", tag = "system", responses((status = 101, description = "Upgrades to a WebSocket streaming RuntimeEvent JSON frames; accepts ping and chat messages")))]
pub async fn websocket(State(state): State<AppState>, upgrade: WebSocketUpgrade) -> Response {
    upgrade.on_upgrade(move |socket| session(socket, state))
}

async fn session(mut socket: WebSocket, state: AppState) {
    let mut events = state.supervisor.bus().subscribe();
    loop {
        let outgoing = tokio::select! {
            event = events.recv() => match event {
                Ok(event) => serde_json::to_string(&event).ok(),
                Err(RecvError::Lagged(skipped)) => Some(serde_json::json!({ "type": "lagged", "skipped": skipped }).to_string()),
                Err(RecvError::Closed) => break,
            },
            inbound = socket.recv() => match inbound {
                Some(Ok(Message::Text(text))) => Some(reply(&state, text.as_str()).await),
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                Some(Ok(_)) => None,
            },
        };
        if let Some(frame) = outgoing
            && socket.send(Message::Text(frame.into())).await.is_err()
        {
            break;
        }
    }
}

async fn reply(state: &AppState, text: &str) -> String {
    match serde_json::from_str::<ClientMessage>(text) {
        Ok(ClientMessage::Ping) => serde_json::json!({ "type": "pong" }).to_string(),
        Ok(ClientMessage::Chat { agent_id, content, channel, sender }) => {
            let request =
                ChatRequest { content, channel: channel.unwrap_or_else(|| "websocket".to_owned()), sender: sender.unwrap_or_else(|| "user".to_owned()) };
            match deliver_chat(state, agent_id, request).await {
                Ok(message) => serde_json::json!({ "type": "chat_accepted", "message": message }).to_string(),
                Err(error) => serde_json::json!({ "type": "error", "error": error.message, "code": error.code }).to_string(),
            }
        }
        Err(error) => serde_json::json!({ "type": "error", "error": error.to_string(), "code": "invalid_message" }).to_string(),
    }
}
