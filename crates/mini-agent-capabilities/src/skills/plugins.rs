use super::*;

pub(super) fn discover_plugins(
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
        load_plugin(workspace, &plugin_root, "installed", skills, discovery);
    }
}

fn load_plugin(
    workspace: &Path,
    plugin_root: &Path,
    source: &str,
    skills: &mut BTreeMap<String, Skill>,
    discovery: &mut Discovery,
) {
    let manifest = match find_plugin_manifest(plugin_root) {
        Ok(Some(path)) => path,
        Ok(None) => {
            discovery.diagnostics.push(format!(
                "{} has no supported plugin manifest",
                plugin_root.display()
            ));
            return;
        }
        Err(error) => {
            discovery.diagnostics.push(error);
            return;
        }
    };
    let value = match read_json(&manifest) {
        Ok(value) => value,
        Err(error) => {
            discovery.diagnostics.push(error);
            return;
        }
    };
    let plugin_name = match validate_plugin_manifest(&value, &manifest, &mut discovery.diagnostics)
    {
        Ok(name) => name,
        Err(error) => {
            discovery.diagnostics.push(error);
            return;
        }
    };
    discovery.plugins.push(plugin_name.clone());
    let source = format!("{source} plugin {plugin_name}");
    super::discovery::discover_skill_root(
        &plugin_root.join("skills"),
        plugin_root,
        workspace,
        &source,
        false,
        skills,
        &mut discovery.diagnostics,
    );
    super::mcp_config::discover_mcp_file(
        workspace,
        plugin_root,
        &plugin_name,
        &plugin_root.join("mcp.json"),
        discovery,
    );
}

fn find_plugin_manifest(root: &Path) -> Result<Option<PathBuf>, String> {
    let path = root.join("plugin.json");
    if path.is_file() {
        contained_file(&path, root).map(Some)
    } else {
        Ok(None)
    }
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
