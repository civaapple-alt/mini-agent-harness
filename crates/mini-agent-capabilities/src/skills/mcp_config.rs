use super::*;

pub(super) fn discover_project_mcp(workspace: &Path, discovery: &mut Discovery) {
    let root = workspace.join(".agents/mcp");
    if !root.exists() {
        return;
    }
    let root = match contained_directory(&root, workspace) {
        Ok(root) => root,
        Err(error) => {
            discovery.diagnostics.push(error);
            return;
        }
    };
    for path in directory_children(&root, "MCP config", &mut discovery.diagnostics) {
        if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
            continue;
        }
        let path = match contained_file(&path, &root) {
            Ok(path) => path,
            Err(error) => {
                discovery.diagnostics.push(error);
                continue;
            }
        };
        discover_native_mcp_file(workspace, &path, discovery);
    }
}

fn discover_native_mcp_file(workspace: &Path, path: &Path, discovery: &mut Discovery) {
    let value = match read_json(path) {
        Ok(value) => value,
        Err(error) => {
            discovery.diagnostics.push(error);
            return;
        }
    };
    let Some(object) = value.as_object() else {
        discovery
            .diagnostics
            .push(format!("{} must contain a JSON object", path.display()));
        return;
    };
    if object.get("enabled").and_then(Value::as_bool) == Some(false) {
        return;
    }
    if object
        .get("enabled")
        .is_some_and(|value| !value.is_boolean())
    {
        discovery.diagnostics.push(format!(
            "{} field \"enabled\" must be a boolean",
            path.display()
        ));
        return;
    }
    let fallback_name = path
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    let server_name = object
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or(fallback_name);
    let mut server = object.clone();
    server.remove("name");
    server.remove("enabled");
    let timeout = match parse_connect_timeout(server.remove("connect_timeout_ms").as_ref()) {
        Ok(timeout) => timeout,
        Err(error) => {
            discovery
                .diagnostics
                .push(format!("{}: {error}", path.display()));
            return;
        }
    };
    if let Some(transport) = server.remove("transport")
        && server.insert("type".to_string(), transport).is_some()
    {
        discovery.diagnostics.push(format!(
            "{} must not contain both \"transport\" and \"type\"",
            path.display()
        ));
        return;
    }
    match parse_mcp_server(
        workspace,
        workspace,
        "project",
        server_name,
        &Value::Object(server),
        timeout,
    ) {
        Ok(server) => push_mcp_server(server, discovery),
        Err(error) => discovery
            .diagnostics
            .push(format!("{}: {error}", path.display())),
    }
}

pub(super) fn discover_mcp_file(
    workspace: &Path,
    plugin_root: &Path,
    plugin_name: &str,
    path: &Path,
    discovery: &mut Discovery,
) {
    if !path.exists() {
        return;
    }
    let path = match contained_file(path, plugin_root) {
        Ok(path) => path,
        Err(error) => {
            discovery.diagnostics.push(error);
            return;
        }
    };
    let value = match read_json(&path) {
        Ok(value) => value,
        Err(error) => {
            discovery.diagnostics.push(error);
            return;
        }
    };
    let object = match value.as_object() {
        Some(object) => object,
        None => {
            discovery
                .diagnostics
                .push(format!("{} must contain a JSON object", path.display()));
            return;
        }
    };
    if object
        .keys()
        .any(|key| key != "$schema" && key != "mcpServers")
    {
        discovery.diagnostics.push(format!(
            "{} contains unknown top-level fields; MCP was disabled",
            path.display()
        ));
        return;
    }
    if required_string(object, "$schema", &path).ok() != Some(MCP_SCHEMA) {
        discovery.diagnostics.push(format!(
            "{} must target Agent Plugins MCP schema 1.0.0",
            path.display()
        ));
        return;
    }
    let Some(servers) = object.get("mcpServers").and_then(Value::as_object) else {
        discovery.diagnostics.push(format!(
            "{} field \"mcpServers\" must be an object",
            path.display()
        ));
        return;
    };
    let mut server_names = servers.keys().collect::<Vec<_>>();
    server_names.sort();
    for server_name in server_names {
        match parse_mcp_server(
            workspace,
            plugin_root,
            plugin_name,
            server_name,
            &servers[server_name],
            DEFAULT_CONNECT_TIMEOUT,
        ) {
            Ok(server) => push_mcp_server(server, discovery),
            Err(error) => discovery.diagnostics.push(format!(
                "{} server {server_name:?}: {error}",
                path.display()
            )),
        }
    }
}

