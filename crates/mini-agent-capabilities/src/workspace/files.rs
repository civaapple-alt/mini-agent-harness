use super::*;

pub(super) struct ReadImage {
    pub(super) workspace: Arc<Workspace>,
    pub(super) store: crate::image::ImageStore,
}

impl ReadImage {
    fn read(&self, path: PathBuf) -> Result<String, ToolError> {
        let declared = crate::image::declared_media_type(&path).ok_or_else(|| {
            ToolError(format!(
                "cannot read \"{}\": read_image only accepts PNG/JPEG/WebP/GIF paths",
                path.display()
            ))
        })?;
        if !path.is_file() {
            return Err(ToolError(format!(
                "cannot read \"{}\": not a regular file",
                path.display()
            )));
        }
        let mut bytes = Vec::new();
        File::open(&path)
            .map_err(io_error)?
            .take(crate::image::MAX_IMAGE_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(io_error)?;
        if bytes.len() > crate::image::MAX_IMAGE_BYTES {
            return Err(ToolError(format!(
                "image exceeds {} byte read_image limit",
                crate::image::MAX_IMAGE_BYTES
            )));
        }
        let actual = crate::image::detect_image(&bytes).ok_or_else(|| {
            ToolError(format!(
                "cannot read \"{}\": the bytes are not a PNG/JPEG/WebP/GIF image",
                path.display()
            ))
        })?;
        if actual != declared {
            return Err(ToolError(format!(
                "cannot read \"{}\": the extension declares {declared}, but the bytes use {actual}; rename the file to match its actual format if it is PNG/JPEG/WebP/GIF",
                path.display()
            )));
        }
        let display = path
            .strip_prefix(&self.workspace.root)
            .unwrap_or(path.as_path());
        let stored = self
            .store
            .save(&display.display().to_string(), actual, bytes)?;
        Ok(crate::image::format_envelope(&stored))
    }
}

impl ToolHandler for ReadImage {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "read_image".to_string(),
            description: "Read a local PNG/JPEG/GIF/WebP file and return it for vision models. Path may be workspace-relative or an absolute path on this machine (for example a file under Pictures). Do not copy outside images into the workspace. Absolute paths outside the workspace require approval. The host uploads once via the Files API and later turns reuse that file_id. This is not a screenshot tool.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": { "path": {"type": "string"} },
                "required": ["path"],
                "additionalProperties": false
            }),
        }
    }

    fn admission(&self, request: &ToolExecutionRequest) -> Result<ToolAdmission, ToolError> {
        let (path, requires_approval) = self
            .workspace
            .local_file_path_with_admission(&request.arguments)?;
        if !requires_approval {
            return Ok(ToolAdmission::Legacy);
        }
        Ok(ToolAdmission::ApprovalRequired {
            action: format!("read_image {}", path.display()),
        })
    }
}

impl ToolRuntime for ReadImage {
    fn execute(&self, arguments: &Value) -> Result<String, ToolError> {
        let path = self.workspace.local_file_path(arguments, "read_image")?;
        self.read(path)
    }

    fn execute_after_admission(&self, request: &ToolExecutionRequest) -> ToolExecutionOutcome {
        match self
            .workspace
            .local_file_path_with_admission(&request.arguments)
        {
            Ok((path, _)) => crate::into_tool_outcome(self.read(path)),
            Err(error) => crate::into_tool_outcome(Err(error)),
        }
    }
}

pub(super) struct ReadFile(pub(super) Arc<Workspace>);

impl ToolHandler for ReadFile {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "read_file".to_string(),
            description: "Read a bounded page from a UTF-8 workspace or configured extension-root file. offset is a zero-based line offset and limit is the maximum number of lines; use the returned next_offset to continue through long files. Output includes stable 1-based line numbers and never relies on shell output truncation.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "offset": {"type": "integer", "minimum": 0, "default": 0},
                    "limit": {"type": "integer", "minimum": 1, "maximum": MAX_READ_LINES, "default": DEFAULT_READ_LINES}
                },
                "required": ["path"],
                "additionalProperties": false
            }),
        }
    }
}

