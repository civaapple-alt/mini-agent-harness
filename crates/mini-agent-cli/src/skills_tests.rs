use super::*;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

#[test]
fn discovers_project_and_plugin_skills_with_progressive_disclosure() {
    let root = test_root();
    write_skill(
        &root.join(".agents/skills/review"),
        "review",
        "Review Rust changes when the user asks for review.",
        "PROJECT BODY MUST LOAD ON DEMAND",
    );
    let plugin = root.join(".agents/plugins/deploy");
    write_plugin_manifest(&plugin, "deploy.tools");
    write_skill(
        &plugin.join("skills/deploy"),
        "deploy",
        "Deploy services when a release is requested.",
        "PLUGIN BODY MUST LOAD ON DEMAND",
    );

    let discovery = discover(&root);
    let prompt = discovery.augment_system_prompt("base").unwrap();

    assert_eq!(discovery.len(), 2);
    assert!(discovery.diagnostics().is_empty());
    assert!(prompt.contains(".agents/skills/review/SKILL.md"));
    assert!(prompt.contains(".agents/plugins/deploy/skills/deploy/SKILL.md"));
    assert!(prompt.contains("Review Rust changes"));
    assert!(!prompt.contains("MUST LOAD ON DEMAND"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn discovers_stdio_mcp_and_isolates_unsupported_transport() {
    let root = test_root();
    let plugin = root.join(".agents/plugins/tools");
    write_plugin_manifest(&plugin, "example.tools");
    fs::write(
        plugin.join("mcp.json"),
        serde_json::to_vec_pretty(&json!({
            "$schema": MCP_SCHEMA,
            "mcpServers": {
                "local": {
                    "type": "stdio",
                    "command": "example-server",
                    "args": ["--root", "${PLUGIN_ROOT}"],
                    "env": {"DATA": "${PLUGIN_DATA}/cache"},
                    "cwd": "${PLUGIN_ROOT}"
                },
                "remote": {
                    "type": "streamable-http",
                    "url": "https://example.com/mcp"
                }
            }
        }))
        .unwrap(),
    )
    .unwrap();

    let discovery = discover(&root);

    assert_eq!(discovery.mcp_server_count(), 1);
    assert_eq!(
        discovery.mcp_servers()[0],
        McpServerConfig {
            plugin_name: "example.tools".to_string(),
            server_name: "local".to_string(),
            workspace_root: root.canonicalize().unwrap(),
            plugin_root: plugin.canonicalize().unwrap(),
            plugin_data: root
                .canonicalize()
                .unwrap()
                .join(".agents/plugin-data/example.tools"),
            command: "example-server".to_string(),
            args: vec!["--root".to_string(), "${PLUGIN_ROOT}".to_string()],
            env: BTreeMap::from([("DATA".to_string(), "${PLUGIN_DATA}/cache".to_string(),)]),
            cwd: Some("${PLUGIN_ROOT}".to_string()),
        }
    );
    assert!(
        discovery
            .diagnostics()
            .iter()
            .any(|message| message.contains("streamable-http"))
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn direct_project_skill_overrides_plugin_skill() {
    let root = test_root();
    let plugin = root.join(".agents/plugins/review");
    write_plugin_manifest(&plugin, "review.tools");
    write_skill(
        &plugin.join("skills/review"),
        "review",
        "Plugin review instructions.",
        "plugin",
    );
    write_skill(
        &root.join(".agents/skills/review"),
        "review",
        "Project review instructions.",
        "project",
    );

    let discovery = discover(&root);
    let prompt = discovery.augment_system_prompt("base").unwrap();

    assert_eq!(discovery.len(), 1);
    assert!(prompt.contains("Project review instructions"));
    assert!(!prompt.contains("Plugin review instructions"));
    assert!(
        discovery
            .diagnostics()
            .iter()
            .any(|message| message.contains("shadowed"))
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn invalid_skill_does_not_hide_valid_sibling() {
    let root = test_root();
    write_skill(
        &root.join(".agents/skills/valid"),
        "valid",
        "Valid instructions.",
        "valid",
    );
    write_skill(
        &root.join(".agents/skills/broken"),
        "wrong-name",
        "Broken instructions.",
        "broken",
    );

    let discovery = discover(&root);

    assert_eq!(discovery.len(), 1);
    assert!(
        discovery
            .diagnostics()
            .iter()
            .any(|message| message.contains("wrong-name"))
    );
    fs::remove_dir_all(root).unwrap();
}

fn write_plugin_manifest(root: &Path, name: &str) {
    fs::create_dir_all(root).unwrap();
    fs::write(
        root.join("plugin.json"),
        serde_json::to_vec_pretty(&json!({
            "$schema": PLUGIN_SCHEMA,
            "name": name,
        }))
        .unwrap(),
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

fn test_root() -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("mini-agent-skills-{nonce}"));
    fs::create_dir(&root).unwrap();
    root
}
