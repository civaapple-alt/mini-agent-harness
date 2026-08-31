use super::*;

pub(super) struct Shell(pub(super) Arc<Workspace>, pub(super) ResultStore);

impl Tool for Shell {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "shell".to_string(),
            description: shell_description(self.0.approval.mode()),
            parameters: json!({
                "type": "object",
                "properties": { "command": {"type": "string"} },
                "required": ["command"],
                "additionalProperties": false
            }),
        }
    }

    fn execute(&self, arguments: &Value) -> Result<String, ToolError> {
        let command = string_arg(arguments, "command")?;
        if command.is_empty() || command.len() > MAX_COMMAND_BYTES {
            return Err(ToolError(format!(
                "command must contain 1..={MAX_COMMAND_BYTES} bytes"
            )));
        }
        self.0.approval.ensure_plan_mode_unlocked()?;
        self.0.approve(&format!("shell command `{command}`"))?;
        let output = run_shell(command, &self.0.root, self.0.sandbox, COMMAND_TIMEOUT)?;
        if output.text.len() <= INLINE_COMMAND_OUTPUT_BYTES {
            return Ok(output.text);
        }
        let stored = self
            .1
            .store(output.text, output.source_bytes, output.source_truncated)?;
        Ok(format!(
            "<tool_result_preview handle=\"{}\" stored_bytes=\"{}\" source_bytes=\"{}\" source_truncated=\"{}\">\n{}\n</tool_result_preview>\nUse read_tool_result with this handle to inspect a byte range or literal query.",
            stored.handle,
            stored.stored_bytes,
            stored.source_bytes,
            stored.source_truncated,
            stored.preview
        ))
    }
}

pub(super) fn shell_description(approval: ApprovalMode) -> String {
    let approval = match approval {
        ApprovalMode::Interactive => "after user approval",
        ApprovalMode::Automatic => "without per-command approval",
    };
    if cfg!(windows) {
        format!(
            "Run one PowerShell 7 command via pwsh in the Windows workspace {approval}, with a 120-second deadline. Use PowerShell syntax and cmdlets; do not use Unix-only commands or options such as `ls -la`, `find -maxdepth`, or `head`. For long-running or interactive programs, use process_start and process_write instead."
        )
    } else {
        format!(
            "Run one POSIX sh command in the workspace {approval}, with a 120-second deadline. For long-running or interactive programs, use process_start and process_write instead"
        )
    }
}

fn apply_utf8_child_env(cmd: &mut Command) {
    cmd.env("PYTHONIOENCODING", "utf-8");
    cmd.env("PYTHONUTF8", "1");
    cmd.env("PYTHONLEGACYWINDOWSSTDIO", "0");
    cmd.env("GIT_TERMINAL_PROMPT", "0");
    cmd.env("GIT_PAGER", "cat");
    cmd.env("PAGER", "cat");
    cmd.env("CI", "1");
    cmd.env("TERM", "dumb");
}

#[cfg(windows)]
fn windows_utf8_shell_script(command: &str) -> String {
    let preamble = concat!(
        "$OutputEncoding = [System.Text.UTF8Encoding]::new($false); ",
        "[Console]::OutputEncoding = $OutputEncoding; ",
        "[Console]::InputEncoding = $OutputEncoding; ",
        "$PSDefaultParameterValues['Get-Content:Encoding'] = 'utf8'; ",
        "$PSDefaultParameterValues['Set-Content:Encoding'] = 'utf8'; ",
        "$PSDefaultParameterValues['Add-Content:Encoding'] = 'utf8'; ",
        "$PSDefaultParameterValues['Out-File:Encoding'] = 'utf8'; ",
        "$env:PYTHONIOENCODING = 'utf-8'; ",
        "$env:PYTHONUTF8 = '1'; ",
    );
    format!("{preamble}{command}")
}

pub(crate) fn shell_command(command: &str) -> Command {
    #[cfg(windows)]
    {
        let wrapped = windows_utf8_shell_script(command);
        let mut process = Command::new("pwsh");
        apply_utf8_child_env(&mut process);
        process.args(["-NoLogo", "-NoProfile", "-NonInteractive", "-Command"]);
        process.arg(wrapped);
        process
    }
    #[cfg(not(windows))]
    {
        let mut process = Command::new("sh");
        apply_utf8_child_env(&mut process);
        process.args(["-lc", command]);
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            process.process_group(0);
        }
        process
    }
}

pub(super) struct CommandOutput {
    pub(super) text: String,
    pub(super) source_bytes: usize,
    pub(super) source_truncated: bool,
}

