use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::VecDeque;
use std::fs;
use std::io::Read;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;

const MAX_CONFIG_BYTES: u64 = 64 * 1024;
const MAX_MARKETPLACE_BYTES: u64 = 256 * 1024;
const MAX_MARKETPLACES: usize = 16;
const MAX_SELECTORS_PER_MARKETPLACE: usize = 32;
const MAX_SKILLS_PER_PLUGIN: usize = 32;
const MAX_SKILL_SEARCH_DEPTH: usize = 5;
const MAX_SKILL_SEARCH_DIRS: usize = 128;
const MAX_SKILL_NAME_PEEK_BYTES: usize = 8 * 1024;
const SKIP_DIR_NAMES: &[&str] = &["node_modules", ".git", "dist", "build", "__pycache__"];

#[derive(Deserialize)]
struct SkillNameMetadata {
    name: String,
}

#[derive(Debug)]
pub struct MarketplacePlugin {
    pub name: String,
    pub root: PathBuf,
    pub explicit_skills: Option<Vec<PathBuf>>,
    pub ecosystem: &'static str,
    pub is_plugin: bool,
}

#[derive(Debug, Default)]
pub struct MarketplaceDiscovery {
    pub plugins: Vec<MarketplacePlugin>,
    pub extra_roots: Vec<PathBuf>,
    pub marketplace_count: usize,
    pub diagnostics: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MarketplaceConfig {
    marketplaces: BTreeMap<String, MarketplaceSelection>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum MarketplaceSelection {
    Legacy(Vec<String>),
    Explicit(ExplicitSelectors),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExplicitSelectors {
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    skills: Vec<String>,
    #[serde(default)]
    plugins: Vec<String>,
}

pub fn discover(workspace: &Path) -> MarketplaceDiscovery {
    let config_path = workspace.join(".agents/marketplaces.json");
    if !config_path.exists() {
        return MarketplaceDiscovery::default();
    }
    let config = match read_config(&config_path) {
        Ok(config) => config,
        Err(error) => {
            return MarketplaceDiscovery {
                diagnostics: vec![error],
                ..MarketplaceDiscovery::default()
            };
        }
    };
    if config.marketplaces.len() > MAX_MARKETPLACES {
        return MarketplaceDiscovery {
            diagnostics: vec![format!(
                "{} contains more than {MAX_MARKETPLACES} marketplaces",
                config_path.display()
            )],
            ..MarketplaceDiscovery::default()
        };
    }

    let marketplaces_root = workspace.join(".agents/marketplaces");
    let mut discovery = MarketplaceDiscovery::default();
    for (directory_name, selection) in config.marketplaces {
        if !safe_component(&directory_name) {
            discovery.diagnostics.push(format!(
                "{} marketplace key {directory_name:?} must be one directory name",
                config_path.display()
            ));
            continue;
        }
        let selector_count = match &selection {
            MarketplaceSelection::Legacy(names) => names.len(),
            MarketplaceSelection::Explicit(explicit) => {
                explicit.skills.len() + explicit.plugins.len()
            }
        };
        if selector_count > MAX_SELECTORS_PER_MARKETPLACE {
            discovery.diagnostics.push(format!(
                "marketplace {directory_name:?} enables more than {MAX_SELECTORS_PER_MARKETPLACE} selectors"
            ));
            continue;
        }
        match selection {
            MarketplaceSelection::Legacy(names) => {
                let root = match contained_directory(
                    &marketplaces_root.join(&directory_name),
                    workspace,
                ) {
                    Ok(root) => root,
                    Err(error) => {
                        discovery.diagnostics.push(error);
                        continue;
                    }
                };
                let Some((manifest_path, ecosystem, manifest)) =
                    load_manifest(&root, &mut discovery)
                else {
                    continue;
                };
                discovery.marketplace_count += 1;
                discover_legacy_selectors(
                    &root,
                    &manifest_path,
                    &manifest,
                    ecosystem,
                    names,
                    &mut discovery,
                );
            }
            MarketplaceSelection::Explicit(explicit) => {
                if explicit.skills.is_empty() && explicit.plugins.is_empty() {
                    discovery.diagnostics.push(format!(
                        "marketplace {directory_name:?} enables no skills or plugins"
                    ));
                    continue;
                }
                let default_root = marketplaces_root.join(&directory_name);
                let root = match resolve_local_directory(
                    workspace,
                    explicit.path.as_deref(),
                    default_root,
                ) {
                    Ok(root) => root,
                    Err(error) => {
                        discovery.diagnostics.push(error);
                        continue;
                    }
                };
                record_extra_root(workspace, &root, &mut discovery.extra_roots);
                discovery.marketplace_count += 1;
                let skill_ecosystem = marketplace_manifest(&root)
                    .ok()
                    .map(|(_, ecosystem)| ecosystem)
                    .unwrap_or("marketplace");
                discover_explicit_skills(&root, skill_ecosystem, explicit.skills, &mut discovery);
                if !explicit.plugins.is_empty() {
                    let Some((manifest_path, ecosystem, manifest)) =
                        load_manifest(&root, &mut discovery)
                    else {
                        continue;
                    };
                    discover_named_plugins(
                        &root,
                        &manifest_path,
                        &manifest,
                        ecosystem,
                        explicit.plugins,
                        &mut discovery,
                    );
                }
            }
        }
    }
    discovery
}

fn load_manifest(
    root: &Path,
    discovery: &mut MarketplaceDiscovery,
) -> Option<(PathBuf, &'static str, Value)> {
    let (manifest_path, ecosystem) = match marketplace_manifest(root) {
        Ok(manifest) => manifest,
        Err(error) => {
            discovery.diagnostics.push(error);
            return None;
        }
    };
    match read_json(&manifest_path) {
        Ok(manifest) => Some((manifest_path, ecosystem, manifest)),
        Err(error) => {
            discovery.diagnostics.push(error);
            None
        }
    }
}

fn discover_legacy_selectors(
    marketplace_root: &Path,
    manifest_path: &Path,
    manifest: &Value,
    ecosystem: &'static str,
    selected_plugins: Vec<String>,
    discovery: &mut MarketplaceDiscovery,
) {
    let Some(entries) = plugin_entries(manifest_path, manifest, discovery) else {
        return;
    };
    for selected_name in selected_plugins.into_iter().collect::<BTreeSet<_>>() {
        if !safe_selector(manifest_path, &selected_name, discovery) {
            continue;
        }
        if try_immediate_skill(marketplace_root, ecosystem, &selected_name, discovery) {
            continue;
        }
        select_plugin(
            marketplace_root,
            manifest_path,
            entries,
            ecosystem,
            selected_name,
            "an immediate skill or plugin",
            discovery,
        );
    }
}

fn discover_named_plugins(
    marketplace_root: &Path,
    manifest_path: &Path,
    manifest: &Value,
    ecosystem: &'static str,
    selected_plugins: Vec<String>,
    discovery: &mut MarketplaceDiscovery,
) {
    let Some(entries) = plugin_entries(manifest_path, manifest, discovery) else {
        return;
    };
    for selected_name in selected_plugins.into_iter().collect::<BTreeSet<_>>() {
        if !safe_selector(manifest_path, &selected_name, discovery) {
            continue;
        }
        select_plugin(
            marketplace_root,
            manifest_path,
            entries,
            ecosystem,
            selected_name,
            "a plugin",
            discovery,
        );
    }
}

fn plugin_entries<'a>(
    manifest_path: &Path,
    manifest: &'a Value,
    discovery: &mut MarketplaceDiscovery,
) -> Option<&'a Vec<Value>> {
    match manifest.get("plugins").and_then(Value::as_array) {
        Some(entries) => Some(entries),
        None => {
            discovery.diagnostics.push(format!(
                "{} field \"plugins\" must be an array",
                manifest_path.display()
            ));
            None
        }
    }
}

fn safe_selector(path: &Path, selected_name: &str, discovery: &mut MarketplaceDiscovery) -> bool {
    if safe_component(selected_name) {
        true
    } else {
        discovery.diagnostics.push(format!(
            "{} selection {selected_name:?} must be one directory or plugin name",
            path.display()
        ));
        false
    }
}

fn try_immediate_skill(
    marketplace_root: &Path,
    ecosystem: &'static str,
    selected_name: &str,
    discovery: &mut MarketplaceDiscovery,
) -> bool {
    let direct_skill = marketplace_root.join("skills").join(selected_name);
    if !direct_skill.join("SKILL.md").is_file() {
        return false;
    }
    match contained_directory(&direct_skill, marketplace_root) {
        Ok(skill_root) => discovery.plugins.push(MarketplacePlugin {
            name: selected_name.to_string(),
            root: marketplace_root.to_path_buf(),
            explicit_skills: Some(vec![skill_root]),
            ecosystem,
            is_plugin: false,
        }),
        Err(error) => discovery.diagnostics.push(error),
    }
    true
}

fn select_plugin(
    marketplace_root: &Path,
    manifest_path: &Path,
    entries: &[Value],
    ecosystem: &'static str,
    selected_name: String,
    missing_kind: &str,
    discovery: &mut MarketplaceDiscovery,
) {
    let matches = entries
        .iter()
        .filter(|entry| entry.get("name").and_then(Value::as_str) == Some(&selected_name))
        .collect::<Vec<_>>();
    let [entry] = matches.as_slice() else {
        if matches.is_empty() {
            discovery.diagnostics.push(format!(
                "{} selection {selected_name:?} was not found as {missing_kind}",
                manifest_path.display()
            ));
        } else {
            discovery.diagnostics.push(format!(
                "{} plugin {selected_name:?} is duplicated",
                manifest_path.display()
            ));
        }
        return;
    };
    let source = match local_source(entry.get("source")) {
        Ok(Some(source)) => source,
        Ok(None) => {
            discovery.diagnostics.push(format!(
                "{} plugin {selected_name:?} is remote; clone or install it as a local marketplace source before enabling it",
                manifest_path.display()
            ));
            return;
        }
        Err(error) => {
            discovery.diagnostics.push(format!(
                "{} plugin {selected_name:?}: {error}",
                manifest_path.display()
            ));
            return;
        }
    };
    let plugin_root = match contained_relative_directory(marketplace_root, &source) {
        Ok(root) => root,
        Err(error) => {
            discovery.diagnostics.push(format!(
                "{} plugin {selected_name:?}: {error}",
                manifest_path.display()
            ));
            return;
        }
    };
    let explicit_skills = match explicit_skill_paths(entry, &plugin_root) {
        Ok(paths) => paths,
        Err(error) => {
            discovery.diagnostics.push(format!(
                "{} plugin {selected_name:?}: {error}",
                manifest_path.display()
            ));
            return;
        }
    };
    discovery.plugins.push(MarketplacePlugin {
        name: selected_name,
        root: plugin_root,
        explicit_skills,
        ecosystem,
        is_plugin: true,
    });
}

fn discover_explicit_skills(
    marketplace_root: &Path,
    ecosystem: &'static str,
    names: Vec<String>,
    discovery: &mut MarketplaceDiscovery,
) {
    for (name, skill_root) in
        find_named_skill_dirs(marketplace_root, names, &mut discovery.diagnostics)
    {
        discovery.plugins.push(MarketplacePlugin {
            name,
            root: marketplace_root.to_path_buf(),
            explicit_skills: Some(vec![skill_root]),
            ecosystem,
            is_plugin: false,
        });
    }
}

pub fn find_named_skill_dirs(
    collection_root: &Path,
    names: Vec<String>,
    diagnostics: &mut Vec<String>,
) -> Vec<(String, PathBuf)> {
    let mut requested = BTreeSet::new();
    for name in names {
        if !safe_component(&name) {
            diagnostics.push(format!(
                "{} skill {name:?} must be one directory name",
                collection_root.display()
            ));
            continue;
        }
        requested.insert(name);
    }
    let mut found = BTreeMap::new();
    for name in &requested {
        let direct = collection_root.join("skills").join(name);
        if direct.join("SKILL.md").is_file() {
            match contained_directory(&direct, collection_root) {
                Ok(skill_root) => {
                    found.insert(name.clone(), skill_root);
                }
                Err(error) => diagnostics.push(error),
            }
        }
    }
    if found.len() < requested.len() {
        search_nested_skills(collection_root, &requested, &mut found, diagnostics);
    }
    let mut matched = Vec::new();
    for name in requested {
        match found.remove(&name) {
            Some(skill_root) => matched.push((name, skill_root)),
            None => diagnostics.push(format!(
                "{} skill {name:?} was not found as a SKILL.md directory or instruction name within {MAX_SKILL_SEARCH_DEPTH} levels",
                collection_root.display()
            )),
        }
    }
    matched
}

fn search_nested_skills(
    collection_root: &Path,
    requested: &BTreeSet<String>,
    found: &mut BTreeMap<String, PathBuf>,
    diagnostics: &mut Vec<String>,
) {
    let mut queue = VecDeque::from([(collection_root.to_path_buf(), 0usize)]);
    let mut scanned = 0usize;
    while let Some((dir, depth)) = queue.pop_front() {
        if found.len() == requested.len() {
            break;
        }
        if scanned >= MAX_SKILL_SEARCH_DIRS {
            diagnostics.push(format!(
                "{} skill search exceeded {MAX_SKILL_SEARCH_DIRS} directories",
                collection_root.display()
            ));
            break;
        }
        scanned += 1;
        if depth > MAX_SKILL_SEARCH_DEPTH {
            continue;
        }
        for child in skill_search_children(&dir, diagnostics) {
            let Some(name) = child.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if SKIP_DIR_NAMES.contains(&name) {
                continue;
            }
            if child.join("SKILL.md").is_file() {
                record_skill_match(&child, collection_root, requested, found, diagnostics);
                continue;
            }
            if depth < MAX_SKILL_SEARCH_DEPTH {
                queue.push_back((child, depth + 1));
            }
        }
    }
}

fn record_skill_match(
    skill_dir: &Path,
    collection_root: &Path,
    requested: &BTreeSet<String>,
    found: &mut BTreeMap<String, PathBuf>,
    diagnostics: &mut Vec<String>,
) {
    let Some(dir_name) = skill_dir.file_name().and_then(|name| name.to_str()) else {
        return;
    };
    let selector = if requested.contains(dir_name) {
        dir_name.to_string()
    } else if let Some(instruction_name) =
        skill_instruction_name(skill_dir).filter(|name| requested.contains(name))
    {
        instruction_name
    } else {
        return;
    };
    if found.contains_key(&selector) {
        return;
    }
    match contained_directory(skill_dir, collection_root) {
        Ok(skill_root) => {
            found.insert(selector, skill_root);
        }
        Err(error) => diagnostics.push(error),
    }
}

fn skill_instruction_name(skill_dir: &Path) -> Option<String> {
    let mut file = fs::File::open(skill_dir.join("SKILL.md")).ok()?;
    let mut buf = vec![0; MAX_SKILL_NAME_PEEK_BYTES];
    let read = file.read(&mut buf).ok()?;
    let content = std::str::from_utf8(&buf[..read]).ok()?;
    let yaml = frontmatter_yaml(content)?;
    yaml_serde::from_str::<SkillNameMetadata>(yaml)
        .ok()
        .map(|metadata| metadata.name)
}

fn frontmatter_yaml(content: &str) -> Option<&str> {
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

fn skill_search_children(root: &Path, diagnostics: &mut Vec<String>) -> Vec<PathBuf> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) => {
            diagnostics.push(format!(
                "cannot scan {} for marketplace skills: {error}",
                root.display()
            ));
            return Vec::new();
        }
    };
    let mut children = entries
        .take(MAX_SKILL_SEARCH_DIRS + 1)
        .filter_map(|entry| match entry {
            Ok(entry) => {
                let path = entry.path();
                path.is_dir().then_some(path)
            }
            Err(error) => {
                diagnostics.push(format!(
                    "cannot read an entry in {}: {error}",
                    root.display()
                ));
                None
            }
        })
        .collect::<Vec<_>>();
    if children.len() > MAX_SKILL_SEARCH_DIRS {
        children.truncate(MAX_SKILL_SEARCH_DIRS);
        diagnostics.push(format!(
            "{} contains more than {MAX_SKILL_SEARCH_DIRS} entries; remaining entries were skipped",
            root.display()
        ));
    }
    children.sort();
    children
}

