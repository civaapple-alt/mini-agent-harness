use crate::skills::McpServerConfig;
use crate::skills::McpTransportConfig;
use crate::workspace::ApprovalController;
use http::HeaderName;
use http::HeaderValue;
use mini_agent_protocol::Tool;
use mini_agent_protocol::ToolAdmission;
use mini_agent_protocol::ToolError;
use mini_agent_protocol::ToolExecutionOutcome;
use mini_agent_protocol::ToolExecutionRequest;
use mini_agent_protocol::ToolHandler;
use mini_agent_protocol::ToolRuntime;
use mini_agent_protocol::ToolSpec;
use rmcp::ClientLifecycleMode;
use rmcp::ClientServiceExt;
use rmcp::model::CallToolRequestParams;
use rmcp::model::PaginatedRequestParams;
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::transport::TokioChildProcess;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::transport::which_command;
use serde_json::Value;
use std::collections::BTreeSet;
use std::collections::HashMap;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;
use tokio::process::Command;
use tokio::sync::mpsc as tokio_mpsc;
use tokio::time::timeout;

const CALL_TIMEOUT: Duration = Duration::from_secs(120);
#[cfg(not(test))]
const PROTOCOL_CALL_TIMEOUT: Duration = Duration::from_secs(118);
#[cfg(test)]
const PROTOCOL_CALL_TIMEOUT: Duration = Duration::from_millis(50);
const MAX_MCP_TOOLS: usize = 32;
const MAX_TOOL_SCHEMA_BYTES: usize = 16 * 1024;
const MAX_TOOL_RESULT_BYTES: usize = 64 * 1024;
const MAX_EXPOSED_NAME_BYTES: usize = 64;

pub struct LoadResult {
    pub tools: Vec<Box<dyn Tool>>,
    pub loaded_servers: BTreeSet<String>,
    pub diagnostics: Vec<String>,
}

struct PendingServer {
    label: String,
    commands: tokio_mpsc::UnboundedSender<ServerCommand>,
    startup: mpsc::Receiver<Result<Vec<RemoteTool>, String>>,
    startup_wait: Duration,
}

struct RemoteTool {
    name: String,
    description: String,
    parameters: Value,
}

enum ServerCommand {
    Call {
        name: String,
        arguments: serde_json::Map<String, Value>,
        reply: mpsc::SyncSender<Result<String, String>>,
    },
}

struct McpTool {
    spec: ToolSpec,
    remote_name: String,
    server_label: String,
    commands: tokio_mpsc::UnboundedSender<ServerCommand>,
    approval: ApprovalController,
}

pub fn load(servers: &[McpServerConfig], approval: ApprovalController) -> LoadResult {
    let mut diagnostics = Vec::new();
    let mut pending = Vec::new();
    for server in servers {
        let label = format!("{}/{}", server.plugin_name, server.server_name);
        if let Err(error) = approval.approve(&format!("connect MCP server {label:?}")) {
            diagnostics.push(format!("MCP server {label} was not started: {error}"));
            continue;
        }
        match start_server(server.clone()) {
            Ok(server) => pending.push(server),
            Err(error) => diagnostics.push(format!("MCP server {label} failed to start: {error}")),
        }
    }

    let mut tools: Vec<Box<dyn Tool>> = Vec::new();
    let mut loaded_servers = BTreeSet::new();
    let mut exposed_names = BTreeSet::new();
    for server in pending {
        let remote_tools = match server.startup.recv_timeout(server.startup_wait) {
            Ok(Ok(tools)) => tools,
            Ok(Err(error)) => {
                diagnostics.push(format!("MCP server {} failed: {error}", server.label));
                continue;
            }
            Err(error) => {
                diagnostics.push(format!(
                    "MCP server {} did not become ready: {error}",
                    server.label
                ));
                continue;
            }
        };
        loaded_servers.insert(server.label.clone());
        for remote in remote_tools {
            if tools.len() >= MAX_MCP_TOOLS {
                diagnostics.push(format!(
                    "MCP tool limit reached ({MAX_MCP_TOOLS}); remaining tools were skipped"
                ));
                return LoadResult {
                    tools,
                    loaded_servers,
                    diagnostics,
                };
            }
            let Some(name) = exposed_tool_name(&server.label, &remote.name) else {
                diagnostics.push(format!(
                    "MCP tool {}/{} cannot be represented as a bounded model tool name",
                    server.label, remote.name
                ));
                continue;
            };
            if !exposed_names.insert(name.clone()) {
                diagnostics.push(format!("duplicate MCP tool name {name:?} was skipped"));
                continue;
            }
            tools.push(Box::new(McpTool {
                spec: ToolSpec {
                    name,
                    description: bounded_description(&format!(
                        "MCP {} — {}",
                        server.label, remote.description
                    )),
                    parameters: remote.parameters,
                },
                remote_name: remote.name,
                server_label: server.label.clone(),
                commands: server.commands.clone(),
                approval: approval.clone(),
            }));
        }
    }
    LoadResult {
        tools,
        loaded_servers,
        diagnostics,
    }
}

