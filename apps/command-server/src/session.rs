use crate::app_state::{AppState, SessionSignal};
use crate::db::ClaimTaskOutcome;
use crate::error::to_io_error;
use axum::extract::ws::{Message as WsMessage, WebSocket};
use ru_command_protocol::pb_mqtt_frame::Body as MqttFrameBody;
use ru_command_protocol::pb_node_payload_envelope::Body as NodePayloadBody;
use ru_command_protocol::{
    node_ack_topic, node_control_topic, node_hello_topic, node_result_topic, node_task_topic,
    NodeRegistration, PbMqttConnAck, PbMqttConnect, PbMqttFrame, PbMqttPingReq, PbMqttPingResp,
    PbMqttPublish, PbMqttSubAck, PbNodeError, PbNodePayloadEnvelope, PbTaskCancel,
    SubmitTaskResultRequest,
};
use std::collections::HashSet;
use std::io;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;

struct InFlightTask {
    task_id: u64,
    timeout_secs: Option<u64>,
    dispatched_at_unix_secs: u64,
    cancel_requested: bool,
    ignore_result: bool,
}

pub(crate) async fn handle_node_socket(
    state: Arc<AppState>,
    path_node_id: String,
    mut socket: WebSocket,
) {
    let connect = match receive_connect(&mut socket).await {
        Ok(connect) => connect,
        Err(error) => {
            let _ = socket
                .send(WsMessage::Binary(
                    mqtt_connack(false, &error).encode_message(),
                ))
                .await;
            let _ = socket.close().await;
            return;
        }
    };

    if connect.client_id != path_node_id {
        let _ = socket
            .send(WsMessage::Binary(
                mqtt_connack(false, "client_id mismatch between path and connect").encode_message(),
            ))
            .await;
        let _ = socket.close().await;
        return;
    }

    if let Err(message) = state
        .auth
        .verify_node_token(&path_node_id, &connect.auth_token)
    {
        let _ = socket
            .send(WsMessage::Binary(
                mqtt_connack(false, message).encode_message(),
            ))
            .await;
        let _ = socket.close().await;
        return;
    }

    if send_mqtt_frame(&mut socket, mqtt_connack(true, "connected"))
        .await
        .is_err()
    {
        let _ = socket.close().await;
        return;
    }

    let (signal_tx, mut signal_rx) = mpsc::unbounded_channel::<SessionSignal>();
    state.insert_session(path_node_id.clone(), signal_tx);

    let mut subscriptions = HashSet::new();
    let mut registered = false;
    let mut in_flight_task: Option<InFlightTask> = None;
    let mut heartbeat =
        tokio::time::interval(Duration::from_secs(state.heartbeat_interval_secs.max(1)));

    loop {
        tokio::select! {
            _ = heartbeat.tick() => {
                if handle_in_flight_timeout(
                    &state,
                    &path_node_id,
                    &mut socket,
                    &mut in_flight_task,
                ).await.is_err() {
                    break;
                }
                if send_mqtt_frame(
                    &mut socket,
                    PbMqttFrame {
                        body: Some(MqttFrameBody::PingReq(PbMqttPingReq {
                            unix_secs: unix_now(),
                        })),
                    },
                ).await.is_err() {
                    break;
                }
            }
            signal = signal_rx.recv() => {
                match signal {
                    Some(SessionSignal::TryDispatch) => {
                        if try_dispatch_next_task(
                            &state,
                            &path_node_id,
                            &mut socket,
                            &subscriptions,
                            registered,
                            &mut in_flight_task,
                        ).await.is_err() {
                            break;
                        }
                    }
                    Some(SessionSignal::CancelTask { task_id, reason }) => {
                        if handle_cancel_signal(
                            &state,
                            &path_node_id,
                            &mut socket,
                            &subscriptions,
                            registered,
                            &mut in_flight_task,
                            task_id,
                            reason,
                        ).await.is_err() {
                            break;
                        }
                    }
                    None => break,
                }
            }
            message = socket.recv() => {
                match message {
                    Some(Ok(WsMessage::Binary(bytes))) => {
                        if handle_binary_message(
                            &state,
                            &path_node_id,
                            &mut socket,
                            &mut subscriptions,
                            &mut registered,
                            &mut in_flight_task,
                            &bytes,
                        ).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(WsMessage::Ping(payload))) => {
                        if socket.send(WsMessage::Pong(payload)).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(WsMessage::Pong(_))) => {}
                    Some(Ok(WsMessage::Close(_))) | None => break,
                    Some(Ok(WsMessage::Text(_))) => {}
                    Some(Err(_)) => break,
                }
            }
        }
    }

    state.remove_session(&path_node_id);
    let _ = state
        .db
        .requeue_incomplete_tasks(&path_node_id, "node session closed");
    let _ = socket.close().await;
}

