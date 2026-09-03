use super::*;
use std::collections::HashSet;

const MAX_PATCH_BYTES: usize = 512 * 1024;
const MAX_PATCH_FILES: usize = 16;
const MAX_PATCH_LINES: usize = 32 * 1024;

pub(super) struct ApplyPatch(pub(super) Arc<Workspace>);

struct PatchPlan {
    effects: Vec<FileEffect>,
    paths: Vec<String>,
}

struct FileEffect {
    path: PathBuf,
    before: Option<Vec<u8>>,
    after: Option<Vec<u8>>,
}

struct Hunk {
    old: Vec<String>,
    new: Vec<String>,
}

enum RawOperation {
    Add {
        path: String,
        lines: Vec<String>,
    },
    Delete {
        path: String,
    },
    Update {
        path: String,
        move_to: Option<String>,
        hunks: Vec<Hunk>,
    },
}

impl ToolHandler for ApplyPatch {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "apply_patch".to_string(),
            description: "Apply a bounded Codex-style patch to workspace files. Use relative paths only. A patch may add, update, move, or delete files; validate the complete patch before relying on its result. Update hunks use context lines prefixed with a space, removed lines with -, and added lines with +. All affected files are validated before any write.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "patch": {
                        "type": "string",
                        "description": "A patch wrapped in *** Begin Patch and *** End Patch",
                        "maxLength": MAX_PATCH_BYTES
                    }
                },
                "required": ["patch"],
                "additionalProperties": false
            }),
        }
    }

    fn admission(&self, request: &ToolExecutionRequest) -> Result<ToolAdmission, ToolError> {
        let plan = self.prepare(&request.arguments)?;
        Ok(ToolAdmission::ApprovalRequired {
            action: format!("apply patch to {} file(s)", plan.paths.len()),
        })
    }
}

impl ToolRuntime for ApplyPatch {
    fn execute(&self, arguments: &Value) -> Result<String, ToolError> {
        let plan = self.prepare(arguments)?;
        self.0
            .approve(&format!("apply patch to {} file(s)", plan.paths.len()))?;
        apply_plan(plan)
    }

    fn execute_after_admission(&self, request: &ToolExecutionRequest) -> ToolExecutionOutcome {
        crate::into_tool_outcome(self.prepare(&request.arguments).and_then(apply_plan))
    }
}

impl ApplyPatch {
    fn prepare(&self, arguments: &Value) -> Result<PatchPlan, ToolError> {
        let patch = string_arg(arguments, "patch")?;
        if patch.is_empty() || patch.len() > MAX_PATCH_BYTES {
            return Err(ToolError(format!(
                "patch must contain 1..={MAX_PATCH_BYTES} bytes"
            )));
        }
        let operations = parse_patch(patch)?;
        let mut seen = HashSet::new();
        let mut effects = Vec::new();
        let mut paths = Vec::new();
        for operation in operations {
            match operation {
                RawOperation::Add { path, lines } => {
                    validate_patch_path(&path)?;
                    let resolved = self.0.create_path(&json!({"path": path}))?;
                    if resolved.exists() {
                        return Err(ToolError(format!(
                            "cannot add {:?}: file already exists",
                            resolved.display()
                        )));
                    }
                    reserve_path(&mut seen, &resolved)?;
                    let content = lines_to_text(&lines);
                    ensure_file_size(&content)?;
                    effects.push(FileEffect {
                        path: resolved,
                        before: None,
                        after: Some(content.into_bytes()),
                    });
                    paths.push(path);
                }
                RawOperation::Delete { path } => {
                    validate_patch_path(&path)?;
                    let resolved = self.0.mutate_path(&json!({"path": path}))?;
                    reserve_path(&mut seen, &resolved)?;
                    let before = read_bytes(&resolved)?;
                    effects.push(FileEffect {
                        path: resolved,
                        before: Some(before),
                        after: None,
                    });
                    paths.push(path);
                }
                RawOperation::Update {
                    path,
                    move_to,
                    hunks,
                } => {
                    validate_patch_path(&path)?;
                    let resolved = self.0.mutate_path(&json!({"path": path}))?;
                    let (before, original) = read_text(&resolved)?;
                    let updated = apply_hunks(&original, &hunks)?;
                    ensure_file_size(&updated)?;
                    if let Some(move_to) = move_to {
                        validate_patch_path(&move_to)?;
                        let destination = self.0.create_path(&json!({"path": move_to}))?;
                        if resolved == destination {
                            return Err(ToolError(
                                "patch move target must differ from source".to_string(),
                            ));
                        }
                        if destination.exists() {
                            return Err(ToolError(format!(
                                "cannot move to {:?}: file already exists",
                                destination.display()
                            )));
                        }
                        reserve_path(&mut seen, &resolved)?;
                        reserve_path(&mut seen, &destination)?;
                        effects.push(FileEffect {
                            path: resolved,
                            before: Some(before),
                            after: None,
                        });
                        effects.push(FileEffect {
                            path: destination,
                            before: None,
                            after: Some(updated.into_bytes()),
                        });
                        paths.push(format!("{path} -> {move_to}"));
                    } else {
                        reserve_path(&mut seen, &resolved)?;
                        effects.push(FileEffect {
                            path: resolved,
                            before: Some(before),
                            after: Some(updated.into_bytes()),
                        });
                        paths.push(path);
                    }
                }
            }
        }
        Ok(PatchPlan { effects, paths })
    }
}

