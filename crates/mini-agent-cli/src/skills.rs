use crate::marketplaces;
use serde::Deserialize;
use serde_json::Map;
use serde_json::Value;
use serde_json::json;
use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

const PLUGIN_SCHEMA: &str = "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json";
const MCP_SCHEMA: &str = "https://agent-plugins.org/schemas/1.0.0/mcp.schema.json";
const MAX_DISCOVERED_SKILLS: usize = 64;
const MAX_DISCOVERED_SERVERS: usize = 8;
const MAX_DIRECTORY_ENTRIES: usize = 128;
const MAX_METADATA_BYTES: u64 = 64 * 1024;
const MAX_CATALOG_BYTES: usize = 16 * 1024;
const MAX_SKILLSETS: usize = 16;
const MAX_SKILLS_PER_SKILLSET: usize = 32;
const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_CONNECT_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum McpTransportConfig {
    Stdio {
        command: String,
        args: Vec<String>,
        env: BTreeMap<String, String>,
        cwd: Option<String>,
    },
    StreamableHttp {
        url: String,
        headers: BTreeMap<String, String>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct McpServerConfig {
    pub plugin_name: String,
    pub server_name: String,
    pub workspace_root: PathBuf,
    pub plugin_root: PathBuf,
    pub plugin_data: PathBuf,
    pub connect_timeout: Duration,
    pub transport: McpTransportConfig,
}

#[derive(Debug, Default)]
pub struct Discovery {
    skills: Vec<Skill>,
    mcp_servers: Vec<McpServerConfig>,
    plugins: Vec<String>,
    extra_read_roots: Vec<PathBuf>,
    marketplaces: usize,
    diagnostics: Vec<String>,
}

#[derive(Clone, Debug)]
struct Skill {
    name: String,
    description: String,
    location: String,
    source: String,
    kind: &'static str,
}

#[derive(Deserialize)]
struct SkillMetadata {
    name: String,
    description: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SkillsetFile {
    skillsets: BTreeMap<String, SkillsetSelection>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum SkillsetSelection {
    Skills(Vec<String>),
    Explicit(ExplicitSkillset),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExplicitSkillset {
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    skills: Vec<String>,
}

#[derive(Clone, Copy)]
enum InstructionNameRule {
    ParentDirectory,
    FileStem,
    Compatible,
}

#[derive(Clone, Copy)]
enum SkillDiscoveryPolicy {
    Direct,
    Strict,
    Compatible,
}

#[derive(Clone, Copy)]
enum PluginAccounting {
    Count,
    Skip,
}

struct PluginLoadOptions<'a> {
    expected_name: Option<&'a str>,
    explicit_skills: Option<&'a [PathBuf]>,
    accounting: PluginAccounting,
    source: &'a str,
}

pub fn discover(workspace: &Path) -> Discovery {
    let workspace = match workspace.canonicalize() {
        Ok(workspace) => workspace,
        Err(error) => {
            return Discovery {
                diagnostics: vec![format!("cannot resolve workspace for extensions: {error}")],
                ..Discovery::default()
            };
        }
    };
    let mut skills = BTreeMap::new();
    let mut discovery = Discovery::default();
    discover_skill_root(
        &workspace.join(".agents/skills"),
        &workspace,
        &workspace,
        "project",
        SkillDiscoveryPolicy::Direct,
        &mut skills,
        &mut discovery.diagnostics,
    );
    discover_skillsets(&workspace, &mut skills, &mut discovery);
    discover_plugins(&workspace, &mut skills, &mut discovery);
    discover_marketplaces(&workspace, &mut skills, &mut discovery);
    discover_project_mcp(&workspace, &mut discovery);
    discovery.skills = bounded_catalog(skills.into_values(), &mut discovery.diagnostics);
    discovery
}

impl Discovery {
    pub fn len(&self) -> usize {
        self.skills.len()
    }

    pub fn skill_count(&self) -> usize {
        self.skills
            .iter()
            .filter(|skill| skill.kind == "skill")
            .count()
    }

    pub fn plugin_agent_count(&self) -> usize {
        self.skills
            .iter()
            .filter(|skill| skill.kind == "plugin-agent")
            .count()
    }

    pub fn mcp_servers(&self) -> &[McpServerConfig] {
        &self.mcp_servers
    }

    pub fn mcp_server_count(&self) -> usize {
        self.mcp_servers.len()
    }

    pub fn plugin_count(&self) -> usize {
        self.plugins.len()
    }

    pub fn skill_names(&self) -> Vec<String> {
        self.skills
            .iter()
            .filter(|skill| skill.kind == "skill")
            .map(|skill| skill.name.clone())
            .collect()
    }

    pub fn plugin_names(&self) -> Vec<String> {
        self.plugins.clone()
    }

    pub fn mcp_server_labels(&self) -> Vec<String> {
        self.mcp_servers
            .iter()
            .map(|server| format!("{}/{}", server.plugin_name, server.server_name))
            .collect()
    }

    pub fn marketplace_count(&self) -> usize {
        self.marketplaces
    }

    pub fn extra_read_roots(&self) -> &[PathBuf] {
        &self.extra_read_roots
    }

    pub fn stdio_mcp_server_count(&self) -> usize {
        self.mcp_servers
            .iter()
            .filter(|server| matches!(server.transport, McpTransportConfig::Stdio { .. }))
            .count()
    }

    pub fn http_mcp_server_count(&self) -> usize {
        self.mcp_servers
            .iter()
            .filter(|server| matches!(server.transport, McpTransportConfig::StreamableHttp { .. }))
            .count()
    }

    pub fn diagnostics(&self) -> &[String] {
        &self.diagnostics
    }

    pub fn augment_system_prompt(&self, base: &str) -> Result<String, String> {
        if self.skills.is_empty() {
            return Ok(base.to_string());
        }
        let mut catalog = String::new();
        for skill in &self.skills {
            let record = json!({
                "name": skill.name,
                "description": skill.description,
                "location": skill.location,
                "kind": skill.kind,
            });
            catalog.push_str(
                &serde_json::to_string(&record)
                    .map_err(|error| format!("cannot serialize skill catalog: {error}"))?,
            );
            catalog.push('\n');
        }
        Ok(format!(
            "{base}\n\nAvailable project extensions (metadata only):\n\
             When a task matches an entry, read its listed instruction file \
             with read_file before proceeding. Resolve relative references from that file's directory. \
             A plugin-agent entry supplies compatible task instructions; it does not create a subagent.\n\
             <available_extensions>\n{}{}</available_extensions>",
            catalog,
            if catalog.ends_with('\n') { "" } else { "\n" }
        ))
    }
}

fn discover_plugins(
    workspace: &Path,
    skills: &mut BTreeMap<String, Skill>,
    discovery: &mut Discovery,
) {
    let plugins_root = workspace.join(".agents/plugins");
    if !plugins_root.exists() {
        return;
    }
    let plugins_root = match contained_directory(&plugins_root, workspace) {
        Ok(root) => root,
        Err(error) => {
            discovery.diagnostics.push(error);
            return;
        }
    };
    for candidate in directory_children(&plugins_root, "plugin", &mut discovery.diagnostics) {
        let plugin_root = match contained_directory(&candidate, workspace) {
            Ok(root) => root,
            Err(error) => {
                discovery.diagnostics.push(error);
                continue;
            }
        };
        load_plugin(
            workspace,
            &plugin_root,
            PluginLoadOptions {
                expected_name: None,
                explicit_skills: None,
                accounting: PluginAccounting::Count,
                source: "installed",
            },
            skills,
            discovery,
        );
    }
}

fn discover_skillsets(
    workspace: &Path,
    skills: &mut BTreeMap<String, Skill>,
    discovery: &mut Discovery,
) {
    let config_path = workspace.join(".agents/skillsets.json");
    if config_path.exists() {
        discover_configured_skillsets(workspace, &config_path, skills, discovery);
        return;
    }
    let root = workspace.join(".agents/skillsets");
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
    for candidate in directory_children(&root, "skillset", &mut discovery.diagnostics) {
        let skillset = match contained_directory(&candidate, workspace) {
            Ok(skillset) => skillset,
            Err(error) => {
                discovery.diagnostics.push(error);
                continue;
            }
        };
        load_entire_skillset(workspace, &skillset, skills, &mut discovery.diagnostics);
    }
}

fn discover_configured_skillsets(
    workspace: &Path,
    config_path: &Path,
    skills: &mut BTreeMap<String, Skill>,
    discovery: &mut Discovery,
) {
    let config = match read_skillset_file(config_path) {
        Ok(config) => config,
        Err(error) => {
            discovery.diagnostics.push(error);
            return;
        }
    };
    if config.skillsets.len() > MAX_SKILLSETS {
        discovery.diagnostics.push(format!(
            "{} contains more than {MAX_SKILLSETS} skillsets",
            config_path.display()
        ));
        return;
    }
    let skillsets_root = workspace.join(".agents/skillsets");
    for (directory_name, selection) in config.skillsets {
        if !marketplaces::safe_component(&directory_name) {
            discovery.diagnostics.push(format!(
                "{} skillset key {directory_name:?} must be one directory name",
                config_path.display()
            ));
            continue;
        }
        let (configured_path, selected_skills) = match selection {
            SkillsetSelection::Skills(names) => (None, names),
            SkillsetSelection::Explicit(explicit) => (explicit.path, explicit.skills),
        };
        if selected_skills.is_empty() {
            discovery
                .diagnostics
                .push(format!("skillset {directory_name:?} enables no skills"));
            continue;
        }
        if selected_skills.len() > MAX_SKILLS_PER_SKILLSET {
            discovery.diagnostics.push(format!(
                "skillset {directory_name:?} enables more than {MAX_SKILLS_PER_SKILLSET} skills"
            ));
            continue;
        }
        let root = match marketplaces::resolve_local_directory(
            workspace,
            configured_path.as_deref(),
            skillsets_root.join(&directory_name),
        ) {
            Ok(root) => root,
            Err(error) => {
                discovery.diagnostics.push(error);
                continue;
            }
        };
        marketplaces::record_extra_root(workspace, &root, &mut discovery.extra_read_roots);
        let source = format!("skillset {directory_name}");
        for (_, skill_root) in
            marketplaces::find_named_skill_dirs(&root, selected_skills, &mut discovery.diagnostics)
        {
            let path = skill_root.join("SKILL.md");
            match parse_instruction(
                &path,
                &root,
                workspace,
                &source,
                "skill",
                InstructionNameRule::Compatible,
            ) {
                Ok(skill) => insert_skill(skill, false, skills, &mut discovery.diagnostics),
                Err(error) => discovery.diagnostics.push(error),
            }
        }
    }
}

fn load_entire_skillset(
    workspace: &Path,
    skillset: &Path,
    skills: &mut BTreeMap<String, Skill>,
    diagnostics: &mut Vec<String>,
) {
    let source = format!(
        "skillset {}",
        skillset.file_name().unwrap_or_default().to_string_lossy()
    );
    let single_skill = skillset.join("SKILL.md");
    if single_skill.is_file() {
        match parse_instruction(
            &single_skill,
            skillset,
            workspace,
            &source,
            "skill",
            InstructionNameRule::Compatible,
        ) {
            Ok(skill) => insert_skill(skill, false, skills, diagnostics),
            Err(error) => diagnostics.push(error),
        }
    }
    discover_skill_root(
        &skillset.join("skills"),
        skillset,
        workspace,
        &source,
        SkillDiscoveryPolicy::Compatible,
        skills,
        diagnostics,
    );
}

fn read_skillset_file(path: &Path) -> Result<SkillsetFile, String> {
    let content = read_bounded(path)?;
    serde_json::from_str(&content)
        .map_err(|error| format!("cannot parse {}: {error}", path.display()))
}

fn discover_marketplaces(
    workspace: &Path,
    skills: &mut BTreeMap<String, Skill>,
    discovery: &mut Discovery,
) {
    let marketplace_discovery = marketplaces::discover(workspace);
    discovery.marketplaces = marketplace_discovery.marketplace_count;
    discovery
        .diagnostics
        .extend(marketplace_discovery.diagnostics);
    for root in marketplace_discovery.extra_roots {
        marketplaces::record_extra_root(workspace, &root, &mut discovery.extra_read_roots);
    }
    for plugin in marketplace_discovery.plugins {
        load_plugin(
            workspace,
            &plugin.root,
            PluginLoadOptions {
                expected_name: Some(&plugin.name),
                explicit_skills: plugin.explicit_skills.as_deref(),
                accounting: if plugin.is_plugin {
                    PluginAccounting::Count
                } else {
                    PluginAccounting::Skip
                },
                source: plugin.ecosystem,
            },
            skills,
            discovery,
        );
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PluginFlavor {
    Portable,
    Claude,
    Grok,
    MarketplaceOnly,
}

fn load_plugin(
    workspace: &Path,
    plugin_root: &Path,
    options: PluginLoadOptions<'_>,
    skills: &mut BTreeMap<String, Skill>,
    discovery: &mut Discovery,
) {
    let PluginLoadOptions {
        expected_name,
        explicit_skills,
        accounting,
        source,
    } = options;
    let manifest = find_plugin_manifest(plugin_root);
    let (plugin_name, flavor) = match manifest {
        Ok(Some((manifest_path, PluginFlavor::Portable))) => {
            let value = match read_json(&manifest_path) {
                Ok(value) => value,
                Err(error) => {
                    discovery.diagnostics.push(error);
                    return;
                }
            };
            match validate_plugin_manifest(&value, &manifest_path, &mut discovery.diagnostics) {
                Ok(name) => (name, PluginFlavor::Portable),
                Err(error) => {
                    discovery.diagnostics.push(error);
                    return;
                }
            }
        }
        Ok(Some((manifest_path, flavor))) => {
            let value = match read_json(&manifest_path) {
                Ok(value) => value,
                Err(error) => {
                    discovery.diagnostics.push(error);
                    return;
                }
            };
            match validate_legacy_plugin_manifest(&value, &manifest_path) {
                Ok(name) => (name, flavor),
                Err(error) => {
                    discovery.diagnostics.push(error);
                    return;
                }
            }
        }
        Ok(None) => match (expected_name, explicit_skills) {
            (Some(name), Some(_)) => (name.to_string(), PluginFlavor::MarketplaceOnly),
            _ => {
                discovery.diagnostics.push(format!(
                    "{} has no supported plugin manifest",
                    plugin_root.display()
                ));
                return;
            }
        },
        Err(error) => {
            discovery.diagnostics.push(error);
            return;
        }
    };
    if let Some(expected_name) = expected_name
        && plugin_name != expected_name
    {
        discovery.diagnostics.push(format!(
            "{} plugin name {plugin_name:?} does not match marketplace entry {expected_name:?}",
            plugin_root.display()
        ));
        return;
    }
    if matches!(accounting, PluginAccounting::Count) {
        discovery.plugins.push(plugin_name.clone());
    }
    let source = format!("{source} plugin {plugin_name}");
    if let Some(explicit_skills) = explicit_skills {
        for skill_root in explicit_skills {
            let path = skill_root.join("SKILL.md");
            let name_rule = if flavor == PluginFlavor::Portable {
                InstructionNameRule::ParentDirectory
            } else {
                InstructionNameRule::Compatible
            };
            match parse_instruction(&path, plugin_root, workspace, &source, "skill", name_rule) {
                Ok(skill) => insert_skill(skill, false, skills, &mut discovery.diagnostics),
                Err(error) => discovery.diagnostics.push(error),
            }
        }
    } else {
        discover_skill_root(
            &plugin_root.join("skills"),
            plugin_root,
            workspace,
            &source,
            if flavor == PluginFlavor::Portable {
                SkillDiscoveryPolicy::Strict
            } else {
                SkillDiscoveryPolicy::Compatible
            },
            skills,
            &mut discovery.diagnostics,
        );
    }
    if matches!(flavor, PluginFlavor::Claude | PluginFlavor::Grok) {
        discover_agent_root(
            &plugin_root.join("agents"),
            plugin_root,
            workspace,
            &source,
            skills,
            &mut discovery.diagnostics,
        );
    }
    match flavor {
        PluginFlavor::Portable => discover_mcp_file(
            workspace,
            plugin_root,
            &plugin_name,
            &plugin_root.join("mcp.json"),
            McpFileFormat::Portable,
            discovery,
        ),
        PluginFlavor::Claude | PluginFlavor::Grok => discover_mcp_file(
            workspace,
            plugin_root,
            &plugin_name,
            &plugin_root.join(".mcp.json"),
            McpFileFormat::Legacy,
            discovery,
        ),
        PluginFlavor::MarketplaceOnly => {}
    }
}

fn find_plugin_manifest(root: &Path) -> Result<Option<(PathBuf, PluginFlavor)>, String> {
    let candidates = [
        (root.join("plugin.json"), PluginFlavor::Portable),
        (
            root.join(".claude-plugin/plugin.json"),
            PluginFlavor::Claude,
        ),
        (root.join(".grok-plugin/plugin.json"), PluginFlavor::Grok),
    ];
    let present = candidates
        .into_iter()
        .filter(|(path, _)| path.is_file())
        .collect::<Vec<_>>();
    match present.as_slice() {
        [] => Ok(None),
        [(path, flavor)] => contained_file(path, root).map(|path| Some((path, *flavor))),
        _ => Err(format!(
            "{} contains multiple supported plugin manifests",
            root.display()
        )),
    }
}

fn validate_legacy_plugin_manifest(value: &Value, path: &Path) -> Result<String, String> {
    let object = value
        .as_object()
        .ok_or_else(|| format!("{} must contain a JSON object", path.display()))?;
    let name = required_string(object, "name", path)?;
    validate_plugin_name(name).map_err(|error| format!("{}: {error}", path.display()))?;
    Ok(name.to_string())
}

fn validate_plugin_manifest(
    value: &Value,
    path: &Path,
    diagnostics: &mut Vec<String>,
) -> Result<String, String> {
    let object = value
        .as_object()
        .ok_or_else(|| format!("{} must contain a JSON object", path.display()))?;
    let schema = required_string(object, "$schema", path)?;
    if schema != PLUGIN_SCHEMA {
        return Err(format!(
            "{} uses unsupported schema {schema}",
            path.display()
        ));
    }
    let name = required_string(object, "name", path)?;
    validate_plugin_name(name).map_err(|error| format!("{}: {error}", path.display()))?;
    for key in object.keys() {
        if !matches!(
            key.as_str(),
            "$schema"
                | "name"
                | "version"
                | "description"
                | "author"
                | "homepage"
                | "repository"
                | "license"
                | "keywords"
                | "extensions"
        ) {
            diagnostics.push(format!(
                "{}: unknown manifest field {key:?} was ignored",
                path.display()
            ));
        }
    }
    for key in [
        "version",
        "description",
        "homepage",
        "repository",
        "license",
    ] {
        if object.get(key).is_some_and(|value| !value.is_string()) {
            return Err(format!("{} field {key:?} must be a string", path.display()));
        }
    }
    if let Some(keywords) = object.get("keywords")
        && !keywords
            .as_array()
            .is_some_and(|items| items.iter().all(Value::is_string))
    {
        return Err(format!(
            "{} field \"keywords\" must be an array of strings",
            path.display()
        ));
    }
    if let Some(author) = object.get("author") {
        let valid = author.as_object().is_some_and(|author| {
            author.iter().all(|(key, value)| {
                matches!(key.as_str(), "name" | "email" | "url") && value.is_string()
            })
        });
        if !valid {
            return Err(format!("{} field \"author\" is invalid", path.display()));
        }
    }
    if object
        .get("extensions")
        .is_some_and(|extensions| !extensions.is_object())
    {
        diagnostics.push(format!(
            "{}: non-object extensions field was ignored",
            path.display()
        ));
    }
    Ok(name.to_string())
}

#[derive(Clone, Copy)]
enum McpFileFormat {
    Portable,
    Legacy,
}

fn discover_project_mcp(workspace: &Path, discovery: &mut Discovery) {
    discover_mcp_file(
        workspace,
        workspace,
        "project",
        &workspace.join(".agents/mcp.json"),
        McpFileFormat::Legacy,
        discovery,
    );
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
        false,
    ) {
        Ok(server) => push_mcp_server(server, discovery),
        Err(error) => discovery
            .diagnostics
            .push(format!("{}: {error}", path.display())),
    }
}

fn discover_mcp_file(
    workspace: &Path,
    plugin_root: &Path,
    plugin_name: &str,
    path: &Path,
    format: McpFileFormat,
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
    let servers = match format {
        McpFileFormat::Portable => {
            if object
                .keys()
                .any(|key| key != "$schema" && key != "mcpServers")
            {
                discovery.diagnostics.push(format!(
                    "{} contains unknown top-level fields; MCP was disabled for this plugin",
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
            servers
        }
        McpFileFormat::Legacy => match object.get("mcpServers") {
            Some(value) => {
                let Some(servers) = value.as_object() else {
                    discovery.diagnostics.push(format!(
                        "{} field \"mcpServers\" must be an object",
                        path.display()
                    ));
                    return;
                };
                if object.keys().any(|key| key != "mcpServers") {
                    discovery.diagnostics.push(format!(
                        "{} contains fields beside \"mcpServers\"",
                        path.display()
                    ));
                    return;
                }
                servers
            }
            None => object,
        },
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
            matches!(format, McpFileFormat::Legacy),
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
    allow_implicit_stdio: bool,
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
        None if allow_implicit_stdio && object.contains_key("command") => "stdio",
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
    if env.keys().any(|key| {
        matches!(
            key.as_str(),
            "PLUGIN_ROOT" | "PLUGIN_DATA" | "CLAUDE_PLUGIN_ROOT"
        )
    }) {
        return Err(
            "env must not define PLUGIN_ROOT, PLUGIN_DATA, or CLAUDE_PLUGIN_ROOT".to_string(),
        );
    }
    let cwd = optional_string(object.get("cwd"), "cwd")?;
    if cwd.as_deref().is_some_and(|cwd| {
        !cwd.starts_with("./")
            && cwd != "${PLUGIN_ROOT}"
            && !cwd.starts_with("${PLUGIN_ROOT}/")
            && cwd != "${PLUGIN_DATA}"
            && !cwd.starts_with("${PLUGIN_DATA}/")
            && cwd != "${CLAUDE_PLUGIN_ROOT}"
            && !cwd.starts_with("${CLAUDE_PLUGIN_ROOT}/")
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

fn discover_skill_root(
    root: &Path,
    boundary: &Path,
    workspace: &Path,
    source: &str,
    policy: SkillDiscoveryPolicy,
    skills: &mut BTreeMap<String, Skill>,
    diagnostics: &mut Vec<String>,
) {
    let (overrides, name_rule) = match policy {
        SkillDiscoveryPolicy::Direct => (true, InstructionNameRule::ParentDirectory),
        SkillDiscoveryPolicy::Strict => (false, InstructionNameRule::ParentDirectory),
        SkillDiscoveryPolicy::Compatible => (false, InstructionNameRule::Compatible),
    };
    if !root.exists() {
        return;
    }
    let root = match contained_directory(root, boundary) {
        Ok(root) => root,
        Err(error) => {
            diagnostics.push(error);
            return;
        }
    };
    for candidate in directory_children(&root, "skill", diagnostics) {
        if skills.len() >= MAX_DISCOVERED_SKILLS {
            diagnostics.push(format!(
                "skill limit reached ({MAX_DISCOVERED_SKILLS}); remaining skills were skipped"
            ));
            return;
        }
        let skill_path = candidate.join("SKILL.md");
        if !skill_path.exists() {
            continue;
        }
        match parse_instruction(&skill_path, boundary, workspace, source, "skill", name_rule) {
            Ok(skill) => insert_skill(skill, overrides, skills, diagnostics),
            Err(error) => diagnostics.push(error),
        }
    }
}

fn discover_agent_root(
    root: &Path,
    boundary: &Path,
    workspace: &Path,
    source: &str,
    skills: &mut BTreeMap<String, Skill>,
    diagnostics: &mut Vec<String>,
) {
    if !root.exists() {
        return;
    }
    let root = match contained_directory(root, boundary) {
        Ok(root) => root,
        Err(error) => {
            diagnostics.push(error);
            return;
        }
    };
    for path in directory_children(&root, "plugin agent", diagnostics) {
        if path.extension().and_then(|extension| extension.to_str()) != Some("md") {
            continue;
        }
        match parse_instruction(
            &path,
            boundary,
            workspace,
            source,
            "plugin-agent",
            InstructionNameRule::FileStem,
        ) {
            Ok(skill) => insert_skill(skill, false, skills, diagnostics),
            Err(error) => diagnostics.push(error),
        }
    }
}

fn insert_skill(
    skill: Skill,
    overrides: bool,
    skills: &mut BTreeMap<String, Skill>,
    diagnostics: &mut Vec<String>,
) {
    if skills.len() >= MAX_DISCOVERED_SKILLS && !skills.contains_key(&skill.name) {
        diagnostics.push(format!(
            "skill limit reached ({MAX_DISCOVERED_SKILLS}); remaining skills were skipped"
        ));
        return;
    }
    if let Some(existing) = skills.get(&skill.name) {
        diagnostics.push(format!(
            "extension {:?} from {} was shadowed by {}",
            skill.name,
            if overrides {
                &existing.source
            } else {
                &skill.source
            },
            if overrides {
                &skill.source
            } else {
                &existing.source
            }
        ));
        if !overrides {
            return;
        }
    }
    skills.insert(skill.name.clone(), skill);
}

fn parse_instruction(
    path: &Path,
    boundary: &Path,
    workspace: &Path,
    source: &str,
    kind: &'static str,
    name_rule: InstructionNameRule,
) -> Result<Skill, String> {
    let path = path
        .canonicalize()
        .map_err(|error| format!("cannot resolve {}: {error}", path.display()))?;
    if !path.starts_with(boundary) || !path.is_file() {
        return Err(format!("{} escapes its package boundary", path.display()));
    }
    let content = read_bounded(&path)?;
    let frontmatter = frontmatter(&content)
        .ok_or_else(|| format!("{} has invalid YAML frontmatter", path.display()))?;
    let metadata: SkillMetadata = yaml_serde::from_str(frontmatter)
        .map_err(|error| format!("cannot parse {} frontmatter: {error}", path.display()))?;
    validate_skill_name(&metadata.name).map_err(|error| format!("{}: {error}", path.display()))?;
    let expected_name = match name_rule {
        InstructionNameRule::ParentDirectory => path
            .parent()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str()),
        InstructionNameRule::FileStem => path.file_stem().and_then(|name| name.to_str()),
        InstructionNameRule::Compatible => None,
    };
    if let Some(expected_name) = expected_name
        && metadata.name != expected_name
    {
        return Err(format!(
            "{} instruction name {:?} must match {expected_name:?}",
            path.display(),
            metadata.name
        ));
    }
    let description = metadata
        .description
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if description.is_empty() || description.chars().count() > 1024 {
        return Err(format!(
            "{} skill description must contain 1-1024 characters",
            path.display()
        ));
    }
    let location = path
        .strip_prefix(workspace)
        .unwrap_or(&path)
        .to_string_lossy()
        .replace('\\', "/");
    Ok(Skill {
        name: metadata.name,
        description,
        location,
        source: source.to_string(),
        kind,
    })
}

fn bounded_catalog(
    skills: impl Iterator<Item = Skill>,
    diagnostics: &mut Vec<String>,
) -> Vec<Skill> {
    let mut catalog = Vec::new();
    let mut bytes = 0;
    for skill in skills {
        let record_bytes = serde_json::to_string(&json!({
            "name": skill.name,
            "description": skill.description,
            "location": skill.location,
            "kind": skill.kind,
        }))
        .expect("skill catalog metadata must serialize")
        .len()
            + 1;
        if bytes + record_bytes > MAX_CATALOG_BYTES {
            diagnostics.push(format!(
                "skill catalog limit reached ({MAX_CATALOG_BYTES} bytes); remaining skills were skipped"
            ));
            break;
        }
        bytes += record_bytes;
        catalog.push(skill);
    }
    catalog
}

fn directory_children(root: &Path, kind: &str, diagnostics: &mut Vec<String>) -> Vec<PathBuf> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Vec::new(),
        Err(error) => {
            diagnostics.push(format!(
                "cannot scan {} {kind} directory: {error}",
                root.display()
            ));
            return Vec::new();
        }
    };
    let mut children = entries
        .take(MAX_DIRECTORY_ENTRIES + 1)
        .filter_map(|entry| match entry {
            Ok(entry) => Some(entry.path()),
            Err(error) => {
                diagnostics.push(format!(
                    "cannot read an entry in {}: {error}",
                    root.display()
                ));
                None
            }
        })
        .collect::<Vec<_>>();
    if children.len() > MAX_DIRECTORY_ENTRIES {
        children.truncate(MAX_DIRECTORY_ENTRIES);
        diagnostics.push(format!(
            "{} contains more than {MAX_DIRECTORY_ENTRIES} entries; remaining entries were skipped",
            root.display()
        ));
    }
    children.sort();
    children
}

fn contained_directory(path: &Path, boundary: &Path) -> Result<PathBuf, String> {
    let resolved = path
        .canonicalize()
        .map_err(|error| format!("cannot resolve {}: {error}", path.display()))?;
    if resolved.is_dir() && resolved.starts_with(boundary) {
        Ok(resolved)
    } else {
        Err(format!("{} escapes its package boundary", path.display()))
    }
}

fn contained_file(path: &Path, boundary: &Path) -> Result<PathBuf, String> {
    let resolved = path
        .canonicalize()
        .map_err(|error| format!("cannot resolve {}: {error}", path.display()))?;
    if resolved.is_file() && resolved.starts_with(boundary) {
        Ok(resolved)
    } else {
        Err(format!("{} escapes its package boundary", path.display()))
    }
}

fn read_json(path: &Path) -> Result<Value, String> {
    let content = read_bounded(path)?;
    serde_json::from_str(&content)
        .map_err(|error| format!("cannot parse {}: {error}", path.display()))
}

fn read_bounded(path: &Path) -> Result<String, String> {
    let metadata = fs::metadata(path)
        .map_err(|error| format!("cannot inspect {}: {error}", path.display()))?;
    if !metadata.is_file() || metadata.len() > MAX_METADATA_BYTES {
        return Err(format!(
            "{} must be a regular file no larger than {MAX_METADATA_BYTES} bytes",
            path.display()
        ));
    }
    fs::read_to_string(path)
        .map_err(|error| format!("cannot read {} as UTF-8: {error}", path.display()))
}

fn frontmatter(content: &str) -> Option<&str> {
    let opening_end = content.find('\n')? + 1;
    if content[..opening_end].trim_end_matches(['\r', '\n']) != "---" {
        return None;
    }
    let rest = &content[opening_end..];
    let mut offset = 0;
    for line in rest.split_inclusive('\n') {
        if line.trim_end_matches(['\r', '\n']) == "---" {
            return Some(&rest[..offset]);
        }
        offset += line.len();
    }
    None
}

fn validate_skill_name(name: &str) -> Result<(), String> {
    if name.is_empty()
        || name.len() > 64
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        || name.starts_with('-')
        || name.ends_with('-')
        || name.contains("--")
    {
        Err(format!("invalid Agent Skill name {name:?}"))
    } else {
        Ok(())
    }
}

fn validate_plugin_name(name: &str) -> Result<(), String> {
    if name.is_empty()
        || name.len() > 64
        || !name.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'.')
        })
        || !name.as_bytes()[0].is_ascii_alphanumeric()
        || !name.as_bytes()[name.len() - 1].is_ascii_alphanumeric()
        || name.contains("--")
        || name.contains("..")
    {
        Err(format!("invalid Agent Plugin name {name:?}"))
    } else {
        Ok(())
    }
}

fn required_string<'a>(
    object: &'a Map<String, Value>,
    key: &str,
    path: &Path,
) -> Result<&'a str, String> {
    object
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            format!(
                "{} field {key:?} must be a non-empty string",
                path.display()
            )
        })
}

fn optional_string(value: Option<&Value>, name: &str) -> Result<Option<String>, String> {
    value
        .map(|value| {
            value
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| format!("field {name:?} must be a string"))
        })
        .transpose()
}

fn string_array(value: Option<&Value>, name: &str) -> Result<Vec<String>, String> {
    value
        .map(|value| {
            value
                .as_array()
                .ok_or_else(|| format!("field {name:?} must be an array"))?
                .iter()
                .map(|item| {
                    item.as_str()
                        .map(str::to_string)
                        .ok_or_else(|| format!("field {name:?} must contain only strings"))
                })
                .collect()
        })
        .transpose()
        .map(Option::unwrap_or_default)
}

fn string_map(value: Option<&Value>, name: &str) -> Result<BTreeMap<String, String>, String> {
    value
        .map(|value| {
            value
                .as_object()
                .ok_or_else(|| format!("field {name:?} must be an object"))?
                .iter()
                .map(|(key, value)| {
                    value
                        .as_str()
                        .map(|value| (key.clone(), value.to_string()))
                        .ok_or_else(|| format!("field {name:?} values must be strings"))
                })
                .collect()
        })
        .transpose()
        .map(Option::unwrap_or_default)
}

#[cfg(test)]
#[path = "skills_tests.rs"]
mod tests;
