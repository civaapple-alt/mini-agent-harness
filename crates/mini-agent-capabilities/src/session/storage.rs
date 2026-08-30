use super::*;

pub(super) fn write_json_atomic(path: &Path, value: &Value) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let parent = path.parent().ok_or_else(|| "no parent dir".to_string())?;
    let temp_path = parent.join(format!(".tmp_{}", new_id("tmp")));
    let encoded =
        serde_json::to_vec_pretty(value).map_err(|e| format!("cannot encode json: {e}"))?;
    fs::write(&temp_path, &encoded).map_err(|e| format!("cannot write temp file: {e}"))?;
    fs::rename(&temp_path, path).map_err(|e| {
        let _ = fs::remove_file(&temp_path);
        format!("cannot rename atomic file: {e}")
    })?;
    Ok(())
}

pub(super) fn write_prompt_context(session_dir: &Path, workspace: &Path, session_id: &str) {
    let agents_md_path = workspace.join("AGENTS.md");
    let agents_md_content = if agents_md_path.is_file() {
        fs::read_to_string(&agents_md_path).ok()
    } else {
        None
    };
    let value = json!({
        "version": 1,
        "session_id": session_id,
        "created_at_ms": timestamp_ms(),
        "os_name": std::env::consts::OS,
        "shell_path": if cfg!(windows) { "pwsh" } else { "sh" },
        "workspace": workspace.to_string_lossy(),
        "agents_md_present": agents_md_content.is_some(),
        "agents_md_content": agents_md_content,
    });
    let _ = write_json_atomic(&session_dir.join(PROMPT_CONTEXT_FILE_NAME), &value);
}

pub(super) struct LoadedRecords {
    pub(super) thread_id: String,
    pub(super) messages: Vec<Message>,
    pub(super) next_seq: u64,
    pub(super) checkpoint_seq: u64,
    pub(super) turn_count: usize,
    pub(super) thread_turn_count: usize,
    pub(super) created_at_ms: u64,
    pub(super) valid_bytes: usize,
}

pub(super) fn load_records(session_id: &str, bytes: &[u8]) -> Result<LoadedRecords, String> {
    let mut offset = 0usize;
    let mut valid_bytes = 0usize;
    let mut expected_seq = 1u64;
    let mut header_seen = false;
    let mut latest_checkpoint = None;
    let mut turn_count = 0usize;
    let mut thread_turn_counts: HashMap<String, usize> = HashMap::new();
    let mut created_at_ms = 0u64;
    while offset < bytes.len() {
        let remaining = &bytes[offset..];
        let Some(end) = remaining.iter().position(|byte| *byte == b'\n') else {
            break;
        };
        let line = &remaining[..end];
        if line.len() > MAX_RECORD_BYTES {
            return Err(format!(
                "session record exceeds {MAX_RECORD_BYTES} byte limit"
            ));
        }
        let record: Value = serde_json::from_slice(line)
            .map_err(|error| format!("invalid session record at byte {offset}: {error}"))?;
        let seq = record
            .get("seq")
            .and_then(Value::as_u64)
            .ok_or_else(|| format!("session record at byte {offset} is missing seq"))?;
        if seq != expected_seq {
            return Err(format!(
                "session sequence mismatch: expected {expected_seq}, found {seq}"
            ));
        }
        expected_seq = expected_seq.saturating_add(1);
        match record.get("kind").and_then(Value::as_str) {
            Some("session_created") if !header_seen => {
                let stored_id = record
                    .get("session_id")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "session header is missing session_id".to_string())?;
                if stored_id != session_id {
                    return Err("session id does not match its file name".to_string());
                }
                if record.get("schema_version").and_then(Value::as_u64) != Some(SCHEMA_VERSION) {
                    return Err("unsupported session schema version".to_string());
                }
                created_at_ms = record
                    .get("timestamp_ms")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                header_seen = true;
            }
            Some("turn_started") if header_seen => {
                turn_count = turn_count.saturating_add(1);
                let thread_id = record
                    .get("thread_id")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "turn_started is missing thread_id".to_string())?;
                let count = thread_turn_counts.entry(thread_id.to_string()).or_insert(0);
                *count = (*count).saturating_add(1);
            }
            Some("checkpoint") if header_seen => {
                let thread_id = record
                    .get("thread_id")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "checkpoint is missing thread_id".to_string())?
                    .to_string();
                let messages: Vec<Message> = serde_json::from_value(
                    record
                        .get("messages")
                        .cloned()
                        .ok_or_else(|| "checkpoint is missing messages".to_string())?,
                )
                .map_err(|error| format!("invalid checkpoint messages: {error}"))?;
                if messages
                    .iter()
                    .any(|message| matches!(message, Message::Tool { outcome: None, .. }))
                {
                    return Err("session checkpoint has a tool record without outcome".to_string());
                }
                latest_checkpoint = Some((seq, thread_id, messages));
            }
            Some(_) if header_seen => {}
            Some(_) => return Err("session header must be the first record".to_string()),
            None => return Err("session record is missing kind".to_string()),
        }
        offset = offset.saturating_add(end + 1);
        valid_bytes = offset;
    }
    let (checkpoint_seq, thread_id, messages) = latest_checkpoint
        .ok_or_else(|| "session has no settled checkpoint to resume".to_string())?;
    let thread_turn_count = thread_turn_counts.get(&thread_id).copied().unwrap_or(0);
    Ok(LoadedRecords {
        thread_id,
        messages,
        next_seq: expected_seq,
        checkpoint_seq,
        turn_count,
        thread_turn_count,
        created_at_ms,
        valid_bytes,
    })
}

