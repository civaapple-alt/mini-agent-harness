use serde::Deserialize;
use serde_json::Map;
use serde_json::Value;
use serde_json::json;
#[path = "skills/discovery.rs"]
mod discovery;
#[path = "skills/mcp_config.rs"]
mod mcp_config;
#[path = "skills/plugins.rs"]
mod plugins;
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::io::Read;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

const PLUGIN_SCHEMA: &str = "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json";
const MCP_SCHEMA: &str = "https://agent-plugins.org/schemas/1.0.0/mcp.schema.json";
const MAX_DISCOVERED_SKILLS: usize = 64;
const MAX_DISCOVERED_SERVERS: usize = 8;
const MAX_DIRECTORY_ENTRIES: usize = 128;
const MAX_METADATA_BYTES: u64 = 64 * 1024;
const MAX_INSTRUCTION_FRONTMATTER_BYTES: usize = 16 * 1024;
const MAX_CATALOG_BYTES: usize = 16 * 1024;
const MAX_SKILL_DEPENDENCIES: usize = 16;
const MAX_SKILL_DEPENDENCY_VALUE_BYTES: usize = 64;
pub const MAX_SELECTED_SKILLS: usize = 8;
pub const MAX_ACTIVATED_SKILL_BYTES: usize = 32 * 1024;
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
    diagnostics: Vec<String>,
}

#[derive(Clone, Debug)]
struct Skill {
    name: String,
    description: String,
    location: String,
    source: String,
    group: Option<String>,
    enabled: bool,
    path: PathBuf,
    dependencies: Vec<SkillDependency>,
}

struct SkillRootOptions<'a> {
    source: &'a str,
    group: Option<&'a str>,
    enabled: bool,
    overrides: bool,
}

#[derive(Deserialize)]
struct SkillMetadata {
    name: String,
    description: String,
    #[serde(default)]
    dependencies: SkillDependenciesMetadata,
}

#[derive(Default, Deserialize)]
struct SkillDependenciesMetadata {
    #[serde(default)]
    tools: Vec<SkillDependencyMetadata>,
}

#[derive(Deserialize)]
struct SkillDependencyMetadata {
    #[serde(rename = "type")]
    kind: String,
    value: String,
}

/// A capability reference declared by a Skill.
///
/// This is metadata only. Resolving or enabling the referenced provider stays
/// with Host policy and does not happen during discovery or activation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SkillDependency {
    BuiltinTool(String),
    McpServer(String),
}

/// The bounded metadata produced when a discovered Skill is explicitly
/// activated for a later Host/Turn resolution step.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkillActivation {
    pub name: String,
    pub qualified_name: String,
    pub location: String,
    pub dependencies: Vec<SkillDependency>,
}

pub fn discover(workspace: &Path) -> Discovery {
    discover_with_builtin_groups(workspace, &["pstack".to_string()])
}

pub fn discover_with_builtin_groups(workspace: &Path, enabled_groups: &[String]) -> Discovery {
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
    if let Some(builtin_root) = builtin_skill_root() {
        let pstack_root = builtin_root.join("pstack");
        let enabled = enabled_groups.iter().any(|group| group == "pstack");
        discovery::discover_skill_root(
            &pstack_root,
            &builtin_root,
            &workspace,
            SkillRootOptions {
                source: "builtin",
                group: Some("pstack"),
                enabled,
                overrides: false,
            },
            &mut skills,
            &mut discovery.diagnostics,
        );
    }
    discovery::discover_skill_root(
        &workspace.join(".agents/skills"),
        &workspace,
        &workspace,
        SkillRootOptions {
            source: "project",
            group: None,
            enabled: true,
            overrides: true,
        },
        &mut skills,
        &mut discovery.diagnostics,
    );
    plugins::discover_plugins(&workspace, &mut skills, &mut discovery);
    mcp_config::discover_project_mcp(&workspace, &mut discovery);
    discovery.skills = discovery::bounded_catalog(skills.into_values(), &mut discovery.diagnostics);
    discovery
}

pub fn builtin_skill_root() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .map(|home| home.join(".mini-agent/skills/builtin"))
}

fn skill_metadata(skill: &Skill) -> Value {
    let mut metadata = json!({
        "name": skill.name,
        "qualifiedName": qualified_name(skill),
        "description": skill.description,
        "location": skill.location,
        "source": skill.source,
        "enabled": skill.enabled,
    });
    let aliases = skill_aliases(skill);
    if !aliases.is_empty() {
        metadata["aliases"] = json!(aliases);
    }
    if let Some(group) = &skill.group {
        metadata["group"] = json!(group);
    }
    if !skill.dependencies.is_empty() {
        metadata["dependencies"] = json!(
            skill
                .dependencies
                .iter()
                .map(|dependency| match dependency {
                    SkillDependency::BuiltinTool(value) => json!({
                        "type": "builtin",
                        "value": value,
                    }),
                    SkillDependency::McpServer(value) => json!({
                        "type": "mcp",
                        "value": value,
                    }),
                })
                .collect::<Vec<_>>()
        );
    }
    metadata
}

