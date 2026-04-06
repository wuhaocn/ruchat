use crate::auth::AuthConfig;
use crate::db::Database;
use axum::http::{HeaderMap, StatusCode};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;

pub(crate) enum SessionSignal {
    TryDispatch,
    CancelTask {
        task_id: u64,
        reason: Option<String>,
    },
}

pub(crate) struct AppState {
    pub(crate) db: Database,
    pub(crate) heartbeat_interval_secs: u64,
    pub(crate) auth: AuthConfig,
    admin_username: String,
    admin_password: String,
    admin_session_ttl_secs: u64,
    admin_sessions: Mutex<HashMap<String, u64>>,
    admin_session_counter: AtomicU64,
    sessions: Mutex<HashMap<String, mpsc::UnboundedSender<SessionSignal>>>,
    public_ws_base: Option<String>,
}

impl AppState {
    pub(crate) fn new(
        db: Database,
        heartbeat_interval_secs: u64,
        public_ws_base: Option<String>,
        auth: AuthConfig,
        admin_username: String,
        admin_password: String,
        admin_session_ttl_secs: u64,
    ) -> Self {
        Self {
            db,
            heartbeat_interval_secs,
            auth,
            admin_username,
            admin_password,
            admin_session_ttl_secs,
            admin_sessions: Mutex::new(HashMap::new()),
            admin_session_counter: AtomicU64::new(1),
            sessions: Mutex::new(HashMap::new()),
            public_ws_base,
        }
    }

    pub(crate) fn verify_admin_credentials(&self, username: &str, password: &str) -> bool {
        username == self.admin_username && password == self.admin_password
    }

    pub(crate) fn create_admin_session(&self) -> String {
        let token = format!(
            "{:x}-{:x}",
            unix_now(),
            self.admin_session_counter.fetch_add(1, Ordering::Relaxed)
        );

        if let Ok(mut sessions) = self.admin_sessions.lock() {
            sessions.insert(token.clone(), unix_now());
        }

        token
    }

    pub(crate) fn admin_session_ttl_secs(&self) -> u64 {
        self.admin_session_ttl_secs
    }

    pub(crate) fn remove_admin_session(&self, token: &str) {
        if let Ok(mut sessions) = self.admin_sessions.lock() {
            sessions.remove(token);
        }
    }

    pub(crate) fn extract_admin_session_token(&self, headers: &HeaderMap) -> Option<String> {
        extract_cookie(headers, "ru_admin_session")
    }

    pub(crate) fn has_valid_admin_session(&self, headers: &HeaderMap) -> bool {
        let Some(token) = extract_cookie(headers, "ru_admin_session") else {
            return false;
        };

        let now = unix_now();
        let Ok(mut sessions) = self.admin_sessions.lock() else {
            return false;
        };
        let Some(created_at) = sessions.get(&token).copied() else {
            return false;
        };

        if now.saturating_sub(created_at) > self.admin_session_ttl_secs {
            sessions.remove(&token);
            return false;
        }

        true
    }

    pub(crate) fn notify_node(&self, node_id: &str) {
        let sender = self
            .sessions
            .lock()
            .ok()
            .and_then(|sessions| sessions.get(node_id).cloned());

        if let Some(sender) = sender {
            let _ = sender.send(SessionSignal::TryDispatch);
        }
    }

    pub(crate) fn notify_task_cancel(&self, node_id: &str, task_id: u64, reason: Option<String>) {
        let sender = self
            .sessions
            .lock()
            .ok()
            .and_then(|sessions| sessions.get(node_id).cloned());

        if let Some(sender) = sender {
            let _ = sender.send(SessionSignal::CancelTask { task_id, reason });
        }
    }

    pub(crate) fn insert_session(
        &self,
        node_id: String,
        sender: mpsc::UnboundedSender<SessionSignal>,
    ) {
        if let Ok(mut sessions) = self.sessions.lock() {
            sessions.insert(node_id, sender);
        }
    }

    pub(crate) fn remove_session(&self, node_id: &str) {
        if let Ok(mut sessions) = self.sessions.lock() {
            sessions.remove(node_id);
        }
    }

    pub(crate) fn resolve_ws_url(
        &self,
        headers: &HeaderMap,
        node_id: &str,
    ) -> Result<String, (StatusCode, String)> {
        if let Some(base) = &self.public_ws_base {
            return Ok(format!("{}/{}", base.trim_end_matches('/'), node_id));
        }

        let host = headers
            .get("host")
            .and_then(|value| value.to_str().ok())
            .ok_or_else(|| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "missing host header and no RU_SERVER_PUBLIC_WS_BASE configured".to_string(),
                )
            })?;

        Ok(format!("ws://{host}/ws/nodes/{node_id}"))
    }
}

fn extract_cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    let cookie_header = headers.get("cookie")?.to_str().ok()?;

    for part in cookie_header.split(';') {
        let trimmed = part.trim();
        let (cookie_name, cookie_value) = trimmed.split_once('=')?;
        if cookie_name == name {
            return Some(cookie_value.to_string());
        }
    }

    None
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}
