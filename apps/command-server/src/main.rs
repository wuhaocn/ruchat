mod app_state;
mod auth;
mod config;
mod console;
mod db;
mod error;
mod http;
mod session;
mod ws;

use app_state::AppState;
use auth::AuthConfig;
use config::ServerConfig;
use db::Database;
use std::net::SocketAddr;
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = ServerConfig::from_env()?;
    let auth = AuthConfig::new(config.shared_token.clone(), config.agent_tokens.clone())?;
    let state = Arc::new(AppState::new(
        Database::open(&config.db_path)?,
        config.heartbeat_interval_secs,
        config.public_ws_base.clone(),
        auth,
        config.admin_username.clone(),
        config.admin_password.clone(),
        config.admin_session_ttl_secs,
    ));
    if config.http_bind == config.ws_bind {
        let app = http::router(state);
        println!(
            "command-plane-server listening on http+ws={}, sqlite={}",
            config.http_bind, config.db_path
        );

        axum::Server::bind(&config.http_bind.parse::<SocketAddr>()?)
            .serve(app.into_make_service())
            .await?;
    } else {
        let http_bind = config.http_bind.parse::<SocketAddr>()?;
        let ws_bind = config.ws_bind.parse::<SocketAddr>()?;
        let http_app = http::control_router(state.clone());
        let ws_app = http::ws_router(state);

        println!(
            "command-plane-server listening on http={}, ws={}, sqlite={}",
            config.http_bind, config.ws_bind, config.db_path
        );

        tokio::try_join!(
            axum::Server::bind(&http_bind).serve(http_app.into_make_service()),
            axum::Server::bind(&ws_bind).serve(ws_app.into_make_service())
        )?;
    }

    Ok(())
}
