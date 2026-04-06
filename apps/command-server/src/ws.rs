use crate::app_state::AppState;
use crate::session::handle_node_socket;
use axum::extract::ws::WebSocketUpgrade;
use axum::extract::{Path, State};
use axum::response::IntoResponse;
use std::sync::Arc;

pub(crate) async fn node_ws(
    State(state): State<Arc<AppState>>,
    Path(node_id): Path<String>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_node_socket(state, node_id, socket))
}