impl ToolRuntime for ReadFile {
    fn execute(&self, arguments: &Value) -> Result<String, ToolError> {
        let path = self.0.read_path(arguments)?;
        if !path.is_file() {
            return Err(ToolError(format!(
                "cannot read {:?}: not a regular file",
                path.display()
            )));
        }
        let mut bytes = Vec::new();
        File::open(&path)
            .map_err(io_error)?
            .take(MAX_READ_SOURCE_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(io_error)?;
        if bytes.len() as u64 > MAX_READ_SOURCE_BYTES {
            return Err(ToolError(format!(
                "file exceeds {MAX_READ_SOURCE_BYTES} byte source limit"
            )));
        }
        if crate::image::detect_image(&bytes).is_some() {
            return Err(ToolError(
                "file is not UTF-8; use read_image for PNG/JPEG/GIF/WebP".to_string(),
            ));
        }
        let text = String::from_utf8(bytes)
            .map_err(|_| ToolError("file is not UTF-8 text".to_string()))?;
        let offset = bounded_usize(arguments, "offset", 0, usize::MAX)?;
        let limit = bounded_usize(arguments, "limit", DEFAULT_READ_LINES, MAX_READ_LINES)?;
        Ok(format_read_page(&path, &text, offset, limit))
    }
}

fn bounded_usize(
    arguments: &Value,
    name: &str,
    default: usize,
    maximum: usize,
) -> Result<usize, ToolError> {
    let Some(value) = arguments.get(name) else {
        return Ok(default);
    };
    let value = value
        .as_u64()
        .and_then(|value| usize::try_from(value).ok())
        .ok_or_else(|| ToolError(format!("{name} must be a non-negative integer")))?;
    if value > maximum || (name == "limit" && value == 0) {
        return Err(ToolError(format!(
            "{name} must be {}..={maximum}",
            if name == "limit" { 1 } else { 0 }
        )));
    }
    Ok(value)
}

fn format_read_page(path: &Path, text: &str, offset: usize, limit: usize) -> String {
    let normalized = text.replace("\r\n", "\n");
    let mut lines: Vec<&str> = normalized.split('\n').collect();
    if normalized.is_empty() {
        lines.clear();
    } else if normalized.ends_with('\n') {
        lines.pop();
    }
    let total = lines.len();
    let requested_end = offset.saturating_add(limit).min(total);
    let display = path.display();
    let mut output = format!(
        "--- file: {display} | total_lines={total} | offset={offset} | limit={limit} ---\n"
    );
    let body_limit = MAX_READ_PAGE_BYTES.saturating_sub(180);
    let mut cursor = offset.min(total);
    let mut line_truncated = false;
    while cursor < requested_end {
        let line = lines[cursor];
        let prefix = format!("{}: ", cursor + 1);
        let rendered_len = prefix.len() + line.len() + 1;
        if output.len().saturating_add(rendered_len) <= body_limit {
            output.push_str(&prefix);
            output.push_str(line);
            output.push('\n');
            cursor += 1;
            continue;
        }
        let marker = " …[line truncated]\n";
        let available = body_limit.saturating_sub(output.len() + prefix.len() + marker.len());
        output.push_str(&prefix);
        let end = floor_char_boundary(line, available);
        output.push_str(&line[..end]);
        output.push_str(marker);
        cursor += 1;
        line_truncated = true;
        break;
    }
    if cursor < total {
        output.push_str(&format!(
            "[page boundary: next_offset={cursor}; call read_file with the same path and this offset]\n"
        ));
    }
    if line_truncated {
        output.push_str(
            "[one source line was clipped to keep this tool result below the harness limit]\n",
        );
    }
    output
}

fn floor_char_boundary(text: &str, mut index: usize) -> usize {
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

pub(super) struct EditFile(pub(super) Arc<Workspace>);

impl EditFile {
    fn prepared(&self, arguments: &Value) -> Result<(PathBuf, String), ToolError> {
        let path = self.0.mutate_path(arguments)?;
        let old_text = string_arg(arguments, "old_text")?;
        let new_text = string_arg(arguments, "new_text")?;
        if old_text.is_empty() {
            return Err(ToolError("old_text must not be empty".to_string()));
        }
        let content = fs::read_to_string(&path).map_err(io_error)?;
        if content.len() > MAX_WRITE_BYTES {
            return Err(ToolError("file exceeds edit limit".to_string()));
        }
        let matches = content.match_indices(old_text).count();
        if matches != 1 {
            return Err(ToolError(format!(
                "old_text must match exactly once; found {matches}"
            )));
        }
        let updated = content.replacen(old_text, new_text, 1);
        if updated.len() > MAX_WRITE_BYTES {
            return Err(ToolError("edited file exceeds write limit".to_string()));
        }
        Ok((path, updated))
    }

    fn write(&self, path: &Path, updated: String) -> Result<String, ToolError> {
        fs::write(path, updated).map_err(io_error)?;
        Ok(format!("edited {}", path.display()))
    }
}

impl ToolHandler for EditFile {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "edit_file".to_string(),
            description: "Replace one exact, unique text occurrence in a workspace file"
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "old_text": {"type": "string"},
                    "new_text": {"type": "string"}
                },
                "required": ["path", "old_text", "new_text"],
                "additionalProperties": false
            }),
        }
    }

    fn admission(&self, request: &ToolExecutionRequest) -> Result<ToolAdmission, ToolError> {
        let (path, _) = self.prepared(&request.arguments)?;
        Ok(ToolAdmission::ApprovalRequired {
            action: format!("edit {}", path.display()),
        })
    }
}