pub fn resolve_local_directory(
    workspace: &Path,
    configured: Option<&str>,
    default: PathBuf,
) -> Result<PathBuf, String> {
    let candidate = match configured.map(str::trim).filter(|path| !path.is_empty()) {
        None => default,
        Some(path) => {
            if looks_remote(path) {
                return Err(format!(
                    "path {path:?} is remote; clone it locally and set a filesystem path"
                ));
            }
            let raw = Path::new(path);
            if raw.is_absolute() {
                raw.to_path_buf()
            } else {
                workspace.join(raw)
            }
        }
    };
    let resolved = candidate
        .canonicalize()
        .map_err(|error| format!("cannot resolve {}: {error}", candidate.display()))?;
    if resolved.is_dir() {
        Ok(resolved)
    } else {
        Err(format!("{} is not a directory", candidate.display()))
    }
}

pub fn record_extra_root(workspace: &Path, root: &Path, extra_roots: &mut Vec<PathBuf>) {
    if !root.starts_with(workspace) && !extra_roots.iter().any(|existing| existing == root) {
        extra_roots.push(root.to_path_buf());
    }
}

fn looks_remote(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.starts_with("git@")
        || lower.starts_with("ssh://")
        || lower.starts_with("git://")
}

fn local_source(value: Option<&Value>) -> Result<Option<String>, String> {
    let Some(value) = value else {
        return Err("field \"source\" is required".to_string());
    };
    if let Some(source) = value.as_str() {
        return Ok(Some(source.to_string()));
    }
    let object = value
        .as_object()
        .ok_or_else(|| "field \"source\" must be a string or object".to_string())?;
    let local = object.get("type").and_then(Value::as_str) == Some("local")
        || object.get("source").and_then(Value::as_str) == Some("local");
    if !local {
        return Ok(None);
    }
    object
        .get("path")
        .and_then(Value::as_str)
        .filter(|path| !path.is_empty())
        .map(|path| Some(path.to_string()))
        .ok_or_else(|| "local source field \"path\" must be a non-empty string".to_string())
}

