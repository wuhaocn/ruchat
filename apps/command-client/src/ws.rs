use crate::config::ClientConfig;
use crate::executor::{RunningCommand, StartTaskOutcome};
use futures_util::{SinkExt, StreamExt};
use ru_command_protocol::pb_agent_payload_envelope::Body as AgentPayloadBody;
use ru_command_protocol::pb_mqtt_frame::Body as MqttFrameBody;
use ru_command_protocol::{
    agent_ack_topic, agent_control_topic, agent_hello_topic, agent_result_topic, agent_task_topic,
    BootstrapResponse, PbAgentPayloadEnvelope, PbClientHello, PbMqttConnect, PbMqttFrame,
    PbMqttPingReq, PbMqttPingResp, PbMqttPublish, PbMqttSubscribe, PbTaskAck, PbTaskCancel,
    PbTaskResult,
};
use std::io;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

struct RunningTaskState {
    task_id: u64,
    runner: RunningCommand,
}

pub(crate) async fn run_ws_session(
    config: &ClientConfig,
    bootstrap: BootstrapResponse,
) -> Result<(), Box<dyn std::error::Error>> {
    let (mut socket, _) = connect_async(bootstrap.ws_url.as_str()).await?;

    send_frame(
        &mut socket,
        PbMqttFrame {
            body: Some(MqttFrameBody::Connect(PbMqttConnect {
                client_id: config.agent_id.clone(),
                clean_session: true,
                auth_token: config.auth_token.clone(),
            })),
        },
    )
    .await?;

    wait_for_connack(&mut socket).await?;

    send_frame(
        &mut socket,
        PbMqttFrame {
            body: Some(MqttFrameBody::Subscribe(PbMqttSubscribe {
                topics: vec![
                    agent_task_topic(&config.agent_id),
                    agent_control_topic(&config.agent_id),
                ],
            })),
        },
    )
    .await?;

    send_publish(
        &mut socket,
        agent_hello_topic(&config.agent_id),
        PbAgentPayloadEnvelope {
            body: Some(AgentPayloadBody::ClientHello(PbClientHello::from(
                &config.registration(),
            ))),
        },
    )
    .await?;

    let mut heartbeat = tokio::time::interval(Duration::from_secs(
        bootstrap.heartbeat_interval_secs.max(1),
    ));
    let mut progress_tick = tokio::time::interval(Duration::from_millis(200));
    let mut running_task: Option<RunningTaskState> = None;

    loop {
        tokio::select! {
            _ = heartbeat.tick() => {
                send_frame(
                    &mut socket,
                    PbMqttFrame {
                        body: Some(MqttFrameBody::PingReq(PbMqttPingReq {
                            unix_secs: unix_now(),
                        })),
                    },
                ).await?;
            }
            _ = progress_tick.tick() => {
                if let Some(finished_task_id) = running_task_finished(&mut running_task)? {
                    let completed_task = running_task.take().expect("running task missing");
                    let result = completed_task.runner.finish().await;
                    publish_task_result(&mut socket, &config.agent_id, finished_task_id, result).await?;
                }
            }
            message = socket.next() => {
                let Some(message) = message else {
                    if let Some(task) = running_task.as_mut() {
                        let _ = task.runner.request_cancel(Some("websocket session closed".to_string()));
                    }
                    return Ok(());
                };

                match message? {
                    WsMessage::Binary(bytes) => {
                        handle_binary_message(config, &mut socket, &mut running_task, &bytes).await?;
                    }
                    WsMessage::Ping(payload) => {
                        socket.send(WsMessage::Pong(payload)).await?;
                    }
                    WsMessage::Pong(_) => {}
                    WsMessage::Close(_) => {
                        if let Some(task) = running_task.as_mut() {
                            let _ = task.runner.request_cancel(Some("websocket session closed".to_string()));
                        }
                        return Ok(());
                    }
                    WsMessage::Text(_) => {}
                    WsMessage::Frame(_) => {}
                }
            }
        }
    }
}

async fn wait_for_connack(
    socket: &mut WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>,
) -> Result<(), Box<dyn std::error::Error>> {
    while let Some(message) = socket.next().await {
        match message? {
            WsMessage::Binary(bytes) => {
                let frame = PbMqttFrame::decode_message(&bytes)?;
                if let Some(MqttFrameBody::ConnAck(connack)) = frame.body {
                    if connack.accepted {
                        return Ok(());
                    }
                    return Err(
                        io::Error::new(io::ErrorKind::PermissionDenied, connack.message).into(),
                    );
                }
            }
            WsMessage::Close(_) => return Ok(()),
            WsMessage::Ping(payload) => {
                socket.send(WsMessage::Pong(payload)).await?;
            }
            WsMessage::Pong(_) | WsMessage::Text(_) | WsMessage::Frame(_) => {}
        }
    }

    Err(io::Error::new(
        io::ErrorKind::UnexpectedEof,
        "websocket closed before connack",
    )
    .into())
}

