use super::*;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
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
fn discovers_stdio_and_http_mcp_transports() {
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

    assert_eq!(discovery.mcp_server_count(), 2);
    assert_eq!(discovery.stdio_mcp_server_count(), 1);
    assert_eq!(discovery.http_mcp_server_count(), 1);
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
            connect_timeout: DEFAULT_CONNECT_TIMEOUT,
            transport: McpTransportConfig::Stdio {
                command: "example-server".to_string(),
                args: vec!["--root".to_string(), "${PLUGIN_ROOT}".to_string()],
                env: BTreeMap::from([("DATA".to_string(), "${PLUGIN_DATA}/cache".to_string(),)]),
                cwd: Some("${PLUGIN_ROOT}".to_string()),
            },
        }
    );
    assert!(discovery.diagnostics().is_empty());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn discovers_skillsets_legacy_plugins_agents_and_standalone_mcp() {
    let root = test_root();
    write_skill(
        &root.join(".agents/skillsets/vercel/skills/react-best-practices"),
        "vercel-react-best-practices",
        "Review React code.",
        "collection",
    );
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
        "---\nname: code-simplifier\ndescription: Simplify recently changed code.\n---\nbody\n",
    )
    .unwrap();
    fs::write(
        plugin.join(".mcp.json"),
        serde_json::to_vec(&json!({
            "formatter": {
                "command": "bun",
                "args": ["run", "--cwd", "${CLAUDE_PLUGIN_ROOT}", "start"]
            }
        }))
        .unwrap(),
    )
    .unwrap();
    fs::create_dir_all(root.join(".agents/mcp")).unwrap();
    fs::write(
        root.join(".agents/mcp/context7.json"),
        serde_json::to_vec_pretty(&json!({
            "name": "context7",
            "transport": "stdio",
            "enabled": true,
            "command": "npx",
            "args": ["-y", "@upstash/context7-mcp"],
            "connect_timeout_ms": 60_000
        }))
        .unwrap(),
    )
    .unwrap();

    let discovery = discover(&root);
    let prompt = discovery.augment_system_prompt("base").unwrap();

    assert_eq!(discovery.len(), 2);
    assert_eq!(discovery.plugin_count(), 1);
    assert_eq!(discovery.mcp_server_count(), 2);
    assert_eq!(discovery.stdio_mcp_server_count(), 2);
    assert!(prompt.contains("vercel-react-best-practices"));
    assert!(prompt.contains("plugin-agent"));
    assert!(prompt.contains("agents/code-simplifier.md"));
    assert_eq!(
        discovery
            .mcp_servers()
            .iter()
            .find(|server| server.server_name == "context7")
            .unwrap()
            .connect_timeout,
        Duration::from_secs(60)
    );
    assert!(discovery.diagnostics().is_empty());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn configured_skillset_enables_only_listed_skills() {
    let root = test_root();
    write_skill(
        &root.join(".agents/skillsets/vercel/skills/keep-me"),
        "keep-me",
        "Keep this skill.",
        "keep",
    );
    write_skill(
        &root.join(".agents/skillsets/vercel/skills/skip-me"),
        "skip-me",
        "Skip this skill.",
        "skip",
    );
    fs::write(
        root.join(".agents/skillsets.json"),
        serde_json::to_vec(&json!({
            "skillsets": {
                "vercel": ["keep-me"]
            }
        }))
        .unwrap(),
    )
    .unwrap();

    let discovery = discover(&root);

    assert_eq!(discovery.skill_names(), vec!["keep-me".to_string()]);
    assert!(
        discovery.diagnostics().is_empty(),
        "{:?}",
        discovery.diagnostics()
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn configured_skillset_path_uses_an_external_collection() {
    let root = test_root();
    let outside = test_root();
    write_skill(
        &outside.join("skills/frontend-design"),
        "frontend-design",
        "Frontend taste.",
        "body",
    );
    write_skill(
        &outside.join("skills/skill-creator"),
        "skill-creator",
        "Create skills.",
        "body",
    );
    let outside_path = outside.canonicalize().unwrap();
    fs::create_dir_all(root.join(".agents")).unwrap();
    fs::write(
        root.join(".agents/skillsets.json"),
        serde_json::to_vec(&json!({
            "skillsets": {
                "anthropics-skills": {
                    "path": outside_path,
                    "skills": ["frontend-design"]
                }
            }
        }))
        .unwrap(),
    )
    .unwrap();

    let discovery = discover(&root);

    assert_eq!(discovery.skill_names(), vec!["frontend-design".to_string()]);
    assert_eq!(discovery.extra_read_roots(), &[outside_path]);
    assert!(
        discovery.diagnostics().is_empty(),
        "{:?}",
        discovery.diagnostics()
    );
    fs::remove_dir_all(outside).unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn discovers_claude_and_grok_marketplace_plugins_selected_explicitly() {
    let root = test_root();
    let claude = root.join(".agents/marketplaces/anthropic");
    fs::create_dir_all(claude.join(".claude-plugin")).unwrap();
    write_skill(
        &claude.join("skills/pdf"),
        "pdf",
        "Work with PDF files.",
        "pdf",
    );
    fs::write(
        claude.join(".claude-plugin/marketplace.json"),
        serde_json::to_vec_pretty(&json!({
            "name": "anthropic-agent-skills",
            "plugins": [{
                "name": "document-skills",
                "source": "./",
                "strict": false,
                "skills": ["./skills/pdf"]
            }]
        }))
        .unwrap(),
    )
    .unwrap();
    let grok = root.join(".agents/marketplaces/xai/external_plugins/neon");
    fs::create_dir_all(grok.join(".grok-plugin")).unwrap();
    fs::create_dir_all(root.join(".agents/marketplaces/xai/.grok-plugin")).unwrap();
    fs::write(
        grok.join(".grok-plugin/plugin.json"),
        serde_json::to_vec(&json!({"name": "neon"})).unwrap(),
    )
    .unwrap();
    write_skill(
        &grok.join("skills/neon"),
        "neon",
        "Work with Neon Postgres.",
        "neon",
    );
    fs::write(
        grok.join(".mcp.json"),
        serde_json::to_vec(&json!({
            "mcpServers": {"neon": {"type": "http", "url": "https://mcp.neon.tech/mcp"}}
        }))
        .unwrap(),
    )
    .unwrap();
    fs::write(
        root.join(".agents/marketplaces/xai/.grok-plugin/marketplace.json"),
        serde_json::to_vec(&json!({
            "name": "xai-official",
            "plugins": [{
                "name": "neon",
                "source": {"type": "local", "path": "./external_plugins/neon"}
            }]
        }))
        .unwrap(),
    )
    .unwrap();
    fs::write(
        root.join(".agents/marketplaces.json"),
        serde_json::to_vec(&json!({
            "marketplaces": {
                "anthropic": ["document-skills"],
                "xai": ["neon"]
            }
        }))
        .unwrap(),
    )
    .unwrap();

    let discovery = discover(&root);

    assert_eq!(discovery.len(), 2);
    assert_eq!(discovery.marketplace_count(), 2);
    assert_eq!(discovery.plugin_count(), 2);
    assert_eq!(discovery.http_mcp_server_count(), 1);
    assert!(
        discovery.diagnostics().is_empty(),
        "{:?}",
        discovery.diagnostics()
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn discovers_an_oversized_skill_from_frontmatter() {
    let root = test_root();
    let skill = root.join(".agents/skills/huge-skill");
    write_skill(
        &skill,
        "huge-skill",
        "A long published skill.",
        &"x".repeat(80_000),
    );

    let discovery = discover(&root);

    assert_eq!(discovery.skill_names(), vec!["huge-skill".to_string()]);
    assert!(
        discovery.diagnostics().is_empty(),
        "{:?}",
        discovery.diagnostics()
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn discovers_an_explicit_nested_marketplace_skill() {
    let root = test_root();
    let skill = root.join(
        ".agents/marketplaces/cursor-plugins/cursor-team-kit/skills/thermo-nuclear-code-quality-review",
    );
    write_skill(
        &skill,
        "thermo-nuclear-code-quality-review",
        "Run a harsh maintainability review.",
        "body",
    );
    fs::write(
        root.join(".agents/marketplaces.json"),
        serde_json::to_vec(&json!({
            "marketplaces": {
                "cursor-plugins": {
                    "skills": ["thermo-nuclear-code-quality-review"]
                }
            }
        }))
        .unwrap(),
    )
    .unwrap();

    let discovery = discover(&root);

    assert_eq!(discovery.skill_count(), 1);
    assert_eq!(discovery.marketplace_count(), 1);
    assert_eq!(discovery.plugin_count(), 0);
    assert_eq!(
        discovery.skill_names(),
        vec!["thermo-nuclear-code-quality-review".to_string()]
    );
    assert!(
        discovery.diagnostics().is_empty(),
        "{:?}",
        discovery.diagnostics()
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
