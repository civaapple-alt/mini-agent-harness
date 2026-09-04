use std::path::Component;
use std::path::Path;
use std::path::PathBuf;

/// Normalizes an existing or not-yet-created path for policy comparisons.
pub fn normalize_path(path: &Path) -> PathBuf {
    if let Ok(canonical) = path.canonicalize() {
        return canonical;
    }
    if let Some(parent) = path.parent()
        && let Ok(parent) = parent.canonicalize()
        && let Some(name) = path.file_name()
    {
        return parent.join(name);
    }
    path.to_path_buf()
}

pub(crate) fn same_path(left: &Path, right: &Path) -> bool {
    left == right || normalize_path(left) == normalize_path(right)
}

pub(crate) fn is_plan_md_alias(path: &Path) -> bool {
    let mut name = None;
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(part) if name.is_none() => name = Some(part),
            _ => return false,
        }
    }
    name.is_some_and(|name| name.eq_ignore_ascii_case("plan.md"))
}

pub(crate) fn plan_scratch_relative_rest(path: &Path) -> Option<PathBuf> {
    let mut parts = path.components();
    let first = parts.next()?;
    let second = parts.next()?;
    if !matches!(first, Component::Normal(part) if part.eq_ignore_ascii_case("plan"))
        || !matches!(second, Component::Normal(part) if part.eq_ignore_ascii_case("scratch"))
    {
        return None;
    }
    let rest: PathBuf = parts.collect();
    (!rest.as_os_str().is_empty()).then_some(rest)
}

pub(crate) fn goal_relative_rest(path: &Path) -> Option<PathBuf> {
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(part) => parts.push(part),
            _ => return None,
        }
    }
    let name = parts.first()?;
    if !name.eq_ignore_ascii_case("goal") || parts.len() < 2 {
        return None;
    }
    Some(parts.into_iter().skip(1).collect())
}

pub(crate) fn is_under_dir(path: &Path, dir: &Path) -> bool {
    let path = normalize_path(path);
    let dir = normalize_path(dir);
    path.starts_with(&dir) && path != dir
}
