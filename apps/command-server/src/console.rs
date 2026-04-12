use crate::app_state::AppState;
use crate::db::{CancelTaskOutcome, CreateTaskOutcome, TaskAudit};
use crate::error::internal_error;
use axum::extract::{Form, Path, Query, State};
use axum::http::header::SET_COOKIE;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::Router;
use ru_command_protocol::{
    node_ack_topic, node_control_topic, node_hello_topic, node_result_topic, node_task_topic,
    CommandDescriptor, CreateTaskRequest, ExecutionResult, NodeSnapshot, SessionEvent,
    SessionEventKind, TaskSnapshot, TaskStatus, CONTROL_PROTOCOL_VERSION, CONTROL_TRANSPORT_STACK,
    MAX_RESULT_OUTPUT_BYTES,
};
use serde::Deserialize;
use std::sync::Arc;

pub(crate) fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/console", get(console_index))
        .route("/console/login", get(login_page).post(login_submit))
        .route("/console/logout", post(logout))
        .route("/console/runtime", get(runtime_page))
        .route("/console/protocol", get(protocol_page))
        .route("/console/session-events", get(session_events_page))
        .route("/console/nodes", get(nodes_page))
        .route("/console/nodes/:node_id", get(node_detail_page))
        .route("/console/tasks", get(tasks_page).post(create_task_submit))
        .route("/console/tasks/:task_id/cancel", post(cancel_task_submit))
}

#[derive(Deserialize)]
struct LoginForm {
    username: String,
    password: String,
}

#[derive(Deserialize)]
struct TaskForm {
    node_id: String,
    command_name: String,
    #[serde(default)]
    args_text: String,
    #[serde(default)]
    timeout_secs: String,
    #[serde(default)]
    return_to: String,
}

#[derive(Deserialize)]
struct CancelTaskForm {
    #[serde(default)]
    return_to: String,
}

#[derive(Deserialize, Default)]
struct SessionEventPageQuery {
    #[serde(default)]
    node_id: String,
    #[serde(default)]
    kind: String,
}

async fn console_index(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if state.has_valid_admin_session(&headers) {
        Redirect::to("/console/nodes").into_response()
    } else {
        Redirect::to("/console/login").into_response()
    }
}

async fn login_page(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if state.has_valid_admin_session(&headers) {
        return Redirect::to("/console/nodes").into_response();
    }

    login_response(None, StatusCode::OK)
}

async fn login_submit(State(state): State<Arc<AppState>>, Form(form): Form<LoginForm>) -> Response {
    if !state.verify_admin_credentials(&form.username, &form.password) {
        return login_response(
            Some("invalid username or password"),
            StatusCode::UNAUTHORIZED,
        );
    }

    let token = state.create_admin_session();
    let cookie = build_session_cookie(&token, state.admin_session_ttl_secs());
    let mut response = Redirect::to("/console/nodes").into_response();
    response
        .headers_mut()
        .insert(SET_COOKIE, HeaderValue::from_str(&cookie).unwrap());
    response
}

async fn logout(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if let Some(token) = state.extract_admin_session_token(&headers) {
        state.remove_admin_session(&token);
    }

    let mut response = Redirect::to("/console/login").into_response();
    response
        .headers_mut()
        .insert(SET_COOKIE, HeaderValue::from_static(CLEAR_SESSION_COOKIE));
    response
}

async fn nodes_page(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if let Some(response) = require_console_session(&state, &headers) {
        return response;
    }
    let nodes = match state.db.list_nodes() {
        Ok(nodes) => state.enrich_node_snapshots(nodes),
        Err(error) => return server_error_response(error),
    };
    let rows = render_nodes_rows(&nodes);
    let summary = render_nodes_summary(&nodes);
    let script = render_nodes_page_script();

    let body = format!(
        "<section><h1>Nodes</h1><p id=\"nodes-summary\">{summary}</p><table><thead><tr><th>Node</th><th>Status</th><th>Hostname</th><th>Platform</th><th>Commands</th><th>Last Seen</th></tr></thead><tbody id=\"nodes-table-body\">{rows}</tbody></table></section>{script}",
        summary = summary,
        rows = rows,
        script = script
    );

    Html(console_html("Nodes", body, true, "")).into_response()
}

fn render_nodes_summary(nodes: &[NodeSnapshot]) -> String {
    let online = nodes.iter().filter(|node| node.online).count();
    format!(
        "Registered clients and the commands they reported. {online}/{total} currently online. Auto refresh 3s.",
        online = online,
        total = nodes.len()
    )
}

fn render_nodes_rows(nodes: &[NodeSnapshot]) -> String {
    let mut rows = String::new();
    for node in nodes {
        let status = if node.online {
            "<span class=\"status-chip active\">online</span>"
        } else {
            "<span class=\"status-chip idle\">offline</span>"
        };
        rows.push_str(&format!(
            "<tr><td><a href=\"/console/nodes/{id}\">{id}</a></td><td>{status}</td><td>{host}</td><td>{platform}</td><td>{count}</td><td>{last_seen}</td></tr>",
            id = escape_html(&node.node_id),
            status = status,
            host = escape_html(&node.hostname),
            platform = escape_html(&node.platform),
            count = node.commands.len(),
            last_seen = node.last_seen_unix_secs
        ));
    }

    if rows.is_empty() {
        rows.push_str("<tr><td colspan=\"6\">no nodes registered</td></tr>");
    }

    rows
}

