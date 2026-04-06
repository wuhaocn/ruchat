use std::collections::HashMap;

#[derive(Clone, Default)]
pub(crate) struct AuthConfig {
    shared_token: Option<String>,
    node_tokens: HashMap<String, String>,
}

impl AuthConfig {
    pub(crate) fn new(
        shared_token: Option<String>,
        node_tokens: HashMap<String, String>,
    ) -> Result<Self, String> {
        if shared_token.is_none() && node_tokens.is_empty() {
            return Err(
                "missing auth config: set RU_SERVER_SHARED_TOKEN or RU_SERVER_NODE_TOKENS_JSON"
                    .to_string(),
            );
        }

        Ok(Self {
            shared_token,
            node_tokens,
        })
    }

    pub(crate) fn verify_node_token(
        &self,
        node_id: &str,
        presented_token: &str,
    ) -> Result<(), &'static str> {
        let expected = self
            .node_tokens
            .get(node_id)
            .map(String::as_str)
            .or(self.shared_token.as_deref())
            .ok_or("server auth is not configured")?;

        if expected == presented_token {
            Ok(())
        } else {
            Err("invalid agent token")
        }
    }
}
