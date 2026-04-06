use crate::app_state::AppState;
use crate::console;
use crate::db::{CancelTaskOutcome, CreateTaskOutcome};
use crate::error::{internal_error, ApiResult};
use crate::ws::node_ws;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use ru_command_protocol::{
    BootstrapRequest, BootstrapResponse, CancelTaskRequest, CreateTaskRequest, NodeSnapshot,
    TaskSnapshot,
};
use std::sync::Arc;

pub(crate) fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .merge(console::router())
        .route("/health", get(health))
        .route("/api/v1/bootstrap", post(bootstrap))
        .route("/api/v1/nodes", get(list_nodes))
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
        .route("/api/v1/nodes", get(list_nodes))
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
    }))
}

async fn list_nodes(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> ApiResult<Vec<NodeSnapshot>> {
    require_admin_session(&state, &headers)?;
    state.db.list_nodes().map(Json).map_err(internal_error)
}

async fn get_node(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(node_id): Path<String>,
) -> ApiResult<NodeSnapshot> {
    require_admin_session(&state, &headers)?;
    match state.db.get_node(&node_id).map_err(internal_error)? {
        Some(node) => Ok(Json(node)),
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

async fn create_task(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<CreateTaskRequest>,
) -> ApiResult<TaskSnapshot> {
    require_admin_session(&state, &headers)?;
    match state.db.create_task(request).map_err(internal_error)? {
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

#[cfg(test)]
mod tests {
    use super::router;
    use crate::app_state::AppState;
    use crate::auth::AuthConfig;
    use crate::db::{CreateTaskOutcome, Database};
    use axum::Server;
    use futures_util::{SinkExt, StreamExt};
    use hyper::body::to_bytes;
    use hyper::client::HttpConnector;
    use hyper::header::CONTENT_TYPE;
    use hyper::{Body, Client, Method, Request};
    use ru_command_protocol::pb_mqtt_frame::Body as MqttFrameBody;
    use ru_command_protocol::pb_node_payload_envelope::Body as NodePayloadBody;
    use ru_command_protocol::{
        node_ack_topic, node_control_topic, node_hello_topic, node_task_topic, BootstrapRequest,
        BootstrapResponse, CreateTaskRequest, PbClientHello, PbCommandDescriptor, PbMqttConnect,
        PbMqttFrame, PbMqttPingResp, PbMqttPublish, PbMqttSubscribe, PbNodePayloadEnvelope,
        PbTaskAck, PbTaskResult, TaskStatus,
    };
    use std::net::TcpListener;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};
    use tokio::sync::oneshot;
    use tokio_tungstenite::connect_async;
    use tokio_tungstenite::tungstenite::Message as WsMessage;

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

        send_publish(
            &mut socket,
            node_hello_topic("node-e2e"),
            PbNodePayloadEnvelope {
                body: Some(NodePayloadBody::ClientHello(PbClientHello {
                    node_id: "node-e2e".to_string(),
                    hostname: "test-host".to_string(),
                    platform: "darwin-arm64".to_string(),
                    poll_interval_secs: 3,
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
            .create_task(CreateTaskRequest {
                node_id: "node-e2e".to_string(),
                command_name: "echo".to_string(),
                args: vec!["hello".to_string(), "world".to_string()],
                timeout_secs: Some(30),
            })
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
