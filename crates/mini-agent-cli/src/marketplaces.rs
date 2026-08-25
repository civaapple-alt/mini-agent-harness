use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::fs;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;

const MAX_CONFIG_BYTES: u64 = 64 * 1024;
const MAX_MARKETPLACE_BYTES: u64 = 256 * 1024;
const MAX_MARKETPLACES: usize = 16;
const MAX_SELECTORS_PER_MARKETPLACE: usize = 32;
const MAX_SKILLS_PER_PLUGIN: usize = 32;

#[derive(Debug)]
pub struct MarketplacePlugin {
    pub name: String,
    pub root: PathBuf,
    pub explicit_skills: Option<Vec<PathBuf>>,
    pub ecosystem: &'static str,
}

#[derive(Debug, Default)]
pub struct MarketplaceDiscovery {
    pub plugins: Vec<MarketplacePlugin>,
    pub marketplace_count: usize,
    pub diagnostics: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MarketplaceConfig {
    marketplaces: BTreeMap<String, Vec<String>>,
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
    for (directory_name, selected_plugins) in config.marketplaces {
        if !safe_component(&directory_name) {
            discovery.diagnostics.push(format!(
                "{} marketplace key {directory_name:?} must be one directory name",
                config_path.display()
            ));
            continue;
        }
        if selected_plugins.len() > MAX_SELECTORS_PER_MARKETPLACE {
            discovery.diagnostics.push(format!(
                "marketplace {directory_name:?} enables more than {MAX_SELECTORS_PER_MARKETPLACE} selectors"
            ));
            continue;
        }
        let root = match contained_directory(&marketplaces_root.join(&directory_name), workspace) {
            Ok(root) => root,
            Err(error) => {
                discovery.diagnostics.push(error);
                continue;
            }
        };
        let (manifest_path, ecosystem) = match marketplace_manifest(&root) {
            Ok(manifest) => manifest,
            Err(error) => {
                discovery.diagnostics.push(error);
                continue;
            }
        };
        let manifest = match read_json(&manifest_path) {
            Ok(manifest) => manifest,
            Err(error) => {
                discovery.diagnostics.push(error);
                continue;
            }
        };
        discovery.marketplace_count += 1;
        discover_selected_plugins(
            &root,
            &manifest_path,
            &manifest,
            ecosystem,
            selected_plugins,
            &mut discovery,
        );
    }
    discovery
}

fn discover_selected_plugins(
    marketplace_root: &Path,
    manifest_path: &Path,
    manifest: &Value,
    ecosystem: &'static str,
    selected_plugins: Vec<String>,
    discovery: &mut MarketplaceDiscovery,
) {
    let Some(entries) = manifest.get("plugins").and_then(Value::as_array) else {
        discovery.diagnostics.push(format!(
            "{} field \"plugins\" must be an array",
            manifest_path.display()
        ));
        return;
    };
    let selected = selected_plugins.into_iter().collect::<BTreeSet<_>>();
    for selected_name in selected {
        if !safe_component(&selected_name) {
            discovery.diagnostics.push(format!(
                "{} selection {selected_name:?} must be one directory or plugin name",
                manifest_path.display()
            ));
            continue;
        }
        let direct_skill = marketplace_root.join("skills").join(&selected_name);
        if direct_skill.join("SKILL.md").is_file() {
            match contained_directory(&direct_skill, marketplace_root) {
                Ok(skill_root) => discovery.plugins.push(MarketplacePlugin {
                    name: selected_name,
                    root: marketplace_root.to_path_buf(),
                    explicit_skills: Some(vec![skill_root]),
                    ecosystem,
                }),
                Err(error) => discovery.diagnostics.push(error),
            }
            continue;
        }
        let matches = entries
            .iter()
            .filter(|entry| entry.get("name").and_then(Value::as_str) == Some(&selected_name))
            .collect::<Vec<_>>();
        let [entry] = matches.as_slice() else {
            if matches.is_empty() {
                discovery.diagnostics.push(format!(
                    "{} selection {selected_name:?} was not found as an immediate skill or plugin",
                    manifest_path.display()
                ));
            } else {
                discovery.diagnostics.push(format!(
                    "{} plugin {selected_name:?} is duplicated",
                    manifest_path.display()
                ));
            }
            continue;
        };
        let source = match local_source(entry.get("source")) {
            Ok(Some(source)) => source,
            Ok(None) => {
                discovery.diagnostics.push(format!(
                    "{} plugin {selected_name:?} is remote; clone or install it as a local marketplace source before enabling it",
                    manifest_path.display()
                ));
                continue;
            }
            Err(error) => {
                discovery.diagnostics.push(format!(
                    "{} plugin {selected_name:?}: {error}",
                    manifest_path.display()
                ));
                continue;
            }
        };
        let plugin_root = match contained_relative_directory(marketplace_root, &source) {
            Ok(root) => root,
            Err(error) => {
                discovery.diagnostics.push(format!(
                    "{} plugin {selected_name:?}: {error}",
                    manifest_path.display()
                ));
                continue;
            }
        };
        let explicit_skills = match explicit_skill_paths(entry, &plugin_root) {
            Ok(paths) => paths,
            Err(error) => {
                discovery.diagnostics.push(format!(
                    "{} plugin {selected_name:?}: {error}",
                    manifest_path.display()
                ));
                continue;
            }
        };
        discovery.plugins.push(MarketplacePlugin {
            name: selected_name,
            root: plugin_root,
            explicit_skills,
            ecosystem,
        });
    }
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

fn safe_component(value: &str) -> bool {
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