impl ToolHandler for McpTool {
    fn spec(&self) -> ToolSpec {
        self.spec.clone()
    }

    fn admission(&self, request: &ToolExecutionRequest) -> Result<ToolAdmission, ToolError> {
        self.approval.ensure_plan_mode_unlocked()?;
        request
            .arguments
            .as_object()
            .ok_or_else(|| ToolError("MCP tool arguments must be a JSON object".to_string()))?;
        Ok(ToolAdmission::ApprovalRequired {
            action: self.action(),
        })
    }
}

impl ToolRuntime for McpTool {
    fn execute(&self, arguments: &Value) -> Result<String, ToolError> {
        self.approval.ensure_plan_mode_unlocked()?;
        self.approval.approve(&self.action())?;
        self.call(arguments)
    }

    fn execute_after_admission(&self, request: &ToolExecutionRequest) -> ToolExecutionOutcome {
        crate::into_tool_outcome(self.call(&request.arguments))
    }
}

impl McpTool {
    fn action(&self) -> String {
        format!(
            "call MCP tool {:?} on {}",
            self.remote_name, self.server_label
        )
    }

    fn call(&self, arguments: &Value) -> Result<String, ToolError> {
        let arguments = arguments
            .as_object()
            .ok_or_else(|| ToolError("MCP tool arguments must be a JSON object".to_string()))?
            .clone();
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        self.commands
            .send(ServerCommand::Call {
                name: self.remote_name.clone(),
                arguments,
                reply: reply_tx,
            })
            .map_err(|_| ToolError(format!("MCP server {} stopped", self.server_label)))?;
        reply_rx
            .recv_timeout(CALL_TIMEOUT)
            .map_err(|error| ToolError(format!("MCP call did not complete: {error}")))?
            .map_err(ToolError)
    }
}

fn start_server(config: McpServerConfig) -> Result<PendingServer, String> {
    let label = format!("{}/{}", config.plugin_name, config.server_name);
    let startup_wait = config
        .connect_timeout
        .saturating_add(Duration::from_secs(2));
    let thread_name = format!("mcp-{}", sanitize_name(&label));
    let (commands_tx, commands_rx) = tokio_mpsc::unbounded_channel();
    let (startup_tx, startup_rx) = mpsc::sync_channel(1);
    thread::Builder::new()
        .name(thread_name.chars().take(48).collect())
        .spawn(move || run_server(config, commands_rx, startup_tx))
        .map_err(|error| error.to_string())?;
    Ok(PendingServer {
        label,
        commands: commands_tx,
        startup: startup_rx,
        startup_wait,
    })
}

const CIRCUIT_BREAKER_THRESHOLD: usize = 3;
const CIRCUIT_BREAKER_COOLDOWN: Duration = Duration::from_secs(30);

#[derive(Default)]
struct CircuitBreaker {
    consecutive_failures: usize,
    tripped_until: Option<tokio::time::Instant>,
}

impl CircuitBreaker {
    fn can_execute(&mut self) -> Result<(), String> {
        if let Some(until) = self.tripped_until {
            if tokio::time::Instant::now() < until {
                return Err(format!(
                    "MCP server circuit breaker is open (failing fast after {} consecutive errors)",
                    self.consecutive_failures
                ));
            }
            self.tripped_until = None;
        }
        Ok(())
    }

    fn record_success(&mut self) {
        self.consecutive_failures = 0;
        self.tripped_until = None;
    }

    fn record_failure(&mut self) {
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        if self.consecutive_failures >= CIRCUIT_BREAKER_THRESHOLD {
            self.tripped_until = Some(tokio::time::Instant::now() + CIRCUIT_BREAKER_COOLDOWN);
        }
    }
}

