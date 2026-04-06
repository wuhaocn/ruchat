use crate::config::ClientConfig;
use hyper::body::to_bytes;
use hyper::client::HttpConnector;
use hyper::header::CONTENT_TYPE;
use hyper::{Body, Client, Method, Request, StatusCode, Uri};
use ru_command_protocol::{BootstrapRequest, BootstrapResponse};
use std::io;
use std::time::Duration;

pub(crate) async fn fetch_bootstrap(
    client: &Client<HttpConnector>,
    config: &ClientConfig,
) -> Result<BootstrapResponse, Box<dyn std::error::Error>> {
    let url = format!(
        "{}/api/v1/bootstrap",
        config.server_url.trim_end_matches('/')
    );

    post_json(
        client,
        &url,
        &BootstrapRequest {
            node_id: config.node_id.clone(),
            auth_token: config.auth_token.clone(),
        },
        config.request_timeout_secs,
    )
    .await
}

async fn post_json<TReq, TResp>(
    client: &Client<HttpConnector>,
    url: &str,
    payload: &TReq,
    timeout_secs: Option<u64>,
) -> Result<TResp, Box<dyn std::error::Error>>
where
    TReq: serde::Serialize,
    TResp: serde::de::DeserializeOwned,
{
    let body = serde_json::to_vec(payload)?;
    let request = Request::builder()
        .method(Method::POST)
        .uri(parse_http_uri(url)?)
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(body))?;

    let response = tokio::time::timeout(
        Duration::from_secs(timeout_secs.unwrap_or(10)),
        client.request(request),
    )
    .await??;

    if response.status() != StatusCode::OK && response.status() != StatusCode::CREATED {
        let status = response.status();
        let body = to_bytes(response.into_body()).await?;
        let message = String::from_utf8_lossy(&body).into_owned();
        return Err(io::Error::new(
            io::ErrorKind::Other,
            format!("request failed: {status} {message}"),
        )
        .into());
    }

    let body = to_bytes(response.into_body()).await?;
    Ok(serde_json::from_slice(&body)?)
}

fn parse_http_uri(url: &str) -> Result<Uri, Box<dyn std::error::Error>> {
    if !url.starts_with("http://") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "only http:// server_url is supported in bootstrap",
        )
        .into());
    }

    Ok(url.parse()?)
}