fn qualified_name(skill: &Skill) -> String {
    skill
        .group
        .as_deref()
        .map(|group| format!("{group}:{}", skill.name))
        .unwrap_or_else(|| skill.name.clone())
}

fn skill_aliases(skill: &Skill) -> Vec<String> {
    let mut aliases = Vec::new();
    if skill.group.as_deref() == Some("pstack") {
        aliases.push(format!("pstack-plugin:{}", skill.name));
        aliases.push(skill.name.clone());
    }
    aliases
}

impl Discovery {
    pub fn mcp_servers(&self) -> &[McpServerConfig] {
        &self.mcp_servers
    }

    pub fn skill_names(&self) -> Vec<String> {
        self.skills
            .iter()
            .filter(|skill| skill.enabled)
            .map(|skill| skill.name.clone())
            .collect()
    }

    pub fn skill_catalog(&self) -> Vec<SkillCatalogEntry> {
        self.skills
            .iter()
            .map(|skill| SkillCatalogEntry {
                name: skill.name.clone(),
                qualified_name: qualified_name(skill),
                aliases: skill_aliases(skill),
                description: skill.description.clone(),
                source: skill.source.clone(),
                group: skill.group.clone(),
                enabled: skill.enabled,
            })
            .collect()
    }

    pub fn skill_path_records(&self) -> Vec<SkillPathRecord> {
        self.skills
            .iter()
            .filter(|skill| skill.enabled)
            .map(|skill| SkillPathRecord {
                path: skill.path.clone(),
                name: skill.name.clone(),
                qualified_name: qualified_name(skill),
                source: skill.source.clone(),
                group: skill.group.clone(),
            })
            .collect()
    }

    /// Returns the bounded declaration for a named Skill without enabling any
    /// tool provider or reading the Skill body.
    pub fn activate_skill(&self, name: &str) -> Result<SkillActivation, String> {
        let skill = self.resolve_skill(name)?;
        if !skill.enabled {
            return Err(format!("skill {name:?} is disabled"));
        }
        Ok(SkillActivation {
            name: skill.name.clone(),
            qualified_name: qualified_name(skill),
            location: skill.location.clone(),
            dependencies: skill.dependencies.clone(),
        })
    }

    pub fn activate_skill_group(&self, group: &str) -> Result<(), String> {
        if self
            .skills
            .iter()
            .any(|skill| skill.group.as_deref() == Some(group) && skill.enabled)
        {
            Ok(())
        } else if self
            .skills
            .iter()
            .any(|skill| skill.group.as_deref() == Some(group))
        {
            Err(format!("skill group {group:?} is disabled"))
        } else {
            Err(format!("skill group {group:?} was not found"))
        }
    }

    pub fn load_skills(&self, names: &[String]) -> Result<Vec<LoadedSkill>, String> {
        let mut unique = Vec::new();
        let mut seen = BTreeSet::new();
        for name in names {
            let skill = self.resolve_skill(name)?;
            let identity = qualified_name(skill);
            if seen.insert(identity) {
                unique.push(skill);
            }
        }
        if unique.len() > MAX_SELECTED_SKILLS {
            return Err(format!(
                "at most {MAX_SELECTED_SKILLS} skills may be activated per turn"
            ));
        }
        let mut total = 0usize;
        let mut loaded = Vec::with_capacity(unique.len());
        for skill in unique {
            if !skill.enabled {
                return Err(format!("skill {:?} is disabled", skill.name));
            }
            let content = read_bounded(&skill.path)?;
            let body = skill_body(&content).to_string();
            total = total.saturating_add(body.len());
            if total > MAX_ACTIVATED_SKILL_BYTES {
                return Err(format!(
                    "selected skill bodies exceed {MAX_ACTIVATED_SKILL_BYTES} bytes"
                ));
            }
            loaded.push(LoadedSkill {
                name: skill.name.clone(),
                qualified_name: qualified_name(skill),
                source: skill.source.clone(),
                group: skill.group.clone(),
                body,
            });
        }
        Ok(loaded)
    }

