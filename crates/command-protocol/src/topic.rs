pub fn node_hello_topic(node_id: &str) -> String {
    format!("nodes/{node_id}/hello")
}

pub fn node_task_topic(node_id: &str) -> String {
    format!("nodes/{node_id}/task")
}

pub fn node_result_topic(node_id: &str) -> String {
    format!("nodes/{node_id}/result")
}

pub fn node_ack_topic(node_id: &str) -> String {
    format!("nodes/{node_id}/ack")
}

pub fn node_control_topic(node_id: &str) -> String {
    format!("nodes/{node_id}/control")
}

#[cfg(test)]
mod tests {
    use super::{
        node_ack_topic, node_control_topic, node_hello_topic, node_result_topic, node_task_topic,
    };

    #[test]
    fn node_topics_follow_single_ws_contract() {
        assert_eq!(node_hello_topic("node-1"), "nodes/node-1/hello");
        assert_eq!(node_task_topic("node-1"), "nodes/node-1/task");
        assert_eq!(node_result_topic("node-1"), "nodes/node-1/result");
        assert_eq!(node_ack_topic("node-1"), "nodes/node-1/ack");
        assert_eq!(node_control_topic("node-1"), "nodes/node-1/control");
    }
}