async fn receive_connect(socket: &mut WebSocket) -> Result<PbMqttConnect, String> {
    let message = tokio::time::timeout(Duration::from_secs(10), socket.recv())
        .await
        .map_err(|_| "timed out waiting for mqtt connect".to_string())?;

    let Some(message) = message else {
        return Err("websocket closed before mqtt connect".to_string());
    };

    let message = message.map_err(|error| error.to_string())?;
    let WsMessage::Binary(bytes) = message else {
        return Err("first websocket frame must be binary mqtt connect".to_string());
    };

    let frame = PbMqttFrame::decode_message(&bytes).map_err(|error| error.to_string())?;
    match frame.body {
        Some(MqttFrameBody::Connect(connect)) => Ok(connect),
        _ => Err("first websocket frame must be mqtt connect".to_string()),
    }
}

async fn handle_binary_message(
    state: &Arc<AppState>,
    node_id: &str,
    socket: &mut WebSocket,
    subscriptions: &mut HashSet<String>,
    registered: &mut bool,
    in_flight_task: &mut Option<InFlightTask>,
    bytes: &[u8],
) -> Result<(), io::Error> {
    let frame = PbMqttFrame::decode_message(bytes)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))?;

    match frame.body {
        Some(MqttFrameBody::Subscribe(subscribe)) => {
            for topic in subscribe.topics {
                subscriptions.insert(topic);
            }
            send_mqtt_frame(
                socket,
                PbMqttFrame {
                    body: Some(MqttFrameBody::SubAck(PbMqttSubAck {
                        topics: subscriptions.iter().cloned().collect(),
                    })),
                },
            )
            .await?;
            try_dispatch_next_task(
                state,
                node_id,
                socket,
                subscriptions,
                *registered,
                in_flight_task,
            )
            .await?;
        }
        Some(MqttFrameBody::Publish(publish)) => {
            handle_publish(
                state,
                node_id,
                socket,
                subscriptions,
                registered,
                in_flight_task,
                publish,
            )
            .await?;
        }
        Some(MqttFrameBody::PingReq(request)) => {
            send_mqtt_frame(
                socket,
                PbMqttFrame {
                    body: Some(MqttFrameBody::PingResp(PbMqttPingResp {
                        unix_secs: request.unix_secs,
                    })),
                },
            )
            .await?;
        }
        Some(MqttFrameBody::PingResp(_))
        | Some(MqttFrameBody::ConnAck(_))
        | Some(MqttFrameBody::Connect(_))
        | Some(MqttFrameBody::SubAck(_))
        | None => {}
    }

    Ok(())
}