fn explicit_skill_paths(entry: &Value, plugin_root: &Path) -> Result<Option<Vec<PathBuf>>, String> {
    let Some(value) = entry.get("skills") else {
        return Ok(None);
    };
    let items = value
        .as_array()
        .ok_or_else(|| "field \"skills\" must be an array".to_string())?;
    if items.len() > MAX_SKILLS_PER_PLUGIN {
        return Err(format!(
            "field \"skills\" contains more than {MAX_SKILLS_PER_PLUGIN} paths"
        ));
    }
    items
        .iter()
        .map(|item| {
            let path = item
                .as_str()
                .ok_or_else(|| "field \"skills\" must contain only strings".to_string())?;
            contained_relative_directory(plugin_root, path)
        })
        .collect::<Result<Vec<_>, _>>()
        .map(Some)
}

fn marketplace_manifest(root: &Path) -> Result<(PathBuf, &'static str), String> {
    let candidates = [
        (root.join(".claude-plugin/marketplace.json"), "claude"),
        (root.join(".grok-plugin/marketplace.json"), "grok"),
    ];
    let present = candidates
        .into_iter()
        .filter(|(path, _)| path.is_file())
        .collect::<Vec<_>>();
    match present.as_slice() {
        [(path, ecosystem)] => Ok((path.clone(), ecosystem)),
        [] => Err(format!(
            "{} has no .claude-plugin or .grok-plugin marketplace.json",
            root.display()
        )),
        _ => Err(format!(
            "{} contains multiple marketplace manifests",
            root.display()
        )),
    }
}