fn run_sandboxed_command(
    mut cmd: Command,
    root: &Path,
    sandbox_kind: SandboxKind,
    timeout: Duration,
) -> Result<CommandOutput, ToolError> {
    let sandbox = ProcessSandbox::new(sandbox_kind);
    apply_utf8_child_env(&mut cmd);
    let mut child = cmd
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(io_error)?;
    sandbox.attach_child(&child);
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| ToolError("cannot capture command stdout".to_string()))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| ToolError("cannot capture command stderr".to_string()))?;
    let stream_limit = MAX_COMMAND_CAPTURE_BYTES / 2;
    let stdout = thread::spawn(move || capture_bounded(stdout, stream_limit));
    let stderr = thread::spawn(move || capture_bounded(stderr, stream_limit));
    let started = Instant::now();
    let (status, timed_out) = loop {
        if let Some(status) = child.try_wait().map_err(io_error)? {
            break (status, false);
        }
        if started.elapsed() >= timeout {
            break (sandbox.terminate(&mut child).map_err(io_error)?, true);
        }
        thread::sleep(Duration::from_millis(10));
    };
    let stdout = stdout
        .join()
        .map_err(|_| ToolError("stdout reader panicked".to_string()))?
        .map_err(io_error)?;
    let stderr = stderr
        .join()
        .map_err(|_| ToolError("stderr reader panicked".to_string()))?
        .map_err(io_error)?;
    let status_str = if timed_out {
        format!("timed out after {} seconds", timeout.as_secs_f64())
    } else {
        status.code().map_or_else(
            || "terminated by signal".to_string(),
            |code| code.to_string(),
        )
    };
    let source_bytes = stdout.total_bytes.saturating_add(stderr.total_bytes);
    let source_truncated = stdout.truncated || stderr.truncated;
    let raw_stdout = stdout.render();
    let raw_stderr = stderr.render();
    Ok(CommandOutput {
        text: format!("exit: {status_str}\nstdout:\n{raw_stdout}\nstderr:\n{raw_stderr}"),
        source_bytes,
        source_truncated,
    })
}

pub(super) fn run_shell(
    command: &str,
    root: &Path,
    sandbox_kind: SandboxKind,
    timeout: Duration,
) -> Result<CommandOutput, ToolError> {
    if sandbox_kind == SandboxKind::Docker {
        let docker_check = Command::new("docker")
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        if docker_check.is_err() || !docker_check.unwrap().success() {
            return Err(ToolError(
                "docker sandbox is unavailable on this host; ensure docker daemon is running, or use '--sandbox native'"
                    .to_string(),
            ));
        }
    }
    let cmd = if sandbox_kind == SandboxKind::Docker {
        let mut docker_cmd = Command::new("docker");
        docker_cmd.args([
            "run",
            "--rm",
            "-i",
            "-v",
            &format!("{}:/workspace", root.display()),
            "-w",
            "/workspace",
            "alpine",
            "sh",
            "-c",
            command,
        ]);
        docker_cmd
    } else {
        shell_command(command)
    };
    run_sandboxed_command(cmd, root, sandbox_kind, timeout)
}

pub(super) struct CapturedOutput {
    pub(super) bytes: Vec<u8>,
    pub(super) total_bytes: usize,
    pub(super) truncated: bool,
}

impl CapturedOutput {
    fn render(&self) -> String {
        let text = String::from_utf8_lossy(&self.bytes);
        if self.truncated {
            format!(
                "[retained head+tail from {} bytes]\n{text}",
                self.total_bytes
            )
        } else {
            text.into_owned()
        }
    }
}

pub(super) fn capture_bounded(
    mut reader: impl Read,
    max_bytes: usize,
) -> io::Result<CapturedOutput> {
    let head_limit = max_bytes.div_ceil(2);
    let tail_limit = max_bytes - head_limit;
    let mut head = Vec::with_capacity(head_limit);
    let mut tail = VecDeque::with_capacity(tail_limit);
    let mut total_bytes = 0usize;
    let mut buffer = [0u8; 8192];
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        total_bytes = total_bytes.saturating_add(count);
        let mut remaining = &buffer[..count];
        if head.len() < head_limit {
            let retained = remaining.len().min(head_limit - head.len());
            head.extend_from_slice(&remaining[..retained]);
            remaining = &remaining[retained..];
        }
        for byte in remaining {
            if tail_limit == 0 {
                break;
            }
            if tail.len() == tail_limit {
                tail.pop_front();
            }
            tail.push_back(*byte);
        }
    }
    head.extend(tail);
    Ok(CapturedOutput {
        bytes: head,
        total_bytes,
        truncated: total_bytes > max_bytes,
    })
}