async fn handle_publish(
    state: &Arc<AppState>,
    node_id: &str,
    socket: &mut WebSocket,
    subscriptions: &HashSet<String>,
    registered: &mut bool,
    in_flight_task: &mut Option<InFlightTask>,
    publish: PbMqttPublish,
) -> Result<(), io::Error> {
    let payload = PbNodePayloadEnvelope::decode_message(&publish.payload)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))?;

    if publish.topic == node_hello_topic(node_id) {
        let Some(NodePayloadBody::ClientHello(hello)) = payload.body else {
            publish_error(socket, node_id, "hello topic requires client hello payload").await?;
            return Ok(());
        };
        if hello.node_id != node_id {
            publish_error(socket, node_id, "node_id mismatch in hello payload").await?;
            return Ok(());
        }

        state
            .db
            .requeue_incomplete_tasks(node_id, "node re-registered")
            .map_err(to_io_error)?;
        state
            .db
            .register_node(NodeRegistration::from(hello))
            .map_err(to_io_error)?;
        *registered = true;
        try_dispatch_next_task(
            state,
            node_id,
            socket,
            subscriptions,
            *registered,
            in_flight_task,
        )
        .await?;
        return Ok(());
    }

    if publish.topic == node_ack_topic(node_id) {
        let Some(NodePayloadBody::TaskAck(ack)) = payload.body else {
            publish_error(socket, node_id, "ack topic requires task ack payload").await?;
            return Ok(());
        };
        let Some(current_task) = in_flight_task.as_ref() else {
            publish_error(
                socket,
                node_id,
                "unexpected task ack without in-flight task",
            )
            .await?;
            return Ok(());
        };
        if current_task.task_id != ack.task_id {
            publish_error(socket, node_id, "unexpected task ack id").await?;
            return Ok(());
        }

        if !current_task.ignore_result {
            state
                .db
                .acknowledge_task(ack.task_id, node_id)
                .map_err(to_io_error)?;
        }
        return Ok(());
    }

    if publish.topic == node_result_topic(node_id) {
        let Some(NodePayloadBody::TaskResult(result)) = payload.body else {
            publish_error(socket, node_id, "result topic requires task result payload").await?;
            return Ok(());
        };
        let Some(current_task) = in_flight_task.as_ref() else {
            if task_result_should_be_ignored(state, result.task_id).map_err(to_io_error)? {
                return Ok(());
            }
            publish_error(
                socket,
                node_id,
                "unexpected task result without in-flight task",
            )
            .await?;
            return Ok(());
        };
        if current_task.task_id != result.task_id {
            if task_result_should_be_ignored(state, result.task_id).map_err(to_io_error)? {
                return Ok(());
            }
            publish_error(socket, node_id, "unexpected task result id").await?;
            return Ok(());
        }

        if current_task.ignore_result {
            *in_flight_task = None;
            try_dispatch_next_task(
                state,
                node_id,
                socket,
                subscriptions,
                *registered,
                in_flight_task,
            )
            .await?;
            return Ok(());
        }

        state
            .db
            .submit_task_result(
                result.task_id,
                SubmitTaskResultRequest {
                    node_id: node_id.to_string(),
                    result: result.into(),
                },
            )
            .map_err(to_io_error)?;
        *in_flight_task = None;
        try_dispatch_next_task(
            state,
            node_id,
            socket,
            subscriptions,
            *registered,
            in_flight_task,
        )
        .await?;
    }

    Ok(())
}

async fn try_dispatch_next_task(
    state: &Arc<AppState>,
    node_id: &str,
    socket: &mut WebSocket,
    subscriptions: &HashSet<String>,
    registered: bool,
    in_flight_task: &mut Option<InFlightTask>,
) -> Result<(), io::Error> {
    if !registered || in_flight_task.is_some() || !subscriptions.contains(&node_task_topic(node_id))
    {
        return Ok(());
    }

    let task = state.db.claim_task(node_id).map_err(to_io_error)?;
    let ClaimTaskOutcome::Claimed(task) = task else {
        return Ok(());
    };

    let payload = PbNodePayloadEnvelope {
        body: Some(NodePayloadBody::TaskAssignment((&task).into())),
    };
    send_mqtt_frame(
        socket,
        PbMqttFrame {
            body: Some(MqttFrameBody::Publish(PbMqttPublish {
                topic: node_task_topic(node_id),
                payload: payload.encode_message(),
            })),
        },
    )
    .await?;
    *in_flight_task = Some(InFlightTask {
        task_id: task.task_id,
        timeout_secs: task.timeout_secs,
        dispatched_at_unix_secs: unix_now(),
        cancel_requested: false,
        ignore_result: false,
    });

    Ok(())
}

async fn handle_cancel_signal(
    state: &Arc<AppState>,
    node_id: &str,
    socket: &mut WebSocket,
    subscriptions: &HashSet<String>,
    registered: bool,
    in_flight_task: &mut Option<InFlightTask>,
    task_id: u64,
    reason: Option<String>,
) -> Result<(), io::Error> {
    if let Some(task) = in_flight_task.as_mut() {
        if task.task_id == task_id {
            if !task.cancel_requested {
                send_task_cancel(socket, node_id, task_id, reason.clone()).await?;
                task.cancel_requested = true;
            }
            task.ignore_result = true;
            return Ok(());
        }
    }

    try_dispatch_next_task(
        state,
        node_id,
        socket,
        subscriptions,
        registered,
        in_flight_task,
    )
    .await
}

