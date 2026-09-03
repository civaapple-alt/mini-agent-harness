use super::*;
use crate::test_support::{python_command, remove_test_root, test_root};
use crate::workspace::ApprovalMode;
use mini_agent_protocol::ToolExecutionStatus;
use serde_json::json;
use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::io::Write;
use std::net::TcpListener;
use std::time::Duration;

#[test]
fn loads_and_calls_stdio_server_through_rmcp() {
    let root = test_root();
    let script = root.join("server.py");
    fs::write(&script, include_str!("../testdata/mcp_stdio_server.py")).unwrap();
    fs::create_dir(root.join(".agents")).unwrap();
    let python = python_command();
    let config = McpServerConfig {
        plugin_name: "fixture.tools".to_string(),
        server_name: "fixture".to_string(),
        workspace_root: root.canonicalize().unwrap(),
        plugin_root: root.canonicalize().unwrap(),
        plugin_data: root
            .canonicalize()
            .unwrap()
            .join(".agents/plugin-data/fixture.tools"),
        connect_timeout: Duration::from_secs(20),
        transport: McpTransportConfig::Stdio {
            command: python,
            args: vec![script.to_string_lossy().into_owned()],
            env: BTreeMap::new(),
            cwd: None,
        },
    };

    let approval = ApprovalController::with_callback(ApprovalMode::Automatic, |_| Ok(false));
    let mut loaded = load(&[config], approval.clone());

    assert!(loaded.diagnostics.is_empty(), "{:?}", loaded.diagnostics);
    assert_eq!(loaded.tools.len(), 1);
    assert_eq!(
        loaded
            .loaded_servers
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["fixture.tools/fixture"]
    );
    let tool = loaded.tools.pop().unwrap();
    assert_eq!(tool.spec().name, "mcp__fixture_tools_fixture__echo");
    let output = tool.execute(&serde_json::json!({"text": "hello"})).unwrap();
    let value: Value = serde_json::from_str(&output).unwrap();
    assert_eq!(value["content"][0]["text"], "echo:hello");
    let timed_out = tool.execute_outcome(&json!({"text": "slow"}));
    assert_eq!(timed_out.status, ToolExecutionStatus::Failed);
    assert_eq!(timed_out.content, "MCP tool call timed out");
    approval.set_mode(ApprovalMode::Interactive);
    let denied = tool.execute_outcome(&json!({"text": "blocked"}));
    assert_eq!(denied.status, ToolExecutionStatus::Failed);
    assert_eq!(
        denied.content,
        "user denied: call MCP tool \"echo\" on fixture.tools/fixture"
    );
    drop(tool);
    remove_test_root(&root);
}

#[test]
fn loads_and_calls_streamable_http_server_with_expanded_headers() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || serve_http_mcp(listener));
    let root = test_root();
    fs::create_dir(root.join(".agents")).unwrap();
    let config = McpServerConfig {
        plugin_name: "fixture.http".to_string(),
        server_name: "fixture".to_string(),
        workspace_root: root.canonicalize().unwrap(),
        plugin_root: root.canonicalize().unwrap(),
        plugin_data: root
            .canonicalize()
            .unwrap()
            .join(".agents/plugin-data/fixture.http"),
        connect_timeout: Duration::from_secs(20),
        transport: McpTransportConfig::StreamableHttp {
            url: format!("http://{address}/mcp"),
            headers: BTreeMap::from([(
                "x-mini-agent-test".to_string(),
                "${MINI_AGENT_UNSET_TEST_HEADER:-present}".to_string(),
            )]),
        },
    };

    let mut loaded = load(&[config], ApprovalController::new(ApprovalMode::Automatic));

    assert!(loaded.diagnostics.is_empty(), "{:?}", loaded.diagnostics);
    assert_eq!(loaded.tools.len(), 1);
    assert_eq!(
        loaded
            .loaded_servers
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["fixture.http/fixture"]
    );
    let tool = loaded.tools.pop().unwrap();
    assert_eq!(tool.spec().name, "mcp__fixture_http_fixture__echo");
    let output = tool.execute(&serde_json::json!({"text": "hello"})).unwrap();
    let value: Value = serde_json::from_str(&output).unwrap();
    assert_eq!(value["content"][0]["text"], "echo:hello");
    assert!(server.join().unwrap());
    drop(tool);
    remove_test_root(&root);
}

