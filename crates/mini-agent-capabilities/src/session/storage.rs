use super::*;
use std::process::Stdio;

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
    pub(super) items: Vec<SessionItem>,
    pub(super) turn_sources: HashMap<String, TurnSource>,
    pub(super) is_forked: bool,
    pub(super) next_seq: u64,
    pub(super) checkpoint_seq: u64,
    pub(super) turn_count: usize,
    pub(super) thread_turn_count: usize,
    pub(super) created_at_ms: u64,
    pub(super) valid_bytes: usize,
    pub(super) execution_state: Option<SessionExecutionState>,
}

pub(super) fn load_records(session_id: &str, bytes: &[u8]) -> Result<LoadedRecords, String> {
    let mut offset = 0usize;
    let mut valid_bytes = 0usize;
    let mut expected_seq = 1u64;
    let mut header_seen = false;
    let mut latest_checkpoint: Option<(u64, String, Vec<Message>)> = None;
    let mut items = Vec::new();
    let mut turn_sources = HashMap::new();
    let mut is_forked = false;
    let mut turn_count = 0usize;
    let mut thread_turn_counts: HashMap<String, usize> = HashMap::new();
    let mut created_at_ms = 0u64;
    let mut execution_state: Option<SessionExecutionState> = None;
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
        let record_timestamp_ms = record
            .get("timestamp_ms")
            .and_then(Value::as_u64)
            .unwrap_or(0);
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
                is_forked = record.get("forked_from").is_some();
                header_seen = true;
            }
            Some("turn_started") if header_seen => {
                let execution_resume = record
                    .get("execution_resume")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                if !execution_resume {
                    turn_count = turn_count.saturating_add(1);
                }
                let thread_id = record
                    .get("thread_id")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "turn_started is missing thread_id".to_string())?;
                if let (Some(turn_id), Some(source)) = (
                    record.get("turn_id").and_then(Value::as_str),
                    record
                        .get("presentation")
                        .and_then(|presentation| presentation.get("turnSource"))
                        .cloned()
                        .and_then(|source| serde_json::from_value::<TurnSource>(source).ok()),
                ) {
                    turn_sources.insert(turn_id.to_string(), source);
                }
                if !execution_resume {
                    let count = thread_turn_counts.entry(thread_id.to_string()).or_insert(0);
                    *count = (*count).saturating_add(1);
                }
            }
            Some("item") if header_seen => {
                let thread_id = record
                    .get("thread_id")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "item is missing thread_id".to_string())?
                    .to_string();
                let item_id = record
                    .get("item_id")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "item is missing item_id".to_string())?
                    .to_string();
                let turn_id = record
                    .get("turn_id")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                let message = serde_json::from_value(
                    record
                        .get("message")
                        .cloned()
                        .ok_or_else(|| "item is missing message".to_string())?,
                )
                .map_err(|error| format!("invalid item message: {error}"))?;
                let arguments = record
                    .get("arguments")
                    .filter(|value| !value.is_null())
                    .cloned();
                items.push(SessionItem {
                    item_id,
                    thread_id,
                    turn_id,
                    message,
                    arguments,
                });
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
            Some("execution_checkpoint") if header_seen => {
                let record_thread = record
                    .get("thread_id")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "execution checkpoint is missing thread_id".to_string())?;
                let turn_id = record
                    .get("turn_id")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "execution checkpoint is missing turn_id".to_string())?;
                let mode = record
                    .get("message_mode")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "execution checkpoint is missing message_mode".to_string())?;
                let delta: Vec<Message> = serde_json::from_value(
                    record
                        .get("messages")
                        .cloned()
                        .ok_or_else(|| "execution checkpoint is missing messages".to_string())?,
                )
                .map_err(|error| format!("invalid execution checkpoint messages: {error}"))?;
                let mut messages = match mode {
                    "append" => execution_state
                        .as_ref()
                        .filter(|state| state.checkpoint.turn_id.as_str() == turn_id)
                        .map(|state| state.checkpoint.messages.clone())
                        .or_else(|| {
                            latest_checkpoint
                                .as_ref()
                                .filter(|(_, id, _)| id == record_thread)
                                .map(|(_, _, messages)| messages.clone())
                        })
                        .ok_or_else(|| {
                            "execution checkpoint append has no conversation base".to_string()
                        })?,
                    "replace" => Vec::new(),
                    _ => return Err("invalid execution checkpoint message mode".to_string()),
                };
                if mode == "append" {
                    messages.extend(delta);
                } else {
                    messages = delta;
                }
                let checkpoint = ExecutionCheckpoint {
                    turn_id: TurnId::new(turn_id),
                    input: serde_json::from_value(
                        record
                            .get("input")
                            .cloned()
                            .ok_or_else(|| "execution checkpoint is missing input".to_string())?,
                    )
                    .map_err(|error| format!("invalid execution checkpoint input: {error}"))?,
                    messages,
                    next_model_step: record
                        .get("next_model_step")
                        .and_then(Value::as_u64)
                        .and_then(|step| usize::try_from(step).ok())
                        .ok_or_else(|| {
                            "execution checkpoint is missing next_model_step".to_string()
                        })?,
                    final_text: record
                        .get("final_text")
                        .and_then(Value::as_str)
                        .ok_or_else(|| "execution checkpoint is missing final_text".to_string())?
                        .to_string(),
                    phase: serde_json::from_value(
                        record
                            .get("phase")
                            .cloned()
                            .ok_or_else(|| "execution checkpoint is missing phase".to_string())?,
                    )
                    .map_err(|error| format!("invalid execution checkpoint phase: {error}"))?,
                };
                apply_execution_journal_entry(
                    &mut execution_state,
                    seq,
                    record_timestamp_ms,
                    ExecutionJournalEntry::Checkpoint { checkpoint },
                );
            }
            Some("execution_tool_batch_started")
            | Some("execution_tool_call_started")
            | Some("execution_tool_call_finished")
            | Some("execution_tool_batch_settled")
            | Some("execution_waiting_for_continue")
            | Some("execution_resumed")
            | Some("execution_heartbeat")
            | Some("execution_needs_reconciliation")
            | Some("execution_settled")
                if header_seen =>
            {
                let mut journal_record = record.clone();
                let kind = journal_record
                    .get("kind")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .trim_start_matches("execution_");
                journal_record["kind"] = json!(kind);
                let entry: ExecutionJournalEntry = serde_json::from_value(journal_record)
                    .map_err(|error| format!("invalid execution journal record: {error}"))?;
                let entry_turn_id = execution_entry_turn_id(&entry);
                if execution_state
                    .as_ref()
                    .is_some_and(|state| state.checkpoint.turn_id != entry_turn_id)
                {
                    execution_state = None;
                }
                apply_execution_journal_entry(
                    &mut execution_state,
                    seq,
                    record_timestamp_ms,
                    entry,
                );
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
        items,
        turn_sources,
        is_forked,
        next_seq: expected_seq,
        checkpoint_seq,
        turn_count,
        thread_turn_count,
        created_at_ms,
        valid_bytes,
        execution_state,
    })
}