async fn handle_binary_message(
    config: &ClientConfig,
    socket: &mut WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>,
    running_task: &mut Option<RunningTaskState>,
    bytes: &[u8],
) -> Result<(), Box<dyn std::error::Error>> {
    let frame = PbMqttFrame::decode_message(bytes)?;

    match frame.body {
        Some(MqttFrameBody::Publish(publish)) => {
            let payload = PbAgentPayloadEnvelope::decode_message(&publish.payload)?;
            match publish.topic.as_str() {
                topic if topic == agent_task_topic(&config.agent_id) => {
                    let Some(AgentPayloadBody::TaskAssignment(task)) = payload.body else {
                        return Ok(());
                    };
                    if running_task.is_some() {
                        eprintln!(
                            "received task {} while another task is still running",
                            task.task_id
                        );
                        return Ok(());
                    }

                    send_publish(
                        socket,
                        agent_ack_topic(&config.agent_id),
                        PbAgentPayloadEnvelope {
                            body: Some(AgentPayloadBody::TaskAck(PbTaskAck {
                                task_id: task.task_id,
                            })),
                        },
                    )
                    .await?;

                    match RunningCommand::start(config, &task.command_name, &task.args) {
                        StartTaskOutcome::Started(runner) => {
                            *running_task = Some(RunningTaskState {
                                task_id: task.task_id,
                                runner,
                            });
                        }
                        StartTaskOutcome::Completed(result) => {
                            publish_task_result(socket, &config.agent_id, task.task_id, result)
                                .await?;
                        }
                    }
                }
                topic if topic == agent_control_topic(&config.agent_id) => match payload.body {
                    Some(AgentPayloadBody::Error(error)) => {
                        eprintln!("server error: {}", error.message);
                    }
                    Some(AgentPayloadBody::TaskCancel(cancel)) => {
                        handle_task_cancel(running_task, cancel)?;
                    }
                    _ => {}
                },
                _ => {}
            }
        }
        Some(MqttFrameBody::SubAck(_)) => {}
        Some(MqttFrameBody::PingReq(request)) => {
            send_frame(
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
        | Some(MqttFrameBody::Subscribe(_))
        | None => {}
    }

    Ok(())
}

fn handle_task_cancel(
    running_task: &mut Option<RunningTaskState>,
    cancel: PbTaskCancel,
) -> Result<(), Box<dyn std::error::Error>> {
    let Some(task) = running_task.as_mut() else {
        return Ok(());
    };
    if task.task_id != cancel.task_id {
        return Ok(());
    }

    task.runner.request_cancel(cancel.reason)?;
    Ok(())
}

fn running_task_finished(
    running_task: &mut Option<RunningTaskState>,
) -> Result<Option<u64>, Box<dyn std::error::Error>> {
    let Some(task) = running_task.as_mut() else {
        return Ok(None);
    };

    Ok(task.runner.try_wait()?.map(|_| task.task_id))
}

async fn publish_task_result(
    socket: &mut WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>,
    agent_id: &str,
    task_id: u64,
    result: ru_command_protocol::ExecutionResult,
) -> Result<(), Box<dyn std::error::Error>> {
    send_publish(
        socket,
        agent_result_topic(agent_id),
        PbAgentPayloadEnvelope {
            body: Some(AgentPayloadBody::TaskResult(PbTaskResult {
                task_id,
                success: result.success,
                exit_code: result.exit_code,
                stdout: result.stdout,
                stderr: result.stderr,
                duration_ms: result.duration_ms,
                error: result.error,
            })),
        },
    )
    .await
}

async fn send_publish(
    socket: &mut WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>,
    topic: String,
    payload: PbAgentPayloadEnvelope,
) -> Result<(), Box<dyn std::error::Error>> {
    send_frame(
        socket,
        PbMqttFrame {
            body: Some(MqttFrameBody::Publish(PbMqttPublish {
                topic,
                payload: payload.encode_message(),
            })),
        },
    )
    .await
}

async fn send_frame(
    socket: &mut WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>,
    frame: PbMqttFrame,
) -> Result<(), Box<dyn std::error::Error>> {
    socket
        .send(WsMessage::Binary(frame.encode_message()))
        .await?;
    Ok(())
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}