fn parse_mcp_server(
    workspace: &Path,
    plugin_root: &Path,
    plugin_name: &str,
    server_name: &str,
    value: &Value,
    connect_timeout: Duration,
) -> Result<McpServerConfig, String> {
    validate_plugin_name(server_name)
        .map_err(|_| format!("invalid MCP server name {server_name:?}"))?;
    let object = value
        .as_object()
        .ok_or_else(|| "configuration must be an object".to_string())?;
    let transport = match object.get("type") {
        Some(value) => value
            .as_str()
            .ok_or_else(|| "field \"type\" must be a string".to_string())?,
        None => return Err("field \"type\" must be a string".to_string()),
    };
    let transport = match transport {
        "stdio" => parse_stdio_transport(object)?,
        "http" | "streamable-http" => parse_http_transport(object)?,
        "sse" => return Err("legacy SSE transport is not supported".to_string()),
        transport => return Err(format!("unknown transport {transport:?}")),
    };
    Ok(McpServerConfig {
        plugin_name: plugin_name.to_string(),
        server_name: server_name.to_string(),
        workspace_root: workspace.to_path_buf(),
        plugin_root: plugin_root.to_path_buf(),
        plugin_data: workspace.join(".agents/plugin-data").join(plugin_name),
        connect_timeout,
        transport,
    })
}

fn parse_stdio_transport(object: &Map<String, Value>) -> Result<McpTransportConfig, String> {
    if object
        .keys()
        .any(|key| !matches!(key.as_str(), "type" | "command" | "args" | "env" | "cwd"))
    {
        return Err("stdio configuration contains unknown fields".to_string());
    }
    let command = object
        .get("command")
        .and_then(Value::as_str)
        .filter(|command| !command.is_empty())
        .ok_or_else(|| "field \"command\" must be a non-empty string".to_string())?;
    let args = string_array(object.get("args"), "args")?;
    let env = string_map(object.get("env"), "env")?;
    if env
        .keys()
        .any(|key| matches!(key.as_str(), "PLUGIN_ROOT" | "PLUGIN_DATA"))
    {
        return Err("env must not define PLUGIN_ROOT or PLUGIN_DATA".to_string());
    }
    let cwd = optional_string(object.get("cwd"), "cwd")?;
    if cwd.as_deref().is_some_and(|cwd| {
        !cwd.starts_with("./")
            && cwd != "${PLUGIN_ROOT}"
            && !cwd.starts_with("${PLUGIN_ROOT}/")
            && cwd != "${PLUGIN_DATA}"
            && !cwd.starts_with("${PLUGIN_DATA}/")
    }) {
        return Err("cwd must be package-relative or rooted at a plugin placeholder".to_string());
    }
    Ok(McpTransportConfig::Stdio {
        command: command.to_string(),
        args,
        env,
        cwd,
    })
}

fn parse_http_transport(object: &Map<String, Value>) -> Result<McpTransportConfig, String> {
    if object
        .keys()
        .any(|key| !matches!(key.as_str(), "type" | "url" | "headers"))
    {
        return Err("HTTP configuration contains unknown fields".to_string());
    }
    let url = object
        .get("url")
        .and_then(Value::as_str)
        .filter(|url| !url.is_empty())
        .ok_or_else(|| "field \"url\" must be a non-empty string".to_string())?;
    let parsed = reqwest::Url::parse(url).map_err(|error| format!("invalid MCP URL: {error}"))?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        return Err("MCP URL must be an absolute http or https URL".to_string());
    }
    Ok(McpTransportConfig::StreamableHttp {
        url: url.to_string(),
        headers: string_map(object.get("headers"), "headers")?,
    })
}

fn parse_connect_timeout(value: Option<&Value>) -> Result<Duration, String> {
    let Some(value) = value else {
        return Ok(DEFAULT_CONNECT_TIMEOUT);
    };
    let milliseconds = value
        .as_u64()
        .filter(|milliseconds| *milliseconds > 0)
        .ok_or_else(|| "field \"connect_timeout_ms\" must be a positive integer".to_string())?;
    let timeout = Duration::from_millis(milliseconds);
    if timeout > MAX_CONNECT_TIMEOUT {
        return Err(format!(
            "field \"connect_timeout_ms\" must not exceed {}",
            MAX_CONNECT_TIMEOUT.as_millis()
        ));
    }
    Ok(timeout)
}

fn push_mcp_server(server: McpServerConfig, discovery: &mut Discovery) {
    if discovery.mcp_servers.len() >= MAX_DISCOVERED_SERVERS {
        discovery.diagnostics.push(format!(
            "MCP server limit reached ({MAX_DISCOVERED_SERVERS}); remaining servers were skipped"
        ));
        return;
    }
    if discovery.mcp_servers.iter().any(|existing| {
        existing.plugin_name == server.plugin_name && existing.server_name == server.server_name
    }) {
        discovery.diagnostics.push(format!(
            "duplicate MCP server {}/{} was skipped",
            server.plugin_name, server.server_name
        ));
        return;
    }
    discovery.mcp_servers.push(server);
}
