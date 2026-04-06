use std::collections::HashMap;

pub(crate) struct ServerConfig {
    pub(crate) http_bind: String,
    pub(crate) ws_bind: String,
    pub(crate) db_path: String,
    pub(crate) heartbeat_interval_secs: u64,
    pub(crate) public_ws_base: Option<String>,
    pub(crate) admin_username: String,
    pub(crate) admin_password: String,
    pub(crate) admin_session_ttl_secs: u64,
    pub(crate) shared_token: Option<String>,
    pub(crate) node_tokens: HashMap<String, String>,
}

impl ServerConfig {
    pub(crate) fn from_env() -> Result<Self, Box<dyn std::error::Error>> {
        let default_bind =
            std::env::var("RU_SERVER_BIND").unwrap_or_else(|_| "0.0.0.0:18080".to_string());
        let http_bind =
            std::env::var("RU_SERVER_HTTP_BIND").unwrap_or_else(|_| default_bind.clone());
        let ws_bind = std::env::var("RU_SERVER_WS_BIND").unwrap_or_else(|_| http_bind.clone());
        let db_path =
            std::env::var("RU_SERVER_DB_PATH").unwrap_or_else(|_| "command-plane.db".to_string());
        let heartbeat_interval_secs = std::env::var("RU_SERVER_HEARTBEAT_SECS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(15);
        let public_ws_base = std::env::var("RU_SERVER_PUBLIC_WS_BASE").ok();
        let admin_username =
            std::env::var("RU_ADMIN_USERNAME").unwrap_or_else(|_| "admin".to_string());
        let admin_password =
            std::env::var("RU_ADMIN_PASSWORD").unwrap_or_else(|_| "admin123".to_string());
        let admin_session_ttl_secs = std::env::var("RU_ADMIN_SESSION_TTL_SECS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(8 * 60 * 60);
        let shared_token = std::env::var("RU_SERVER_SHARED_TOKEN").ok();
        let node_tokens = match std::env::var("RU_SERVER_NODE_TOKENS_JSON") {
            Ok(value) => serde_json::from_str::<HashMap<String, String>>(&value)?,
            Err(_) => HashMap::new(),
        };

        Ok(Self {
            http_bind,
            ws_bind,
            db_path,
            heartbeat_interval_secs,
            public_ws_base,
            admin_username,
            admin_password,
            admin_session_ttl_secs,
            shared_token,
            node_tokens,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::ServerConfig;
    use std::ffi::OsString;
    use std::sync::{Mutex, OnceLock};

    static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

    const ENV_KEYS: &[&str] = &[
        "RU_SERVER_BIND",
        "RU_SERVER_HTTP_BIND",
        "RU_SERVER_WS_BIND",
        "RU_SERVER_DB_PATH",
        "RU_SERVER_HEARTBEAT_SECS",
        "RU_SERVER_PUBLIC_WS_BASE",
        "RU_ADMIN_USERNAME",
        "RU_ADMIN_PASSWORD",
        "RU_ADMIN_SESSION_TTL_SECS",
        "RU_SERVER_SHARED_TOKEN",
        "RU_SERVER_NODE_TOKENS_JSON",
    ];

    #[test]
    fn config_defaults_to_single_port_mode() {
        let _guard = ENV_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
        let _env = TestEnv::capture();
        clear_env();
        std::env::set_var("RU_SERVER_SHARED_TOKEN", "dev-shared-token");

        let config = ServerConfig::from_env().unwrap();

        assert_eq!(config.http_bind, "0.0.0.0:18080");
        assert_eq!(config.ws_bind, "0.0.0.0:18080");
        assert_eq!(config.public_ws_base, None);
    }

    #[test]
    fn config_supports_split_http_and_ws_ports() {
        let _guard = ENV_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
        let _env = TestEnv::capture();
        clear_env();
        std::env::set_var("RU_SERVER_SHARED_TOKEN", "dev-shared-token");
        std::env::set_var("RU_SERVER_HTTP_BIND", "127.0.0.1:18080");
        std::env::set_var("RU_SERVER_WS_BIND", "127.0.0.1:18081");
        std::env::set_var("RU_SERVER_PUBLIC_WS_BASE", "ws://127.0.0.1:18081/ws/nodes");

        let config = ServerConfig::from_env().unwrap();

        assert_eq!(config.http_bind, "127.0.0.1:18080");
        assert_eq!(config.ws_bind, "127.0.0.1:18081");
        assert_eq!(
            config.public_ws_base.as_deref(),
            Some("ws://127.0.0.1:18081/ws/nodes")
        );
    }

    fn clear_env() {
        for key in ENV_KEYS {
            std::env::remove_var(key);
        }
    }

    struct TestEnv {
        values: Vec<(&'static str, Option<OsString>)>,
    }

    impl TestEnv {
        fn capture() -> Self {
            Self {
                values: ENV_KEYS
                    .iter()
                    .map(|key| (*key, std::env::var_os(key)))
                    .collect(),
            }
        }
    }

    impl Drop for TestEnv {
        fn drop(&mut self) {
            for (key, value) in &self.values {
                match value {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
        }
    }
}
