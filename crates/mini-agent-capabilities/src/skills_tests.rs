use super::*;
use crate::workspace::ApprovalMode;
use std::process::Command;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

#[test]
fn discovers_project_plugin_and_mcp_metadata_without_loading_bodies() {
    let root = test_root();
    write_skill(
        &root.join(".agents/skills/review"),
        "review",
        "Review Rust changes.",
        "PROJECT BODY MUST LOAD ON DEMAND",
    );
    let plugin = root.join(".agents/plugins/deploy");
    write_plugin_manifest(&plugin, "deploy.tools");
    write_skill(
        &plugin.join("skills/deploy"),
        "deploy",
        "Deploy services.",
        "PLUGIN BODY MUST LOAD ON DEMAND",
    );
    fs::write(
        plugin.join("mcp.json"),
        serde_json::to_vec(&json!({
            "$schema": MCP_SCHEMA,
            "mcpServers": {"local": {"type": "stdio", "command": "example-server"}}
        }))
        .unwrap(),
    )
    .unwrap();

    let discovery = discover(&root);
    let prompt = discovery.augment_system_prompt("base").unwrap();

    assert_eq!(discovery.len(), 2);
    assert_eq!(discovery.plugin_count(), 1);
    assert_eq!(discovery.mcp_server_labels(), ["deploy.tools/local"]);
    assert!(prompt.contains(".agents/skills/review/SKILL.md"));
    assert!(prompt.contains(".agents/plugins/deploy/skills/deploy/SKILL.md"));
    assert!(!prompt.contains("MUST LOAD ON DEMAND"));
    assert_eq!(discovery.prompt_fingerprint().unwrap().unwrap().len(), 16);
    assert!(discovery.diagnostics().is_empty());
    remove_test_root(&root);
}

#[test]
fn selected_extensions_keep_named_entries_and_report_missing_names() {
    let root = test_root();
    write_skill(
        &root.join(".agents/skills/keep"),
        "keep",
        "Keep this extension.",
        "KEEP BODY",
    );
    write_skill(
        &root.join(".agents/skills/drop"),
        "drop",
        "Drop this extension.",
        "DROP BODY",
    );

    let mut discovery = discover(&root);
    discovery.retain_selected(&["keep".to_string(), "missing".to_string()]);

    assert_eq!(discovery.skill_names(), ["keep"]);
    assert!(discovery.prompt_fingerprint().unwrap().is_some());
    assert!(
        discovery
            .diagnostics()
            .iter()
            .any(|message| { message.contains("selected extension \"missing\" was not found") })
    );
    remove_test_root(&root);
}

#[test]
fn discovers_and_selects_bounded_mcp_transports() {
    let root = test_root();
    let plugin = root.join(".agents/plugins/tools");
    write_plugin_manifest(&plugin, "example.tools");
    let script = root.join("server.py");
    fs::write(
        &script,
        r#"import json
import sys
for line in sys.stdin:
    request = json.loads(line)
    if request.get("method") == "initialize":
        result = {"protocolVersion": "2025-06-18", "capabilities": {"tools": {}}, "serverInfo": {"name": "fixture", "version": "1.0.0"}}
    elif request.get("method") == "tools/list":
        result = {"resultType": "complete", "tools": []}
    else:
        continue
    print(json.dumps({"jsonrpc": "2.0", "id": request["id"], "result": result}), flush=True)
"#,
    )
    .unwrap();
    fs::write(
        plugin.join("mcp.json"),
        serde_json::to_vec(&json!({
            "$schema": MCP_SCHEMA,
            "mcpServers": {
                "keep": {"type": "stdio", "command": python_command(), "args": [script.to_string_lossy()]},
                "drop": {"type": "stdio", "command": "mini-agent-command-must-not-run"},
                "remote": {"type": "streamable-http", "url": "https://example.com/mcp"}
            }
        }))
        .unwrap(),
    )
    .unwrap();

    let mut discovery = discover(&root);
    assert_eq!(discovery.stdio_mcp_server_count(), 2);
    assert_eq!(discovery.http_mcp_server_count(), 1);
    discovery.retain_selected(&["keep".to_string()]);
    let loaded = crate::mcp::load(
        discovery.mcp_servers(),
        crate::workspace::ApprovalController::new(ApprovalMode::Automatic),
    );
    assert_eq!(discovery.mcp_server_labels(), ["example.tools/keep"]);
    assert!(loaded.diagnostics.is_empty(), "{:?}", loaded.diagnostics);
    assert_eq!(
        loaded.loaded_servers,
        std::collections::BTreeSet::from(["example.tools/keep".to_string()])
    );
    remove_test_root(&root);
}

