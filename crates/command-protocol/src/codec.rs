use crate::mqtt::PbMqttFrame;
use crate::pb::PbNodePayloadEnvelope;
use prost::Message;

impl PbMqttFrame {
    pub fn encode_message(&self) -> Vec<u8> {
        self.encode_to_vec()
    }

    pub fn decode_message(bytes: &[u8]) -> Result<Self, prost::DecodeError> {
        Self::decode(bytes)
    }
}

impl PbNodePayloadEnvelope {
    pub fn encode_message(&self) -> Vec<u8> {
        self.encode_to_vec()
    }

    pub fn decode_message(bytes: &[u8]) -> Result<Self, prost::DecodeError> {
        Self::decode(bytes)
    }
}

#[cfg(test)]
mod tests {
    use crate::mqtt::pb_mqtt_frame::Body as MqttFrameBody;
    use crate::mqtt::{PbMqttFrame, PbMqttPublish};
    use crate::pb::pb_node_payload_envelope::Body as NodePayloadBody;
    use crate::pb::{
        PbCommandCatalog, PbCommandDescriptor, PbNodePayloadEnvelope, PbSessionInfo,
        PbTaskAssignment, PbTaskPullRequest, PbTaskPullResponse,
    };
    use crate::topic::node_task_topic;
    use crate::{CONTROL_PROTOCOL_VERSION, MAX_RESULT_OUTPUT_BYTES};

    #[test]
    fn mqtt_frame_roundtrip_preserves_publish_payload() {
        let frame = PbMqttFrame {
            body: Some(MqttFrameBody::Publish(PbMqttPublish {
                topic: node_task_topic("node-1"),
                payload: vec![1, 2, 3, 4],
            })),
        };

        let decoded = PbMqttFrame::decode_message(&frame.encode_message()).unwrap();
        let Some(MqttFrameBody::Publish(publish)) = decoded.body else {
            panic!("expected publish frame");
        };

        assert_eq!(publish.topic, "nodes/node-1/task");
        assert_eq!(publish.payload, vec![1, 2, 3, 4]);
    }

    #[test]
    fn node_payload_envelope_roundtrip_preserves_session_info() {
        let envelope = PbNodePayloadEnvelope {
            body: Some(NodePayloadBody::SessionInfo(PbSessionInfo {
                session_id: "node-1-123".to_string(),
                node_id: "node-1".to_string(),
                heartbeat_interval_secs: 15,
                max_result_output_bytes: MAX_RESULT_OUTPUT_BYTES as u64,
                protocol_version: CONTROL_PROTOCOL_VERSION.to_string(),
                server_unix_secs: 1_700_000_000,
            })),
        };

        let decoded = PbNodePayloadEnvelope::decode_message(&envelope.encode_message()).unwrap();
        let Some(NodePayloadBody::SessionInfo(info)) = decoded.body else {
            panic!("expected session info payload");
        };

        assert_eq!(info.node_id, "node-1");
        assert_eq!(info.protocol_version, CONTROL_PROTOCOL_VERSION);
        assert_eq!(info.max_result_output_bytes, MAX_RESULT_OUTPUT_BYTES as u64);
    }

    #[test]
    fn node_payload_envelope_roundtrip_preserves_catalog_and_pull_messages() {
        let catalog = PbNodePayloadEnvelope {
            body: Some(NodePayloadBody::CommandCatalog(PbCommandCatalog {
                commands: vec![PbCommandDescriptor {
                    name: "echo".to_string(),
                    description: "echo text".to_string(),
                    default_args: vec!["hello".to_string()],
                    allow_extra_args: true,
                }],
            })),
        };
        let pull_request = PbNodePayloadEnvelope {
            body: Some(NodePayloadBody::TaskPullRequest(PbTaskPullRequest {
                node_id: "node-1".to_string(),
                limit: 1,
            })),
        };
        let pull_response = PbNodePayloadEnvelope {
            body: Some(NodePayloadBody::TaskPullResponse(PbTaskPullResponse {
                tasks: vec![PbTaskAssignment {
                    task_id: 42,
                    command_name: "echo".to_string(),
                    args: vec!["hello".to_string(), "world".to_string()],
                    created_at_unix_secs: 1_700_000_001,
                    timeout_secs: Some(30),
                    attempt: 1,
                }],
            })),
        };

        let decoded_catalog =
            PbNodePayloadEnvelope::decode_message(&catalog.encode_message()).unwrap();
        let Some(NodePayloadBody::CommandCatalog(catalog)) = decoded_catalog.body else {
            panic!("expected command catalog payload");
        };
        assert_eq!(catalog.commands.len(), 1);
        assert_eq!(catalog.commands[0].name, "echo");

        let decoded_pull_request =
            PbNodePayloadEnvelope::decode_message(&pull_request.encode_message()).unwrap();
        let Some(NodePayloadBody::TaskPullRequest(request)) = decoded_pull_request.body else {
            panic!("expected task pull request payload");
        };
        assert_eq!(request.node_id, "node-1");
        assert_eq!(request.limit, 1);

        let decoded_pull_response =
            PbNodePayloadEnvelope::decode_message(&pull_response.encode_message()).unwrap();
        let Some(NodePayloadBody::TaskPullResponse(response)) = decoded_pull_response.body else {
            panic!("expected task pull response payload");
        };
        assert_eq!(response.tasks.len(), 1);
        assert_eq!(response.tasks[0].task_id, 42);
        assert_eq!(response.tasks[0].command_name, "echo");
    }
}
