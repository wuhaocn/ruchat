use crate::app_state::AppState;
use crate::session::handle_agent_socket;
use axum::extract::ws::WebSocketUpgrade;
use axum::extract::{Path, State};
use axum::response::IntoResponse;
use std::sync::Arc;

pub(crate) async fn agent_ws(
    State(state): State<Arc<AppState>>,
    Path(agent_id): Path<String>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_agent_socket(state, agent_id, socket))
}