#[test]
fn discovers_legacy_plugin_agents_and_standalone_mcp() {
    let root = test_root();
    let plugin = root.join(".agents/plugins/code-simplifier");
    fs::create_dir_all(plugin.join(".claude-plugin")).unwrap();
    fs::write(
        plugin.join(".claude-plugin/plugin.json"),
        serde_json::to_vec(&json!({"name": "code-simplifier"})).unwrap(),
    )
    .unwrap();
    fs::create_dir_all(plugin.join("agents")).unwrap();
    fs::write(
        plugin.join("agents/code-simplifier.md"),
        "---\nname: code-simplifier\ndescription: Simplify changed code.\n---\nbody\n",
    )
    .unwrap();
    fs::write(
        plugin.join(".mcp.json"),
        serde_json::to_vec(&json!({"formatter": {"command": "bun"}})).unwrap(),
    )
    .unwrap();
    fs::create_dir_all(root.join(".agents/mcp")).unwrap();
    fs::write(
        root.join(".agents/mcp/context7.json"),
        serde_json::to_vec(&json!({
            "name": "context7", "transport": "stdio", "enabled": true, "command": "npx"
        }))
        .unwrap(),
    )
    .unwrap();

    let discovery = discover(&root);
    let prompt = discovery.augment_system_prompt("base").unwrap();

    assert_eq!(discovery.len(), 1);
    assert_eq!(discovery.plugin_count(), 1);
    assert_eq!(discovery.mcp_server_count(), 2);
    assert!(prompt.contains("plugin-agent"));
    assert!(prompt.contains("agents/code-simplifier.md"));
    assert!(
        discovery.diagnostics().is_empty(),
        "{:?}",
        discovery.diagnostics()
    );
    remove_test_root(&root);
}

#[test]
fn project_skill_overrides_invalid_or_plugin_duplicate() {
    let root = test_root();
    let plugin = root.join(".agents/plugins/review");
    write_plugin_manifest(&plugin, "review.tools");
    write_skill(
        &plugin.join("skills/review"),
        "review",
        "Plugin review.",
        "plugin",
    );
    write_skill(
        &root.join(".agents/skills/review"),
        "review",
        "Project review.",
        "project",
    );
    write_skill(
        &root.join(".agents/skills/broken"),
        "wrong-name",
        "Broken.",
        "broken",
    );

    let discovery = discover(&root);
    let prompt = discovery.augment_system_prompt("base").unwrap();

    assert_eq!(discovery.len(), 1);
    assert!(prompt.contains("Project review"));
    assert!(!prompt.contains("Plugin review"));
    assert!(
        discovery
            .diagnostics()
            .iter()
            .any(|message| message.contains("shadowed"))
    );
    assert!(
        discovery
            .diagnostics()
            .iter()
            .any(|message| message.contains("wrong-name"))
    );
    remove_test_root(&root);
}

fn write_plugin_manifest(root: &Path, name: &str) {
    fs::create_dir_all(root).unwrap();
    fs::write(
        root.join("plugin.json"),
        serde_json::to_vec(&json!({"$schema": PLUGIN_SCHEMA, "name": name})).unwrap(),
    )
    .unwrap();
}

fn write_skill(root: &Path, name: &str, description: &str, body: &str) {
    fs::create_dir_all(root).unwrap();
    fs::write(
        root.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: {description}\n---\n{body}\n"),
    )
    .unwrap();
}

fn python_command() -> String {
    ["python3", "python"]
        .into_iter()
        .find(|command| {
            Command::new(command)
                .arg("--version")
                .output()
                .is_ok_and(|output| output.status.success())
        })
        .expect("Python 3 is required by the MCP fixture")
        .to_string()
}

fn remove_test_root(root: &Path) {
    for _ in 0..50 {
        match fs::remove_dir_all(root) {
            Ok(()) => return,
            Err(_) => std::thread::sleep(std::time::Duration::from_millis(20)),
        }
    }
    fs::remove_dir_all(root).unwrap();
}

fn test_root() -> PathBuf {
    static NEXT_ROOT: AtomicU64 = AtomicU64::new(0);
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let sequence = NEXT_ROOT.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!("mini-agent-skills-{nonce}-{sequence}"));
    fs::create_dir(&root).unwrap();
    root
}