fn parse_patch(input: &str) -> Result<Vec<RawOperation>, ToolError> {
    let normalized = input.replace("\r\n", "\n");
    let lines: Vec<&str> = normalized.split('\n').collect();
    if lines.first().copied() != Some("*** Begin Patch") {
        return Err(ToolError(
            "patch must start with *** Begin Patch".to_string(),
        ));
    }
    let mut index = 1;
    let mut operations = Vec::new();
    let mut patch_lines = 0usize;
    while index < lines.len() {
        let line = lines[index];
        if line == "*** End Patch" {
            if lines[index + 1..].iter().any(|line| !line.is_empty()) {
                return Err(ToolError(
                    "unexpected content after *** End Patch".to_string(),
                ));
            }
            if operations.is_empty() {
                return Err(ToolError("patch must contain a file operation".to_string()));
            }
            return Ok(operations);
        }
        if operations.len() >= MAX_PATCH_FILES {
            return Err(ToolError(format!(
                "patch exceeds {MAX_PATCH_FILES} file operations"
            )));
        }
        if let Some(path) = line.strip_prefix("*** Add File: ") {
            if path.is_empty() {
                return Err(ToolError("add operation is missing a path".to_string()));
            }
            index += 1;
            let mut added = Vec::new();
            while index < lines.len() && lines[index].starts_with('+') {
                added.push(lines[index][1..].to_string());
                index += 1;
                patch_lines += 1;
                if patch_lines > MAX_PATCH_LINES {
                    return Err(ToolError(format!(
                        "patch exceeds {MAX_PATCH_LINES} hunk lines"
                    )));
                }
            }
            operations.push(RawOperation::Add {
                path: path.to_string(),
                lines: added,
            });
            continue;
        }
        if let Some(path) = line.strip_prefix("*** Delete File: ") {
            if path.is_empty() {
                return Err(ToolError("delete operation is missing a path".to_string()));
            }
            operations.push(RawOperation::Delete {
                path: path.to_string(),
            });
            index += 1;
            continue;
        }
        if let Some(path) = line.strip_prefix("*** Update File: ") {
            if path.is_empty() {
                return Err(ToolError("update operation is missing a path".to_string()));
            }
            index += 1;
            let move_to = if index < lines.len() {
                lines[index]
                    .strip_prefix("*** Move to: ")
                    .map(|path| path.to_string())
            } else {
                None
            };
            if move_to.is_some() {
                index += 1;
            }
            let mut hunks = Vec::new();
            while index < lines.len()
                && lines[index] != "*** End Patch"
                && !is_file_operation_header(lines[index])
            {
                if !lines[index].starts_with("@@") {
                    return Err(ToolError(format!(
                        "expected a hunk header, found {:?}",
                        lines[index]
                    )));
                }
                index += 1;
                let mut old = Vec::new();
                let mut new = Vec::new();
                let mut changed = false;
                while index < lines.len()
                    && lines[index] != "*** End Patch"
                    && !is_file_operation_header(lines[index])
                    && !lines[index].starts_with("@@")
                {
                    let hunk_line = lines[index];
                    if hunk_line == "*** End of File" {
                        index += 1;
                        break;
                    }
                    let mut characters = hunk_line.chars();
                    let Some(prefix) = characters.next() else {
                        return Err(ToolError("hunk lines must have a prefix".to_string()));
                    };
                    let content = characters.as_str();
                    match prefix {
                        ' ' => {
                            old.push(content.to_string());
                            new.push(content.to_string());
                        }
                        '-' => {
                            old.push(content.to_string());
                            changed = true;
                        }
                        '+' => {
                            new.push(content.to_string());
                            changed = true;
                        }
                        _ => {
                            return Err(ToolError(
                                "hunk lines must start with space, -, or +".to_string(),
                            ));
                        }
                    }
                    index += 1;
                    patch_lines += 1;
                    if patch_lines > MAX_PATCH_LINES {
                        return Err(ToolError(format!(
                            "patch exceeds {MAX_PATCH_LINES} hunk lines"
                        )));
                    }
                }
                if !changed {
                    return Err(ToolError(
                        "patch hunk must add or remove a line".to_string(),
                    ));
                }
                hunks.push(Hunk { old, new });
            }
            if move_to.is_none() && hunks.is_empty() {
                return Err(ToolError(
                    "update operation must contain a hunk".to_string(),
                ));
            }
            operations.push(RawOperation::Update {
                path: path.to_string(),
                move_to,
                hunks,
            });
            continue;
        }
        return Err(ToolError(format!("unexpected patch line: {line:?}")));
    }
    Err(ToolError("patch is missing *** End Patch".to_string()))
}

fn is_file_operation_header(line: &str) -> bool {
    line.starts_with("*** Add File: ")
        || line.starts_with("*** Delete File: ")
        || line.starts_with("*** Update File: ")
}