fn run_server(
    config: McpServerConfig,
    mut commands: tokio_mpsc::UnboundedReceiver<ServerCommand>,
    startup: mpsc::SyncSender<Result<Vec<RemoteTool>, String>>,
) {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            let _ = startup.send(Err(format!("cannot create MCP runtime: {error}")));
            return;
        }
    };
    runtime.block_on(async move {
        let connected = timeout(config.connect_timeout, connect(&config)).await;
        let (client, tools) = match connected {
            Ok(Ok(connected)) => connected,
            Ok(Err(error)) => {
                let _ = startup.send(Err(error));
                return;
            }
            Err(_) => {
                let _ = startup.send(Err("connection timed out".to_string()));
                return;
            }
        };
        if startup.send(Ok(tools)).is_err() {
            let _ = client.cancel().await;
            return;
        }
        let mut circuit_breaker = CircuitBreaker::default();
        while let Some(command) = commands.recv().await {
            match command {
                ServerCommand::Call {
                    name,
                    arguments,
                    reply,
                } => {
                    if let Err(error) = circuit_breaker.can_execute() {
                        let _ = reply.send(Err(error));
                        continue;
                    }
                    let params = CallToolRequestParams::new(name).with_arguments(arguments);
                    let result = timeout(PROTOCOL_CALL_TIMEOUT, client.call_tool(params)).await;
                    let result = match result {
                        Ok(Ok(result)) => {
                            circuit_breaker.record_success();
                            bounded_result(&result)
                        }
                        Ok(Err(error)) => {
                            circuit_breaker.record_failure();
                            Err(error.to_string())
                        }
                        Err(_) => {
                            circuit_breaker.record_failure();
                            Err("MCP tool call timed out".to_string())
                        }
                    };
                    let _ = reply.send(result);
                }
            }
        }
        let _ = client.cancel().await;
    });
}

async fn connect(
    config: &McpServerConfig,
) -> Result<
    (
        rmcp::service::RunningService<rmcp::RoleClient, ()>,
        Vec<RemoteTool>,
    ),
    String,