pub(super) fn acquire_lock(directory: &Path, session_id: &str) -> Result<SessionLock, String> {
    fs::create_dir_all(directory)
        .map_err(|error| format!("cannot create session directory: {error}"))?;
    let path = directory.join(format!("{session_id}.lock"));
    let mut file = match OpenOptions::new().write(true).create_new(true).open(&path) {
        Ok(file) => file,
        Err(error)
            if error.kind() == std::io::ErrorKind::AlreadyExists && reclaim_stale_lock(&path) =>
        {
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .map_err(|error| format!("cannot lock session {session_id}: {error}"))?
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            return Err(format!(
                "session {session_id} is locked by another process or a stale lock"
            ));
        }
        Err(error) => return Err(format!("cannot lock session {session_id}: {error}")),
    };
    writeln!(
        file,
        "pid={} timestamp_ms={}",
        std::process::id(),
        timestamp_ms()
    )
    .and_then(|()| file.sync_data())
    .map_err(|error| format!("cannot write session lock: {error}"))?;
    Ok(SessionLock(path))
}

fn reclaim_stale_lock(path: &Path) -> bool {
    let Ok(contents) = fs::read_to_string(path) else {
        return false;
    };
    let Some(pid) = contents
        .split_whitespace()
        .find_map(|field| field.strip_prefix("pid=")?.parse::<u32>().ok())
    else {
        return false;
    };
    if process_exists(pid) {
        return false;
    }
    let stale_path = path.with_extension(format!("stale-{}", timestamp_ms()));
    fs::rename(path, &stale_path)
        .and_then(|()| fs::remove_file(stale_path))
        .is_ok()
}

fn process_exists(pid: u32) -> bool {
    let pid = pid.to_string();
    if cfg!(windows) {
        Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH"])
            .output()
            .map(|output| String::from_utf8_lossy(&output.stdout).contains(&pid))
            .unwrap_or(true)
    } else {
        Command::new("kill")
            .args(["-0", &pid])
            .status()
            .map(|status| status.success())
            .unwrap_or(true)
    }
}

pub(super) fn copy_attachments(src: &Path, dst: &Path) {
    if !src.is_dir() {
        return;
    }
    let Ok(entries) = fs::read_dir(src) else {
        return;
    };
    let _ = fs::create_dir_all(dst);
    for entry in entries.flatten() {
        let from = entry.path();
        if !from.is_file() {
            continue;
        }
        let Some(name) = from.file_name() else {
            continue;
        };
        let _ = fs::copy(&from, dst.join(name));
    }
}

pub fn session_directory(workspace: &Path) -> Result<PathBuf, String> {
    let home = mini_agent_home()
        .ok_or_else(|| "cannot resolve home directory for ~/.mini-agent/sessions".to_string())?;
    let workspace = workspace
        .canonicalize()
        .map_err(|error| format!("cannot resolve workspace for sessions: {error}"))?;
    let key = percent_encode_path(&display_workspace_path(&workspace));
    if key.is_empty() || key.len() > MAX_WORKSPACE_KEY {
        return Err("workspace path is too long to name a session directory".to_string());
    }
    Ok(home.join("sessions").join(key))
}

pub fn resolve_session_file(
    workspace: &Path,
    session_id: &str,
) -> Result<(PathBuf, PathBuf), String> {
    let session_dir = session_directory(workspace)?.join(session_id);
    let path = session_dir.join(SESSION_FILE_NAME);
    Ok((session_dir, path))
}

fn mini_agent_home() -> Option<PathBuf> {
    let key = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    env::var_os(key)
        .or_else(|| {
            if cfg!(windows) {
                env::var_os("HOME")
            } else {
                None
            }
        })
        .map(|home| PathBuf::from(home).join(".mini-agent"))
}

fn display_workspace_path(path: &Path) -> String {
    let raw = path.to_string_lossy();
    raw.strip_prefix(r"\\?\")
        .or_else(|| raw.strip_prefix("//?/"))
        .unwrap_or(&raw)
        .to_string()
}

fn percent_encode_path(path: &str) -> String {
    let mut encoded = String::new();
    for byte in path.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' => {
                encoded.push(*byte as char);
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

pub(super) fn validate_session_id(session_id: &str) -> Result<(), String> {
    if session_id.is_empty()
        || session_id.len() > 64
        || !session_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        Err("session id must contain 1..=64 ASCII letters, digits, '-' or '_'".to_string())
    } else {
        Ok(())
    }
}