fn render_nodes_page_script() -> String {
    "<script>
const nodesSummary = document.getElementById('nodes-summary');
const nodesTableBody = document.getElementById('nodes-table-body');

function escapeHtml(value) {
  return String(value ?? '')
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/\"/g, '&quot;')
    .replace(/'/g, '&#39;');
}

function renderNodesSummary(nodes) {
  const online = nodes.filter((node) => node.online).length;
  return `Registered clients and the commands they reported. ${online}/${nodes.length} currently online. Auto refresh 3s.`;
}

function renderNodeRow(node) {
  const status = node.online
    ? '<span class=\"status-chip active\">online</span>'
    : '<span class=\"status-chip idle\">offline</span>';
  const commandCount = Array.isArray(node.commands) ? node.commands.length : 0;
  return `<tr><td><a href=\"/console/nodes/${escapeHtml(node.node_id)}\">${escapeHtml(node.node_id)}</a></td><td>${status}</td><td>${escapeHtml(node.hostname)}</td><td>${escapeHtml(node.platform)}</td><td>${escapeHtml(commandCount)}</td><td>${escapeHtml(node.last_seen_unix_secs)}</td></tr>`;
}

async function refreshNodesPage() {
  try {
    const response = await fetch('/api/v1/nodes', { credentials: 'same-origin' });
    if (!response.ok) return;
    const nodes = await response.json();
    if (nodesSummary) nodesSummary.textContent = renderNodesSummary(nodes);
    if (nodesTableBody) {
      nodesTableBody.innerHTML = nodes.length
        ? nodes.map(renderNodeRow).join('')
        : '<tr><td colspan=\"6\">no nodes registered</td></tr>';
    }
  } catch (_error) {}
}

refreshNodesPage();
setInterval(refreshNodesPage, 3000);
</script>"
        .to_string()
}

async fn protocol_page(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if let Some(response) = require_console_session(&state, &headers) {
        return response;
    }

    let snapshot = state.protocol_snapshot();
    let capabilities = if snapshot.capabilities.is_empty() {
        "<span class=\"protocol-chip\">none</span>".to_string()
    } else {
        snapshot
            .capabilities
            .iter()
            .map(|item| format!("<span class=\"protocol-chip\">{}</span>", escape_html(item)))
            .collect::<Vec<_>>()
            .join("")
    };

    let body = format!(
        "<section><h1>Protocol</h1><p>Current command plane transport and business-layer contract exposed by this server.</p><div class=\"protocol-page-card\"><dl class=\"protocol-facts\"><div><dt>Protocol Version</dt><dd>{protocol_version}</dd></div><div><dt>Transport Stack</dt><dd>{transport_stack}</dd></div><div><dt>Heartbeat</dt><dd>{heartbeat_secs}s</dd></div><div><dt>Max Result Output</dt><dd>{max_result_output_bytes} bytes</dd></div><div><dt>Max Pull Items</dt><dd>{max_pull_items}</dd></div></dl><div><p class=\"protocol-label\">Capabilities</p><div class=\"protocol-strip\">{capabilities}</div></div></div><div class=\"protocol-grid\"><article class=\"protocol-section-card\"><div class=\"protocol-section-head\"><div><p class=\"eyebrow\">Layers</p><h2>Transport Stack</h2></div></div>{transport_layers}</article><article class=\"protocol-section-card\"><div class=\"protocol-section-head\"><div><p class=\"eyebrow\">Frames</p><h2>MQTT-like Lifecycle</h2></div></div>{transport_frames}</article><article class=\"protocol-section-card protocol-span-2\"><div class=\"protocol-section-head\"><div><p class=\"eyebrow\">Topics</p><h2>Route Map</h2></div><p class=\"muted\">Example node: <code>node-1</code></p></div><div class=\"protocol-table-wrap\"><table class=\"protocol-table\"><thead><tr><th>Topic</th><th>Direction</th><th>Payload</th><th>Use</th></tr></thead><tbody>{topic_rows}</tbody></table></div></article><article class=\"protocol-section-card protocol-span-2\"><div class=\"protocol-section-head\"><div><p class=\"eyebrow\">Payloads</p><h2>Business Message Types</h2></div></div><div class=\"protocol-table-wrap\"><table class=\"protocol-table\"><thead><tr><th>Message</th><th>Carrier Topic</th><th>Direction</th><th>Meaning</th></tr></thead><tbody>{payload_rows}</tbody></table></div></article><article class=\"protocol-section-card protocol-span-2\"><div class=\"protocol-section-head\"><div><p class=\"eyebrow\">Sequence</p><h2>Minimum Session Flow</h2></div></div>{sequence_rows}</article><article class=\"protocol-section-card protocol-span-2\"><div class=\"protocol-section-head\"><div><p class=\"eyebrow\">Examples</p><h2>Reference Payloads</h2></div><p class=\"muted\">Shown as JSON-like views for easier debugging. WS business payloads are still protobuf on wire.</p></div>{example_rows}</article><article class=\"protocol-section-card\"><div class=\"protocol-section-head\"><div><p class=\"eyebrow\">Constraints</p><h2>Current Implementation Limits</h2></div></div>{constraint_rows}</article><article class=\"protocol-section-card\"><div class=\"protocol-section-head\"><div><p class=\"eyebrow\">Errors</p><h2>Common Rejects And Rejections</h2></div></div>{error_rows}</article></div></section>",
        protocol_version = escape_html(&snapshot.protocol_version),
        transport_stack = escape_html(&snapshot.transport_stack),
        heartbeat_secs = snapshot.heartbeat_interval_secs,
        max_result_output_bytes = snapshot.max_result_output_bytes,
        max_pull_items = snapshot.max_task_pull_response_items,
        capabilities = capabilities,
        transport_layers = render_protocol_layers(),
        transport_frames = render_protocol_frames(),
        topic_rows = render_protocol_topic_rows(),
        payload_rows = render_protocol_payload_rows(),
        sequence_rows = render_protocol_sequence_rows(),
        example_rows = render_protocol_example_rows(&snapshot),
        constraint_rows = render_protocol_constraint_rows(),
        error_rows = render_protocol_error_rows(),
    );

    Html(console_html("Protocol", body, true, "")).into_response()
}

async fn runtime_page(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if let Some(response) = require_console_session(&state, &headers) {
        return response;
    }

    let nodes = match state.db.list_nodes() {
        Ok(nodes) => state.enrich_node_snapshots(nodes),
        Err(error) => return server_error_response(error),
    };
    let tasks = match state.db.list_recent_tasks(20) {
        Ok(tasks) => tasks,
        Err(error) => return server_error_response(error),
    };
    let events = match state.db.list_session_events(None, None, 12) {
        Ok(events) => events,
        Err(error) => return server_error_response(error),
    };

    let body = format!(
        "<section><h1>Runtime</h1><p id=\"runtime-summary\">{summary}</p><div id=\"runtime-overview\" class=\"runtime-overview-grid\">{overview_cards}</div><div class=\"runtime-monitor-grid\"><article class=\"runtime-panel\"><div class=\"runtime-panel-head\"><div><p class=\"eyebrow\">Nodes</p><h2>Online Nodes</h2></div><div class=\"pane-meta\">auto refresh 3s</div></div><div id=\"runtime-nodes-list\" class=\"runtime-list\">{nodes_list}</div></article><article class=\"runtime-panel\"><div class=\"runtime-panel-head\"><div><p class=\"eyebrow\">Tasks</p><h2>Active Tasks</h2></div><a class=\"runtime-mini-link\" href=\"/console/tasks\">open tasks</a></div><div id=\"runtime-active-tasks\" class=\"runtime-list\">{active_tasks}</div></article><article class=\"runtime-panel runtime-panel-wide\"><div class=\"runtime-panel-head\"><div><p class=\"eyebrow\">Session</p><h2>Recent Session Events</h2></div><a class=\"runtime-mini-link\" href=\"/console/session-events\">open events</a></div><div id=\"runtime-session-events\" class=\"session-events-list global-feed\">{session_events}</div></article><article class=\"runtime-panel runtime-panel-wide\"><div class=\"runtime-panel-head\"><div><p class=\"eyebrow\">Results</p><h2>Latest Task Outcomes</h2></div><a class=\"runtime-mini-link\" href=\"/console/tasks\">open tasks</a></div><div id=\"runtime-recent-results\" class=\"runtime-list\">{recent_results}</div></article></div></section>{script}",
        summary = render_runtime_summary_text(&nodes, &tasks, &events),
        overview_cards = render_runtime_overview_cards(&nodes, &tasks, &events),
        nodes_list = render_runtime_nodes_list(&nodes),
        active_tasks = render_runtime_active_tasks(&tasks),
        session_events = render_session_events(&events, true),
        recent_results = render_runtime_recent_results(&tasks),
        script = render_runtime_page_script(),
    );

    Html(console_html("Runtime", body, true, "")).into_response()
}

fn render_protocol_layers() -> String {
    [
        ("1", "HTTP", "bootstrap address discovery and admin control API"),
        ("2", "WebSocket", "single binary long connection carrier"),
        ("3", "MQTT-like", "CONNECT / SUBSCRIBE / PUBLISH / PING semantics"),
        ("4", "Protobuf", "business payload envelope and task messages"),
    ]
    .iter()
    .map(|(index, name, detail)| {
        format!(
            "<div class=\"protocol-layer-row\"><span class=\"protocol-layer-index\">{index}</span><div><p class=\"protocol-layer-name\">{name}</p><p class=\"muted\">{detail}</p></div></div>",
            index = escape_html(index),
            name = escape_html(name),
            detail = escape_html(detail)
        )
    })
    .collect::<Vec<_>>()
    .join("")
}

fn render_protocol_frames() -> String {
    [
        ("CONNECT", "client -> server", "authenticate and bind the WS session"),
        ("CONNACK", "server -> client", "accept or reject the session"),
        ("SUBSCRIBE", "client -> server", "request task/control topics"),
        ("SUBACK", "server -> client", "confirm topic subscription"),
        ("PUBLISH", "both", "carry protobuf business messages"),
        ("PINGREQ", "both", "heartbeat request"),
        ("PINGRESP", "both", "heartbeat response"),
    ]
    .iter()
    .map(|(frame, direction, detail)| {
        format!(
            "<div class=\"protocol-frame-row\"><div><p class=\"protocol-layer-name\">{frame}</p><p class=\"muted\">{detail}</p></div><span class=\"direction-chip\">{direction}</span></div>",
            frame = escape_html(frame),
            direction = escape_html(direction),
            detail = escape_html(detail)
        )
    })
    .collect::<Vec<_>>()
    .join("")
}

fn render_protocol_topic_rows() -> String {
    let example_node = "node-1";
    [
        (
            node_hello_topic(example_node),
            "client -> server",
            "ClientHello, CommandCatalog",
            "register node identity and command set",
        ),
        (
            node_task_topic(example_node),
            "server -> client",
            "TaskAssignment",
            "push the next executable task",
        ),
        (
            node_ack_topic(example_node),
            "client -> server",
            "TaskAck",
            "confirm task receipt and execution start",
        ),
        (
            node_result_topic(example_node),
            "client -> server",
            "TaskResult",
            "return stdout, stderr, exit code, duration",
        ),
        (
            node_control_topic(example_node),
            "both",
            "SessionInfo, TaskPull*, TaskCancel, Error",
            "session metadata, pull path, cancel, protocol errors",
        ),
    ]
    .iter()
    .map(|(topic, direction, payload, detail)| {
        format!(
            "<tr><td><code>{topic}</code></td><td><span class=\"direction-chip\">{direction}</span></td><td>{payload}</td><td>{detail}</td></tr>",
            topic = escape_html(topic),
            direction = escape_html(direction),
            payload = escape_html(payload),
            detail = escape_html(detail)
        )
    })
    .collect::<Vec<_>>()
    .join("")
}

fn render_protocol_payload_rows() -> String {
    [
        (
            "SessionInfo",
            "nodes/{node_id}/control",
            "server -> client",
            "declare session metadata, heartbeat, protocol version",
        ),
        (
            "ClientHello",
            "nodes/{node_id}/hello",
            "client -> server",
            "register hostname, platform, poll interval",
        ),
        (
            "CommandCatalog",
            "nodes/{node_id}/hello",
            "client -> server",
            "report supported commands separately from hello",
        ),
        (
            "TaskAssignment",
            "nodes/{node_id}/task",
            "server -> client",
            "deliver a queued task to execute",
        ),
        (
            "TaskAck",
            "nodes/{node_id}/ack",
            "client -> server",
            "mark task as accepted and running",
        ),
        (
            "TaskResult",
            "nodes/{node_id}/result",
            "client -> server",
            "complete the task with execution output",
        ),
        (
            "TaskPullRequest",
            "nodes/{node_id}/control",
            "client -> server",
            "compensating pull when the client wants pending work",
        ),
        (
            "TaskPullResponse",
            "nodes/{node_id}/control",
            "server -> client",
            "return zero or one queued task in current implementation",
        ),
        (
            "TaskCancel",
            "nodes/{node_id}/control",
            "server -> client",
            "request client-side process termination",
        ),
        (
            "Error",
            "nodes/{node_id}/control",
            "server -> client",
            "report protocol or business-level rejection",
        ),
    ]
    .iter()
    .map(|(message, topic, direction, detail)| {
        format!(
            "<tr><td><code>{message}</code></td><td><code>{topic}</code></td><td><span class=\"direction-chip\">{direction}</span></td><td>{detail}</td></tr>",
            message = escape_html(message),
            topic = escape_html(topic),
            direction = escape_html(direction),
            detail = escape_html(detail)
        )
    })
    .collect::<Vec<_>>()
    .join("")
}

fn render_protocol_sequence_rows() -> String {
    [
        ("1", "HTTP", "client -> server", "POST /api/v1/bootstrap"),
        ("2", "MQTT-like", "client -> server", "CONNECT"),
        ("3", "MQTT-like", "server -> client", "CONNACK"),
        ("4", "MQTT-like", "client -> server", "SUBSCRIBE(task, control)"),
        ("5", "MQTT-like", "server -> client", "SUBACK"),
        ("6", "PB over PUBLISH", "server -> client", "SessionInfo on control topic"),
        ("7", "PB over PUBLISH", "client -> server", "ClientHello on hello topic"),
        ("8", "PB over PUBLISH", "client -> server", "CommandCatalog on hello topic"),
        ("9", "PB over PUBLISH", "server -> client", "TaskAssignment on task topic"),
        ("10", "PB over PUBLISH", "client -> server", "TaskAck then TaskResult"),
    ]
    .iter()
    .map(|(step, layer, direction, detail)| {
        format!(
            "<div class=\"protocol-sequence-row\"><span class=\"protocol-layer-index\">{step}</span><div><p class=\"protocol-layer-name\">{detail}</p><p class=\"muted\">{layer}</p></div><span class=\"direction-chip\">{direction}</span></div>",
            step = escape_html(step),
            detail = escape_html(detail),
            layer = escape_html(layer),
            direction = escape_html(direction)
        )
    })
    .collect::<Vec<_>>()
    .join("")
}

fn render_protocol_example_rows(snapshot: &ru_command_protocol::ProtocolSnapshot) -> String {
    let bootstrap_example = format!(
        "{{\n  \"node_id\": \"node-1\",\n  \"ws_url\": \"ws://127.0.0.1:18080/ws/nodes/node-1\",\n  \"heartbeat_interval_secs\": {heartbeat},\n  \"protocol_version\": \"{protocol_version}\",\n  \"transport_stack\": \"{transport_stack}\",\n  \"capabilities\": [\n{capabilities}\n  ]\n}}",
        heartbeat = snapshot.heartbeat_interval_secs,
        protocol_version = snapshot.protocol_version,
        transport_stack = snapshot.transport_stack,
        capabilities = snapshot
            .capabilities
            .iter()
            .map(|item| format!("    \"{item}\""))
            .collect::<Vec<_>>()
            .join(",\n")
    );
    let session_info_example = format!(
        "{{\n  \"session_info\": {{\n    \"session_id\": \"node-1-1710000000\",\n    \"node_id\": \"node-1\",\n    \"heartbeat_interval_secs\": {heartbeat},\n    \"max_result_output_bytes\": {max_result},\n    \"protocol_version\": \"{protocol_version}\",\n    \"server_unix_secs\": 1710000000\n  }}\n}}",
        heartbeat = snapshot.heartbeat_interval_secs,
        max_result = snapshot.max_result_output_bytes,
        protocol_version = snapshot.protocol_version
    );
    let task_assignment_example = "{\n  \"task_assignment\": {\n    \"task_id\": 42,\n    \"command_name\": \"echo\",\n    \"args\": [\"hello\", \"world\"],\n    \"created_at_unix_secs\": 1710000001,\n    \"timeout_secs\": 30,\n    \"attempt\": 1\n  }\n}".to_string();
    let task_result_example = "{\n  \"task_result\": {\n    \"task_id\": 42,\n    \"success\": true,\n    \"exit_code\": 0,\n    \"stdout\": \"hello world\\n\",\n    \"stderr\": \"\",\n    \"duration_ms\": 12,\n    \"error\": null,\n    \"stdout_truncated\": false,\n    \"stderr_truncated\": false\n  }\n}".to_string();
    let task_pull_example = format!(
        "{{\n  \"task_pull_request\": {{\n    \"node_id\": \"node-1\",\n    \"limit\": {limit}\n  }}\n}}",
        limit = snapshot.max_task_pull_response_items
    );

    [
        (
            "Bootstrap Response",
            "HTTP /api/v1/bootstrap response body",
            bootstrap_example,
        ),
        (
            "SessionInfo",
            "protobuf business payload published on nodes/{node_id}/control",
            session_info_example,
        ),
        (
            "TaskAssignment",
            "protobuf business payload published on nodes/{node_id}/task",
            task_assignment_example,
        ),
        (
            "TaskResult",
            "protobuf business payload published on nodes/{node_id}/result",
            task_result_example,
        ),
        (
            "TaskPullRequest",
            "protobuf business payload published on nodes/{node_id}/control",
            task_pull_example,
        ),
    ]
    .iter()
    .map(|(title, detail, example)| {
        format!(
            "<div class=\"protocol-example-row\"><div><p class=\"protocol-layer-name\">{title}</p><p class=\"muted\">{detail}</p></div><pre class=\"protocol-example-block\">{example}</pre></div>",
            title = escape_html(title),
            detail = escape_html(detail),
            example = escape_html(example)
        )
    })
    .collect::<Vec<_>>()
    .join("")
}

fn render_runtime_summary_text(
    nodes: &[NodeSnapshot],
    tasks: &[TaskSnapshot],
    events: &[SessionEvent],
) -> String {
    let online = nodes.iter().filter(|node| node.online).count();
    let active = tasks
        .iter()
        .filter(|task| {
            matches!(
                task.status,
                TaskStatus::Queued | TaskStatus::Dispatched | TaskStatus::Running
            )
        })
        .count();
    let failed = tasks
        .iter()
        .filter(|task| matches!(task.status, TaskStatus::Failed | TaskStatus::Canceled))
        .count();
    format!(
        "Runtime snapshot across nodes, tasks, and session lifecycle. {online}/{total} nodes online, {active} active tasks, {failed} recent failed or canceled tasks, {events} recent session events. Auto refresh 3s.",
        online = online,
        total = nodes.len(),
        active = active,
        failed = failed,
        events = events.len()
    )
}

fn render_runtime_overview_cards(
    nodes: &[NodeSnapshot],
    tasks: &[TaskSnapshot],
    events: &[SessionEvent],
) -> String {
    let online = nodes.iter().filter(|node| node.online).count();
    let active = tasks
        .iter()
        .filter(|task| {
            matches!(
                task.status,
                TaskStatus::Queued | TaskStatus::Dispatched | TaskStatus::Running
            )
        })
        .count();
    let failed = tasks
        .iter()
        .filter(|task| matches!(task.status, TaskStatus::Failed | TaskStatus::Canceled))
        .count();
    let last_event = events
        .first()
        .map(|event| {
            format!(
                "{} @ {}",
                session_event_kind_label(event.kind),
                event.created_at_unix_secs
            )
        })
        .unwrap_or_else(|| "none".to_string());

    [
        ("Online Nodes", format!("{online}/{}", nodes.len()), "connected control sessions"),
        ("Active Tasks", active.to_string(), "queued, dispatched, or running"),
        ("Recent Failures", failed.to_string(), "failed or canceled task outcomes"),
        ("Last Session Event", last_event, "most recent lifecycle signal"),
    ]
    .iter()
    .map(|(label, value, detail)| {
        format!(
            "<article class=\"runtime-stat-card\"><p class=\"eyebrow\">{label}</p><p class=\"runtime-stat-value\">{value}</p><p class=\"muted\">{detail}</p></article>",
            label = escape_html(label),
            value = escape_html(value),
            detail = escape_html(detail)
        )
    })
    .collect::<Vec<_>>()
    .join("")
}

fn render_runtime_nodes_list(nodes: &[NodeSnapshot]) -> String {
    let online_nodes = nodes.iter().filter(|node| node.online).collect::<Vec<_>>();
    if online_nodes.is_empty() {
        return "<div class=\"runtime-empty\">No online nodes right now.</div>".to_string();
    }

    online_nodes
        .into_iter()
        .map(|node| {
            format!(
                "<article class=\"runtime-row-card\"><div><p class=\"runtime-row-title\"><a href=\"/console/nodes/{node_id}\">{node_id}</a></p><p class=\"muted\">{hostname} | {platform}</p></div><div class=\"runtime-row-meta\"><span class=\"status-chip active\">online</span><span class=\"runtime-inline-meta\">commands {commands}</span></div></article>",
                node_id = escape_html(&node.node_id),
                hostname = escape_html(&node.hostname),
                platform = escape_html(&node.platform),
                commands = node.commands.len()
            )
        })
        .collect::<Vec<_>>()
        .join("")
}

fn render_runtime_active_tasks(tasks: &[TaskSnapshot]) -> String {
    let active_tasks = tasks
        .iter()
        .filter(|task| {
            matches!(
                task.status,
                TaskStatus::Queued | TaskStatus::Dispatched | TaskStatus::Running
            )
        })
        .collect::<Vec<_>>();
    if active_tasks.is_empty() {
        return "<div class=\"runtime-empty\">No active tasks.</div>".to_string();
    }

    active_tasks
        .into_iter()
        .map(|task| {
            format!(
                "<article class=\"runtime-row-card\"><div><p class=\"runtime-row-title\">#{task_id} {command}</p><p class=\"muted\">node {node_id} | args {args}</p></div><div class=\"runtime-row-meta\"><span class=\"status-badge badge-{status}\">{status}</span></div></article>",
                task_id = task.task_id,
                command = escape_html(&task.command_name),
                node_id = escape_html(&task.node_id),
                args = escape_html(&display_args(&task.args)),
                status = task_status_label(task.status),
            )
        })
        .collect::<Vec<_>>()
        .join("")
}

fn render_runtime_recent_results(tasks: &[TaskSnapshot]) -> String {
    let recent_results = tasks
        .iter()
        .filter(|task| {
            matches!(
                task.status,
                TaskStatus::Succeeded | TaskStatus::Failed | TaskStatus::Canceled
            )
        })
        .take(8)
        .collect::<Vec<_>>();
    if recent_results.is_empty() {
        return "<div class=\"runtime-empty\">No finished task outcomes yet.</div>".to_string();
    }

    recent_results
        .into_iter()
        .map(|task| {
            let summary = match &task.result {
                Some(result) => format!(
                    "exit={:?} duration={}ms stdout={}",
                    result.exit_code,
                    result.duration_ms,
                    truncate_text(result.stdout.trim(), 80)
                ),
                None => task
                    .cancel_reason
                    .clone()
                    .or_else(|| task.retry_reason.clone())
                    .unwrap_or_else(|| "no result payload".to_string()),
            };
            format!(
                "<article class=\"runtime-row-card\"><div><p class=\"runtime-row-title\">#{task_id} {command}</p><p class=\"muted\">node {node_id} | {summary}</p></div><div class=\"runtime-row-meta\"><span class=\"status-badge badge-{status}\">{status}</span></div></article>",
                task_id = task.task_id,
                command = escape_html(&task.command_name),
                node_id = escape_html(&task.node_id),
                summary = escape_html(&summary),
                status = task_status_label(task.status),
            )
        })
        .collect::<Vec<_>>()
        .join("")
}

fn render_runtime_page_script() -> String {
    r#"<script>
const runtimeSummary = document.getElementById('runtime-summary');
const runtimeOverview = document.getElementById('runtime-overview');
const runtimeNodesList = document.getElementById('runtime-nodes-list');
const runtimeActiveTasks = document.getElementById('runtime-active-tasks');
const runtimeSessionEvents = document.getElementById('runtime-session-events');
const runtimeRecentResults = document.getElementById('runtime-recent-results');

function escapeHtml(value) {
  return String(value ?? '')
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/\"/g, '&quot;')
    .replace(/'/g, '&#39;');
}

function taskStatusLabel(status) {
  return String(status || 'unknown');
}

function sessionKindLabel(kind) {
  return String(kind || 'unknown');
}

function displayArgs(args) {
  return Array.isArray(args) && args.length ? args.join(' ') : '[]';
}

function truncateText(value, limit) {
  const text = String(value || '');
  return text.length <= limit ? text : text.slice(0, limit - 3) + '...';
}

function renderRuntimeSummary(nodes, tasks, events) {
  const online = nodes.filter((node) => node.online).length;
  const active = tasks.filter((task) => ['queued', 'dispatched', 'running'].includes(task.status)).length;
  const failed = tasks.filter((task) => ['failed', 'canceled'].includes(task.status)).length;
  return `Runtime snapshot across nodes, tasks, and session lifecycle. ${online}/${nodes.length} nodes online, ${active} active tasks, ${failed} recent failed or canceled tasks, ${events.length} recent session events. Auto refresh 3s.`;
}

function renderOverviewCards(nodes, tasks, events) {
  const online = nodes.filter((node) => node.online).length;
  const active = tasks.filter((task) => ['queued', 'dispatched', 'running'].includes(task.status)).length;
  const failed = tasks.filter((task) => ['failed', 'canceled'].includes(task.status)).length;
  const lastEvent = events.length ? `${sessionKindLabel(events[0].kind)} @ ${events[0].created_at_unix_secs}` : 'none';
  const cards = [
    { label: 'Online Nodes', value: `${online}/${nodes.length}`, detail: 'connected control sessions' },
    { label: 'Active Tasks', value: `${active}`, detail: 'queued, dispatched, or running' },
    { label: 'Recent Failures', value: `${failed}`, detail: 'failed or canceled task outcomes' },
    { label: 'Last Session Event', value: lastEvent, detail: 'most recent lifecycle signal' },
  ];
  return cards.map((card) => `<article class="runtime-stat-card"><p class="eyebrow">${escapeHtml(card.label)}</p><p class="runtime-stat-value">${escapeHtml(card.value)}</p><p class="muted">${escapeHtml(card.detail)}</p></article>`).join('');
}

function renderNodesList(nodes) {
  const onlineNodes = nodes.filter((node) => node.online);
  if (!onlineNodes.length) return '<div class="runtime-empty">No online nodes right now.</div>';
  return onlineNodes.map((node) => `<article class="runtime-row-card"><div><p class="runtime-row-title"><a href="/console/nodes/${escapeHtml(node.node_id)}">${escapeHtml(node.node_id)}</a></p><p class="muted">${escapeHtml(node.hostname)} | ${escapeHtml(node.platform)}</p></div><div class="runtime-row-meta"><span class="status-chip active">online</span><span class="runtime-inline-meta">commands ${escapeHtml((node.commands || []).length)}</span></div></article>`).join('');
}

function renderActiveTasks(tasks) {
  const activeTasks = tasks.filter((task) => ['queued', 'dispatched', 'running'].includes(task.status));
  if (!activeTasks.length) return '<div class="runtime-empty">No active tasks.</div>';
  return activeTasks.map((task) => `<article class="runtime-row-card"><div><p class="runtime-row-title">#${escapeHtml(task.task_id)} ${escapeHtml(task.command_name)}</p><p class="muted">node ${escapeHtml(task.node_id)} | args ${escapeHtml(displayArgs(task.args))}</p></div><div class="runtime-row-meta"><span class="status-badge badge-${escapeHtml(taskStatusLabel(task.status))}">${escapeHtml(taskStatusLabel(task.status))}</span></div></article>`).join('');
}

function renderSessionEvents(events) {
  if (!events.length) return '<div class="session-event empty">No connection events recorded yet.</div>';
  return events.map((event) => `<article class="session-event kind-${escapeHtml(sessionKindLabel(event.kind))}"><div class="session-event-head"><span class="session-kind">${escapeHtml(sessionKindLabel(event.kind))}</span><span class="session-time">${escapeHtml(event.created_at_unix_secs)}</span></div><p class="session-node"><a href="/console/nodes/${escapeHtml(event.node_id)}">${escapeHtml(event.node_id)}</a></p><p class="session-message">${escapeHtml(event.message || '')}</p></article>`).join('');
}

function renderRecentResults(tasks) {
  const finished = tasks.filter((task) => ['succeeded', 'failed', 'canceled'].includes(task.status)).slice(0, 8);
  if (!finished.length) return '<div class="runtime-empty">No finished task outcomes yet.</div>';
  return finished.map((task) => {
    const summary = task.result
      ? `exit=${task.result.exit_code} duration=${task.result.duration_ms}ms stdout=${truncateText((task.result.stdout || '').trim(), 80)}`
      : (task.cancel_reason || task.retry_reason || 'no result payload');
    return `<article class="runtime-row-card"><div><p class="runtime-row-title">#${escapeHtml(task.task_id)} ${escapeHtml(task.command_name)}</p><p class="muted">node ${escapeHtml(task.node_id)} | ${escapeHtml(summary)}</p></div><div class="runtime-row-meta"><span class="status-badge badge-${escapeHtml(taskStatusLabel(task.status))}">${escapeHtml(taskStatusLabel(task.status))}</span></div></article>`;
  }).join('');
}

async function refreshRuntimePage() {
  try {
    const [nodesResponse, tasksResponse, eventsResponse] = await Promise.all([
      fetch('/api/v1/nodes', { credentials: 'same-origin' }),
      fetch('/api/v1/tasks?limit=20', { credentials: 'same-origin' }),
      fetch('/api/v1/session-events?limit=12', { credentials: 'same-origin' }),
    ]);
    if (!nodesResponse.ok || !tasksResponse.ok || !eventsResponse.ok) return;
    const [nodes, tasks, events] = await Promise.all([
      nodesResponse.json(),
      tasksResponse.json(),
      eventsResponse.json(),
    ]);
    if (runtimeSummary) runtimeSummary.textContent = renderRuntimeSummary(nodes, tasks, events);
    if (runtimeOverview) runtimeOverview.innerHTML = renderOverviewCards(nodes, tasks, events);
    if (runtimeNodesList) runtimeNodesList.innerHTML = renderNodesList(nodes);
    if (runtimeActiveTasks) runtimeActiveTasks.innerHTML = renderActiveTasks(tasks);
    if (runtimeSessionEvents) runtimeSessionEvents.innerHTML = renderSessionEvents(events);
    if (runtimeRecentResults) runtimeRecentResults.innerHTML = renderRecentResults(tasks);
  } catch (_error) {}
}

refreshRuntimePage();
setInterval(refreshRuntimePage, 3000);
</script>"#
        .to_string()
}

fn render_protocol_constraint_rows() -> String {
    [
        (
            "single_inflight",
            "one node currently holds at most one in-flight task",
        ),
        (
            "hello split",
            "ClientHello only carries node identity; commands must be sent in CommandCatalog",
        ),
        (
            "pull limit",
            "TaskPullRequest.limit is capped by MAX_TASK_PULL_RESPONSE_ITEMS and currently returns at most one task",
        ),
        (
            "pull binding",
            "TaskPullRequest.node_id must match the authenticated WS session node_id",
        ),
        (
            "http/json",
            "HTTP bootstrap and admin APIs are JSON; protobuf only applies inside WS PUBLISH payloads",
        ),
        (
            "result mode",
            "stdout/stderr are returned at task completion, not streamed incrementally",
        ),
        (
            "cancel model",
            "task timeout retries use TaskCancel plus server-side retry, not a full two-phase shutdown protocol",
        ),
    ]
    .iter()
    .map(|(name, detail)| {
        format!(
            "<div class=\"protocol-note-row\"><p class=\"protocol-layer-name\">{name}</p><p class=\"muted\">{detail}</p></div>",
            name = escape_html(name),
            detail = escape_html(detail)
        )
    })
    .collect::<Vec<_>>()
    .join("")
}

fn render_protocol_error_rows() -> String {
    [
        (
            "CONNACK reject",
            "first websocket frame must be binary mqtt connect",
            "client does not start the WS session with a binary MQTT CONNECT frame",
        ),
        (
            "CONNACK reject",
            "first websocket frame must be mqtt connect",
            "client sends a binary frame, but it is not a CONNECT frame",
        ),
        (
            "CONNACK reject",
            "timed out waiting for mqtt connect",
            "client opens WS but does not send CONNECT within 10 seconds",
        ),
        (
            "CONNACK reject",
            "client_id mismatch between path and connect",
            "WS path node_id and CONNECT.client_id do not match",
        ),
        (
            "CONNACK reject",
            "auth_failed",
            "shared token or per-node token verification fails",
        ),
        (
            "PUBLISH error",
            "node_id mismatch in hello payload",
            "ClientHello.node_id does not match the bound session node_id",
        ),
        (
            "PUBLISH error",
            "command catalog received before registration",
            "client sends CommandCatalog before a valid ClientHello registration",
        ),
        (
            "PUBLISH error",
            "hello topic requires client hello or command catalog payload",
            "hello topic carries an unsupported payload type",
        ),
        (
            "PUBLISH error",
            "unsupported payload on control topic",
            "control topic carries a payload other than TaskPullRequest in client->server direction",
        ),
        (
            "PUBLISH error",
            "node_id mismatch in task pull request",
            "TaskPullRequest.node_id differs from the authenticated session node_id",
        ),
        (
            "PUBLISH error",
            "unexpected task ack without in-flight task / unexpected task ack id",
            "client acks a task that is not currently dispatched to the session",
        ),
        (
            "PUBLISH error",
            "unexpected task result without in-flight task / unexpected task result id",
            "client returns a result for a task that is not the current in-flight task",
        ),
    ]
    .iter()
    .map(|(kind, message, detail)| {
        format!(
            "<div class=\"protocol-note-row\"><p class=\"protocol-layer-name\">{kind}</p><p><code>{message}</code></p><p class=\"muted\">{detail}</p></div>",
            kind = escape_html(kind),
            message = escape_html(message),
            detail = escape_html(detail)
        )
    })
    .collect::<Vec<_>>()
    .join("")
}

async fn session_events_page(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<SessionEventPageQuery>,
) -> Response {
    if let Some(response) = require_console_session(&state, &headers) {
        return response;
    }

    let node_filter = normalize_query_value(&query.node_id);
    let kind_filter = match parse_session_event_kind_value(&query.kind) {
        Ok(kind) => kind,
        Err(message) => {
            return error_page(StatusCode::BAD_REQUEST, &message, "/console/session-events")
        }
    };
    let events = match state
        .db
        .list_session_events(node_filter.as_deref(), kind_filter, 40)
    {
        Ok(events) => events,
        Err(error) => return server_error_response(error),
    };
    let body = format!(
        "<section><h1>Session Events</h1><p id=\"session-events-summary\">{summary}</p><form id=\"session-events-filters\" class=\"session-filter-bar\" method=\"get\" action=\"/console/session-events\"><label>Node ID<input id=\"session-filter-node-id\" name=\"node_id\" value=\"{node_id}\" placeholder=\"node-1\"></label><label>Event Kind<select id=\"session-filter-kind\" name=\"kind\">{kind_options}</select></label><button type=\"submit\">Apply</button></form><div id=\"session-events-list\" class=\"session-events-list global-feed\">{events}</div></section>{script}",
        summary = render_session_events_summary(node_filter.as_deref(), kind_filter, events.len()),
        node_id = escape_html(query.node_id.trim()),
        kind_options = render_session_event_kind_options(query.kind.trim()),
        events = render_session_events(&events, true),
        script = render_session_events_page_script(query.node_id.trim(), query.kind.trim())
    );

    Html(console_html("Session Events", body, true, "")).into_response()
}

async fn node_detail_page(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(node_id): Path<String>,
) -> Response {
    if let Some(response) = require_console_session(&state, &headers) {
        return response;
    }

    let Some(node) = (match state.db.get_node(&node_id) {
        Ok(node) => node.map(|node| state.enrich_node_snapshot(node)),
        Err(error) => return server_error_response(error),
    }) else {
        return not_found_page("node not found");
    };

    let tasks = match state.db.list_tasks_for_node(&node.node_id, 20) {
        Ok(tasks) => tasks,
        Err(error) => return server_error_response(error),
    };
    let session_events = match state.db.list_session_events_for_node(&node.node_id, 8) {
        Ok(events) => events,
        Err(error) => return server_error_response(error),
    };
    let body = render_node_workspace(&node, &tasks, &session_events);

    Html(console_html(
        &format!("Node {}", node.node_id),
        body,
        false,
        "workspace-page",
    ))
    .into_response()
}

async fn tasks_page(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if let Some(response) = require_console_session(&state, &headers) {
        return response;
    }
    let tasks = match state.db.list_tasks() {
        Ok(tasks) => tasks,
        Err(error) => return server_error_response(error),
    };

    let mut rows = String::new();
    for task in tasks.iter().rev() {
        rows.push_str(&render_task_row(task, true));
    }
    if rows.is_empty() {
        rows.push_str("<tr><td colspan=\"8\">no tasks created</td></tr>");
    }

    let body = format!(
        "<section><h1>Tasks</h1><p>Execution history and latest results.</p><table><thead><tr><th>Task</th><th>Node</th><th>Command</th><th>Status</th><th>Args</th><th>Retry</th><th>Created</th><th>Result</th></tr></thead><tbody>{rows}</tbody></table></section>",
        rows = rows
    );

    Html(console_html("Tasks", body, true, "")).into_response()
}

async fn create_task_submit(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Form(form): Form<TaskForm>,
) -> Response {
    if let Some(response) = require_console_session(&state, &headers) {
        return response;
    }

    let timeout_secs = if form.timeout_secs.trim().is_empty() {
        None
    } else {
        match form.timeout_secs.trim().parse::<u64>() {
            Ok(value) => Some(value),
            Err(_) => {
                return error_page(
                    StatusCode::BAD_REQUEST,
                    "timeout_secs must be a positive integer",
                    &format!("/console/nodes/{}", escape_html(&form.node_id)),
                );
            }
        }
    };

    let request = CreateTaskRequest {
        node_id: form.node_id.clone(),
        command_name: form.command_name.clone(),
        args: parse_args_text(&form.args_text),
        timeout_secs,
    };
    let return_to = default_return_to(&form.return_to, "/console/tasks");

    match state.db.create_task(
        request,
        TaskAudit::new("console", Some(state.admin_actor())),
    ) {
        Err(error) => server_error_response(error),
        Ok(CreateTaskOutcome::NodeNotFound) => {
            error_page(StatusCode::NOT_FOUND, "node not found", "/console/nodes")
        }
        Ok(CreateTaskOutcome::UnsupportedCommand) => error_page(
            StatusCode::BAD_REQUEST,
            "command is not supported by the node",
            &format!("/console/nodes/{}", escape_html(&form.node_id)),
        ),
        Ok(CreateTaskOutcome::ExtraArgsNotAllowed) => error_page(
            StatusCode::BAD_REQUEST,
            "command does not allow extra args",
            &format!("/console/nodes/{}", escape_html(&form.node_id)),
        ),
        Ok(CreateTaskOutcome::Created(task)) => {
            state.notify_node(&task.node_id);
            Redirect::to(&return_to).into_response()
        }
    }
}

async fn cancel_task_submit(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(task_id): Path<u64>,
    Form(form): Form<CancelTaskForm>,
) -> Response {
    if let Some(response) = require_console_session(&state, &headers) {
        return response;
    }
    let return_to = default_return_to(&form.return_to, "/console/tasks");

    match state.db.cancel_task(
        task_id,
        Some("canceled from console".to_string()),
        TaskAudit::new("console", Some(state.admin_actor())),
    ) {
        Err(error) => server_error_response(error),
        Ok(CancelTaskOutcome::NotFound) => {
            error_page(StatusCode::NOT_FOUND, "task not found", "/console/tasks")
        }
        Ok(CancelTaskOutcome::NotCancelable) => error_page(
            StatusCode::CONFLICT,
            "task is already finished and cannot be canceled",
            &return_to,
        ),
        Ok(CancelTaskOutcome::Canceled(task)) => {
            state.notify_task_cancel(&task.node_id, task.task_id, task.cancel_reason.clone());
            Redirect::to(&return_to).into_response()
        }
    }
}

fn require_console_session(state: &AppState, headers: &HeaderMap) -> Option<Response> {
    if state.has_valid_admin_session(headers) {
        None
    } else {
        Some(Redirect::to("/console/login").into_response())
    }
}

fn login_response(message: Option<&str>, status: StatusCode) -> Response {
    let error_html = message
        .map(|text| format!("<p class=\"error\">{}</p>", escape_html(text)))
        .unwrap_or_default();

    (
        status,
        Html(console_html(
            "Login",
            format!(
                "<section class=\"login-shell\"><h1>Admin Login</h1><p>Sign in to view nodes and dispatch commands.</p>{error}<form method=\"post\" action=\"/console/login\"><label>Username<input name=\"username\" autocomplete=\"username\"></label><label>Password<input type=\"password\" name=\"password\" autocomplete=\"current-password\"></label><button type=\"submit\">Login</button></form></section>",
                error = error_html
            ),
            false,
            "",
        )),
    )
        .into_response()
}

fn error_page(status: StatusCode, message: &str, back_href: &str) -> Response {
    (
        status,
        Html(console_html(
            "Error",
            format!(
                "<section><h1>Request Failed</h1><p class=\"error\">{message}</p><p><a href=\"{back_href}\">Back</a></p></section>",
                message = escape_html(message),
                back_href = escape_html(back_href)
            ),
            true,
            "",
        )),
    )
        .into_response()
}

fn not_found_page(message: &str) -> Response {
    error_page(StatusCode::NOT_FOUND, message, "/console/nodes")
}

fn render_node_workspace(
    node: &NodeSnapshot,
    tasks: &[TaskSnapshot],
    session_events: &[SessionEvent],
) -> String {
    let task_log = if tasks.is_empty() {
        "<div class=\"empty-state\">No execution logs yet. Send a command from the dialogue panel.</div>"
            .to_string()
    } else {
        tasks
            .iter()
            .map(|task| render_task_timeline_card(task, &node.node_id))
            .collect::<Vec<_>>()
            .join("")
    };
    let dialog_feed = render_dialog_feed(tasks);
    let session_event_feed = render_session_events(session_events, false);

    let command_options = node
        .commands
        .iter()
        .map(render_command_option)
        .collect::<Vec<_>>()
        .join("");

    let first_command = node.commands.first();
    let command_help = first_command
        .map(render_command_meta_panel)
        .unwrap_or_else(|| {
            "<div class=\"dialog-meta-card\"><p>No commands reported by this node.</p></div>"
                .to_string()
        });

    let script = render_node_workspace_script(&node.node_id);
    let protocol_summary = render_protocol_summary(node);
    let runtime_summary = render_runtime_summary(node);
    let status_class = if node.online { "active" } else { "idle" };
    let status_label = if node.online { "online" } else { "offline" };

    format!(
        "<div class=\"workspace-bar\"><div class=\"workspace-bar-title\"><p class=\"eyebrow\">Node Workspace</p><h1 id=\"node-title\">{node_id}</h1><p id=\"node-subline\" class=\"workspace-subline\">{hostname} | {platform}</p><div id=\"node-runtime-summary\">{runtime_summary}</div><div id=\"node-protocol-summary\">{protocol_summary}</div></div><div class=\"workspace-bar-meta\"><a class=\"workspace-link\" href=\"/console/runtime\">Runtime</a><a class=\"workspace-link\" href=\"/console/nodes\">Nodes</a><a class=\"workspace-link\" href=\"/console/tasks\">Tasks</a><a class=\"workspace-link\" href=\"/console/session-events\">Session Events</a><a class=\"workspace-link\" href=\"/console/protocol\">Protocol</a><form method=\"post\" action=\"/console/logout\"><button type=\"submit\" class=\"workspace-logout\">Logout</button></form><span id=\"node-status-chip\" class=\"status-chip {status_class}\">{status_label}</span><span id=\"node-last-seen\" class=\"workspace-mini\">last seen {last_seen}</span></div></div><section class=\"workspace-shell\"><div class=\"workspace-log\"><div class=\"pane-head compact\"><div><p class=\"eyebrow\">Execution Log</p><h2>Task Timeline</h2></div><div class=\"pane-meta\">auto refresh 3s</div></div><div class=\"session-events-panel\"><div class=\"session-events-head\"><p class=\"eyebrow\">Session Events</p><p class=\"session-events-subline\">connect, auth, register, close</p></div><div id=\"session-events-list\" class=\"session-events-list\">{session_event_feed}</div></div><div id=\"task-log\" class=\"log-stream\">{task_log}</div></div><aside class=\"workspace-chat\"><div class=\"pane-head compact\"><div><p class=\"eyebrow\">Operator Dialogue</p><h2>Command Session</h2></div></div><div id=\"dialog-feed\" class=\"dialog-feed\">{dialog_feed}</div><form id=\"composer-form\" class=\"composer-card\" method=\"post\" action=\"/console/tasks\"><input type=\"hidden\" name=\"node_id\" value=\"{node_id_raw}\"><input type=\"hidden\" name=\"return_to\" value=\"/console/nodes/{node_id_raw}\"><label>Command<select id=\"command-select\" name=\"command_name\">{command_options}</select></label><div id=\"command-meta\">{command_help}</div><label>Extra args<textarea id=\"args-text\" name=\"args_text\" rows=\"2\" placeholder=\"hello&#10;world\"></textarea></label><label>Timeout seconds<input id=\"timeout-input\" type=\"number\" min=\"1\" name=\"timeout_secs\" placeholder=\"30\"></label><p id=\"dialog-status\" class=\"dialog-status\" hidden></p><button id=\"send-button\" type=\"submit\">Send Task</button></form></aside></section>{script}",
        node_id = escape_html(&node.node_id),
        node_id_raw = escape_html(&node.node_id),
        hostname = escape_html(&node.hostname),
        platform = escape_html(&node.platform),
        status_class = status_class,
        status_label = status_label,
        last_seen = node.last_seen_unix_secs,
        runtime_summary = runtime_summary,
        protocol_summary = protocol_summary,
        session_event_feed = session_event_feed,
        task_log = task_log,
        dialog_feed = dialog_feed,
        command_options = command_options,
        command_help = command_help,
        script = script
    )
}

fn render_session_events(events: &[SessionEvent], include_node_id: bool) -> String {
    if events.is_empty() {
        return "<div class=\"session-event empty\">No connection events recorded yet.</div>"
            .to_string();
    }

    events
        .iter()
        .map(|event| {
            let node_html = if include_node_id {
                format!(
                    "<p class=\"session-node\"><a href=\"/console/nodes/{node_id}\">{node_id}</a></p>",
                    node_id = escape_html(&event.node_id)
                )
            } else {
                String::new()
            };
            format!(
                "<article class=\"session-event kind-{kind}\"><div class=\"session-event-head\"><span class=\"session-kind\">{kind}</span><span class=\"session-time\">{time}</span></div>{node_html}<p class=\"session-message\">{message}</p></article>",
                kind = escape_html(session_event_kind_label(event.kind)),
                time = event.created_at_unix_secs,
                node_html = node_html,
                message = escape_html(&event.message)
            )
        })
        .collect::<Vec<_>>()
        .join("")
}

fn render_session_events_summary(
    node_id: Option<&str>,
    kind: Option<SessionEventKind>,
    count: usize,
) -> String {
    let mut filters = Vec::new();
    if let Some(node_id) = node_id {
        filters.push(format!("node={node_id}"));
    }
    if let Some(kind) = kind {
        filters.push(format!("kind={}", session_event_kind_label(kind)));
    }

    if filters.is_empty() {
        format!(
            "Recent connection lifecycle events across all nodes. {count} shown. Auto refresh 3s."
        )
    } else {
        format!(
            "Recent connection lifecycle events. {}. {count} shown. Auto refresh 3s.",
            filters.join(" | ")
        )
    }
}

fn render_session_event_kind_options(selected: &str) -> String {
    let selected = selected.trim();
    let options = [
        ("", "all"),
        ("connect_rejected", "connect_rejected"),
        ("auth_failed", "auth_failed"),
        ("session_opened", "session_opened"),
        ("node_registered", "node_registered"),
        ("session_closed", "session_closed"),
    ];

    options
        .iter()
        .map(|(value, label)| {
            let selected_attr = if *value == selected { " selected" } else { "" };
            format!(
                "<option value=\"{value}\"{selected}>{label}</option>",
                value = escape_html(value),
                selected = selected_attr,
                label = escape_html(label)
            )
        })
        .collect::<Vec<_>>()
        .join("")
}

fn render_session_events_page_script(initial_node_id: &str, initial_kind: &str) -> String {
    r#"<script>
const sessionEventsSummary = document.getElementById('session-events-summary');
const sessionEventsList = document.getElementById('session-events-list');
const sessionFilterNodeId = document.getElementById('session-filter-node-id');
const sessionFilterKind = document.getElementById('session-filter-kind');
const initialNodeId = __INITIAL_NODE_ID__;
const initialKind = __INITIAL_KIND__;

function escapeHtml(value) {
  return String(value ?? '')
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/\"/g, '&quot;')
    .replace(/'/g, '&#39;');
}

function renderSessionEvent(event) {
  const nodeId = escapeHtml(event.node_id || '-');
  const kind = escapeHtml(event.kind || 'unknown');
  const message = escapeHtml(event.message || '');
  const createdAt = escapeHtml(event.created_at_unix_secs ?? '-');
  return `<article class=\"session-event kind-${kind}\"><div class=\"session-event-head\"><span class=\"session-kind\">${kind}</span><span class=\"session-time\">${createdAt}</span></div><p class=\"session-node\"><a href=\"/console/nodes/${nodeId}\">${nodeId}</a></p><p class=\"session-message\">${message}</p></article>`;
}

function renderSummary(events, nodeId, kind) {
  const filters = [];
  if (nodeId) filters.push(`node=${nodeId}`);
  if (kind) filters.push(`kind=${kind}`);
  if (!filters.length) {
    return `Recent connection lifecycle events across all nodes. ${events.length} shown. Auto refresh 3s.`;
  }
  return `Recent connection lifecycle events. ${filters.join(' | ')}. ${events.length} shown. Auto refresh 3s.`;
}

function currentFilters() {
  return {
    nodeId: (sessionFilterNodeId?.value || '').trim(),
    kind: (sessionFilterKind?.value || '').trim(),
  };
}

async function refreshSessionEventsPage() {
  try {
    const { nodeId, kind } = currentFilters();
    const params = new URLSearchParams();
    params.set('limit', '40');
    if (nodeId) params.set('node_id', nodeId);
    if (kind) params.set('kind', kind);
    const response = await fetch(`/api/v1/session-events?${params.toString()}`, { credentials: 'same-origin' });
    if (!response.ok) return;
    const events = await response.json();
    if (sessionEventsSummary) sessionEventsSummary.textContent = renderSummary(events, nodeId, kind);
    if (sessionEventsList) {
      sessionEventsList.innerHTML = events.length
        ? events.map(renderSessionEvent).join('')
        : '<div class=\"session-event empty\">No connection events recorded yet.</div>';
    }
  } catch (_error) {}
}

if (sessionFilterNodeId) sessionFilterNodeId.value = initialNodeId;
if (sessionFilterKind) sessionFilterKind.value = initialKind;

refreshSessionEventsPage();
setInterval(refreshSessionEventsPage, 3000);
</script>"#
        .replace("__INITIAL_NODE_ID__", &format!("{initial_node_id:?}"))
        .replace("__INITIAL_KIND__", &format!("{initial_kind:?}"))
}

fn normalize_query_value(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn parse_session_event_kind_value(value: &str) -> Result<Option<SessionEventKind>, String> {
    match value.trim() {
        "" => Ok(None),
        "connect_rejected" => Ok(Some(SessionEventKind::ConnectRejected)),
        "auth_failed" => Ok(Some(SessionEventKind::AuthFailed)),
        "session_opened" => Ok(Some(SessionEventKind::SessionOpened)),
        "node_registered" => Ok(Some(SessionEventKind::NodeRegistered)),
        "session_closed" => Ok(Some(SessionEventKind::SessionClosed)),
        other => Err(format!("unknown session event kind: {other}")),
    }
}

fn session_event_kind_label(kind: SessionEventKind) -> &'static str {
    match kind {
        SessionEventKind::ConnectRejected => "connect_rejected",
        SessionEventKind::AuthFailed => "auth_failed",
        SessionEventKind::SessionOpened => "session_opened",
        SessionEventKind::NodeRegistered => "node_registered",
        SessionEventKind::SessionClosed => "session_closed",
    }
}

fn render_runtime_summary(node: &NodeSnapshot) -> String {
    let heartbeat = node
        .session_heartbeat_interval_secs
        .map(|value| format!("{value}s"))
        .unwrap_or_else(|| "n/a".to_string());
    let session_state = if node.online {
        "live session"
    } else {
        "awaiting reconnect"
    };

    format!(
        "<div class=\"runtime-strip\"><span class=\"runtime-chip\">state: {state}</span><span class=\"runtime-chip\">heartbeat: {heartbeat}</span><span class=\"runtime-chip\">commands: {commands}</span><span class=\"runtime-chip\">poll: {poll}s</span></div>",
        state = escape_html(session_state),
        heartbeat = escape_html(&heartbeat),
        commands = node.commands.len(),
        poll = node.poll_interval_secs
    )
}

fn render_protocol_summary(node: &NodeSnapshot) -> String {
    let capability_chips = if node.online {
        node.session_capabilities
            .iter()
            .map(|capability| {
                format!(
                    "<span class=\"protocol-chip\">{}</span>",
                    escape_html(capability)
                )
            })
            .collect::<Vec<_>>()
            .join("")
    } else {
        "<span class=\"protocol-chip\">session_offline</span>".to_string()
    };
    let version = node
        .session_protocol_version
        .as_deref()
        .unwrap_or(CONTROL_PROTOCOL_VERSION);
    let transport = node
        .session_transport_stack
        .as_deref()
        .unwrap_or(CONTROL_TRANSPORT_STACK);
    let heartbeat = node
        .session_heartbeat_interval_secs
        .map(|value| format!("<span class=\"protocol-chip\">heartbeat={}s</span>", value))
        .unwrap_or_else(|| "<span class=\"protocol-chip\">heartbeat=n/a</span>".to_string());

    format!(
        "<div class=\"protocol-strip\"><span class=\"protocol-badge\">{transport}</span><span class=\"protocol-badge\">{version}</span>{heartbeat}{capability_chips}</div>",
        transport = escape_html(transport),
        version = escape_html(version),
        heartbeat = heartbeat,
        capability_chips = capability_chips
    )
}

fn render_task_row(task: &TaskSnapshot, include_node: bool) -> String {
    let result_html = match &task.result {
        Some(result) => format!(
            "<details><summary>success={success} exit={exit_code:?} duration={duration_ms}ms</summary>{audit}{notes}<pre>stdout:\n{stdout}\n\nstderr:\n{stderr}\n\nerror:\n{error}</pre></details>",
            success = result.success,
            exit_code = result.exit_code,
            duration_ms = result.duration_ms,
            audit = render_task_audit_block(task),
            notes = render_result_truncation_notice(result),
            stdout = escape_html(&result.stdout),
            stderr = escape_html(&result.stderr),
            error = escape_html(result.error.as_deref().unwrap_or(""))
        ),
        None => format!(
            "{reason}{audit}",
            reason = escape_html(task.cancel_reason.as_deref().unwrap_or("-")),
            audit = render_task_audit_block(task),
        ),
    };

    let cancel_cell = if matches!(
        task.status,
        TaskStatus::Queued | TaskStatus::Dispatched | TaskStatus::Running
    ) {
        format!(
            "<form method=\"post\" action=\"/console/tasks/{task_id}/cancel\"><input type=\"hidden\" name=\"return_to\" value=\"/console/tasks\"><button type=\"submit\">Cancel</button></form>",
            task_id = task.task_id,
        )
    } else {
        String::new()
    };

    if include_node {
        format!(
            "<tr><td>{task_id}{cancel}</td><td><a href=\"/console/nodes/{node_id}\">{node_id}</a></td><td>{command}</td><td>{status}</td><td>{args}</td><td>{retry}</td><td>{created}</td><td>{result}</td></tr>",
            task_id = task.task_id,
            cancel = cancel_cell,
            node_id = escape_html(&task.node_id),
            command = escape_html(&task.command_name),
            status = task_status_label(task.status),
            args = escape_html(&display_args(&task.args)),
            retry = task.retry_count,
            created = task.created_at_unix_secs,
            result = result_html
        )
    } else {
        format!(
            "<tr><td>{task_id}{cancel}</td><td>{command}</td><td>{status}</td><td>{args}</td><td>{retry}</td><td>{created}</td><td>{result}</td></tr>",
            task_id = task.task_id,
            cancel = cancel_cell,
            command = escape_html(&task.command_name),
            status = task_status_label(task.status),
            args = escape_html(&display_args(&task.args)),
            retry = task.retry_count,
            created = task.created_at_unix_secs,
            result = result_html
        )
    }
}

fn render_task_timeline_card(task: &TaskSnapshot, node_id: &str) -> String {
    let output = match &task.result {
        Some(result) => format!(
            "<details class=\"log-output\"><summary>stdout / stderr</summary>{notes}<pre>stdout:\n{stdout}\n\nstderr:\n{stderr}\n\nerror:\n{error}</pre></details>",
            notes = render_result_truncation_notice(result),
            stdout = escape_html(&result.stdout),
            stderr = escape_html(&result.stderr),
            error = escape_html(result.error.as_deref().unwrap_or("-"))
        ),
        None => String::new(),
    };
    let cancel = if matches!(
        task.status,
        TaskStatus::Queued | TaskStatus::Dispatched | TaskStatus::Running
    ) {
        format!(
            "<form method=\"post\" action=\"/console/tasks/{task_id}/cancel\"><input type=\"hidden\" name=\"return_to\" value=\"/console/nodes/{node_id}\"><button type=\"submit\" class=\"secondary\">Cancel</button></form>",
            task_id = task.task_id,
            node_id = escape_html(node_id),
        )
    } else {
        String::new()
    };
    let result_summary = match &task.result {
        Some(result) => format!(
            "exit={:?} duration={}ms",
            result.exit_code, result.duration_ms
        ),
        None => task
            .cancel_reason
            .clone()
            .or_else(|| task.retry_reason.clone())
            .unwrap_or_else(|| "waiting for execution".to_string()),
    };

    format!(
        "<article class=\"log-card status-{status}\"><div class=\"log-card-head\"><div><span class=\"status-badge badge-{status}\">{status}</span><h3>{command}</h3></div><div class=\"log-card-actions\">{cancel}</div></div><p class=\"log-meta-line\">task #{task_id} | {created} | args: {args}</p><p class=\"log-summary\">{summary}</p>{output}</article>",
        status = task_status_label(task.status),
        command = escape_html(&task.command_name),
        task_id = task.task_id,
        args = escape_html(&display_args(&task.args)),
        cancel = cancel,
        created = escape_html(&task_card_meta(task)),
        summary = escape_html(&result_summary),
        output = output
    )
}

fn render_dialog_feed(tasks: &[TaskSnapshot]) -> String {
    if tasks.is_empty() {
        return "<div class=\"dialog-bubble system\">This workspace only runs commands that the client explicitly declared.</div><div class=\"dialog-bubble operator\">Select a command, adjust args, and send it as a tracked task.</div>".to_string();
    }

    tasks
        .iter()
        .rev()
        .take(8)
        .map(render_dialog_pair)
        .collect::<Vec<_>>()
        .join("")
}

fn render_dialog_pair(task: &TaskSnapshot) -> String {
    let origin = task_origin_summary(task);
    let operator = format!(
        "<article class=\"dialog-bubble operator\"><p class=\"dialog-role\">operator</p><p>run <strong>{command}</strong></p><p class=\"dialog-detail\">args: {args}</p><p class=\"dialog-detail\">task #{task_id} at {created}</p>{origin}</article>",
        command = escape_html(&task.command_name),
        args = escape_html(&display_args(&task.args)),
        task_id = task.task_id,
        created = task.created_at_unix_secs,
        origin = render_optional_dialog_detail(&origin),
    );
    let summary = match &task.result {
        Some(result) => format!(
            "status={} exit={:?} duration={}ms",
            task_status_label(task.status),
            result.exit_code,
            result.duration_ms
        ),
        None => format!("status={}", task_status_label(task.status)),
    };
    let extra = match &task.result {
        Some(result) => {
            let stdout = result.stdout.trim();
            let truncation = result_truncation_summary(result);
            if stdout.is_empty() {
                if truncation.is_empty() {
                    "no stdout".to_string()
                } else {
                    truncation
                }
            } else if truncation.is_empty() {
                format!("stdout: {}", truncate_text(stdout, 120))
            } else {
                format!("{} | stdout: {}", truncation, truncate_text(stdout, 120))
            }
        }
        None => task
            .cancel_reason
            .clone()
            .or_else(|| task.retry_reason.clone())
            .unwrap_or_else(|| "waiting for execution".to_string()),
    };
    let system = format!(
        "<article class=\"dialog-bubble system\"><p class=\"dialog-role\">system</p><p>{summary}</p><p class=\"dialog-detail\">{extra}</p></article>",
        summary = escape_html(&summary),
        extra = escape_html(&extra),
    );

    format!("{operator}{system}")
}

fn task_origin_summary(task: &TaskSnapshot) -> String {
    match (task.created_via.as_deref(), task.created_by.as_deref()) {
        (Some(via), Some(actor)) => format!("created via {via} by {actor}"),
        (Some(via), None) => format!("created via {via}"),
        (None, Some(actor)) => format!("created by {actor}"),
        (None, None) => String::new(),
    }
}

fn task_cancel_summary(task: &TaskSnapshot) -> String {
    match (task.canceled_via.as_deref(), task.canceled_by.as_deref()) {
        (Some(via), Some(actor)) => format!("canceled via {via} by {actor}"),
        (Some(via), None) => format!("canceled via {via}"),
        (None, Some(actor)) => format!("canceled by {actor}"),
        (None, None) => String::new(),
    }
}

fn render_task_audit_block(task: &TaskSnapshot) -> String {
    let origin = task_origin_summary(task);
    let canceled = task_cancel_summary(task);
    let mut html = String::new();

    if !origin.is_empty() {
        html.push_str(&format!(
            "<p class=\"muted task-audit\">{}</p>",
            escape_html(&origin)
        ));
    }
    if !canceled.is_empty() {
        html.push_str(&format!(
            "<p class=\"muted task-audit\">{}</p>",
            escape_html(&canceled)
        ));
    }

    html
}

fn task_card_meta(task: &TaskSnapshot) -> String {
    let mut parts = vec![task.created_at_unix_secs.to_string()];
    let origin = task_origin_summary(task);
    if !origin.is_empty() {
        parts.push(origin);
    }
    parts.join(" | ")
}

fn render_optional_dialog_detail(value: &str) -> String {
    if value.is_empty() {
        String::new()
    } else {
        format!("<p class=\"dialog-detail\">{}</p>", escape_html(value))
    }
}

fn render_result_truncation_notice(result: &ExecutionResult) -> String {
    let summary = result_truncation_summary(result);
    if summary.is_empty() {
        String::new()
    } else {
        format!(
            "<p class=\"muted truncation-note\">{}</p>",
            escape_html(&summary)
        )
    }
}

fn result_truncation_summary(result: &ExecutionResult) -> String {
    let mut notes = Vec::new();
    if result.stdout_truncated {
        notes.push(format!(
            "stdout truncated after {} bytes",
            MAX_RESULT_OUTPUT_BYTES
        ));
    }
    if result.stderr_truncated {
        notes.push(format!(
            "stderr truncated after {} bytes",
            MAX_RESULT_OUTPUT_BYTES
        ));
    }
    notes.join(" | ")
}

fn render_command_option(command: &CommandDescriptor) -> String {
    format!(
        "<option value=\"{name}\" data-description=\"{description}\" data-default-args=\"{default_args}\" data-allow-extra=\"{allow_extra}\">{name}</option>",
        name = escape_html(&command.name),
        description = escape_html(&command.description),
        default_args = escape_html(&display_args(&command.default_args)),
        allow_extra = if command.allow_extra_args { "true" } else { "false" },
    )
}

fn render_command_meta_panel(command: &CommandDescriptor) -> String {
    format!(
        "<details class=\"dialog-meta-card\"><summary class=\"dialog-meta-summary\">Command details</summary><p id=\"command-description\">{description}</p><p><strong>Default args:</strong> <span id=\"command-default-args\">{default_args}</span></p><p id=\"command-extra-help\" class=\"muted\">{extra_help}</p></details>",
        description = escape_html(&command.description),
        default_args = escape_html(&display_args(&command.default_args)),
        extra_help = if command.allow_extra_args {
            "Extra args are enabled for this command."
        } else {
            "Extra args are disabled for this command."
        }
    )
}

fn render_node_workspace_script(node_id: &str) -> String {
    format!(
        "<script>
const nodeId = {node_id:?};
const maxResultOutputBytes = {max_result_output_bytes};
const taskLog = document.getElementById('task-log');
const dialogFeed = document.getElementById('dialog-feed');
const sessionEventsList = document.getElementById('session-events-list');
const nodeSubline = document.getElementById('node-subline');
const nodeStatusChip = document.getElementById('node-status-chip');
const nodeLastSeen = document.getElementById('node-last-seen');
const nodeProtocolSummary = document.getElementById('node-protocol-summary');
const nodeRuntimeSummary = document.getElementById('node-runtime-summary');
const composerForm = document.getElementById('composer-form');
const commandSelect = document.getElementById('command-select');
const argsField = document.getElementById('args-text');
const timeoutInput = document.getElementById('timeout-input');
const dialogStatus = document.getElementById('dialog-status');
const sendButton = document.getElementById('send-button');
const commandDescription = document.getElementById('command-description');
const commandDefaultArgs = document.getElementById('command-default-args');
const commandExtraHelp = document.getElementById('command-extra-help');
let latestTasks = [];
let pendingMessages = [];

function setDialogStatus(message, tone) {{
  if (!message) {{
    dialogStatus.hidden = true;
    dialogStatus.className = 'dialog-status';
    dialogStatus.textContent = '';
    return;
  }}
  dialogStatus.hidden = false;
  dialogStatus.className = `dialog-status ${{tone || 'muted'}}`;
  dialogStatus.textContent = message;
}}

function escapeHtml(value) {{
  return String(value ?? '')
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/\"/g, '&quot;')
    .replace(/'/g, '&#39;');
}}

function renderTaskCard(task) {{
  const canCancel = ['queued', 'dispatched', 'running'].includes(task.status);
  const outputNotes = task.result ? renderResultNotes(task.result) : '';
  const output = task.result
    ? `<details class=\"log-output\"><summary>stdout / stderr</summary>${{outputNotes}}<pre>stdout:\\n${{escapeHtml(task.result.stdout)}}\\n\\nstderr:\\n${{escapeHtml(task.result.stderr)}}\\n\\nerror:\\n${{escapeHtml(task.result.error ?? '-')}}</pre></details>`
    : '';
  const summary = task.result
    ? `exit=${{task.result.exit_code}} duration=${{task.result.duration_ms}}ms`
    : (task.cancel_reason || task.retry_reason || 'waiting for execution');
  const cancel = canCancel
    ? `<form method=\"post\" action=\"/console/tasks/${{task.task_id}}/cancel\"><input type=\"hidden\" name=\"return_to\" value=\"/console/nodes/${{escapeHtml(nodeId)}}\"><button type=\"submit\" class=\"secondary\">Cancel</button></form>`
    : '';
  const meta = taskCardMeta(task);
  return `<article class=\"log-card status-${{escapeHtml(task.status)}}\"><div class=\"log-card-head\"><div><span class=\"status-badge badge-${{escapeHtml(task.status)}}\">${{escapeHtml(task.status)}}</span><h3>${{escapeHtml(task.command_name)}}</h3><p class=\"muted\">task #${{task.task_id}} | ${{escapeHtml(meta)}} | args: ${{escapeHtml((task.args || []).join(' ') || '[]')}}</p></div><div class=\"log-card-actions\">${{cancel}}</div></div><dl class=\"log-metrics\"><div><dt>created</dt><dd>${{escapeHtml(task.created_at_unix_secs)}}</dd></div><div><dt>started</dt><dd>${{escapeHtml(task.started_at_unix_secs ?? '-')}}</dd></div><div><dt>finished</dt><dd>${{escapeHtml(task.finished_at_unix_secs ?? '-')}}</dd></div><div><dt>retry</dt><dd>${{escapeHtml(task.retry_count)}}</dd></div></dl><p class=\"log-summary\">${{escapeHtml(summary)}}</p>${{renderTaskAudit(task)}}${{output}}</article>`;
}}

function truncateText(value, limit) {{
  if (value.length <= limit) return value;
  return value.slice(0, limit - 3) + '...';
}}

function resultTruncationSummary(result) {{
  const notes = [];
  if (result.stdout_truncated) notes.push(`stdout truncated after ${{maxResultOutputBytes}} bytes`);
  if (result.stderr_truncated) notes.push(`stderr truncated after ${{maxResultOutputBytes}} bytes`);
  return notes.join(' | ');
}}

function renderResultNotes(result) {{
  const summary = resultTruncationSummary(result);
  return summary ? `<p class=\"muted truncation-note\">${{escapeHtml(summary)}}</p>` : '';
}}

function taskOriginSummary(task) {{
  if (task.created_via && task.created_by) return `created via ${{task.created_via}} by ${{task.created_by}}`;
  if (task.created_via) return `created via ${{task.created_via}}`;
  if (task.created_by) return `created by ${{task.created_by}}`;
  return '';
}}

function taskCanceledSummary(task) {{
  if (task.canceled_via && task.canceled_by) return `canceled via ${{task.canceled_via}} by ${{task.canceled_by}}`;
  if (task.canceled_via) return `canceled via ${{task.canceled_via}}`;
  if (task.canceled_by) return `canceled by ${{task.canceled_by}}`;
  return '';
}}

function renderTaskAudit(task) {{
  const parts = [taskOriginSummary(task), taskCanceledSummary(task)].filter(Boolean);
  return parts.map((item) => `<p class=\"muted task-audit\">${{escapeHtml(item)}}</p>`).join('');
}}

function taskCardMeta(task) {{
  const parts = [String(task.created_at_unix_secs)];
  const origin = taskOriginSummary(task);
  if (origin) parts.push(origin);
  return parts.join(' | ');
}}

function renderDialogPair(task) {{
  const stdout = task.result && task.result.stdout ? task.result.stdout.trim() : '';
  const truncation = task.result ? resultTruncationSummary(task.result) : '';
  const origin = taskOriginSummary(task);
  const summary = task.result
    ? `status=${{task.status}} exit=${{task.result.exit_code}} duration=${{task.result.duration_ms}}ms`
    : `status=${{task.status}}`;
  const extra = task.result
    ? (stdout
      ? (truncation ? `${{truncation}} | stdout: ${{truncateText(stdout, 120)}}` : `stdout: ${{truncateText(stdout, 120)}}`)
      : (truncation || 'no stdout'))
    : (task.cancel_reason || task.retry_reason || 'waiting for execution');
  const originLine = origin ? `<p class=\"dialog-detail\">${{escapeHtml(origin)}}</p>` : '';
  return `<article class=\"dialog-bubble operator\"><p class=\"dialog-role\">operator</p><p>run <strong>${{escapeHtml(task.command_name)}}</strong></p><p class=\"dialog-detail\">args: ${{escapeHtml((task.args || []).join(' ') || '[]')}}</p><p class=\"dialog-detail\">task #${{task.task_id}} at ${{escapeHtml(task.created_at_unix_secs)}}</p>${{originLine}}</article><article class=\"dialog-bubble system\"><p class=\"dialog-role\">system</p><p>${{escapeHtml(summary)}}</p><p class=\"dialog-detail\">${{escapeHtml(extra)}}</p></article>`;
}}

function renderDialogFeed(tasks) {{
  const confirmed = tasks.slice().reverse().slice(-8).map(renderDialogPair).join('');
  const pending = pendingMessages.map(renderPendingPair).join('');
  if (!tasks.length) {{
    const intro = '<div class=\"dialog-bubble system\">This workspace only runs commands that the client explicitly declared.</div><div class=\"dialog-bubble operator\">Select a command, adjust args, and send it as a tracked task.</div>';
    return intro + pending;
  }}
  return confirmed + pending;
}}

function renderPendingPair(item) {{
  const operator = `<article class=\"dialog-bubble operator pending\"><p class=\"dialog-role\">operator</p><p>run <strong>${{escapeHtml(item.command_name)}}</strong></p><p class=\"dialog-detail\">args: ${{escapeHtml((item.args || []).join(' ') || '[]')}}</p><p class=\"dialog-detail\">local request at ${{escapeHtml(item.created_at_unix_secs)}}</p></article>`;
  const system = item.kind === 'error'
    ? `<article class=\"dialog-bubble system pending error-bubble\"><p class=\"dialog-role\">system</p><p>${{escapeHtml(item.note)}}</p><p class=\"dialog-detail\">request was not accepted by the server</p></article>`
    : `<article class=\"dialog-bubble system pending\"><p class=\"dialog-role\">system</p><p>submitting task...</p><p class=\"dialog-detail\">waiting for server task id</p></article>`;
  return operator + system;
}}

function renderSessionEvent(event) {{
  const kind = escapeHtml(event.kind || 'unknown');
  const message = escapeHtml(event.message || '');
  const createdAt = escapeHtml(event.created_at_unix_secs ?? '-');
  return `<article class=\"session-event kind-${{kind}}\"><div class=\"session-event-head\"><span class=\"session-kind\">${{kind}}</span><span class=\"session-time\">${{createdAt}}</span></div><p class=\"session-message\">${{message}}</p></article>`;
}}

function updateSessionEvents(events) {{
  if (!sessionEventsList) return;
  sessionEventsList.innerHTML = events.length
    ? events.map(renderSessionEvent).join('')
    : '<div class=\"session-event empty\">No connection events recorded yet.</div>';
}}

function renderProtocolSummary(node) {{
  const capabilities = Array.isArray(node.session_capabilities) && node.session_capabilities.length
    ? node.session_capabilities.map((item) => `<span class=\"protocol-chip\">${{escapeHtml(item)}}</span>`).join('')
    : '<span class=\"protocol-chip\">session_offline</span>';
  const version = node.session_protocol_version || '{protocol_version}';
  const transport = node.session_transport_stack || '{transport_stack}';
  const heartbeat = Number.isFinite(node.session_heartbeat_interval_secs)
    ? `<span class=\"protocol-chip\">heartbeat=${{node.session_heartbeat_interval_secs}}s</span>`
    : '<span class=\"protocol-chip\">heartbeat=n/a</span>';
  return `<div class=\"protocol-strip\"><span class=\"protocol-badge\">${{escapeHtml(transport)}}</span><span class=\"protocol-badge\">${{escapeHtml(version)}}</span>${{heartbeat}}${{capabilities}}</div>`;
}}

function renderRuntimeSummary(node) {{
  const heartbeat = Number.isFinite(node.session_heartbeat_interval_secs) ? `${{node.session_heartbeat_interval_secs}}s` : 'n/a';
  const state = node.online ? 'live session' : 'awaiting reconnect';
  const commands = Array.isArray(node.commands) ? node.commands.length : 0;
  return `<div class=\"runtime-strip\"><span class=\"runtime-chip\">state: ${{escapeHtml(state)}}</span><span class=\"runtime-chip\">heartbeat: ${{escapeHtml(heartbeat)}}</span><span class=\"runtime-chip\">commands: ${{escapeHtml(commands)}}</span><span class=\"runtime-chip\">poll: ${{escapeHtml(node.poll_interval_secs ?? 'n/a')}}s</span></div>`;
}}

function updateNodeSummary(node) {{
  if (nodeSubline) nodeSubline.textContent = `${{node.hostname || '-'}} | ${{node.platform || '-'}}`;
  if (nodeStatusChip) {{
    nodeStatusChip.textContent = node.online ? 'online' : 'offline';
    nodeStatusChip.className = `status-chip ${{node.online ? 'active' : 'idle'}}`;
  }}
  if (nodeLastSeen) nodeLastSeen.textContent = `last seen ${{node.last_seen_unix_secs ?? '-'}}`;
  if (nodeProtocolSummary) nodeProtocolSummary.innerHTML = renderProtocolSummary(node);
  if (nodeRuntimeSummary) nodeRuntimeSummary.innerHTML = renderRuntimeSummary(node);
}}

async function refreshWorkspace() {{
  try {{
    const [nodeResponse, tasksResponse, eventsResponse] = await Promise.all([
      fetch(`/api/v1/nodes/${{encodeURIComponent(nodeId)}}`, {{ credentials: 'same-origin' }}),
      fetch(`/api/v1/nodes/${{encodeURIComponent(nodeId)}}/tasks?limit=20`, {{ credentials: 'same-origin' }}),
      fetch(`/api/v1/nodes/${{encodeURIComponent(nodeId)}}/session-events?limit=8`, {{ credentials: 'same-origin' }}),
    ]);
    if (nodeResponse.ok) {{
      const node = await nodeResponse.json();
      updateNodeSummary(node);
    }}
    if (eventsResponse.ok) {{
      const events = await eventsResponse.json();
      updateSessionEvents(events);
    }}
    if (!tasksResponse.ok) return;
    const tasks = await tasksResponse.json();
    latestTasks = tasks;
    taskLog.innerHTML = tasks.length
      ? tasks.map(renderTaskCard).join('')
      : '<div class=\"empty-state\">No execution logs yet. Send a command from the dialogue panel.</div>';
    updateDialogFeed(tasks);
  }} catch (_error) {{}}
}}

function parseArgsText(value) {{
  return value
    .split('\\n')
    .map((line) => line.trim())
    .filter((line) => line.length > 0);
}}

function updateDialogFeed(tasks) {{
  dialogFeed.innerHTML = renderDialogFeed(tasks);
  dialogFeed.scrollTop = dialogFeed.scrollHeight;
}}

async function submitComposer(event) {{
  event.preventDefault();
  const commandName = commandSelect.value;
  const args = parseArgsText(argsField.value);
  const timeoutText = timeoutInput.value.trim();
  const timeoutSecs = timeoutText ? Number(timeoutText) : null;
  const pendingId = `pending-${{Date.now()}}`;

  if (!commandName) {{
    dialogStatus.textContent = 'Select a command first.';
    dialogStatus.className = 'dialog-status error';
    return;
  }}

  pendingMessages.push({{
    id: pendingId,
    kind: 'pending',
    command_name: commandName,
    args,
    created_at_unix_secs: Math.floor(Date.now() / 1000),
  }});
  updateDialogFeed(latestTasks);
  sendButton.disabled = true;
  sendButton.textContent = 'Sending...';
  setDialogStatus('Submitting command through the task API...', 'muted');

  try {{
    const response = await fetch('/api/v1/tasks', {{
      method: 'POST',
      credentials: 'same-origin',
      headers: {{ 'content-type': 'application/json' }},
      body: JSON.stringify({{
        node_id: nodeId,
        command_name: commandName,
        args,
        timeout_secs: Number.isFinite(timeoutSecs) ? timeoutSecs : null,
      }}),
    }});

    if (!response.ok) {{
      let message = 'task submission failed';
      try {{
        message = await response.text() || message;
      }} catch (_error) {{}}
      pendingMessages = pendingMessages.map((item) =>
        item.id === pendingId ? {{ ...item, kind: 'error', note: message }} : item
      );
      updateDialogFeed(latestTasks);
      setDialogStatus('Server rejected the command.', 'error');
      return;
    }}

    pendingMessages = pendingMessages.filter((item) => item.id !== pendingId);
    argsField.value = '';
    timeoutInput.value = '';
    setDialogStatus('Task accepted. Waiting for execution updates...', 'success');
    await refreshWorkspace();
  }} catch (_error) {{
    pendingMessages = pendingMessages.map((item) =>
      item.id === pendingId ? {{ ...item, kind: 'error', note: 'network error while submitting task' }} : item
    );
    updateDialogFeed(latestTasks);
    setDialogStatus('Network error while submitting the task.', 'error');
  }} finally {{
    sendButton.disabled = false;
    sendButton.textContent = 'Send Task';
  }}
}}

function syncCommandMeta() {{
  const selected = commandSelect.selectedOptions[0];
  if (!selected) return;
  const allowExtra = selected.dataset.allowExtra === 'true';
  commandDescription.textContent = selected.dataset.description || '';
  commandDefaultArgs.textContent = selected.dataset.defaultArgs || '[]';
  commandExtraHelp.textContent = allowExtra
    ? 'Extra args are enabled for this command.'
    : 'Extra args are disabled for this command.';
  argsField.disabled = !allowExtra;
  if (!allowExtra) {{
    argsField.value = '';
    argsField.placeholder = 'extra args disabled';
  }} else {{
    argsField.placeholder = 'hello\\nworld';
  }}
}}

if (commandSelect) {{
  syncCommandMeta();
  commandSelect.addEventListener('change', syncCommandMeta);
  composerForm.addEventListener('submit', submitComposer);
  refreshWorkspace();
  setInterval(refreshWorkspace, 3000);
}}
</script>",
        node_id = node_id,
        max_result_output_bytes = MAX_RESULT_OUTPUT_BYTES,
        protocol_version = CONTROL_PROTOCOL_VERSION,
        transport_stack = CONTROL_TRANSPORT_STACK
    )
}

fn task_status_label(status: TaskStatus) -> &'static str {
    match status {
        TaskStatus::Queued => "queued",
        TaskStatus::Dispatched => "dispatched",
        TaskStatus::Running => "running",
        TaskStatus::Succeeded => "succeeded",
        TaskStatus::Failed => "failed",
        TaskStatus::Canceled => "canceled",
    }
}

fn parse_args_text(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn default_return_to(value: &str, fallback: &str) -> String {
    let trimmed = value.trim();
    if trimmed.starts_with("/console/") {
        trimmed.to_string()
    } else {
        fallback.to_string()
    }
}

fn truncate_text(input: &str, limit: usize) -> String {
    if input.chars().count() <= limit {
        input.to_string()
    } else {
        input
            .chars()
            .take(limit.saturating_sub(3))
            .collect::<String>()
            + "..."
    }
}

fn display_args(args: &[String]) -> String {
    if args.is_empty() {
        "[]".to_string()
    } else {
        args.join(" ")
    }
}

fn build_session_cookie(token: &str, ttl_secs: u64) -> String {
    format!(
        "ru_admin_session={token}; HttpOnly; Path=/; SameSite=Lax; Max-Age={ttl_secs}",
        token = token,
        ttl_secs = ttl_secs
    )
}

fn server_error_response(error: crate::db::DbError) -> Response {
    let (status, message) = internal_error(error);
    error_page(status, &message, "/console")
}

fn console_html(title: &str, body: String, show_nav: bool, page_class: &str) -> String {
    let header = if show_nav {
        "<header><nav><a href=\"/console/runtime\">Runtime</a><a href=\"/console/nodes\">Nodes</a><a href=\"/console/tasks\">Tasks</a><a href=\"/console/session-events\">Session Events</a><a href=\"/console/protocol\">Protocol</a><form method=\"post\" action=\"/console/logout\"><button type=\"submit\">Logout</button></form></nav></header>"
    } else {
        ""
    };

    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><title>{title}</title><style>{style}</style></head><body class=\"{page_class}\">{header}<main class=\"{page_class}\">{body}</main></body></html>",
        title = escape_html(title),
        style = STYLE,
        page_class = escape_html(page_class),
        header = header,
        body = body
    )
}

fn escape_html(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

const CLEAR_SESSION_COOKIE: &str =
    "ru_admin_session=deleted; HttpOnly; Path=/; SameSite=Lax; Max-Age=0";

const STYLE: &str = "
html {
    height: 100%;
}
body {
    margin: 0;
    font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif;
    background: linear-gradient(180deg, #e7eef5 0%, #f7f9fb 100%);
    color: #1f2937;
}
header {
    background: #111827;
    color: #f9fafb;
    padding: 12px 18px;
}
nav {
    display: flex;
    align-items: center;
    gap: 12px;
}
nav a {
    color: #f9fafb;
    text-decoration: none;
    font-size: 14px;
}
nav form {
    margin-left: auto;
}
nav form button {
    margin-top: 0;
    padding: 6px 10px;
    font-size: 13px;
}
main {
    max-width: 1320px;
    margin: 0 auto;
    padding: 24px;
}
body.workspace-page {
    height: 100vh;
    overflow: hidden;
}
body.workspace-page header {
    padding: 6px 10px;
}
body.workspace-page nav {
    gap: 10px;
    min-height: 30px;
}
main.workspace-page {
    height: calc(100vh - 42px);
    padding: 10px 12px;
    overflow: hidden;
    display: grid;
    grid-template-rows: auto minmax(0, 1fr);
}
section {
    background: #ffffff;
    border: 1px solid #e5e7eb;
    border-radius: 12px;
    padding: 20px;
    margin-bottom: 20px;
}
main.workspace-page section {
    margin-bottom: 12px;
}
table {
    width: 100%;
    border-collapse: collapse;
}
th, td {
    text-align: left;
    padding: 12px 10px;
    border-bottom: 1px solid #e5e7eb;
    vertical-align: top;
}
label {
    display: block;
    margin-bottom: 12px;
    font-weight: 600;
}
input, textarea, button {
    width: 100%;
    box-sizing: border-box;
    margin-top: 6px;
    padding: 10px 12px;
    border-radius: 8px;
    border: 1px solid #cbd5e1;
    font: inherit;
}
button {
    width: auto;
    background: #111827;
    color: #ffffff;
    cursor: pointer;
}
.session-filter-bar {
    display: grid;
    grid-template-columns: minmax(180px, 1fr) minmax(180px, 220px) auto;
    gap: 12px;
    align-items: end;
    margin: 14px 0 18px;
}
.session-filter-bar label {
    margin-bottom: 0;
}
.session-filter-bar button {
    margin-top: 0;
}
.secondary {
    background: #fff7ed;
    color: #9a3412;
    border-color: #fdba74;
}
.command-grid {
    display: grid;
    gap: 16px;
}
.command-card {
    border: 1px solid #e5e7eb;
    border-radius: 10px;
    padding: 16px;
    background: #f9fafb;
}
.muted {
    color: #6b7280;
}
.truncation-note {
    margin: 0 0 8px;
    font-size: 12px;
}
.task-audit {
    margin: 0 0 8px;
    font-size: 12px;
}
.error {
    color: #b91c1c;
}
.login-shell {
    max-width: 420px;
    margin: 80px auto;
}
pre {
    white-space: pre-wrap;
    word-break: break-word;
}
.eyebrow {
    margin: 0 0 4px;
    font-size: 11px;
    font-weight: 700;
    letter-spacing: 0.12em;
    text-transform: uppercase;
    color: #64748b;
}
.workspace-bar {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 12px;
    background: linear-gradient(135deg, #0f172a, #1e293b);
    color: #f8fafc;
    padding: 10px 14px;
    border-radius: 16px;
}
.workspace-bar-title {
    min-width: 0;
}
.workspace-bar h1 {
    margin: 0;
    font-size: 20px;
    line-height: 1.1;
}
.workspace-subline {
    margin: 2px 0 0;
    color: #cbd5e1;
    font-size: 13px;
}
.runtime-strip {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin-top: 8px;
}
.runtime-chip {
    display: inline-flex;
    align-items: center;
    border-radius: 999px;
    padding: 4px 8px;
    font-size: 11px;
    line-height: 1;
    white-space: nowrap;
    background: rgba(15, 23, 42, 0.32);
    color: #e2e8f0;
}
.protocol-strip {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin-top: 8px;
}
.protocol-badge,
.protocol-chip {
    display: inline-flex;
    align-items: center;
    border-radius: 999px;
    padding: 4px 8px;
    font-size: 11px;
    line-height: 1;
    white-space: nowrap;
}
.protocol-badge {
    background: rgba(191, 219, 254, 0.2);
    color: #dbeafe;
    font-weight: 700;
}
.protocol-chip {
    background: rgba(255, 255, 255, 0.08);
    color: #cbd5e1;
}
.protocol-page-card {
    display: grid;
    gap: 16px;
    padding: 18px;
    border-radius: 16px;
    background: #ffffff;
    border: 1px solid #d7e1ea;
    box-shadow: 0 8px 24px rgba(15, 23, 42, 0.06);
}
.protocol-facts {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(180px, 1fr));
    gap: 12px;
    margin: 0;
}
.protocol-facts div {
    padding: 12px;
    border-radius: 12px;
    background: #f8fbfd;
    border: 1px solid #e2e8f0;
}
.protocol-facts dt {
    margin: 0 0 4px;
    font-size: 12px;
    font-weight: 700;
    color: #64748b;
}
.protocol-facts dd {
    margin: 0;
    font-size: 14px;
    color: #0f172a;
}
.protocol-label {
    margin: 0 0 6px;
    font-size: 12px;
    font-weight: 700;
    color: #475569;
}
.protocol-grid {
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: 16px;
    margin-top: 16px;
}
.protocol-section-card {
    padding: 18px;
    border-radius: 16px;
    background: #ffffff;
    border: 1px solid #d7e1ea;
    box-shadow: 0 8px 24px rgba(15, 23, 42, 0.06);
}
.protocol-span-2 {
    grid-column: span 2;
}
.protocol-section-head {
    display: flex;
    justify-content: space-between;
    align-items: flex-start;
    gap: 12px;
    margin-bottom: 12px;
}
.protocol-section-head h2 {
    margin: 0;
    font-size: 18px;
    color: #0f172a;
}
.protocol-layer-row,
.protocol-frame-row,
.protocol-sequence-row,
.protocol-note-row {
    display: flex;
    gap: 12px;
    padding: 12px 0;
    border-bottom: 1px solid #e2e8f0;
}
.protocol-layer-row,
.protocol-frame-row,
.protocol-sequence-row {
    align-items: flex-start;
    justify-content: space-between;
}
.protocol-layer-row:last-child,
.protocol-frame-row:last-child,
.protocol-sequence-row:last-child,
.protocol-note-row:last-child {
    border-bottom: 0;
    padding-bottom: 0;
}
.protocol-layer-index {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 28px;
    height: 28px;
    flex: 0 0 28px;
    border-radius: 999px;
    background: #dbeafe;
    color: #1d4ed8;
    font-size: 12px;
    font-weight: 700;
}
.protocol-layer-name {
    margin: 0 0 4px;
    font-size: 14px;
    font-weight: 700;
    color: #0f172a;
}
.protocol-note-row {
    display: block;
}
.protocol-note-row p {
    margin: 0 0 6px;
}
.protocol-note-row code {
    font-size: 12px;
}
.protocol-example-row {
    display: grid;
    gap: 10px;
    padding: 12px 0;
    border-bottom: 1px solid #e2e8f0;
}
.protocol-example-row:last-child {
    border-bottom: 0;
    padding-bottom: 0;
}
.protocol-example-block {
    margin: 0;
    padding: 14px;
    border-radius: 12px;
    background: #0f172a;
    color: #e2e8f0;
    border: 1px solid #1e293b;
    font-size: 12px;
    line-height: 1.5;
    overflow: auto;
}
.direction-chip {
    display: inline-flex;
    align-items: center;
    border-radius: 999px;
    padding: 4px 8px;
    background: #eef2ff;
    color: #4338ca;
    font-size: 11px;
    font-weight: 700;
    white-space: nowrap;
}
.protocol-table-wrap {
    overflow: auto;
}
.protocol-table {
    width: 100%;
    border-collapse: collapse;
}
.protocol-table th,
.protocol-table td {
    padding: 10px 8px;
    border-bottom: 1px solid #e2e8f0;
    vertical-align: top;
}
.protocol-table th {
    font-size: 12px;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: #64748b;
}
.protocol-table code {
    font-size: 12px;
}
.runtime-overview-grid {
    display: grid;
    grid-template-columns: repeat(4, minmax(0, 1fr));
    gap: 14px;
    margin: 18px 0;
}
.runtime-stat-card {
    padding: 16px;
    border-radius: 14px;
    background: #ffffff;
    border: 1px solid #d7e1ea;
    box-shadow: 0 8px 24px rgba(15, 23, 42, 0.06);
}
.runtime-stat-value {
    margin: 0 0 6px;
    font-size: 24px;
    font-weight: 800;
    color: #0f172a;
}
.runtime-monitor-grid {
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: 16px;
}
.runtime-panel {
    padding: 18px;
    border-radius: 16px;
    background: #ffffff;
    border: 1px solid #d7e1ea;
    box-shadow: 0 8px 24px rgba(15, 23, 42, 0.06);
    min-height: 0;
}
.runtime-panel-wide {
    grid-column: span 2;
}
.runtime-panel-head {
    display: flex;
    justify-content: space-between;
    align-items: flex-start;
    gap: 12px;
    margin-bottom: 12px;
}
.runtime-panel-head h2 {
    margin: 0;
    font-size: 18px;
    color: #0f172a;
}
.runtime-mini-link {
    color: #1d4ed8;
    text-decoration: none;
    font-size: 13px;
}
.runtime-mini-link:hover {
    text-decoration: underline;
}
.runtime-list {
    display: grid;
    gap: 10px;
}
.runtime-row-card {
    display: flex;
    justify-content: space-between;
    align-items: flex-start;
    gap: 12px;
    padding: 12px 14px;
    border-radius: 12px;
    border: 1px solid #e2e8f0;
    background: #f8fbfd;
}
.runtime-row-title {
    margin: 0 0 4px;
    font-size: 14px;
    font-weight: 700;
    color: #0f172a;
}
.runtime-row-title a {
    color: #0f172a;
    text-decoration: none;
}
.runtime-row-title a:hover {
    text-decoration: underline;
}
.runtime-row-meta {
    display: inline-flex;
    flex-wrap: wrap;
    justify-content: flex-end;
    gap: 8px;
}
.runtime-inline-meta {
    display: inline-flex;
    align-items: center;
    padding: 4px 8px;
    border-radius: 999px;
    background: #eff6ff;
    color: #1d4ed8;
    font-size: 11px;
    font-weight: 700;
}
.runtime-empty {
    padding: 16px;
    border: 1px dashed #cbd5e1;
    border-radius: 12px;
    color: #64748b;
    background: #f8fafc;
}
.workspace-bar-meta {
    display: flex;
    gap: 8px;
    flex-wrap: wrap;
    justify-content: flex-end;
    align-items: center;
}
.workspace-link {
    color: #e2e8f0;
    text-decoration: none;
    font-size: 12px;
    padding: 4px 8px;
    border-radius: 999px;
    background: rgba(255, 255, 255, 0.08);
}
.workspace-link:hover {
    background: rgba(255, 255, 255, 0.14);
}
.workspace-bar-meta form {
    margin: 0;
}
.workspace-logout {
    margin-top: 0;
    padding: 4px 8px;
    border-radius: 999px;
    background: rgba(255, 255, 255, 0.08);
    color: #f8fafc;
    border: 1px solid rgba(255, 255, 255, 0.08);
    font-size: 12px;
}
.workspace-logout:hover {
    background: rgba(255, 255, 255, 0.14);
}
.status-chip,
.workspace-mini {
    display: inline-flex;
    align-items: center;
    padding: 6px 10px;
    border-radius: 999px;
    background: rgba(255, 255, 255, 0.08);
    font-size: 12px;
}
.status-chip.active {
    background: #166534;
    color: #dcfce7;
}
.status-chip.idle {
    background: #475569;
    color: #e2e8f0;
}
.workspace-mini {
    color: #e2e8f0;
}
.workspace-shell {
    display: grid;
    grid-template-columns: minmax(0, 1.7fr) minmax(320px, 0.95fr);
    gap: 16px;
    align-items: stretch;
    height: 100%;
    min-height: 0;
}
.workspace-log,
.workspace-chat {
    background: #ffffff;
    border: 1px solid #dbe2ea;
    border-radius: 16px;
    overflow: hidden;
    min-height: 0;
}
.workspace-log {
    background: linear-gradient(180deg, #0f172a 0%, #111827 100%);
    color: #e5eef8;
    display: grid;
    grid-template-rows: auto auto minmax(0, 1fr);
}
.session-events-panel {
    padding: 0 16px 12px;
    border-bottom: 1px solid rgba(148, 163, 184, 0.18);
}
.session-events-head {
    display: flex;
    justify-content: space-between;
    align-items: baseline;
    gap: 8px;
    margin-bottom: 8px;
}
.session-events-subline {
    margin: 0;
    font-size: 12px;
    color: #94a3b8;
}
.session-events-list {
    display: grid;
    gap: 8px;
    max-height: 132px;
    overflow: auto;
    padding-right: 4px;
}
.session-event {
    border: 1px solid rgba(148, 163, 184, 0.18);
    border-radius: 12px;
    background: rgba(15, 23, 42, 0.28);
    padding: 8px 10px;
}
.session-event.empty {
    color: #cbd5e1;
    font-size: 13px;
}
.session-event-head {
    display: flex;
    justify-content: space-between;
    gap: 8px;
    margin-bottom: 4px;
}
.session-kind {
    font-size: 11px;
    font-weight: 700;
    text-transform: uppercase;
    color: #bfdbfe;
}
.session-time {
    font-size: 11px;
    color: #94a3b8;
}
.session-message {
    margin: 0;
    font-size: 13px;
    color: #e2e8f0;
}
.session-node {
    margin: 0 0 6px;
    font-size: 12px;
    color: #cbd5e1;
}
.session-node a {
    color: #bfdbfe;
    text-decoration: none;
}
.session-node a:hover {
    text-decoration: underline;
}
.session-events-list.global-feed {
    max-height: none;
    grid-template-columns: repeat(auto-fit, minmax(280px, 1fr));
}
.pane-head {
    display: flex;
    justify-content: space-between;
    align-items: flex-start;
    gap: 12px;
    padding: 12px 14px 8px;
}
.pane-head h2 {
    margin: 0;
    font-size: 18px;
}
.pane-meta {
    font-size: 12px;
    color: #94a3b8;
}
.log-stream {
    display: grid;
    gap: 10px;
    padding: 0 16px 16px;
    min-height: 0;
    overflow: auto;
}
.log-card {
    border: 1px solid rgba(148, 163, 184, 0.22);
    border-radius: 14px;
    background: rgba(15, 23, 42, 0.88);
    padding: 12px 14px;
    box-shadow: inset 0 1px 0 rgba(255, 255, 255, 0.03);
}
.log-card-head {
    display: flex;
    justify-content: space-between;
    gap: 12px;
    align-items: flex-start;
}
.log-card h3 {
    margin: 6px 0 2px;
    color: #f8fafc;
    font-size: 15px;
}
.log-card-actions form {
    margin: 0;
}
.log-meta-line {
    margin: 0 0 8px;
    font-size: 12px;
    color: #94a3b8;
}
.log-metrics {
    display: grid;
    grid-template-columns: repeat(4, minmax(0, 1fr));
    gap: 8px;
    margin: 12px 0;
}
.log-metrics div {
    border-radius: 10px;
    background: rgba(30, 41, 59, 0.88);
    padding: 8px 10px;
}
.log-metrics dt {
    font-size: 12px;
    text-transform: uppercase;
    color: #94a3b8;
}
.log-metrics dd {
    margin: 8px 0 0;
    color: #f8fafc;
}
.log-summary {
    margin: 0 0 10px;
    color: #dbeafe;
    font-size: 14px;
}
.log-output summary {
    cursor: pointer;
    color: #bfdbfe;
    font-size: 14px;
}
.log-output pre {
    margin-top: 10px;
    padding: 12px;
    border-radius: 10px;
    background: #020617;
    color: #e2e8f0;
    border: 1px solid rgba(148, 163, 184, 0.16);
    font-size: 13px;
}
.status-badge {
    display: inline-flex;
    align-items: center;
    padding: 6px 10px;
    border-radius: 999px;
    font-size: 12px;
    font-weight: 700;
    letter-spacing: 0.06em;
    text-transform: uppercase;
}
.badge-queued,
.badge-dispatched {
    background: #1d4ed8;
    color: #dbeafe;
}
.badge-running {
    background: #7c3aed;
    color: #ede9fe;
}
.badge-succeeded {
    background: #166534;
    color: #dcfce7;
}
.badge-failed {
    background: #991b1b;
    color: #fee2e2;
}
.badge-canceled {
    background: #9a3412;
    color: #ffedd5;
}
.workspace-chat {
    display: grid;
    grid-template-rows: auto minmax(0, 1fr) auto;
    min-height: 0;
}
.dialog-feed {
    display: grid;
    gap: 10px;
    padding: 0 16px 16px;
    min-height: 0;
    overflow: auto;
}
.dialog-bubble {
    max-width: 92%;
    border-radius: 16px;
    padding: 12px 14px;
    line-height: 1.4;
    box-shadow: 0 12px 24px rgba(15, 23, 42, 0.08);
}
.dialog-bubble.system {
    background: #e2e8f0;
    color: #0f172a;
}
.dialog-bubble.operator {
    margin-left: auto;
    background: #dbeafe;
    color: #1e3a8a;
}
.dialog-role {
    margin: 0 0 8px;
    font-size: 11px;
    font-weight: 700;
    letter-spacing: 0.12em;
    text-transform: uppercase;
    opacity: 0.7;
}
.dialog-detail {
    margin: 4px 0 0;
    font-size: 12px;
    opacity: 0.9;
}
.dialog-status {
    margin: 6px 0 10px;
    font-size: 12px;
}
.dialog-status[hidden] {
    display: none;
}
.dialog-status.success {
    color: #166534;
}
.pending {
    outline: 1px dashed rgba(59, 130, 246, 0.35);
}
.error-bubble {
    background: #fee2e2;
    color: #7f1d1d;
}
.composer-card {
    border-top: 1px solid #e2e8f0;
    padding: 14px 16px;
    background: #f8fafc;
}
.dialog-meta-card {
    margin: 8px 0 10px;
    border: 1px solid #dbe2ea;
    border-radius: 12px;
    padding: 10px 12px;
    background: #ffffff;
}
.dialog-meta-summary {
    cursor: pointer;
    font-size: 12px;
    font-weight: 700;
    color: #334155;
    list-style: none;
}
.dialog-meta-summary::-webkit-details-marker {
    display: none;
}
.dialog-meta-title {
    margin: 0 0 8px;
    font-size: 12px;
    font-weight: 700;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    color: #64748b;
}
.empty-state {
    border: 1px dashed rgba(148, 163, 184, 0.3);
    border-radius: 14px;
    padding: 18px;
    color: #cbd5e1;
    text-align: center;
}
@media (max-width: 980px) {
    body.workspace-page {
        height: auto;
        overflow: auto;
    }
    main.workspace-page {
        height: auto;
        overflow: visible;
        display: block;
        padding: 16px;
    }
    .workspace-shell {
        grid-template-columns: 1fr;
        height: auto;
        min-height: 0;
    }
    .workspace-bar {
        align-items: flex-start;
    }
    .workspace-bar-meta {
        justify-content: flex-start;
    }
    .workspace-bar h1 {
        font-size: 18px;
    }
    .log-stream {
        max-height: none;
    }
    .workspace-log,
    .workspace-chat,
    .dialog-feed {
        min-height: 0;
    }
    .log-metrics {
        grid-template-columns: repeat(2, minmax(0, 1fr));
    }
    .protocol-grid {
        grid-template-columns: 1fr;
    }
    .protocol-span-2 {
        grid-column: span 1;
    }
    .runtime-overview-grid,
    .runtime-monitor-grid {
        grid-template-columns: 1fr;
    }
    .runtime-panel-wide {
        grid-column: span 1;
    }
    .runtime-row-card {
        flex-direction: column;
    }
    .runtime-row-meta {
        justify-content: flex-start;
    }
    .session-filter-bar {
        grid-template-columns: 1fr;
        align-items: stretch;
    }
}
";