fn execution_entry_turn_id(entry: &ExecutionJournalEntry) -> TurnId {
    match entry {
        ExecutionJournalEntry::Checkpoint { checkpoint } => checkpoint.turn_id.clone(),
        ExecutionJournalEntry::ToolBatchStarted { batch } => batch.turn_id.clone(),
        ExecutionJournalEntry::ToolCallStarted { turn_id, .. }
        | ExecutionJournalEntry::ToolCallFinished { turn_id, .. }
        | ExecutionJournalEntry::ToolBatchSettled { turn_id, .. }
        | ExecutionJournalEntry::WaitingForContinue { turn_id, .. }
        | ExecutionJournalEntry::Resumed { turn_id, .. }
        | ExecutionJournalEntry::Heartbeat { turn_id, .. }
        | ExecutionJournalEntry::NeedsReconciliation { turn_id, .. }
        | ExecutionJournalEntry::Settled { turn_id } => turn_id.clone(),
    }
}

pub(super) fn apply_execution_journal_entry(
    current: &mut Option<SessionExecutionState>,
    seq: u64,
    record_timestamp_ms: u64,
    entry: ExecutionJournalEntry,
) {
    match entry {
        ExecutionJournalEntry::Checkpoint { checkpoint } => {
            *current = Some(SessionExecutionState {
                phase: checkpoint.phase,
                checkpoint,
                checkpoint_seq: seq,
                status: SessionExecutionStatus::Running,
                last_heartbeat_ms: None,
                last_progress_ms: Some(record_timestamp_ms),
                reason: None,
                pending_batch: None,
                resume_requests: HashMap::new(),
            });
        }
        ExecutionJournalEntry::ToolBatchStarted { batch } => {
            if let Some(state) = current.as_mut() {
                state.phase = ExecutionPhase::ToolBatch;
                state.last_progress_ms = Some(record_timestamp_ms);
                state.pending_batch = Some(ExecutionToolBatch {
                    calls: batch
                        .calls
                        .iter()
                        .cloned()
                        .map(|call| ExecutionToolCall {
                            call,
                            started: false,
                            outcome: None,
                        })
                        .collect(),
                    intent: batch,
                });
            }
        }
        ExecutionJournalEntry::ToolCallStarted {
            turn_id,
            step,
            call_id,
        } => {
            if let Some(state) = current.as_mut()
                && state.checkpoint.turn_id == turn_id
                && let Some(batch) = state
                    .pending_batch
                    .as_mut()
                    .filter(|batch| batch.intent.step == step)
                && let Some(call) = batch.calls.iter_mut().find(|call| call.call.id == call_id)
            {
                call.started = true;
                state.last_progress_ms = Some(record_timestamp_ms);
            }
        }
        ExecutionJournalEntry::ToolCallFinished {
            turn_id,
            step,
            call_id,
            outcome,
        } => {
            if let Some(state) = current.as_mut()
                && state.checkpoint.turn_id == turn_id
                && let Some(batch) = state
                    .pending_batch
                    .as_mut()
                    .filter(|batch| batch.intent.step == step)
                && let Some(call) = batch.calls.iter_mut().find(|call| call.call.id == call_id)
            {
                call.outcome = Some(outcome);
                state.last_progress_ms = Some(record_timestamp_ms);
            }
        }
        ExecutionJournalEntry::ToolBatchSettled { turn_id, step } => {
            if let Some(state) = current.as_mut()
                && state.checkpoint.turn_id == turn_id
                && state
                    .pending_batch
                    .as_ref()
                    .is_some_and(|batch| batch.intent.step == step)
            {
                // Keep the completed batch until the post-batch checkpoint is
                // durable. A crash between these records can then replay the
                // journaled outcomes without asking the model or rerunning tools.
                state.last_progress_ms = Some(record_timestamp_ms);
            }
        }
        ExecutionJournalEntry::WaitingForContinue { turn_id, reason } => {
            if let Some(state) = current.as_mut()
                && state.checkpoint.turn_id == turn_id
            {
                state.status = SessionExecutionStatus::WaitingForContinue;
                state.reason = Some(reason);
            }
        }
        ExecutionJournalEntry::Resumed {
            turn_id,
            request_id,
            checkpoint_seq,
        } => {
            if let Some(state) = current.as_mut()
                && state.checkpoint.turn_id == turn_id
            {
                state.status = SessionExecutionStatus::Running;
                state.reason = None;
                state.resume_requests.insert(request_id, checkpoint_seq);
            }
        }
        ExecutionJournalEntry::Heartbeat {
            turn_id,
            phase,
            at_ms,
            last_progress_ms,
        } => {
            if let Some(state) = current.as_mut()
                && state.checkpoint.turn_id == turn_id
            {
                state.phase = phase;
                state.last_heartbeat_ms = Some(at_ms);
                state.last_progress_ms = Some(last_progress_ms);
            }
        }
        ExecutionJournalEntry::NeedsReconciliation { turn_id, reason } => {
            if let Some(state) = current.as_mut()
                && state.checkpoint.turn_id == turn_id
            {
                state.status = SessionExecutionStatus::NeedsReconciliation;
                state.reason = Some(reason);
            }
        }
        ExecutionJournalEntry::Settled { turn_id } => {
            if let Some(state) = current.as_mut()
                && state.checkpoint.turn_id == turn_id
            {
                state.status = SessionExecutionStatus::Settled;
                state.reason = None;
                state.pending_batch = None;
            }
        }
    }
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
            .stderr(Stdio::null())
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