> {
    let client = match &config.transport {
        McpTransportConfig::Stdio { .. } => {
            let command = build_command(config)?;
            let transport = TokioChildProcess::new(command)
                .map_err(|error| format!("cannot spawn stdio transport: {error}"))?;
            ().serve_with_lifecycle(transport, client_lifecycle()).await
        }
        McpTransportConfig::StreamableHttp { url, headers } => {
            let headers = http_headers(headers)?;
            let transport_config = StreamableHttpClientTransportConfig::with_uri(url.clone())
                .custom_headers(headers)
                .max_sse_event_size(MAX_TOOL_RESULT_BYTES);
            let transport = StreamableHttpClientTransport::from_config(transport_config);
            ().serve_with_lifecycle(transport, client_lifecycle()).await
        }
    }
    .map_err(|error| format!("MCP handshake failed: {error}"))?;
    let mut cursor = None;
    let mut tools = Vec::new();
    loop {
        let params = cursor
            .take()
            .map(|cursor| PaginatedRequestParams::default().with_cursor(Some(cursor)));
        let page = client
            .list_tools(params)
            .await
            .map_err(|error| format!("tools/list failed: {error}"))?;
        for tool in page.tools {
            if tools.len() >= MAX_MCP_TOOLS {
                break;
            }
            let parameters = Value::Object((*tool.input_schema).clone());
            let schema_bytes = serde_json::to_vec(&parameters)
                .map_err(|error| format!("cannot serialize tool schema: {error}"))?
                .len();
            if schema_bytes > MAX_TOOL_SCHEMA_BYTES {
                continue;
            }
            tools.push(RemoteTool {
                name: tool.name.into_owned(),
                description: bounded_description(tool.description.as_deref().unwrap_or("MCP tool")),
                parameters,
            });
        }
        if tools.len() >= MAX_MCP_TOOLS {
            break;
        }
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    Ok((client, tools))
}

fn client_lifecycle() -> ClientLifecycleMode {
    ClientLifecycleMode::Initialize
}

fn build_command(config: &McpServerConfig) -> Result<Command, String> {
    let McpTransportConfig::Stdio {
        command: command_name,
        args,
        env,
        cwd,
    } = &config.transport
    else {
        return Err("cannot build a command for an HTTP MCP server".to_string());
    };
    let plugin_data = prepare_plugin_data(config)?;
    let plugin_root = path_text(&config.plugin_root, "PLUGIN_ROOT")?;
    let plugin_data_text = path_text(&plugin_data, "PLUGIN_DATA")?;
    let mut command = if let Some(relative) = command_name.strip_prefix("./") {
        let path = contained_existing_path(&config.plugin_root, relative, false)?;
        Command::new(path)
    } else {
        if Path::new(command_name).components().count() != 1 {
            return Err("bare MCP command must be a single executable name".to_string());
        }
        which_command(command_name)
            .map_err(|error| format!("cannot resolve executable {command_name:?}: {error}"))?
    };
    apply_safe_environment(&mut command);
    command.args(
        args.iter()
            .map(|arg| expand_placeholders(arg, plugin_root, plugin_data_text)),
    );
    for (key, value) in env {
        command.env(
            key,
            expand_placeholders(value, plugin_root, plugin_data_text),
        );
    }
    command.env("PLUGIN_ROOT", &config.plugin_root);
    command.env("PLUGIN_DATA", &plugin_data);
    let cwd = match cwd {
        None => config.plugin_root.clone(),
        Some(cwd) => resolve_cwd(cwd, &config.plugin_root, &plugin_data)?,
    };
    command.current_dir(cwd);
    Ok(command)
}

fn apply_safe_environment(command: &mut Command) {
    const SAFE_KEYS: &[&str] = &[
        "PATH",
        "PATHEXT",
        "SystemRoot",
        "WINDIR",
        "HOME",
        "USERPROFILE",
        "TMP",
        "TEMP",
        "TMPDIR",
        "LANG",
        "LC_ALL",
    ];
    command.env_clear();
    for (key, value) in std::env::vars_os() {
        if SAFE_KEYS
            .iter()
            .any(|safe| key.to_string_lossy().eq_ignore_ascii_case(safe))
        {
            command.env(key, value);
        }
    }
}

fn prepare_plugin_data(config: &McpServerConfig) -> Result<PathBuf, String> {
    let agents_root = config
        .workspace_root
        .join(".agents")
        .canonicalize()
        .map_err(|error| format!("cannot resolve .agents directory: {error}"))?;
    if !agents_root.starts_with(&config.workspace_root) {
        return Err(".agents directory escapes the workspace".to_string());
    }
    let data_root = ensure_child_directory(&agents_root, "plugin-data")?;
    let plugin_data = ensure_child_directory(&data_root, &config.plugin_name)?;
    if plugin_data != config.plugin_data {
        return Err("resolved PLUGIN_DATA does not match its configured path".to_string());
    }
    Ok(plugin_data)
}

fn ensure_child_directory(parent: &Path, name: &str) -> Result<PathBuf, String> {
    let candidate = parent.join(name);
    if !candidate.exists() {
        match std::fs::create_dir(&candidate) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(format!("cannot create {}: {error}", candidate.display()));
            }
        }
    }
    let resolved = candidate
        .canonicalize()
        .map_err(|error| format!("cannot resolve {}: {error}", candidate.display()))?;
    if !resolved.is_dir() || !resolved.starts_with(parent) {
        return Err(format!("{} escapes its allowed root", candidate.display()));
    }
    Ok(resolved)
}

fn resolve_cwd(value: &str, plugin_root: &Path, plugin_data: &Path) -> Result<PathBuf, String> {
    let (boundary, relative) = if let Some(relative) = value.strip_prefix("./") {
        (plugin_root, relative)
    } else if value == "${PLUGIN_ROOT}" {
        (plugin_root, "")
    } else if let Some(relative) = value.strip_prefix("${PLUGIN_ROOT}/") {
        (plugin_root, relative)
    } else if value == "${PLUGIN_DATA}" {
        (plugin_data, "")
    } else if let Some(relative) = value.strip_prefix("${PLUGIN_DATA}/") {
        (plugin_data, relative)
    } else {
        return Err("invalid MCP cwd".to_string());
    };
    contained_existing_path(boundary, relative, true)
}

fn contained_existing_path(
    boundary: &Path,
    relative: &str,
    directory: bool,
) -> Result<PathBuf, String> {
    let relative_path = Path::new(relative);
    if relative_path
        .components()
        .any(|component| matches!(component, Component::RootDir | Component::Prefix(_)))
    {
        return Err("MCP package path must remain relative".to_string());
    }
    let candidate = boundary.join(relative_path);
    let resolved = candidate
        .canonicalize()
        .map_err(|error| format!("cannot resolve {}: {error}", candidate.display()))?;
    if !resolved.starts_with(boundary)
        || (directory && !resolved.is_dir())
        || (!directory && !resolved.is_file())
    {
        return Err(format!("{} escapes its allowed root", candidate.display()));
    }
    Ok(resolved)
}

