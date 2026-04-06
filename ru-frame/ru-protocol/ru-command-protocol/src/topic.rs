pub fn agent_hello_topic(agent_id: &str) -> String {
    format!("agents/{agent_id}/hello")
}

pub fn agent_task_topic(agent_id: &str) -> String {
    format!("agents/{agent_id}/task")
}

pub fn agent_result_topic(agent_id: &str) -> String {
    format!("agents/{agent_id}/result")
}

pub fn agent_ack_topic(agent_id: &str) -> String {
    format!("agents/{agent_id}/ack")
}

pub fn agent_control_topic(agent_id: &str) -> String {
    format!("agents/{agent_id}/control")
}

#[cfg(test)]
mod tests {
    use super::{
        agent_ack_topic, agent_control_topic, agent_hello_topic, agent_result_topic,
        agent_task_topic,
    };

    #[test]
    fn agent_topics_follow_single_ws_contract() {
        assert_eq!(agent_hello_topic("node-1"), "agents/node-1/hello");
        assert_eq!(agent_task_topic("node-1"), "agents/node-1/task");
        assert_eq!(agent_result_topic("node-1"), "agents/node-1/result");
        assert_eq!(agent_ack_topic("node-1"), "agents/node-1/ack");
        assert_eq!(agent_control_topic("node-1"), "agents/node-1/control");
    }
}