fn task_result_should_be_ignored(
    state: &Arc<AppState>,
    task_id: u64,
) -> Result<bool, crate::db::DbError> {
    Ok(matches!(
        state.db.get_task(task_id)?,
        Some(task) if !task_status_accepts_result(task.status)
    ))
}

async fn handle_in_flight_timeout(
    state: &Arc<AppState>,
    node_id: &str,
    socket: &mut WebSocket,
    in_flight_task: &mut Option<InFlightTask>,
) -> Result<(), io::Error> {
    let Some(task) = in_flight_task.as_mut() else {
        return Ok(());
    };
    let Some(timeout_secs) = task.timeout_secs else {
        return Ok(());
    };
    if task.ignore_result {
        return Ok(());
    }

    if unix_now() < task.dispatched_at_unix_secs.saturating_add(timeout_secs) {
        return Ok(());
    }

    state
        .db
        .retry_task(task.task_id, node_id, "task timeout expired")
        .map_err(to_io_error)?;
    send_task_cancel(
        socket,
        node_id,
        task.task_id,
        Some("task timeout expired".to_string()),
    )
    .await?;
    task.cancel_requested = true;
    task.ignore_result = true;
    Ok(())
}

async fn send_task_cancel(
    socket: &mut WebSocket,
    node_id: &str,
    task_id: u64,
    reason: Option<String>,
) -> Result<(), io::Error> {
    let payload = PbNodePayloadEnvelope {
        body: Some(NodePayloadBody::TaskCancel(PbTaskCancel {
            task_id,
            reason,
        })),
    };
    send_mqtt_frame(
        socket,
        PbMqttFrame {
            body: Some(MqttFrameBody::Publish(PbMqttPublish {
                topic: node_control_topic(node_id),
                payload: payload.encode_message(),
            })),
        },
    )
    .await
}

async fn publish_error(
    socket: &mut WebSocket,
    node_id: &str,
    message: &str,
) -> Result<(), io::Error> {
    let payload = PbNodePayloadEnvelope {
        body: Some(NodePayloadBody::Error(PbNodeError {
            message: message.to_string(),
        })),
    };
    send_mqtt_frame(
        socket,
        PbMqttFrame {
            body: Some(MqttFrameBody::Publish(PbMqttPublish {
                topic: node_control_topic(node_id),
                payload: payload.encode_message(),
            })),
        },
    )
    .await
}

async fn send_mqtt_frame(socket: &mut WebSocket, frame: PbMqttFrame) -> Result<(), io::Error> {
    socket
        .send(WsMessage::Binary(frame.encode_message()))
        .await
        .map_err(|error| io::Error::new(io::ErrorKind::BrokenPipe, error.to_string()))
}

fn mqtt_connack(accepted: bool, message: &str) -> PbMqttFrame {
    PbMqttFrame {
        body: Some(MqttFrameBody::ConnAck(PbMqttConnAck {
            accepted,
            message: message.to_string(),
        })),
    }
}

fn task_status_accepts_result(status: ru_command_protocol::TaskStatus) -> bool {
    matches!(
        status,
        ru_command_protocol::TaskStatus::Dispatched | ru_command_protocol::TaskStatus::Running
    )
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::task_status_accepts_result;
    use ru_command_protocol::TaskStatus;

    #[test]
    fn only_dispatched_and_running_tasks_accept_results() {
        assert!(!task_status_accepts_result(TaskStatus::Queued));
        assert!(task_status_accepts_result(TaskStatus::Dispatched));
        assert!(task_status_accepts_result(TaskStatus::Running));
        assert!(!task_status_accepts_result(TaskStatus::Succeeded));
        assert!(!task_status_accepts_result(TaskStatus::Failed));
        assert!(!task_status_accepts_result(TaskStatus::Canceled));
    }
}