fn exposed_tool_name(server: &str, remote: &str) -> Option<String> {
    let name = format!("mcp__{}__{}", sanitize_name(server), sanitize_name(remote));
    (!remote.is_empty() && name.len() <= MAX_EXPOSED_NAME_BYTES).then_some(name)
}

fn sanitize_name(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '_' | '-') {
                character
            } else {
                '_'
            }
        })
        .collect()
}

fn bounded_description(value: &str) -> String {
    value.chars().take(1024).collect()
}

fn bounded_result(result: &rmcp::model::CallToolResult) -> Result<String, String> {
    let body = serde_json::to_string(result)
        .map_err(|error| format!("cannot serialize MCP tool result: {error}"))?;
    if body.len() <= MAX_TOOL_RESULT_BYTES {
        return Ok(body);
    }
    let mut boundary = MAX_TOOL_RESULT_BYTES;
    loop {
        while !body.is_char_boundary(boundary) {
            boundary -= 1;
        }
        let truncated = serde_json::to_string(&serde_json::json!({
            "truncated": true,
            "preview": &body[..boundary],
        }))
        .map_err(|error| format!("cannot serialize truncated MCP result: {error}"))?;
        if truncated.len() <= MAX_TOOL_RESULT_BYTES {
            return Ok(truncated);
        }
        boundary /= 2;
    }
}

fn expand_placeholders(value: &str, plugin_root: &str, plugin_data: &str) -> String {
    const ROOT: &str = "${PLUGIN_ROOT}";
    const DATA: &str = "${PLUGIN_DATA}";
    let mut expanded = String::with_capacity(value.len());
    let mut remaining = value;
    while !remaining.is_empty() {
        if let Some(rest) = remaining.strip_prefix(ROOT) {
            expanded.push_str(plugin_root);
            remaining = rest;
        } else if let Some(rest) = remaining.strip_prefix(DATA) {
            expanded.push_str(plugin_data);
            remaining = rest;
        } else {
            let character = remaining.chars().next().expect("remaining is non-empty");
            expanded.push(character);
            remaining = &remaining[character.len_utf8()..];
        }
    }
    expanded
}

fn http_headers(
    values: &std::collections::BTreeMap<String, String>,
) -> Result<HashMap<HeaderName, HeaderValue>, String> {
    values
        .iter()
        .map(|(name, value)| {
            let name = HeaderName::from_bytes(name.as_bytes())
                .map_err(|error| format!("invalid MCP HTTP header name: {error}"))?;
            let value = expand_environment(value)?;
            let value = HeaderValue::from_str(&value)
                .map_err(|error| format!("invalid MCP HTTP header value: {error}"))?;
            Ok((name, value))
        })
        .collect()
}

fn expand_environment(value: &str) -> Result<String, String> {
    let mut expanded = String::with_capacity(value.len());
    let mut remaining = value;
    while let Some(start) = remaining.find("${") {
        expanded.push_str(&remaining[..start]);
        let after_open = &remaining[start + 2..];
        let end = after_open
            .find('}')
            .ok_or_else(|| "unterminated environment placeholder in MCP header".to_string())?;
        let token = &after_open[..end];
        let expression = token.strip_prefix("env:").unwrap_or(token);
        let (name, default) = expression
            .split_once(":-")
            .map_or((expression, None), |(name, default)| (name, Some(default)));
        if name.is_empty()
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        {
            return Err(format!("invalid environment placeholder {expression:?}"));
        }
        match std::env::var(name) {
            Ok(value) if value.is_empty() => expanded.push_str(default.unwrap_or_default()),
            Ok(value) => expanded.push_str(&value),
            Err(std::env::VarError::NotPresent) => match default {
                Some(default) => expanded.push_str(default),
                None => return Err(format!("environment variable {name} is not configured")),
            },
            Err(std::env::VarError::NotUnicode(_)) => {
                return Err(format!("environment variable {name} is not UTF-8"));
            }
        }
        remaining = &after_open[end + 1..];
    }
    expanded.push_str(remaining);
    Ok(expanded)
}

fn path_text<'a>(path: &'a Path, name: &str) -> Result<&'a str, String> {
    path.to_str()
        .ok_or_else(|| format!("{name} must be valid UTF-8"))
}

#[cfg(test)]
#[path = "mcp_tests.rs"]
mod tests;
