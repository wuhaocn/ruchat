mod bootstrap;
mod config;
mod executor;
mod ws;

use config::ClientConfig;
use hyper::Client;
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = ClientConfig::load_from_args()?;
    let client = Client::new();

    println!(
        "ru-command-client started: agent_id={}, server={}",
        config.agent_id, config.server_url
    );

    loop {
        let bootstrap = match bootstrap::fetch_bootstrap(&client, &config).await {
            Ok(bootstrap) => bootstrap,
            Err(error) => {
                eprintln!("bootstrap failed: {error}");
                tokio::time::sleep(Duration::from_secs(config.poll_interval_secs)).await;
                continue;
            }
        };

        if let Err(error) = ws::run_ws_session(&config, bootstrap).await {
            eprintln!("session ended: {error}");
            tokio::time::sleep(Duration::from_secs(config.poll_interval_secs)).await;
        }
    }
}
