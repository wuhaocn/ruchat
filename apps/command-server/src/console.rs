use crate::app_state::AppState;
use crate::db::{CancelTaskOutcome, CreateTaskOutcome};
use crate::error::internal_error;
use axum::extract::{Form, Path, State};
use axum::http::header::SET_COOKIE;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::Router;
use ru_command_protocol::{
    CommandDescriptor, CreateTaskRequest, NodeSnapshot, TaskSnapshot, TaskStatus,
};
use serde::Deserialize;
use std::sync::Arc;

pub(crate) fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/console", get(console_index))
        .route("/console/login", get(login_page).post(login_submit))
        .route("/console/logout", post(logout))
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
        Ok(nodes) => nodes,
        Err(error) => return server_error_response(error),
    };

    let mut rows = String::new();
    for node in &nodes {
        rows.push_str(&format!(
            "<tr><td><a href=\"/console/nodes/{id}\">{id}</a></td><td>{host}</td><td>{platform}</td><td>{count}</td><td>{last_seen}</td></tr>",
            id = escape_html(&node.node_id),
            host = escape_html(&node.hostname),
            platform = escape_html(&node.platform),
            count = node.commands.len(),
            last_seen = node.last_seen_unix_secs
        ));
    }

    if rows.is_empty() {
        rows.push_str("<tr><td colspan=\"5\">no nodes registered</td></tr>");
    }

    let body = format!(
        "<section><h1>Nodes</h1><p>Registered clients and the commands they reported.</p><table><thead><tr><th>Node</th><th>Hostname</th><th>Platform</th><th>Commands</th><th>Last Seen</th></tr></thead><tbody>{rows}</tbody></table></section>",
        rows = rows
    );

    Html(console_html("Nodes", body, true, "")).into_response()
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
        Ok(node) => node,
        Err(error) => return server_error_response(error),
    }) else {
        return not_found_page("node not found");
    };

    let tasks = match state.db.list_tasks_for_node(&node.node_id, 20) {
        Ok(tasks) => tasks,
        Err(error) => return server_error_response(error),
    };
    let body = render_node_workspace(&node, &tasks);

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

    match state.db.create_task(request) {
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

    match state
        .db
        .cancel_task(task_id, Some("canceled from console".to_string()))
    {
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

fn render_node_workspace(node: &NodeSnapshot, tasks: &[TaskSnapshot]) -> String {
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

    format!(
        "<div class=\"workspace-bar\"><div class=\"workspace-bar-title\"><p class=\"eyebrow\">Node Workspace</p><h1>{node_id}</h1><p class=\"workspace-subline\">{hostname} | {platform}</p></div><div class=\"workspace-bar-meta\"><a class=\"workspace-link\" href=\"/console/nodes\">Nodes</a><a class=\"workspace-link\" href=\"/console/tasks\">Tasks</a><form method=\"post\" action=\"/console/logout\"><button type=\"submit\" class=\"workspace-logout\">Logout</button></form><span class=\"status-chip active\">active</span><span class=\"workspace-mini\">last seen {last_seen}</span></div></div><section class=\"workspace-shell\"><div class=\"workspace-log\"><div class=\"pane-head compact\"><div><p class=\"eyebrow\">Execution Log</p><h2>Task Timeline</h2></div><div class=\"pane-meta\">auto refresh 3s</div></div><div id=\"task-log\" class=\"log-stream\">{task_log}</div></div><aside class=\"workspace-chat\"><div class=\"pane-head compact\"><div><p class=\"eyebrow\">Operator Dialogue</p><h2>Command Session</h2></div></div><div id=\"dialog-feed\" class=\"dialog-feed\">{dialog_feed}</div><form id=\"composer-form\" class=\"composer-card\" method=\"post\" action=\"/console/tasks\"><input type=\"hidden\" name=\"node_id\" value=\"{node_id_raw}\"><input type=\"hidden\" name=\"return_to\" value=\"/console/nodes/{node_id_raw}\"><label>Command<select id=\"command-select\" name=\"command_name\">{command_options}</select></label><div id=\"command-meta\">{command_help}</div><label>Extra args<textarea id=\"args-text\" name=\"args_text\" rows=\"2\" placeholder=\"hello&#10;world\"></textarea></label><label>Timeout seconds<input id=\"timeout-input\" type=\"number\" min=\"1\" name=\"timeout_secs\" placeholder=\"30\"></label><p id=\"dialog-status\" class=\"dialog-status\" hidden></p><button id=\"send-button\" type=\"submit\">Send Task</button></form></aside></section>{script}",
        node_id = escape_html(&node.node_id),
        node_id_raw = escape_html(&node.node_id),
        hostname = escape_html(&node.hostname),
        platform = escape_html(&node.platform),
        last_seen = node.last_seen_unix_secs,
        task_log = task_log,
        dialog_feed = dialog_feed,
        command_options = command_options,
        command_help = command_help,
        script = script
    )
}

fn render_task_row(task: &TaskSnapshot, include_agent: bool) -> String {
    let result_html = match &task.result {
        Some(result) => format!(
            "<details><summary>success={} exit={:?} duration={}ms</summary><pre>stdout:\n{}\n\nstderr:\n{}\n\nerror:\n{}</pre></details>",
            result.success,
            result.exit_code,
            result.duration_ms,
            escape_html(&result.stdout),
            escape_html(&result.stderr),
            escape_html(result.error.as_deref().unwrap_or(""))
        ),
        None => escape_html(task.cancel_reason.as_deref().unwrap_or("-")),
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

    if include_agent {
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
            "<details class=\"log-output\"><summary>stdout / stderr</summary><pre>stdout:\n{}\n\nstderr:\n{}\n\nerror:\n{}</pre></details>",
            escape_html(&result.stdout),
            escape_html(&result.stderr),
            escape_html(result.error.as_deref().unwrap_or("-"))
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
        created = task.created_at_unix_secs,
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
    let operator = format!(
        "<article class=\"dialog-bubble operator\"><p class=\"dialog-role\">operator</p><p>run <strong>{command}</strong></p><p class=\"dialog-detail\">args: {args}</p><p class=\"dialog-detail\">task #{task_id} at {created}</p></article>",
        command = escape_html(&task.command_name),
        args = escape_html(&display_args(&task.args)),
        task_id = task.task_id,
        created = task.created_at_unix_secs,
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
            if stdout.is_empty() {
                "no stdout".to_string()
            } else {
                format!("stdout: {}", truncate_text(stdout, 120))
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
const taskLog = document.getElementById('task-log');
const dialogFeed = document.getElementById('dialog-feed');
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
  const output = task.result
    ? `<details class=\"log-output\"><summary>stdout / stderr</summary><pre>stdout:\\n${{escapeHtml(task.result.stdout)}}\\n\\nstderr:\\n${{escapeHtml(task.result.stderr)}}\\n\\nerror:\\n${{escapeHtml(task.result.error ?? '-')}}</pre></details>`
    : '';
  const summary = task.result
    ? `exit=${{task.result.exit_code}} duration=${{task.result.duration_ms}}ms`
    : (task.cancel_reason || task.retry_reason || 'waiting for execution');
  const cancel = canCancel
    ? `<form method=\"post\" action=\"/console/tasks/${{task.task_id}}/cancel\"><input type=\"hidden\" name=\"return_to\" value=\"/console/nodes/${{escapeHtml(nodeId)}}\"><button type=\"submit\" class=\"secondary\">Cancel</button></form>`
    : '';
  return `<article class=\"log-card status-${{escapeHtml(task.status)}}\"><div class=\"log-card-head\"><div><span class=\"status-badge badge-${{escapeHtml(task.status)}}\">${{escapeHtml(task.status)}}</span><h3>${{escapeHtml(task.command_name)}}</h3><p class=\"muted\">task #${{task.task_id}} | args: ${{escapeHtml((task.args || []).join(' ') || '[]')}}</p></div><div class=\"log-card-actions\">${{cancel}}</div></div><dl class=\"log-metrics\"><div><dt>created</dt><dd>${{escapeHtml(task.created_at_unix_secs)}}</dd></div><div><dt>started</dt><dd>${{escapeHtml(task.started_at_unix_secs ?? '-')}}</dd></div><div><dt>finished</dt><dd>${{escapeHtml(task.finished_at_unix_secs ?? '-')}}</dd></div><div><dt>retry</dt><dd>${{escapeHtml(task.retry_count)}}</dd></div></dl><p class=\"log-summary\">${{escapeHtml(summary)}}</p>${{output}}</article>`;
}}

function truncateText(value, limit) {{
  if (value.length <= limit) return value;
  return value.slice(0, limit - 3) + '...';
}}

function renderDialogPair(task) {{
  const stdout = task.result && task.result.stdout ? task.result.stdout.trim() : '';
  const summary = task.result
    ? `status=${{task.status}} exit=${{task.result.exit_code}} duration=${{task.result.duration_ms}}ms`
    : `status=${{task.status}}`;
  const extra = task.result
    ? (stdout ? `stdout: ${{truncateText(stdout, 120)}}` : 'no stdout')
    : (task.cancel_reason || task.retry_reason || 'waiting for execution');
  return `<article class=\"dialog-bubble operator\"><p class=\"dialog-role\">operator</p><p>run <strong>${{escapeHtml(task.command_name)}}</strong></p><p class=\"dialog-detail\">args: ${{escapeHtml((task.args || []).join(' ') || '[]')}}</p><p class=\"dialog-detail\">task #${{task.task_id}} at ${{escapeHtml(task.created_at_unix_secs)}}</p></article><article class=\"dialog-bubble system\"><p class=\"dialog-role\">system</p><p>${{escapeHtml(summary)}}</p><p class=\"dialog-detail\">${{escapeHtml(extra)}}</p></article>`;
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

async function refreshTasks() {{
  try {{
    const response = await fetch(`/api/v1/nodes/${{encodeURIComponent(nodeId)}}/tasks?limit=20`, {{ credentials: 'same-origin' }});
    if (!response.ok) return;
    const tasks = await response.json();
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
    await refreshTasks();
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
  setInterval(refreshTasks, 3000);
}}
</script>",
        node_id = node_id
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
        "<header><nav><a href=\"/console/nodes\">Nodes</a><a href=\"/console/tasks\">Tasks</a><form method=\"post\" action=\"/console/logout\"><button type=\"submit\">Logout</button></form></nav></header>"
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
    grid-template-rows: auto minmax(0, 1fr);
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
}
";
