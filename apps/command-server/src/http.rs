use crate::app_state::AppState;
use crate::console;
use crate::db::{CancelTaskOutcome, CreateTaskOutcome, TaskAudit};
use crate::error::{internal_error, ApiResult};
use crate::ws::node_ws;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use ru_command_protocol::{
    BootstrapRequest, BootstrapResponse, CancelTaskRequest, CreateTaskRequest, NodeSnapshot,
    ProtocolSnapshot, SessionEvent, SessionEventKind, TaskSnapshot, CONTROL_CAPABILITIES,
    CONTROL_PROTOCOL_VERSION, CONTROL_TRANSPORT_STACK,
};
use std::sync::Arc;

pub(crate) fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .merge(console::router())
        .route("/health", get(health))
        .route("/api/v1/bootstrap", post(bootstrap))
        .route("/api/v1/protocol", get(protocol_snapshot))
        .route("/api/v1/session-events", get(list_session_events))
        .route("/api/v1/nodes", get(list_nodes))
        .route(
            "/api/v1/nodes/:node_id/session-events",
            get(list_node_session_events),
        )
        .route("/api/v1/nodes/:node_id/tasks", get(list_node_tasks))
        .route("/api/v1/nodes/:node_id", get(get_node))
        .route("/api/v1/tasks", get(list_tasks).post(create_task))
        .route("/api/v1/tasks/:task_id", get(get_task))
        .route("/api/v1/tasks/:task_id/cancel", post(cancel_task))
        .route("/ws/nodes/:node_id", get(node_ws))
        .with_state(state)
}

pub(crate) fn control_router(state: Arc<AppState>) -> Router {
    Router::new()
        .merge(console::router())
        .route("/health", get(health))
        .route("/api/v1/bootstrap", post(bootstrap))
        .route("/api/v1/protocol", get(protocol_snapshot))
        .route("/api/v1/session-events", get(list_session_events))
        .route("/api/v1/nodes", get(list_nodes))
        .route(
            "/api/v1/nodes/:node_id/session-events",
            get(list_node_session_events),
        )
        .route("/api/v1/nodes/:node_id/tasks", get(list_node_tasks))
        .route("/api/v1/nodes/:node_id", get(get_node))
        .route("/api/v1/tasks", get(list_tasks).post(create_task))
        .route("/api/v1/tasks/:task_id", get(get_task))
        .route("/api/v1/tasks/:task_id/cancel", post(cancel_task))
        .with_state(state)
}

#[derive(serde::Deserialize)]
struct NodeTaskListQuery {
    limit: Option<usize>,
}

#[derive(serde::Deserialize)]
pub(crate) struct SessionEventListQuery {
    pub(crate) limit: Option<usize>,
    pub(crate) node_id: Option<String>,
    pub(crate) kind: Option<String>,
}

pub(crate) fn ws_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/ws/nodes/:node_id", get(node_ws))
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
        .verify_node_token(&request.node_id, &request.auth_token)
        .map_err(|message| (axum::http::StatusCode::UNAUTHORIZED, message.to_string()))?;

    let node_id = request.node_id.clone();
    let ws_url = state.resolve_ws_url(&headers, &node_id)?;
    Ok(Json(BootstrapResponse {
        node_id,
        ws_url,
        heartbeat_interval_secs: state.heartbeat_interval_secs,
        protocol_version: CONTROL_PROTOCOL_VERSION.to_string(),
        transport_stack: CONTROL_TRANSPORT_STACK.to_string(),
        capabilities: CONTROL_CAPABILITIES
            .iter()
            .map(|item| (*item).to_string())
            .collect(),
    }))
}

async fn protocol_snapshot(State(state): State<Arc<AppState>>) -> ApiResult<ProtocolSnapshot> {
    Ok(Json(state.protocol_snapshot()))
}

