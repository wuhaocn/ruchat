use ru_command_protocol::{CommandDescriptor, NodeRegistration};
use serde::Deserialize;
use std::env;
use std::fs;

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct LocalCommandConfig {
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) program: String,
    #[serde(default)]
    pub(crate) default_args: Vec<String>,
    #[serde(default)]
    pub(crate) allow_extra_args: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ClientConfig {
    pub(crate) node_id: String,
    pub(crate) auth_token: String,
    pub(crate) server_url: String,
    pub(crate) poll_interval_secs: u64,
    pub(crate) request_timeout_secs: Option<u64>,
    #[serde(default)]
    pub(crate) commands: Vec<LocalCommandConfig>,
}

impl ClientConfig {
    pub(crate) fn load_from_args() -> Result<Self, Box<dyn std::error::Error>> {
        let config_path = env::args()
            .nth(1)
            .unwrap_or_else(|| "client-config.json".to_string());
        let config_raw = fs::read_to_string(&config_path)?;
        Ok(serde_json::from_str(&config_raw)?)
    }

    pub(crate) fn command_descriptors(&self) -> Vec<CommandDescriptor> {
        self.commands
            .iter()
            .map(|command| CommandDescriptor {
                name: command.name.clone(),
                description: command.description.clone(),
                default_args: command.default_args.clone(),
                allow_extra_args: command.allow_extra_args,
            })
            .collect()
    }

    pub(crate) fn hello_registration(&self) -> NodeRegistration {
        NodeRegistration {
            node_id: self.node_id.clone(),
            hostname: hostname(),
            platform: format!("{}-{}", env::consts::OS, env::consts::ARCH),
            poll_interval_secs: self.poll_interval_secs,
            commands: Vec::new(),
        }
    }
}

fn hostname() -> String {
    env::var("HOSTNAME")
        .or_else(|_| env::var("COMPUTERNAME"))
        .unwrap_or_else(|_| "unknown-host".to_string())
}