fn read_config(path: &Path) -> Result<MarketplaceConfig, String> {
    let content = read_bounded(path, MAX_CONFIG_BYTES)?;
    serde_json::from_str(&content)
        .map_err(|error| format!("cannot parse {}: {error}", path.display()))
}

fn read_json(path: &Path) -> Result<Value, String> {
    let content = read_bounded(path, MAX_MARKETPLACE_BYTES)?;
    serde_json::from_str(&content)
        .map_err(|error| format!("cannot parse {}: {error}", path.display()))
}

fn read_bounded(path: &Path, max_bytes: u64) -> Result<String, String> {
    let metadata = fs::metadata(path)
        .map_err(|error| format!("cannot inspect {}: {error}", path.display()))?;
    if !metadata.is_file() || metadata.len() > max_bytes {
        return Err(format!(
            "{} must be a regular file no larger than {max_bytes} bytes",
            path.display()
        ));
    }
    fs::read_to_string(path)
        .map_err(|error| format!("cannot read {} as UTF-8: {error}", path.display()))
}

fn contained_relative_directory(boundary: &Path, value: &str) -> Result<PathBuf, String> {
    let relative = value
        .strip_prefix("./")
        .ok_or_else(|| format!("path {value:?} must start with ./"))?;
    if Path::new(relative).components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        return Err(format!(
            "path {value:?} is not a contained relative directory"
        ));
    }
    contained_directory(&boundary.join(relative), boundary)
}

fn contained_directory(path: &Path, boundary: &Path) -> Result<PathBuf, String> {
    let resolved = path
        .canonicalize()
        .map_err(|error| format!("cannot resolve {}: {error}", path.display()))?;
    if resolved.is_dir() && resolved.starts_with(boundary) {
        Ok(resolved)
    } else {
        Err(format!(
            "{} escapes its marketplace boundary",
            path.display()
        ))
    }
}

pub fn safe_component(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && Path::new(value)
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
        && Path::new(value).components().count() == 1
}

#[cfg(test)]
#[path = "marketplaces_tests.rs"]
mod tests;
