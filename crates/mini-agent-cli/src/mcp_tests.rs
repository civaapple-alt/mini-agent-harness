use super::*;
use crate::workspace::ApprovalMode;
use serde_json::json;
use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::io::Write;
use std::net::TcpListener;
use std::process::Command as StdCommand;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::time::Duration;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

#[test]
fn expands_plugin_placeholders_once() {
    assert_eq!(
        expand_placeholders(
            "${PLUGIN_ROOT}/a:${PLUGIN_DATA}/b:${UNKNOWN}",
            "root/${PLUGIN_DATA}",
            "data"
        ),
        "root/${PLUGIN_DATA}/a:data/b:${UNKNOWN}"
    );
}

#[test]
fn namespaces_and_bounds_exposed_tool_names() {
    assert_eq!(
        exposed_tool_name("plugin.name/server name", "read/resource"),
        Some("mcp__plugin_name_server_name__read_resource".to_string())
    );
    assert_eq!(exposed_tool_name("server", ""), None);
    assert_eq!(exposed_tool_name("server", &"x".repeat(64)), None);
}

#[test]
fn truncates_large_tool_results_as_valid_json() {
    let result = rmcp::model::CallToolResult::success(vec![rmcp::model::ContentBlock::text(
        "x".repeat(MAX_TOOL_RESULT_BYTES),
    )]);

    let body = bounded_result(&result).unwrap();
    let value: Value = serde_json::from_str(&body).unwrap();

    assert_eq!(value["truncated"], true);
    assert!(value["preview"].as_str().unwrap().len() <= MAX_TOOL_RESULT_BYTES);
}

#[test]
fn loads_and_calls_stdio_server_through_rmcp() {
    let root = test_root();
    let script = root.join("server.py");
    fs::write(
        &script,
        r#"import json
import sys

for line in sys.stdin:
    request = json.loads(line)
    method = request.get("method")
    if method == "initialize":
        result = {
            "protocolVersion": "2025-06-18",
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "fixture", "version": "1.0.0"},
        }
    elif method == "tools/list":
        result = {
            "resultType": "complete",
            "tools": [{
                "name": "echo",
                "description": "Echo text",
                "inputSchema": {
                    "type": "object",
                    "properties": {"text": {"type": "string"}},
                    "required": ["text"],
                },
            }],
        }
    elif method == "tools/call":
        text = request.get("params", {}).get("arguments", {}).get("text", "")
        result = {
            "resultType": "complete",
            "content": [{"type": "text", "text": "echo:" + text}],
            "isError": False,
        }
    else:
        continue
    response = {"jsonrpc": "2.0", "id": request["id"], "result": result}
    print(json.dumps(response), flush=True)
"#,
    )
    .unwrap();
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

    let mut loaded = load(&[config], ApprovalController::new(ApprovalMode::Automatic));

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
    assert!(
        loaded
            .diagnostics
            .iter()
            .any(|message| message.contains("user denied"))
    );
    assert!(!plugin_data.exists());
    fs::remove_dir_all(root).unwrap();
}

fn python_command() -> String {
    ["python3", "python"]
        .into_iter()
        .find(|command| {
            StdCommand::new(command)
                .arg("--version")
                .output()
                .is_ok_and(|output| output.status.success())
        })
        .expect("Python 3 is required by the repository verification scripts")
        .to_string()
}

fn test_root() -> PathBuf {
    static NEXT_ROOT: AtomicU64 = AtomicU64::new(0);
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let sequence = NEXT_ROOT.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!("mini-agent-mcp-{nonce}-{sequence}"));
    fs::create_dir(&root).unwrap();
    root
}

fn remove_test_root(root: &Path) {
    for _ in 0..50 {
        match fs::remove_dir_all(root) {
            Ok(()) => return,
            Err(_) => std::thread::sleep(Duration::from_millis(20)),
        }
    }
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