impl ToolRuntime for EditFile {
    fn execute(&self, arguments: &Value) -> Result<String, ToolError> {
        let (path, updated) = self.prepared(arguments)?;
        self.0.approve(&format!("edit {}", path.display()))?;
        self.write(&path, updated)
    }

    fn execute_after_admission(&self, request: &ToolExecutionRequest) -> ToolExecutionOutcome {
        match self.prepared(&request.arguments) {
            Ok((path, updated)) => crate::into_tool_outcome(self.write(&path, updated)),
            Err(error) => crate::into_tool_outcome(Err(error)),
        }
    }
}

pub(super) struct WriteFile(pub(super) Arc<Workspace>);

impl WriteFile {
    fn prepared(&self, arguments: &Value) -> Result<(PathBuf, String), ToolError> {
        let path = self.0.create_path(arguments)?;
        let content = string_arg(arguments, "content")?;
        if content.len() > MAX_WRITE_BYTES {
            return Err(ToolError(format!(
                "content exceeds {MAX_WRITE_BYTES} byte write limit"
            )));
        }
        Ok((path, content.to_string()))
    }

    fn write(&self, path: &Path, content: &str) -> Result<String, ToolError> {
        if self.0.is_session_artifact(path) {
            fs::write(path, content.as_bytes()).map_err(io_error)?;
        } else {
            let mut file = File::options()
                .write(true)
                .create_new(true)
                .open(path)
                .map_err(io_error)?;
            file.write_all(content.as_bytes()).map_err(io_error)?;
        }
        Ok(format!(
            "wrote {} bytes to {}",
            content.len(),
            path.display()
        ))
    }
}

impl ToolHandler for WriteFile {
    fn spec(&self) -> ToolSpec {
        file_tool_spec(
            "write_file",
            "Create a new UTF-8 file in an existing workspace directory",
            true,
        )
    }

    fn admission(&self, request: &ToolExecutionRequest) -> Result<ToolAdmission, ToolError> {
        let (path, content) = self.prepared(&request.arguments)?;
        Ok(ToolAdmission::ApprovalRequired {
            action: format!("write {} ({} bytes)", path.display(), content.len()),
        })
    }
}

impl ToolRuntime for WriteFile {
    fn execute(&self, arguments: &Value) -> Result<String, ToolError> {
        let (path, content) = self.prepared(arguments)?;
        self.0.approve(&format!(
            "write {} ({} bytes)",
            path.display(),
            content.len()
        ))?;
        self.write(&path, &content)
    }

    fn execute_after_admission(&self, request: &ToolExecutionRequest) -> ToolExecutionOutcome {
        match self.prepared(&request.arguments) {
            Ok((path, content)) => crate::into_tool_outcome(self.write(&path, &content)),
            Err(error) => crate::into_tool_outcome(Err(error)),
        }
    }
}
