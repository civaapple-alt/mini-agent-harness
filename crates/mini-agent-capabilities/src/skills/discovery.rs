use super::*;

pub(super) fn discover_skill_root(
    root: &Path,
    boundary: &Path,
    workspace: &Path,
    source: &str,
    overrides: bool,
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
        match parse_instruction(&skill_path, boundary, workspace, source) {
            Ok(skill) => insert_skill(skill, overrides, skills, diagnostics),
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
) -> Result<Skill, String> {
    let path = path
        .canonicalize()
        .map_err(|error| format!("cannot resolve {}: {error}", path.display()))?;
    if !path.starts_with(boundary) || !path.is_file() {
        return Err(format!("{} escapes its package boundary", path.display()));
    }
    let content = read_instruction_prefix(&path)?;
    let frontmatter = frontmatter(&content)
        .ok_or_else(|| format!("{} has invalid YAML frontmatter", path.display()))?;
    let metadata: SkillMetadata = yaml_serde::from_str(frontmatter)
        .map_err(|error| format!("cannot parse {} frontmatter: {error}", path.display()))?;
    validate_skill_name(&metadata.name).map_err(|error| format!("{}: {error}", path.display()))?;
    let expected_name = path
        .parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str());
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
    })
}

pub(super) fn bounded_catalog(
    skills: impl Iterator<Item = Skill>,
    diagnostics: &mut Vec<String>,
) -> Vec<Skill> {
    let mut catalog = Vec::new();
    let mut bytes = 0;
    for skill in skills {
        let record_bytes = serde_json::to_string(&skill_metadata(&skill))
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
