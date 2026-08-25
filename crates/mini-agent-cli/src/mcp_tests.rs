use super::*;
use crate::workspace::ApprovalMode;
use std::collections::BTreeMap;
use std::fs;
use std::process::Command as StdCommand;
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
    if method == "server/discover":
        result = {
            "resultType": "complete",
            "supportedVersions": ["2026-07-28"],
            "capabilities": {"tools": {}},
            "ttlMs": 0,
            "cacheScope": "private",
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
        command: python,
        args: vec![script.to_string_lossy().into_owned()],
        env: BTreeMap::new(),
        cwd: None,
    };

    let mut loaded = load(&[config], ApprovalController::new(ApprovalMode::Automatic));

    assert!(loaded.diagnostics.is_empty(), "{:?}", loaded.diagnostics);
    assert_eq!(loaded.tools.len(), 1);
    let tool = loaded.tools.pop().unwrap();
    assert_eq!(tool.spec().name, "mcp__fixture_tools_fixture__echo");
    let output = tool.execute(&serde_json::json!({"text": "hello"})).unwrap();
    let value: Value = serde_json::from_str(&output).unwrap();
    assert_eq!(value["content"][0]["text"], "echo:hello");
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
        command: "must-not-run".to_string(),
        args: Vec::new(),
        env: BTreeMap::new(),
        cwd: None,
    };
    let approval = ApprovalController::with_callback(ApprovalMode::Interactive, |_| Ok(false));

    let loaded = load(&[config], approval);

    assert!(loaded.tools.is_empty());
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
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("mini-agent-mcp-{nonce}"));
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