async fn list_nodes(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> ApiResult<Vec<NodeSnapshot>> {
    require_admin_session(&state, &headers)?;
    state
        .db
        .list_nodes()
        .map(|nodes| state.enrich_node_snapshots(nodes))
        .map(Json)
        .map_err(internal_error)
}

async fn list_session_events(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<SessionEventListQuery>,
) -> ApiResult<Vec<SessionEvent>> {
    require_admin_session(&state, &headers)?;
    let limit = query.limit.unwrap_or(20).clamp(1, 100);
    let kind = parse_session_event_kind(query.kind.as_deref())?;
    state
        .db
        .list_session_events(query.node_id.as_deref(), kind, limit)
        .map(Json)
        .map_err(internal_error)
}

async fn get_node(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(node_id): Path<String>,
) -> ApiResult<NodeSnapshot> {
    require_admin_session(&state, &headers)?;
    match state.db.get_node(&node_id).map_err(internal_error)? {
        Some(node) => Ok(Json(state.enrich_node_snapshot(node))),
        None => Err((
            axum::http::StatusCode::NOT_FOUND,
            "node not found".to_string(),
        )),
    }
}

async fn list_node_tasks(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(node_id): Path<String>,
    Query(query): Query<NodeTaskListQuery>,
) -> ApiResult<Vec<TaskSnapshot>> {
    require_admin_session(&state, &headers)?;
    let limit = query.limit.unwrap_or(20).clamp(1, 100);
    state
        .db
        .list_tasks_for_node(&node_id, limit)
        .map(Json)
        .map_err(internal_error)
}

async fn list_node_session_events(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(node_id): Path<String>,
    Query(query): Query<NodeTaskListQuery>,
) -> ApiResult<Vec<SessionEvent>> {
    require_admin_session(&state, &headers)?;
    let limit = query.limit.unwrap_or(20).clamp(1, 100);
    state
        .db
        .list_session_events_for_node(&node_id, limit)
        .map(Json)
        .map_err(internal_error)
}

async fn create_task(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<CreateTaskRequest>,
) -> ApiResult<TaskSnapshot> {
    require_admin_session(&state, &headers)?;
    match state
        .db
        .create_task(request, TaskAudit::new("api", Some(state.admin_actor())))
        .map_err(internal_error)?
    {
        CreateTaskOutcome::NodeNotFound => Err((
            axum::http::StatusCode::NOT_FOUND,
            "node not found".to_string(),
        )),
        CreateTaskOutcome::UnsupportedCommand => Err((
            axum::http::StatusCode::BAD_REQUEST,
            "command is not supported by the node".to_string(),
        )),
        CreateTaskOutcome::ExtraArgsNotAllowed => Err((
            axum::http::StatusCode::BAD_REQUEST,
            "command does not allow extra args".to_string(),
        )),
        CreateTaskOutcome::Created(task) => {
            state.notify_node(&task.node_id);
            Ok(Json(task))
        }
    }
}

async fn list_tasks(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<NodeTaskListQuery>,
) -> ApiResult<Vec<TaskSnapshot>> {
    require_admin_session(&state, &headers)?;
    if let Some(limit) = query.limit {
        let limit = limit.clamp(1, 100);
        state
            .db
            .list_recent_tasks(limit)
            .map(Json)
            .map_err(internal_error)
    } else {
        state.db.list_tasks().map(Json).map_err(internal_error)
    }
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
        .cancel_task(
            task_id,
            request.reason,
            TaskAudit::new("api", Some(state.admin_actor())),
        )
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
            state.notify_task_cancel(&task.node_id, task.task_id, task.cancel_reason.clone());
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

fn parse_session_event_kind(
    value: Option<&str>,
) -> Result<Option<SessionEventKind>, (StatusCode, String)> {
    match value {
        None | Some("") => Ok(None),
        Some("connect_rejected") => Ok(Some(SessionEventKind::ConnectRejected)),
        Some("auth_failed") => Ok(Some(SessionEventKind::AuthFailed)),
        Some("session_opened") => Ok(Some(SessionEventKind::SessionOpened)),
        Some("node_registered") => Ok(Some(SessionEventKind::NodeRegistered)),
        Some("session_closed") => Ok(Some(SessionEventKind::SessionClosed)),
        Some(other) => Err((
            StatusCode::BAD_REQUEST,
            format!("unknown session event kind: {other}"),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::router;
    use crate::app_state::AppState;
    use crate::auth::AuthConfig;
    use crate::db::{CreateTaskOutcome, Database, TaskAudit};
    use axum::Server;
    use futures_util::{SinkExt, StreamExt};
    use hyper::body::to_bytes;
    use hyper::client::HttpConnector;
    use hyper::header::{CONTENT_TYPE, COOKIE};
    use hyper::{Body, Client, Method, Request};
    use ru_command_protocol::pb_mqtt_frame::Body as MqttFrameBody;
    use ru_command_protocol::pb_node_payload_envelope::Body as NodePayloadBody;
    use ru_command_protocol::{
        node_ack_topic, node_control_topic, node_hello_topic, node_task_topic, BootstrapRequest,
        BootstrapResponse, CreateTaskRequest, NodeSnapshot, PbClientHello, PbCommandCatalog,
        PbCommandDescriptor, PbMqttConnect, PbMqttFrame, PbMqttPingResp, PbMqttPublish,
        PbMqttSubscribe, PbNodePayloadEnvelope, PbTaskAck, PbTaskPullRequest, PbTaskResult,
        ProtocolSnapshot, SessionEvent, SessionEventKind, TaskStatus,
    };
    use std::net::TcpListener;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};
    use tokio::sync::oneshot;
    use tokio_tungstenite::connect_async;
    use tokio_tungstenite::tungstenite::Message as WsMessage;

    #[tokio::test]
    async fn protocol_snapshot_endpoint_reports_server_contract() {
        let db_path = temp_db_path("command-plane-http-protocol-e2e");
        let state = Arc::new(AppState::new(
            Database::open(db_path.to_str().unwrap()).unwrap(),
            30,
            None,
            AuthConfig::new(Some("dev-shared-token".to_string()), Default::default()).unwrap(),
            "admin".to_string(),
            "admin123".to_string(),
            3600,
        ));

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        let app = router(state.clone());
        let server = Server::from_tcp(listener)
            .unwrap()
            .serve(app.into_make_service())
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            });
        let server_task = tokio::spawn(server);

        let client: Client<HttpConnector> = Client::new();
        let request = Request::builder()
            .method(Method::GET)
            .uri(format!("http://127.0.0.1:{}/api/v1/protocol", addr.port()))
            .body(Body::empty())
            .unwrap();

        let response = client.request(request).await.unwrap();
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let body = to_bytes(response.into_body()).await.unwrap();
        let snapshot: ProtocolSnapshot = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            snapshot.protocol_version,
            ru_command_protocol::CONTROL_PROTOCOL_VERSION
        );
        assert_eq!(
            snapshot.transport_stack,
            ru_command_protocol::CONTROL_TRANSPORT_STACK
        );
        assert_eq!(snapshot.heartbeat_interval_secs, 30);
        assert_eq!(
            snapshot.max_result_output_bytes,
            ru_command_protocol::MAX_RESULT_OUTPUT_BYTES as u64
        );
        assert_eq!(
            snapshot.max_task_pull_response_items,
            ru_command_protocol::MAX_TASK_PULL_RESPONSE_ITEMS
        );
        assert!(snapshot.capabilities.contains(&"task_push".to_string()));
        assert!(snapshot.capabilities.contains(&"task_pull".to_string()));

        let _ = shutdown_tx.send(());
        server_task.await.unwrap().unwrap();
        let _ = std::fs::remove_file(&db_path);
        let _ = std::fs::remove_file(format!("{}-shm", db_path.display()));
        let _ = std::fs::remove_file(format!("{}-wal", db_path.display()));
    }

    #[tokio::test]
    async fn bootstrap_ws_hello_task_ack_result_roundtrip() {
        let db_path = temp_db_path("command-plane-http-e2e");
        let state = Arc::new(AppState::new(
            Database::open(db_path.to_str().unwrap()).unwrap(),
            30,
            None,
            AuthConfig::new(Some("dev-shared-token".to_string()), Default::default()).unwrap(),
            "admin".to_string(),
            "admin123".to_string(),
            3600,
        ));

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        let app = router(state.clone());
        let server = Server::from_tcp(listener)
            .unwrap()
            .serve(app.into_make_service())
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            });
        let server_task = tokio::spawn(server);

        let client: Client<HttpConnector> = Client::new();
        let bootstrap = bootstrap(&client, addr.port()).await;
        assert_eq!(bootstrap.node_id, "node-e2e");
        assert_eq!(bootstrap.heartbeat_interval_secs, 30);
        assert_eq!(
            bootstrap.protocol_version,
            ru_command_protocol::CONTROL_PROTOCOL_VERSION
        );
        assert_eq!(
            bootstrap.transport_stack,
            ru_command_protocol::CONTROL_TRANSPORT_STACK
        );
        assert!(bootstrap.capabilities.contains(&"task_push".to_string()));
        assert!(bootstrap.capabilities.contains(&"task_pull".to_string()));

        let (mut socket, _) = connect_async(bootstrap.ws_url.as_str()).await.unwrap();

        send_ws_frame(
            &mut socket,
            PbMqttFrame {
                body: Some(MqttFrameBody::Connect(PbMqttConnect {
                    client_id: "node-e2e".to_string(),
                    clean_session: true,
                    auth_token: "dev-shared-token".to_string(),
                })),
            },
        )
        .await;

        let connack = recv_mqtt_frame(&mut socket).await;
        match connack.body {
            Some(MqttFrameBody::ConnAck(connack)) => assert!(connack.accepted),
            other => panic!("expected connack, got {:?}", other),
        }

        send_ws_frame(
            &mut socket,
            PbMqttFrame {
                body: Some(MqttFrameBody::Subscribe(PbMqttSubscribe {
                    topics: vec![node_task_topic("node-e2e"), node_control_topic("node-e2e")],
                })),
            },
        )
        .await;

        let suback = recv_mqtt_frame(&mut socket).await;
        match suback.body {
            Some(MqttFrameBody::SubAck(suback)) => {
                assert!(suback.topics.contains(&node_task_topic("node-e2e")));
                assert!(suback.topics.contains(&node_control_topic("node-e2e")));
            }
            other => panic!("expected suback, got {:?}", other),
        }

        let session_info_frame = recv_mqtt_frame(&mut socket).await;
        let publish = match session_info_frame.body {
            Some(MqttFrameBody::Publish(publish)) => publish,
            other => panic!("expected session info publish, got {:?}", other),
        };
        assert_eq!(publish.topic, node_control_topic("node-e2e"));
        let session_info_payload = PbNodePayloadEnvelope::decode_message(&publish.payload).unwrap();
        match session_info_payload.body {
            Some(NodePayloadBody::SessionInfo(info)) => {
                assert_eq!(info.node_id, "node-e2e");
                assert!(!info.protocol_version.is_empty());
            }
            other => panic!("expected session info payload, got {:?}", other),
        }

        send_publish(
            &mut socket,
            node_hello_topic("node-e2e"),
            PbNodePayloadEnvelope {
                body: Some(NodePayloadBody::ClientHello(PbClientHello {
                    node_id: "node-e2e".to_string(),
                    hostname: "test-host".to_string(),
                    platform: "darwin-arm64".to_string(),
                    poll_interval_secs: 3,
                })),
            },
        )
        .await;

        send_publish(
            &mut socket,
            node_hello_topic("node-e2e"),
            PbNodePayloadEnvelope {
                body: Some(NodePayloadBody::CommandCatalog(PbCommandCatalog {
                    commands: vec![PbCommandDescriptor {
                        name: "echo".to_string(),
                        description: "echo text".to_string(),
                        default_args: Vec::new(),
                        allow_extra_args: true,
                    }],
                })),
            },
        )
        .await;

        wait_for(
            || async {
                state
                    .db
                    .get_node("node-e2e")
                    .unwrap()
                    .map(|node| node.commands.len() == 1)
                    .unwrap_or(false)
            },
            Duration::from_secs(2),
        )
        .await;

        let node_snapshot = get_node_snapshot(&client, addr.port(), &state, "node-e2e").await;
        assert!(node_snapshot.online);
        assert_eq!(
            node_snapshot.session_protocol_version.as_deref(),
            Some(ru_command_protocol::CONTROL_PROTOCOL_VERSION)
        );
        assert_eq!(
            node_snapshot.session_transport_stack.as_deref(),
            Some(ru_command_protocol::CONTROL_TRANSPORT_STACK)
        );
        assert_eq!(node_snapshot.session_heartbeat_interval_secs, Some(30));
        assert!(node_snapshot
            .session_capabilities
            .contains(&"task_push".to_string()));
        let node_list = list_node_snapshots(&client, addr.port(), &state).await;
        assert_eq!(node_list.len(), 1);
        assert_eq!(node_list[0].node_id, "node-e2e");
        assert!(node_list[0].online);
        let session_events = get_session_events(&client, addr.port(), &state, "node-e2e").await;
        assert!(session_events
            .iter()
            .any(|event| event.kind == SessionEventKind::SessionOpened));
        assert!(session_events
            .iter()
            .any(|event| event.kind == SessionEventKind::NodeRegistered));
        let all_session_events = get_all_session_events(&client, addr.port(), &state).await;
        assert!(all_session_events
            .iter()
            .any(|event| event.kind == SessionEventKind::SessionOpened));
        assert!(all_session_events
            .iter()
            .any(|event| event.kind == SessionEventKind::NodeRegistered));
        assert!(all_session_events
            .iter()
            .all(|event| event.node_id == "node-e2e"));
        let registered_events = get_filtered_session_events(
            &client,
            addr.port(),
            &state,
            Some("node-e2e"),
            Some("node_registered"),
        )
        .await;
        assert_eq!(registered_events.len(), 1);
        assert_eq!(registered_events[0].kind, SessionEventKind::NodeRegistered);
        assert_eq!(registered_events[0].node_id, "node-e2e");

        let created_task = match state
            .db
            .create_task(
                CreateTaskRequest {
                    node_id: "node-e2e".to_string(),
                    command_name: "echo".to_string(),
                    args: vec!["hello".to_string(), "world".to_string()],
                    timeout_secs: Some(30),
                },
                TaskAudit::new("test", Some("http-e2e".to_string())),
            )
            .unwrap()
        {
            CreateTaskOutcome::Created(task) => task,
            _ => panic!("expected task creation to succeed"),
        };
        state.notify_node("node-e2e");

        let task_frame = recv_mqtt_frame(&mut socket).await;
        let publish = match task_frame.body {
            Some(MqttFrameBody::Publish(publish)) => publish,
            other => panic!("expected task publish, got {:?}", other),
        };
        assert_eq!(publish.topic, node_task_topic("node-e2e"));
        let task_payload = PbNodePayloadEnvelope::decode_message(&publish.payload).unwrap();
        let assigned_task = match task_payload.body {
            Some(NodePayloadBody::TaskAssignment(task)) => task,
            other => panic!("expected task assignment, got {:?}", other),
        };
        assert_eq!(assigned_task.task_id, created_task.task_id);
        assert_eq!(assigned_task.command_name, "echo");
        assert_eq!(assigned_task.args, vec!["hello", "world"]);

        send_publish(
            &mut socket,
            node_ack_topic("node-e2e"),
            PbNodePayloadEnvelope {
                body: Some(NodePayloadBody::TaskAck(PbTaskAck {
                    task_id: assigned_task.task_id,
                })),
            },
        )
        .await;

        send_publish(
            &mut socket,
            ru_command_protocol::node_result_topic("node-e2e"),
            PbNodePayloadEnvelope {
                body: Some(NodePayloadBody::TaskResult(PbTaskResult {
                    task_id: assigned_task.task_id,
                    success: true,
                    exit_code: Some(0),
                    stdout: "hello world\n".to_string(),
                    stderr: String::new(),
                    duration_ms: 12,
                    error: None,
                    stdout_truncated: false,
                    stderr_truncated: false,
                })),
            },
        )
        .await;

        wait_for(
            || async {
                state
                    .db
                    .get_task(created_task.task_id)
                    .unwrap()
                    .map(|task| {
                        task.status == TaskStatus::Succeeded
                            && task.result.as_ref().map(|result| result.stdout.as_str())
                                == Some("hello world\n")
                    })
                    .unwrap_or(false)
            },
            Duration::from_secs(2),
        )
        .await;

        let _ = shutdown_tx.send(());
        server_task.await.unwrap().unwrap();
        let _ = std::fs::remove_file(&db_path);
        let _ = std::fs::remove_file(format!("{}-shm", db_path.display()));
        let _ = std::fs::remove_file(format!("{}-wal", db_path.display()));
    }

    #[tokio::test]
    async fn bootstrap_ws_pull_request_returns_task_pull_response() {
        let db_path = temp_db_path("command-plane-http-pull-e2e");
        let state = Arc::new(AppState::new(
            Database::open(db_path.to_str().unwrap()).unwrap(),
            30,
            None,
            AuthConfig::new(Some("dev-shared-token".to_string()), Default::default()).unwrap(),
            "admin".to_string(),
            "admin123".to_string(),
            3600,
        ));

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        let app = router(state.clone());
        let server = Server::from_tcp(listener)
            .unwrap()
            .serve(app.into_make_service())
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            });
        let server_task = tokio::spawn(server);

        let client: Client<HttpConnector> = Client::new();
        let bootstrap = bootstrap(&client, addr.port()).await;
        let (mut socket, _) = connect_async(bootstrap.ws_url.as_str()).await.unwrap();

        send_ws_frame(
            &mut socket,
            PbMqttFrame {
                body: Some(MqttFrameBody::Connect(PbMqttConnect {
                    client_id: "node-e2e".to_string(),
                    clean_session: true,
                    auth_token: "dev-shared-token".to_string(),
                })),
            },
        )
        .await;

        match recv_mqtt_frame(&mut socket).await.body {
            Some(MqttFrameBody::ConnAck(connack)) => assert!(connack.accepted),
            other => panic!("expected connack, got {:?}", other),
        }

        send_ws_frame(
            &mut socket,
            PbMqttFrame {
                body: Some(MqttFrameBody::Subscribe(PbMqttSubscribe {
                    topics: vec![node_task_topic("node-e2e"), node_control_topic("node-e2e")],
                })),
            },
        )
        .await;

        match recv_mqtt_frame(&mut socket).await.body {
            Some(MqttFrameBody::SubAck(suback)) => {
                assert!(suback.topics.contains(&node_task_topic("node-e2e")));
                assert!(suback.topics.contains(&node_control_topic("node-e2e")));
            }
            other => panic!("expected suback, got {:?}", other),
        }

        let session_info_frame = recv_mqtt_frame(&mut socket).await;
        let publish = match session_info_frame.body {
            Some(MqttFrameBody::Publish(publish)) => publish,
            other => panic!("expected session info publish, got {:?}", other),
        };
        assert_eq!(publish.topic, node_control_topic("node-e2e"));

        send_publish(
            &mut socket,
            node_hello_topic("node-e2e"),
            PbNodePayloadEnvelope {
                body: Some(NodePayloadBody::ClientHello(PbClientHello {
                    node_id: "node-e2e".to_string(),
                    hostname: "test-host".to_string(),
                    platform: "darwin-arm64".to_string(),
                    poll_interval_secs: 3,
                })),
            },
        )
        .await;

        send_publish(
            &mut socket,
            node_hello_topic("node-e2e"),
            PbNodePayloadEnvelope {
                body: Some(NodePayloadBody::CommandCatalog(PbCommandCatalog {
                    commands: vec![PbCommandDescriptor {
                        name: "echo".to_string(),
                        description: "echo text".to_string(),
                        default_args: Vec::new(),
                        allow_extra_args: true,
                    }],
                })),
            },
        )
        .await;

        wait_for(
            || async {
                state
                    .db
                    .get_node("node-e2e")
                    .unwrap()
                    .map(|node| node.commands.len() == 1)
                    .unwrap_or(false)
            },
            Duration::from_secs(2),
        )
        .await;

        let created_task = match state
            .db
            .create_task(
                CreateTaskRequest {
                    node_id: "node-e2e".to_string(),
                    command_name: "echo".to_string(),
                    args: vec!["pull".to_string(), "path".to_string()],
                    timeout_secs: Some(30),
                },
                TaskAudit::new("test", Some("http-pull-e2e".to_string())),
            )
            .unwrap()
        {
            CreateTaskOutcome::Created(task) => task,
            _ => panic!("expected task creation to succeed"),
        };

        send_publish(
            &mut socket,
            node_control_topic("node-e2e"),
            PbNodePayloadEnvelope {
                body: Some(NodePayloadBody::TaskPullRequest(PbTaskPullRequest {
                    node_id: "node-e2e".to_string(),
                    limit: 1,
                })),
            },
        )
        .await;

        let pull_response_frame = recv_mqtt_frame(&mut socket).await;
        let publish = match pull_response_frame.body {
            Some(MqttFrameBody::Publish(publish)) => publish,
            other => panic!("expected task pull response publish, got {:?}", other),
        };
        assert_eq!(publish.topic, node_control_topic("node-e2e"));

        let payload = PbNodePayloadEnvelope::decode_message(&publish.payload).unwrap();
        let response = match payload.body {
            Some(NodePayloadBody::TaskPullResponse(response)) => response,
            other => panic!("expected task pull response payload, got {:?}", other),
        };
        assert_eq!(response.tasks.len(), 1);
        assert_eq!(response.tasks[0].task_id, created_task.task_id);
        assert_eq!(response.tasks[0].command_name, "echo");
        assert_eq!(response.tasks[0].args, vec!["pull", "path"]);

        let _ = shutdown_tx.send(());
        server_task.await.unwrap().unwrap();
        let _ = std::fs::remove_file(&db_path);
        let _ = std::fs::remove_file(format!("{}-shm", db_path.display()));
        let _ = std::fs::remove_file(format!("{}-wal", db_path.display()));
    }

    #[tokio::test]
    async fn bootstrap_ws_pull_request_rejects_mismatched_node_id() {
        let db_path = temp_db_path("command-plane-http-pull-mismatch-e2e");
        let state = Arc::new(AppState::new(
            Database::open(db_path.to_str().unwrap()).unwrap(),
            30,
            None,
            AuthConfig::new(Some("dev-shared-token".to_string()), Default::default()).unwrap(),
            "admin".to_string(),
            "admin123".to_string(),
            3600,
        ));

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        let app = router(state.clone());
        let server = Server::from_tcp(listener)
            .unwrap()
            .serve(app.into_make_service())
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            });
        let server_task = tokio::spawn(server);

        let client: Client<HttpConnector> = Client::new();
        let bootstrap = bootstrap(&client, addr.port()).await;
        let (mut socket, _) = connect_async(bootstrap.ws_url.as_str()).await.unwrap();

        send_ws_frame(
            &mut socket,
            PbMqttFrame {
                body: Some(MqttFrameBody::Connect(PbMqttConnect {
                    client_id: "node-e2e".to_string(),
                    clean_session: true,
                    auth_token: "dev-shared-token".to_string(),
                })),
            },
        )
        .await;

        match recv_mqtt_frame(&mut socket).await.body {
            Some(MqttFrameBody::ConnAck(connack)) => assert!(connack.accepted),
            other => panic!("expected connack, got {:?}", other),
        }

        send_ws_frame(
            &mut socket,
            PbMqttFrame {
                body: Some(MqttFrameBody::Subscribe(PbMqttSubscribe {
                    topics: vec![node_task_topic("node-e2e"), node_control_topic("node-e2e")],
                })),
            },
        )
        .await;

        match recv_mqtt_frame(&mut socket).await.body {
            Some(MqttFrameBody::SubAck(suback)) => {
                assert!(suback.topics.contains(&node_task_topic("node-e2e")));
                assert!(suback.topics.contains(&node_control_topic("node-e2e")));
            }
            other => panic!("expected suback, got {:?}", other),
        }

        let _ = recv_mqtt_frame(&mut socket).await;

        send_publish(
            &mut socket,
            node_hello_topic("node-e2e"),
            PbNodePayloadEnvelope {
                body: Some(NodePayloadBody::ClientHello(PbClientHello {
                    node_id: "node-e2e".to_string(),
                    hostname: "test-host".to_string(),
                    platform: "darwin-arm64".to_string(),
                    poll_interval_secs: 3,
                })),
            },
        )
        .await;

        send_publish(
            &mut socket,
            node_hello_topic("node-e2e"),
            PbNodePayloadEnvelope {
                body: Some(NodePayloadBody::CommandCatalog(PbCommandCatalog {
                    commands: vec![PbCommandDescriptor {
                        name: "echo".to_string(),
                        description: "echo text".to_string(),
                        default_args: Vec::new(),
                        allow_extra_args: true,
                    }],
                })),
            },
        )
        .await;

        wait_for(
            || async {
                state
                    .db
                    .get_node("node-e2e")
                    .unwrap()
                    .map(|node| node.commands.len() == 1)
                    .unwrap_or(false)
            },
            Duration::from_secs(2),
        )
        .await;

        let created_task = match state
            .db
            .create_task(
                CreateTaskRequest {
                    node_id: "node-e2e".to_string(),
                    command_name: "echo".to_string(),
                    args: vec!["wrong".to_string(), "node".to_string()],
                    timeout_secs: Some(30),
                },
                TaskAudit::new("test", Some("http-pull-mismatch-e2e".to_string())),
            )
            .unwrap()
        {
            CreateTaskOutcome::Created(task) => task,
            _ => panic!("expected task creation to succeed"),
        };

        send_publish(
            &mut socket,
            node_control_topic("node-e2e"),
            PbNodePayloadEnvelope {
                body: Some(NodePayloadBody::TaskPullRequest(PbTaskPullRequest {
                    node_id: "node-other".to_string(),
                    limit: 1,
                })),
            },
        )
        .await;

        let error_frame = recv_mqtt_frame(&mut socket).await;
        let publish = match error_frame.body {
            Some(MqttFrameBody::Publish(publish)) => publish,
            other => panic!("expected error publish, got {:?}", other),
        };
        assert_eq!(publish.topic, node_control_topic("node-e2e"));

        let payload = PbNodePayloadEnvelope::decode_message(&publish.payload).unwrap();
        let error = match payload.body {
            Some(NodePayloadBody::Error(error)) => error,
            other => panic!("expected error payload, got {:?}", other),
        };
        assert_eq!(error.message, "node_id mismatch in task pull request");

        let task = state.db.get_task(created_task.task_id).unwrap().unwrap();
        assert_eq!(task.status, TaskStatus::Queued);

        let _ = shutdown_tx.send(());
        server_task.await.unwrap().unwrap();
        let _ = std::fs::remove_file(&db_path);
        let _ = std::fs::remove_file(format!("{}-shm", db_path.display()));
        let _ = std::fs::remove_file(format!("{}-wal", db_path.display()));
    }

    async fn bootstrap(client: &Client<HttpConnector>, port: u16) -> BootstrapResponse {
        let request = Request::builder()
            .method(Method::POST)
            .uri(format!("http://127.0.0.1:{port}/api/v1/bootstrap"))
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(
                serde_json::to_vec(&BootstrapRequest {
                    node_id: "node-e2e".to_string(),
                    auth_token: "dev-shared-token".to_string(),
                })
                .unwrap(),
            ))
            .unwrap();

        let response = client.request(request).await.unwrap();
        let body = to_bytes(response.into_body()).await.unwrap();
        serde_json::from_slice(&body).unwrap()
    }

    async fn get_node_snapshot(
        client: &Client<HttpConnector>,
        port: u16,
        state: &Arc<AppState>,
        node_id: &str,
    ) -> NodeSnapshot {
        let admin_session = state.create_admin_session();
        let request = Request::builder()
            .method(Method::GET)
            .uri(format!("http://127.0.0.1:{port}/api/v1/nodes/{node_id}"))
            .header(COOKIE, format!("ru_admin_session={admin_session}"))
            .body(Body::empty())
            .unwrap();

        let response = client.request(request).await.unwrap();
        let body = to_bytes(response.into_body()).await.unwrap();
        serde_json::from_slice(&body).unwrap()
    }

    async fn list_node_snapshots(
        client: &Client<HttpConnector>,
        port: u16,
        state: &Arc<AppState>,
    ) -> Vec<NodeSnapshot> {
        let admin_session = state.create_admin_session();
        let request = Request::builder()
            .method(Method::GET)
            .uri(format!("http://127.0.0.1:{port}/api/v1/nodes"))
            .header(COOKIE, format!("ru_admin_session={admin_session}"))
            .body(Body::empty())
            .unwrap();

        let response = client.request(request).await.unwrap();
        let body = to_bytes(response.into_body()).await.unwrap();
        serde_json::from_slice(&body).unwrap()
    }

    async fn get_session_events(
        client: &Client<HttpConnector>,
        port: u16,
        state: &Arc<AppState>,
        node_id: &str,
    ) -> Vec<SessionEvent> {
        let admin_session = state.create_admin_session();
        let request = Request::builder()
            .method(Method::GET)
            .uri(format!(
                "http://127.0.0.1:{port}/api/v1/nodes/{node_id}/session-events?limit=8"
            ))
            .header(COOKIE, format!("ru_admin_session={admin_session}"))
            .body(Body::empty())
            .unwrap();

        let response = client.request(request).await.unwrap();
        let body = to_bytes(response.into_body()).await.unwrap();
        serde_json::from_slice(&body).unwrap()
    }

    async fn get_all_session_events(
        client: &Client<HttpConnector>,
        port: u16,
        state: &Arc<AppState>,
    ) -> Vec<SessionEvent> {
        get_filtered_session_events(client, port, state, None, None).await
    }

    async fn get_filtered_session_events(
        client: &Client<HttpConnector>,
        port: u16,
        state: &Arc<AppState>,
        node_id: Option<&str>,
        kind: Option<&str>,
    ) -> Vec<SessionEvent> {
        let admin_session = state.create_admin_session();
        let mut uri = format!("http://127.0.0.1:{port}/api/v1/session-events?limit=8");
        if let Some(node_id) = node_id {
            uri.push_str("&node_id=");
            uri.push_str(node_id);
        }
        if let Some(kind) = kind {
            uri.push_str("&kind=");
            uri.push_str(kind);
        }
        let request = Request::builder()
            .method(Method::GET)
            .uri(uri)
            .header(COOKIE, format!("ru_admin_session={admin_session}"))
            .body(Body::empty())
            .unwrap();

        let response = client.request(request).await.unwrap();
        let body = to_bytes(response.into_body()).await.unwrap();
        serde_json::from_slice(&body).unwrap()
    }

    async fn send_publish(
        socket: &mut tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        topic: String,
        payload: PbNodePayloadEnvelope,
    ) {
        send_ws_frame(
            socket,
            PbMqttFrame {
                body: Some(MqttFrameBody::Publish(PbMqttPublish {
                    topic,
                    payload: payload.encode_message(),
                })),
            },
        )
        .await;
    }

    async fn send_ws_frame(
        socket: &mut tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        frame: PbMqttFrame,
    ) {
        socket
            .send(WsMessage::Binary(frame.encode_message()))
            .await
            .unwrap();
    }

    async fn recv_mqtt_frame(
        socket: &mut tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    ) -> PbMqttFrame {
        loop {
            match socket.next().await.unwrap().unwrap() {
                WsMessage::Binary(bytes) => {
                    let frame = PbMqttFrame::decode_message(&bytes).unwrap();
                    match frame.body {
                        Some(MqttFrameBody::PingReq(request)) => {
                            send_ws_frame(
                                socket,
                                PbMqttFrame {
                                    body: Some(MqttFrameBody::PingResp(PbMqttPingResp {
                                        unix_secs: request.unix_secs,
                                    })),
                                },
                            )
                            .await;
                        }
                        Some(MqttFrameBody::PingResp(_)) => {}
                        _ => return frame,
                    }
                }
                WsMessage::Ping(payload) => socket.send(WsMessage::Pong(payload)).await.unwrap(),
                WsMessage::Pong(_) | WsMessage::Text(_) | WsMessage::Frame(_) => {}
                WsMessage::Close(frame) => panic!("unexpected websocket close: {:?}", frame),
            }
        }
    }

    async fn wait_for<F, Fut>(mut condition: F, timeout: Duration)
    where
        F: FnMut() -> Fut,
        Fut: std::future::Future<Output = bool>,
    {
        let started = tokio::time::Instant::now();
        loop {
            if condition().await {
                return;
            }
            if started.elapsed() >= timeout {
                panic!("condition was not met within {:?}", timeout);
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    fn temp_db_path(prefix: &str) -> PathBuf {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("{prefix}-{suffix}.db"))
    }
}
