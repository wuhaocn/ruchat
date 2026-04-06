use crate::app_state::AppState;
use crate::console;
use crate::db::{CancelTaskOutcome, CreateTaskOutcome};
use crate::error::{internal_error, ApiResult};
use crate::ws::agent_ws;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use ru_command_protocol::{
    AgentSnapshot, BootstrapRequest, BootstrapResponse, CancelTaskRequest, CreateTaskRequest,
    TaskSnapshot,
};
use std::sync::Arc;

pub(crate) fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .merge(console::router())
        .route("/health", get(health))
        .route("/api/v1/bootstrap", post(bootstrap))
        .route("/api/v1/agents", get(list_agents))
        .route("/api/v1/agents/:agent_id/tasks", get(list_agent_tasks))
        .route("/api/v1/agents/:agent_id", get(get_agent))
        .route("/api/v1/tasks", get(list_tasks).post(create_task))
        .route("/api/v1/tasks/:task_id", get(get_task))
        .route("/api/v1/tasks/:task_id/cancel", post(cancel_task))
        .route("/ws/agents/:agent_id", get(agent_ws))
        .with_state(state)
}

pub(crate) fn control_router(state: Arc<AppState>) -> Router {
    Router::new()
        .merge(console::router())
        .route("/health", get(health))
        .route("/api/v1/bootstrap", post(bootstrap))
        .route("/api/v1/agents", get(list_agents))
        .route("/api/v1/agents/:agent_id/tasks", get(list_agent_tasks))
        .route("/api/v1/agents/:agent_id", get(get_agent))
        .route("/api/v1/tasks", get(list_tasks).post(create_task))
        .route("/api/v1/tasks/:task_id", get(get_task))
        .route("/api/v1/tasks/:task_id/cancel", post(cancel_task))
        .with_state(state)
}

#[derive(serde::Deserialize)]
struct AgentTaskListQuery {
    limit: Option<usize>,
}

pub(crate) fn ws_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/ws/agents/:agent_id", get(agent_ws))
        .with_state(state)
}

async fn health() -> &'static str {
    "ok"
}

async fn bootstrap(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<BootstrapRequest>,
) -> ApiResult<BootstrapResponse> {
    state
        .auth
        .verify_agent_token(&request.agent_id, &request.auth_token)
        .map_err(|message| (axum::http::StatusCode::UNAUTHORIZED, message.to_string()))?;

    let agent_id = request.agent_id.clone();
    let ws_url = state.resolve_ws_url(&headers, &agent_id)?;
    Ok(Json(BootstrapResponse {
        agent_id,
        ws_url,
        heartbeat_interval_secs: state.heartbeat_interval_secs,
    }))
}

async fn list_agents(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> ApiResult<Vec<AgentSnapshot>> {
    require_admin_session(&state, &headers)?;
    state.db.list_agents().map(Json).map_err(internal_error)
}

async fn get_agent(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(agent_id): Path<String>,
) -> ApiResult<AgentSnapshot> {
    require_admin_session(&state, &headers)?;
    match state.db.get_agent(&agent_id).map_err(internal_error)? {
        Some(agent) => Ok(Json(agent)),
        None => Err((
            axum::http::StatusCode::NOT_FOUND,
            "agent not found".to_string(),
        )),
    }
}

async fn list_agent_tasks(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(agent_id): Path<String>,
    Query(query): Query<AgentTaskListQuery>,
) -> ApiResult<Vec<TaskSnapshot>> {
    require_admin_session(&state, &headers)?;
    let limit = query.limit.unwrap_or(20).clamp(1, 100);
    state
        .db
        .list_tasks_for_agent(&agent_id, limit)
        .map(Json)
        .map_err(internal_error)
}

async fn create_task(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<CreateTaskRequest>,
) -> ApiResult<TaskSnapshot> {
    require_admin_session(&state, &headers)?;
    match state.db.create_task(request).map_err(internal_error)? {
        CreateTaskOutcome::AgentNotFound => Err((
            axum::http::StatusCode::NOT_FOUND,
            "agent not found".to_string(),
        )),
        CreateTaskOutcome::UnsupportedCommand => Err((
            axum::http::StatusCode::BAD_REQUEST,
            "command is not supported by the agent".to_string(),
        )),
        CreateTaskOutcome::ExtraArgsNotAllowed => Err((
            axum::http::StatusCode::BAD_REQUEST,
            "command does not allow extra args".to_string(),
        )),
        CreateTaskOutcome::Created(task) => {
            state.notify_agent(&task.agent_id);
            Ok(Json(task))
        }
    }
}

async fn list_tasks(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> ApiResult<Vec<TaskSnapshot>> {
    require_admin_session(&state, &headers)?;
    state.db.list_tasks().map(Json).map_err(internal_error)
}

async fn get_task(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(task_id): Path<u64>,
) -> ApiResult<TaskSnapshot> {
    require_admin_session(&state, &headers)?;
    match state.db.get_task(task_id).map_err(internal_error)? {
        Some(task) => Ok(Json(task)),
        None => Err((
            axum::http::StatusCode::NOT_FOUND,
            "task not found".to_string(),
        )),
    }
}

async fn cancel_task(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(task_id): Path<u64>,
    Json(request): Json<CancelTaskRequest>,
) -> ApiResult<TaskSnapshot> {
    require_admin_session(&state, &headers)?;
    match state
        .db
        .cancel_task(task_id, request.reason)
        .map_err(internal_error)?
    {
        CancelTaskOutcome::NotFound => Err((
            axum::http::StatusCode::NOT_FOUND,
            "task not found".to_string(),
        )),
        CancelTaskOutcome::NotCancelable => Err((
            axum::http::StatusCode::CONFLICT,
            "task is already running or finished".to_string(),
        )),
        CancelTaskOutcome::Canceled(task) => {
            state.notify_task_cancel(&task.agent_id, task.task_id, task.cancel_reason.clone());
            Ok(Json(task))
        }
    }
}

fn require_admin_session(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<(), (StatusCode, String)> {
    if state.has_valid_admin_session(headers) {
        Ok(())
    } else {
        Err((StatusCode::UNAUTHORIZED, "admin login required".to_string()))
    }
}