#[test]
fn approval_denial_prevents_server_start_and_data_creation() {
    let root = test_root();
    fs::create_dir(root.join(".agents")).unwrap();
    let plugin_data = root
        .canonicalize()
        .unwrap()
        .join(".agents/plugin-data/fixture.tools");
    let config = McpServerConfig {
        plugin_name: "fixture.tools".to_string(),
        server_name: "fixture".to_string(),
        workspace_root: root.canonicalize().unwrap(),
        plugin_root: root.canonicalize().unwrap(),
        plugin_data: plugin_data.clone(),
        connect_timeout: Duration::from_secs(20),
        transport: McpTransportConfig::Stdio {
            command: "must-not-run".to_string(),
            args: Vec::new(),
            env: BTreeMap::new(),
            cwd: None,
        },
    };
    let approval = ApprovalController::with_callback(ApprovalMode::Interactive, |_| Ok(false));

    let loaded = load(&[config], approval);

    assert!(loaded.tools.is_empty());
    assert!(loaded.loaded_servers.is_empty());
    assert_eq!(
        loaded.diagnostics,
        vec![
            "MCP server fixture.tools/fixture was not started: user denied: connect MCP server \"fixture.tools/fixture\""
        ]
    );
    assert!(!plugin_data.exists());
    fs::remove_dir_all(root).unwrap();
}

fn serve_http_mcp(listener: TcpListener) -> bool {
    let mut saw_header = false;
    loop {
        let (mut stream, _) = listener.accept().unwrap();
        let (headers, body) = read_http_request(&mut stream);
        saw_header |= headers
            .lines()
            .any(|line| line.eq_ignore_ascii_case("x-mini-agent-test: present"));
        let request: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
        let method = request.get("method").and_then(Value::as_str);
        let Some(id) = request.get("id") else {
            write!(
                stream,
                "HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            )
            .unwrap();
            stream.flush().unwrap();
            continue;
        };
        let result = match method {
            Some("initialize") => json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "fixture", "version": "1.0.0"}
            }),
            Some("tools/list") => json!({
                "resultType": "complete",
                "tools": [{
                    "name": "echo",
                    "description": "Echo text",
                    "inputSchema": {
                        "type": "object",
                        "properties": {"text": {"type": "string"}},
                        "required": ["text"]
                    }
                }]
            }),
            Some("tools/call") => json!({
                "resultType": "complete",
                "content": [{
                    "type": "text",
                    "text": format!(
                        "echo:{}",
                        request["params"]["arguments"]["text"].as_str().unwrap_or("")
                    )
                }],
                "isError": false
            }),
            _ => continue,
        };
        let response = serde_json::to_vec(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": result
        }))
        .unwrap();
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            response.len()
        )
        .unwrap();
        stream.write_all(&response).unwrap();
        stream.flush().unwrap();
        if method == Some("tools/call") {
            return saw_header;
        }
    }
}

fn read_http_request(stream: &mut std::net::TcpStream) -> (String, Vec<u8>) {
    let mut received = Vec::new();
    let mut buffer = [0_u8; 4096];
    let header_end = loop {
        let count = stream.read(&mut buffer).unwrap();
        assert!(count > 0, "connection ended before HTTP headers");
        received.extend_from_slice(&buffer[..count]);
        if let Some(index) = received.windows(4).position(|window| window == b"\r\n\r\n") {
            break index + 4;
        }
    };
    let headers = String::from_utf8(received[..header_end].to_vec()).unwrap();
    let content_length = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().unwrap())
        })
        .unwrap_or(0);
    while received.len() - header_end < content_length {
        let count = stream.read(&mut buffer).unwrap();
        assert!(count > 0, "connection ended before HTTP body");
        received.extend_from_slice(&buffer[..count]);
    }
    (
        headers,
        received[header_end..header_end + content_length].to_vec(),
    )
}

#[test]
fn circuit_breaker_trips_after_failures_and_recovers() {
    let mut cb = CircuitBreaker::default();
    assert!(cb.can_execute().is_ok());

    cb.record_failure();
    assert!(cb.can_execute().is_ok());

    cb.record_failure();
    assert!(cb.can_execute().is_ok());

    cb.record_failure();
    // 3rd failure trips the breaker
    assert!(cb.can_execute().is_err());
    let err = cb.can_execute().unwrap_err();
    assert!(err.contains("circuit breaker is open"));

    // When cooldown expires (timestamp in the past), it allows a probe
    cb.tripped_until = Some(tokio::time::Instant::now() - Duration::from_millis(10));
    assert!(cb.can_execute().is_ok());

    // Successful probe resets consecutive failures
    cb.record_success();
    assert!(cb.can_execute().is_ok());
    assert_eq!(cb.consecutive_failures, 0);
}
