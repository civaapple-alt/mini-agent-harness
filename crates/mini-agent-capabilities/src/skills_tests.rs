use super::*;
use crate::test_support::{approval_controller, python_command, remove_test_root, test_root};
use mini_agent_protocol::{ApprovalOutcome, ApprovalPolicy};

fn discover_for_tests(workspace: &Path, enabled_groups: &[String]) -> Discovery {
    super::discover_with_roots(workspace, enabled_groups, None, None)
}

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

    let discovery = discover_for_tests(&root, &[]);
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

    let discovery = discover_for_tests(&root, &[]);
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
            qualified_name: "review".to_string(),
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

    let discovery = discover_for_tests(&root, &[]);
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

    let mut discovery = discover_for_tests(&root, &[]);
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

    let mut discovery = discover_for_tests(&root, &[]);
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
    fs::write(&script, include_str!("../testdata/mcp_discovery_server.py")).unwrap();
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

    let mut discovery = discover_for_tests(&root, &[]);
    discovery.retain_selected(&["keep".to_string()]);
    let loaded = crate::mcp::load(
        discovery.mcp_servers(),
        approval_controller(ApprovalPolicy::Automatic, ApprovalOutcome::Approved),
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

    let discovery = discover_for_tests(&root, &[]);
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

#[test]
fn catalogs_and_loads_selected_skills_with_bounded_activation() {
    let root = test_root();
    write_skill(
        &root.join(".agents/skills/review"),
        "review",
        "Review Rust changes.",
        "REVIEW BODY",
    );

    let discovery = discover_for_tests(&root, &[]);
    let review = discovery
        .skill_catalog()
        .into_iter()
        .find(|skill| skill.name == "review")
        .expect("project Skill should be present in the bounded catalog");
    assert_eq!(
        review,
        SkillCatalogEntry {
            name: "review".to_string(),
            qualified_name: "review".to_string(),
            aliases: Vec::new(),
            description: "Review Rust changes.".to_string(),
            source: "project".to_string(),
            group: None,
            enabled: true,
        }
    );
    let loaded = discovery
        .load_skills(&["review".to_string(), "review".to_string()])
        .unwrap();
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].qualified_name, "review");
    assert_eq!(loaded[0].body, "REVIEW BODY\n");

    let too_many = (0..=MAX_SELECTED_SKILLS)
        .map(|index| format!("skill-{index}"))
        .collect::<Vec<_>>();
    assert!(discovery.load_skills(&too_many).is_err());
    remove_test_root(&root);
}

#[test]
fn skill_read_roots_include_only_enabled_discovered_skills() {
    let root = test_root();
    let enabled = root.join(".agents/skills/enabled");
    let disabled = root.join(".agents/skills/disabled");
    write_skill(&enabled, "enabled", "Enabled Skill.", "ENABLED BODY");
    write_skill(&disabled, "disabled", "Disabled Skill.", "DISABLED BODY");

    let mut discovery = discover_for_tests(&root, &[]);
    discovery.retain_selected(&["enabled".to_string()]);

    assert_eq!(
        discovery.skill_read_roots(),
        vec![enabled.canonicalize().unwrap()]
    );
    remove_test_root(&root);
}

#[test]
fn canonicalizes_skill_root_boundaries_before_containment_checks() {
    let root = test_root();
    let boundary = root.join("builtin");
    let child = boundary.join("pstack");
    fs::create_dir_all(&child).unwrap();

    let resolved = super::contained_directory(&child, &boundary.join("."));

    assert_eq!(resolved.unwrap(), child.canonicalize().unwrap());
    remove_test_root(&root);
}

#[test]
fn rejects_pstack_namespace_when_the_builtin_group_is_not_enabled() {
    let root = test_root();
    write_skill(
        &root.join(".agents/skills/how"),
        "how",
        "Explain the selected subsystem.",
        "HOW BODY",
    );
    let discovery = discover_for_tests(&root, &[]);
    let loaded = discovery.load_skills(&["how".to_string()]).unwrap();
    assert_eq!(loaded[0].qualified_name, "how");
    assert!(discovery.load_skills(&["pstack:how".to_string()]).is_err());
    assert!(
        discovery
            .load_skills(&["pstack-plugin:how".to_string()])
            .is_err()
    );
    remove_test_root(&root);
}

#[test]
fn loads_canonical_and_codex_aliases_once_for_a_pstack_skill() {
    let root = test_root();
    let path = root.join("how/SKILL.md");
    write_skill(
        &root.join("how"),
        "how",
        "Explain the selected subsystem.",
        "HOW BODY",
    );
    let discovery = Discovery {
        skills: vec![Skill {
            name: "how".to_string(),
            description: "Explain the selected subsystem.".to_string(),
            location: "how/SKILL.md".to_string(),
            source: "builtin".to_string(),
            group: Some("pstack".to_string()),
            enabled: true,
            path,
            dependencies: Vec::new(),
        }],
        ..Discovery::default()
    };

    let catalog = discovery.skill_catalog();
    assert_eq!(catalog[0].qualified_name, "pstack:how");
    assert_eq!(
        catalog[0].aliases,
        ["pstack-plugin:how".to_string(), "how".to_string()]
    );
    let loaded = discovery
        .load_skills(&[
            "pstack:how".to_string(),
            "pstack-plugin:how".to_string(),
            "how".to_string(),
        ])
        .unwrap();
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].qualified_name, "pstack:how");
    remove_test_root(&root);
}

#[test]
fn discovers_builtin_and_global_skill_roots_with_fixed_precedence() {
    let workspace = test_root();
    let home = test_root();
    write_skill(
        &home.join(".mini-agent/skills/builtin/pstack/how"),
        "how",
        "Explain pstack behavior.",
        "PSTACK HOW",
    );
    write_skill(
        &home.join(".mini-agent/skills/review"),
        "review",
        "Review from Mini Agent.",
        "MINI REVIEW",
    );
    write_skill(
        &home.join(".agents/skills/review"),
        "review",
        "Review from Agent Skills.",
        "AGENTS REVIEW",
    );
    write_skill(
        &workspace.join(".agents/skills/review"),
        "review",
        "Review from the project.",
        "PROJECT REVIEW",
    );

    let discovery = super::discover_with_roots(
        &workspace,
        &["pstack".to_string()],
        Some(home.join(".mini-agent/skills")),
        Some(home.join(".agents/skills")),
    );
    let catalog = discovery.skill_catalog();
    let review = catalog
        .iter()
        .find(|skill| skill.qualified_name == "review")
        .expect("the highest-priority review Skill should remain");
    assert_eq!(review.source, "project");
    assert!(
        discovery
            .diagnostics()
            .iter()
            .any(|item| item.contains("shadowed"))
    );
    assert!(
        discovery
            .augment_system_prompt("base")
            .unwrap()
            .contains(".mini-agent/skills/builtin/pstack/how/SKILL.md")
    );
    remove_test_root(&workspace);
    remove_test_root(&home);
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