    fn resolve_skill(&self, reference: &str) -> Result<&Skill, String> {
        if let Some(skill) = self
            .skills
            .iter()
            .find(|skill| qualified_name(skill) == reference)
        {
            return Ok(skill);
        }
        if let Some(skill) = self
            .skills
            .iter()
            .find(|skill| skill_aliases(skill).iter().any(|alias| alias == reference))
        {
            return Ok(skill);
        }
        let short = self
            .skills
            .iter()
            .filter(|skill| skill.name == reference)
            .collect::<Vec<_>>();
        match short.as_slice() {
            [skill] => Ok(skill),
            [] => Err(format!("skill {reference:?} was not found")),
            _ => Err(format!(
                "skill {reference:?} is ambiguous; use its qualified name"
            )),
        }
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

    pub fn diagnostics(&self) -> &[String] {
        &self.diagnostics
    }

    /// Retains only explicitly named extension entries.
    ///
    /// Selection is applied after bounded discovery and before prompt/tool
    /// assembly, so omitted MCP servers are never started. Missing names are
    /// reported as bounded diagnostics for the caller.
    pub fn retain_selected(&mut self, names: &[String]) {
        let requested: BTreeSet<&str> = names.iter().map(String::as_str).collect();
        let mut matched = BTreeSet::<String>::new();
        self.skills.retain(|skill| {
            if requested.contains(skill.name.as_str()) {
                matched.insert(skill.name.clone());
                true
            } else {
                false
            }
        });
        self.plugins.retain(|name| {
            if requested.contains(name.as_str()) {
                matched.insert(name.clone());
                true
            } else {
                false
            }
        });
        self.mcp_servers.retain(|server| {
            let label = format!("{}/{}", server.plugin_name, server.server_name);
            if requested.contains(label.as_str())
                || requested.contains(server.server_name.as_str())
                || requested.contains(server.plugin_name.as_str())
            {
                matched.insert(if requested.contains(label.as_str()) {
                    label
                } else if requested.contains(server.plugin_name.as_str()) {
                    server.plugin_name.clone()
                } else {
                    server.server_name.clone()
                });
                true
            } else {
                false
            }
        });
        for name in requested {
            if matched.contains(name) {
                continue;
            }
            self.diagnostics
                .push(format!("selected extension {name:?} was not found"));
        }
    }

    pub fn prompt_fingerprint(&self) -> Result<Option<String>, String> {
        if !self.skills.iter().any(|skill| skill.enabled) {
            return Ok(None);
        }
        Ok(Some(crate::registry::stable_fingerprint(
            self.metadata_catalog()?.as_bytes(),
        )))
    }

    pub fn augment_system_prompt(&self, base: &str) -> Result<String, String> {
        if !self.skills.iter().any(|skill| skill.enabled) {
            return Ok(base.to_string());
        }
        let catalog = self.metadata_catalog()?;
        Ok(format!(
            "{base}\n\nAvailable project extensions (metadata only):\n\
             When a task matches an entry, read its listed instruction file \
             with read_file before proceeding. Resolve relative references from that file's directory. \
             <available_extensions>\n{}{}</available_extensions>",
            catalog,
            if catalog.ends_with('\n') { "" } else { "\n" }
        ))
    }

    fn metadata_catalog(&self) -> Result<String, String> {
        let mut catalog = String::new();
        for skill in &self.skills {
            if !skill.enabled {
                continue;
            }
            catalog.push_str(
                &serde_json::to_string(&skill_metadata(skill))
                    .map_err(|error| format!("cannot serialize skill catalog: {error}"))?,
            );
            catalog.push('\n');
        }
        Ok(catalog)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillCatalogEntry {
    pub name: String,
    pub qualified_name: String,
    pub aliases: Vec<String>,
    pub description: String,
    pub source: String,
    pub group: Option<String>,
    pub enabled: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadedSkill {
    pub name: String,
    pub qualified_name: String,
    pub source: String,
    pub group: Option<String>,
    pub body: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkillPathRecord {
    pub path: PathBuf,
    pub name: String,
    pub qualified_name: String,
    pub source: String,
    pub group: Option<String>,
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

fn read_instruction_prefix(path: &Path) -> Result<String, String> {
    let mut file =
        fs::File::open(path).map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    let mut buf = vec![0; MAX_INSTRUCTION_FRONTMATTER_BYTES];
    let read = file
        .read(&mut buf)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    String::from_utf8(buf[..read].to_vec()).map_err(|_| format!("{} is not UTF-8", path.display()))
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

fn skill_body(content: &str) -> &str {
    let Some(opening_end) = content.find('\n').map(|index| index + 1) else {
        return content;
    };
    if content[..opening_end].trim_end_matches(['\r', '\n']) != "---" {
        return content;
    }
    let rest = &content[opening_end..];
    let mut offset = 0;
    for line in rest.split_inclusive('\n') {
        if line.trim_end_matches(['\r', '\n']) == "---" {
            return rest[offset + line.len()..].trim_start_matches(['\r', '\n']);
        }
        offset += line.len();
    }
    content
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

fn parse_skill_dependency(
    dependency: SkillDependencyMetadata,
    path: &Path,
) -> Result<SkillDependency, String> {
    let value = dependency.value.trim();
    if value.is_empty()
        || value.len() > MAX_SKILL_DEPENDENCY_VALUE_BYTES
        || value.chars().any(char::is_control)
    {
        return Err(format!(
            "{} Skill dependency value must contain 1-{MAX_SKILL_DEPENDENCY_VALUE_BYTES} non-control characters",
            path.display()
        ));
    }
    match dependency.kind.as_str() {
        "builtin" => Ok(SkillDependency::BuiltinTool(value.to_string())),
        "mcp" => Ok(SkillDependency::McpServer(value.to_string())),
        kind => Err(format!(
            "{} has unsupported Skill dependency type {kind:?}",
            path.display()
        )),
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
