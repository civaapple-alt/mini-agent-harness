use super::*;
use crate::test_support::{python_command, remove_test_root, test_root};
use crate::workspace::ApprovalMode;

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

    assert_eq!(discovery.mcp_server_labels(), ["deploy.tools/local"]);
    assert!(prompt.contains(".agents/skills/review/SKILL.md"));
    assert!(prompt.contains(".agents/plugins/deploy/skills/deploy/SKILL.md"));
    assert!(!prompt.contains("MUST LOAD ON DEMAND"));
    assert_eq!(discovery.prompt_fingerprint().unwrap().unwrap().len(), 16);
    assert!(discovery.diagnostics().is_empty());
    remove_test_root(&root);
}

#[test]
fn activates_typed_skill_dependencies_without_enabling_providers() {
    let root = test_root();
    write_skill_with_dependencies(
        &root.join(".agents/skills/review"),
        "review",
        "Review Rust changes.",
        "  tools:\n    - type: builtin\n      value: read_file\n    - type: mcp\n      value: github\n",
    );

    let discovery = discover(&root);
    assert_eq!(
        discovery.skill_names(),
        ["review"],
        "{:?}",
        discovery.diagnostics()
    );
    let activation = discovery.activate_skill("review").unwrap();
    assert_eq!(
        activation,
        SkillActivation {
            name: "review".to_string(),
            location: ".agents/skills/review/SKILL.md".to_string(),
            dependencies: vec![
                SkillDependency::BuiltinTool("read_file".to_string()),
                SkillDependency::McpServer("github".to_string()),
            ],
        }
    );
    let prompt = discovery.augment_system_prompt("base").unwrap();
    assert!(prompt.contains("\"type\":\"builtin\""));
    assert!(prompt.contains("\"value\":\"github\""));
    assert!(discovery.mcp_servers().is_empty());
    remove_test_root(&root);
}

#[test]
fn rejects_unsupported_skill_dependency_types() {
    let root = test_root();
    write_skill_with_dependencies(
        &root.join(".agents/skills/review"),
        "review",
        "Review Rust changes.",
        "  tools:\n    - type: process\n      value: shell\n",
    );

    let discovery = discover(&root);
    assert!(
        !discovery.diagnostics().is_empty(),
        "{:?}",
        discovery.diagnostics()
    );
    assert!(discovery.skill_names().is_empty());
    assert!(
        discovery
            .diagnostics()
            .iter()
            .any(|message| message.contains("unsupported Skill dependency type"))
    );
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
fn selecting_plugin_retains_its_provider_inputs() {
    let root = test_root();
    let plugin = root.join(".agents/plugins/deploy");
    write_plugin_manifest(&plugin, "deploy.tools");
    fs::write(
        plugin.join("mcp.json"),
        serde_json::to_vec(&json!({
            "$schema": MCP_SCHEMA,
            "mcpServers": {
                "local": {"type": "stdio", "command": "example-server"},
                "remote": {"type": "streamable-http", "url": "https://example.com/mcp"}
            }
        }))
        .unwrap(),
    )
    .unwrap();

    let mut discovery = discover(&root);
    discovery.retain_selected(&["deploy.tools".to_string()]);

    assert_eq!(discovery.plugin_names(), ["deploy.tools"]);
    assert_eq!(
        discovery.mcp_server_labels(),
        ["deploy.tools/local", "deploy.tools/remote"]
    );
    assert!(
        discovery.diagnostics().is_empty(),
        "{:?}",
        discovery.diagnostics()
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
    write_skill_frontmatter(root, name, description, "", body);
}

fn write_skill_with_dependencies(root: &Path, name: &str, description: &str, dependencies: &str) {
    write_skill_frontmatter(root, name, description, dependencies, "SKILL BODY");
}

fn write_skill_frontmatter(
    root: &Path,
    name: &str,
    description: &str,
    dependencies: &str,
    body: &str,
) {
    fs::create_dir_all(root).unwrap();
    let dependency_block = if dependencies.is_empty() {
        String::new()
    } else {
        format!("dependencies:\n{dependencies}")
    };
    fs::write(
        root.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: {description}\n{dependency_block}---\n{body}\n"),
    )
    .unwrap();
}
