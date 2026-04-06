use crate::mqtt::PbMqttFrame;
use crate::pb::PbAgentPayloadEnvelope;
use prost::Message;

impl PbMqttFrame {
    pub fn encode_message(&self) -> Vec<u8> {
        self.encode_to_vec()
    }

    pub fn decode_message(bytes: &[u8]) -> Result<Self, prost::DecodeError> {
        Self::decode(bytes)
    }
}

impl PbAgentPayloadEnvelope {
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
    use crate::topic::agent_task_topic;

    #[test]
    fn mqtt_frame_roundtrip_preserves_publish_payload() {
        let frame = PbMqttFrame {
            body: Some(MqttFrameBody::Publish(PbMqttPublish {
                topic: agent_task_topic("node-1"),
                payload: vec![1, 2, 3, 4],
            })),
        };

        let decoded = PbMqttFrame::decode_message(&frame.encode_message()).unwrap();
        let Some(MqttFrameBody::Publish(publish)) = decoded.body else {
            panic!("expected publish frame");
        };

        assert_eq!(publish.topic, "agents/node-1/task");
        assert_eq!(publish.payload, vec![1, 2, 3, 4]);
    }
}