fn validate_patch_path(path: &str) -> Result<(), ToolError> {
    let path = Path::new(path);
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || has_git_component(path)
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(ToolError(
            "patch paths must be relative, remain in the workspace, and avoid .git".to_string(),
        ));
    }
    Ok(())
}

fn reserve_path(seen: &mut HashSet<PathBuf>, path: &Path) -> Result<(), ToolError> {
    if seen.insert(path.to_path_buf()) {
        Ok(())
    } else {
        Err(ToolError(format!(
            "patch references the same file more than once: {}",
            path.display()
        )))
    }
}

fn lines_to_text(lines: &[String]) -> String {
    if lines.is_empty() {
        String::new()
    } else {
        format!("{}\n", lines.join("\n"))
    }
}

fn ensure_file_size(text: &str) -> Result<(), ToolError> {
    if text.len() > MAX_WRITE_BYTES {
        Err(ToolError(format!(
            "patched file exceeds {MAX_WRITE_BYTES} byte write limit"
        )))
    } else {
        Ok(())
    }
}

fn read_bytes(path: &Path) -> Result<Vec<u8>, ToolError> {
    if !path.is_file() {
        return Err(ToolError(format!(
            "cannot patch {:?}: not a regular file",
            path.display()
        )));
    }
    let bytes = fs::read(path).map_err(io_error)?;
    if bytes.len() > MAX_WRITE_BYTES {
        return Err(ToolError(format!(
            "file exceeds {MAX_WRITE_BYTES} byte patch limit"
        )));
    }
    Ok(bytes)
}

fn read_text(path: &Path) -> Result<(Vec<u8>, String), ToolError> {
    let bytes = read_bytes(path)?;
    if crate::image::detect_image(&bytes).is_some() {
        return Err(ToolError(
            "apply_patch only supports UTF-8 text files; use read_image for images".to_string(),
        ));
    }
    let text = String::from_utf8(bytes.clone())
        .map_err(|_| ToolError("apply_patch only supports UTF-8 text files".to_string()))?;
    Ok((bytes, text))
}

fn apply_hunks(original: &str, hunks: &[Hunk]) -> Result<String, ToolError> {
    let newline = if original.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let normalized = original.replace("\r\n", "\n");
    let trailing_newline = normalized.ends_with('\n');
    let mut lines: Vec<String> = if normalized.is_empty() {
        Vec::new()
    } else {
        normalized.split('\n').map(str::to_string).collect()
    };
    if trailing_newline {
        lines.pop();
    }
    for hunk in hunks {
        let start = find_hunk(&lines, &hunk.old)?;
        lines.splice(start..start + hunk.old.len(), hunk.new.clone());
    }
    let mut updated = lines.join("\n");
    if trailing_newline {
        updated.push('\n');
    }
    if newline == "\r\n" {
        updated = updated.replace('\n', "\r\n");
    }
    Ok(updated)
}

fn find_hunk(lines: &[String], old: &[String]) -> Result<usize, ToolError> {
    if old.is_empty() {
        return Ok(lines.len());
    }
    let matches: Vec<usize> = lines
        .windows(old.len())
        .enumerate()
        .filter_map(|(index, window)| (window == old).then_some(index))
        .collect();
    match matches.as_slice() {
        [index] => Ok(*index),
        [] => Err(ToolError(
            "patch hunk did not match the current file contents".to_string(),
        )),
        _ => Err(ToolError(
            "patch hunk matched more than once; add context to disambiguate".to_string(),
        )),
    }
}

fn apply_plan(plan: PatchPlan) -> Result<String, ToolError> {
    for effect in &plan.effects {
        let current = if effect.path.exists() {
            Some(fs::read(&effect.path).map_err(io_error)?)
        } else {
            None
        };
        if current != effect.before {
            return Err(ToolError(format!(
                "file changed after patch validation: {}",
                effect.path.display()
            )));
        }
    }
    for (applied, effect) in plan.effects.iter().enumerate() {
        if let Err(error) = apply_effect(effect) {
            let rollback_ok = plan.effects[..applied]
                .iter()
                .rev()
                .all(|effect| restore_effect(effect).is_ok());
            let suffix = if rollback_ok {
                "; earlier changes were rolled back"
            } else {
                "; rollback also failed"
            };
            return Err(ToolError(format!("cannot apply patch: {error}{suffix}")));
        }
    }
    Ok(format!("applied patch to {} file(s)", plan.paths.len()))
}

fn apply_effect(effect: &FileEffect) -> Result<(), io::Error> {
    match &effect.after {
        Some(bytes) => {
            let mut file = if effect.before.is_some() {
                File::create(&effect.path)?
            } else {
                File::options()
                    .write(true)
                    .create_new(true)
                    .open(&effect.path)?
            };
            file.write_all(bytes)
        }
        None => fs::remove_file(&effect.path),
    }
}

fn restore_effect(effect: &FileEffect) -> Result<(), io::Error> {
    match &effect.before {
        Some(bytes) => fs::write(&effect.path, bytes),
        None if effect.path.exists() => fs::remove_file(&effect.path),
        None => Ok(()),
    }
}
