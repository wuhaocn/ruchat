use prost::Message;

#[derive(Clone, PartialEq, Message)]
pub struct PbMqttConnect {
    #[prost(string, tag = "1")]
    pub client_id: String,
    #[prost(bool, tag = "2")]
    pub clean_session: bool,
    #[prost(string, tag = "3")]
    pub auth_token: String,
}

#[derive(Clone, PartialEq, Message)]
pub struct PbMqttConnAck {
    #[prost(bool, tag = "1")]
    pub accepted: bool,
    #[prost(string, tag = "2")]
    pub message: String,
}

#[derive(Clone, PartialEq, Message)]
pub struct PbMqttSubscribe {
    #[prost(string, repeated, tag = "1")]
    pub topics: Vec<String>,
}

#[derive(Clone, PartialEq, Message)]
pub struct PbMqttSubAck {
    #[prost(string, repeated, tag = "1")]
    pub topics: Vec<String>,
}

#[derive(Clone, PartialEq, Message)]
pub struct PbMqttPublish {
    #[prost(string, tag = "1")]
    pub topic: String,
    #[prost(bytes = "vec", tag = "2")]
    pub payload: Vec<u8>,
}

#[derive(Clone, PartialEq, Message)]
pub struct PbMqttPingReq {
    #[prost(uint64, tag = "1")]
    pub unix_secs: u64,
}

#[derive(Clone, PartialEq, Message)]
pub struct PbMqttPingResp {
    #[prost(uint64, tag = "1")]
    pub unix_secs: u64,
}

#[derive(Clone, PartialEq, Message)]
pub struct PbMqttFrame {
    #[prost(oneof = "pb_mqtt_frame::Body", tags = "1, 2, 3, 4, 5, 6, 7")]
    pub body: Option<pb_mqtt_frame::Body>,
}

pub mod pb_mqtt_frame {
    use super::{
        PbMqttConnAck, PbMqttConnect, PbMqttPingReq, PbMqttPingResp, PbMqttPublish, PbMqttSubAck,
        PbMqttSubscribe,
    };
    use prost::Oneof;

    #[derive(Clone, PartialEq, Oneof)]
    pub enum Body {
        #[prost(message, tag = "1")]
        Connect(PbMqttConnect),
        #[prost(message, tag = "2")]
        ConnAck(PbMqttConnAck),
        #[prost(message, tag = "3")]
        Subscribe(PbMqttSubscribe),
        #[prost(message, tag = "4")]
        SubAck(PbMqttSubAck),
        #[prost(message, tag = "5")]
        Publish(PbMqttPublish),
        #[prost(message, tag = "6")]
        PingReq(PbMqttPingReq),
        #[prost(message, tag = "7")]
        PingResp(PbMqttPingResp),
    }
}
