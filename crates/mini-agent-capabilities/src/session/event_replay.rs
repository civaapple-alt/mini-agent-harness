use mini_agent_protocol::{Event, EventEnvelope, ThreadId, TurnId, TurnSource};
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub const MAX_SESSION_EVENT_REPLAY_ENTRIES: usize = 512;
const REPLAY_SCHEMA_VERSION: u64 = 1;
const MAX_REPLAY_BYTES: usize = 1024 * 1024;

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionEventContextSource {
    pub source_id: String,
    pub version_fingerprint: String,
}

/// A compact lifecycle record. It never contains prompts, deltas, tool
/// arguments, tool output, or injected context bodies.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionEventReplayEntry {
    pub thread_id: ThreadId,
    pub turn_id: Option<TurnId>,
    pub sequence: u64,
    pub item_id: Option<String>,
    pub turn_source: Option<TurnSource>,
    pub event_type: String,
    pub tool_call_id: Option<String>,
    pub tool_name: Option<String>,
    pub context_sources: Vec<SessionEventContextSource>,
    pub recorded_at_ms: u64,
}

#[derive(Clone)]
pub struct SessionEventReplayStore {
    session_id: String,
    path: PathBuf,
    append_lock: Arc<Mutex<()>>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ReplayFile {
    schema_version: u64,
    session_id: String,
    entries: Vec<SessionEventReplayEntry>,
}

impl SessionEventReplayEntry {
    pub fn from_event(event: &EventEnvelope, recorded_at_ms: u64) -> Option<Self> {
        let (event_type, tool_call_id, tool_name, context_sources) = match &event.event {
            Event::TurnStarted { .. } => ("turn_started", None, None, Vec::new()),
            Event::SkillsLoaded { .. } => ("skills_loaded", None, None, Vec::new()),
            Event::SkillsLoadFailed { .. } => ("skills_load_failed", None, None, Vec::new()),
            Event::SkillGroupActivated { .. } => ("skill_group_activated", None, None, Vec::new()),
            Event::ContextInjected { records } => (
                "context_injected",
                None,
                None,
                records
                    .iter()
                    .take(32)
                    .map(|record| SessionEventContextSource {
                        source_id: record.id.clone(),
                        version_fingerprint: record.fingerprint.clone(),
                    })
                    .collect(),
            ),
            Event::RunStarted { .. } => ("run_started", None, None, Vec::new()),
            Event::ModelStarted { .. } => ("model_started", None, None, Vec::new()),
            Event::AssistantReasoningDelta { .. } | Event::AssistantTextDelta { .. } => {
                return None;
            }
            Event::ModelResponded { .. } => ("model_responded", None, None, Vec::new()),
            Event::ToolStarted { call } => (
                "tool_started",
                Some(call.id.clone()),
                Some(call.name.clone()),
                Vec::new(),
            ),
            Event::ToolFinished { call_id, name, .. } => (
                "tool_finished",
                Some(call_id.clone()),
                Some(name.clone()),
                Vec::new(),
            ),
            Event::ContextCompactionStarted { .. } => {
                ("context_compaction_started", None, None, Vec::new())
            }
            Event::ContextCompactionFinished { .. } => {
                ("context_compaction_finished", None, None, Vec::new())
            }
            Event::RunFinished { .. } => ("run_finished", None, None, Vec::new()),
            Event::TurnFinished { .. } => ("turn_finished", None, None, Vec::new()),
            Event::RunFailed { .. } => ("run_failed", None, None, Vec::new()),
        };
        Some(Self {
            thread_id: event.thread_id.clone(),
            turn_id: event.turn_id.clone(),
            sequence: event.sequence,
            item_id: event.item_id.clone(),
            turn_source: event.turn_source,
            event_type: event_type.to_string(),
            tool_call_id,
            tool_name,
            context_sources,
            recorded_at_ms,
        })
    }
}

impl SessionEventReplayStore {
    pub(super) fn new(
        session_dir: &Path,
        session_id: impl Into<String>,
        append_lock: Arc<Mutex<()>>,
    ) -> Self {
        Self {
            session_id: session_id.into(),
            path: session_dir.join("turn_event_replay.json"),
            append_lock,
        }
    }

    pub fn append(&self, entry: SessionEventReplayEntry) -> Result<(), String> {
        validate_entry(&entry)?;
        let _lock = self.append_lock.lock().unwrap();
        let mut file = self.read_file()?;
        if file.entries.iter().any(|existing| {
            existing.thread_id == entry.thread_id && existing.sequence == entry.sequence
        }) {
            return Ok(());
        }
        file.entries.push(entry);
        if file.entries.len() > MAX_SESSION_EVENT_REPLAY_ENTRIES {
            let remove = file.entries.len() - MAX_SESSION_EVENT_REPLAY_ENTRIES;
            file.entries.drain(..remove);
        }
        self.write_file(&file)
    }

    pub fn entries(&self) -> Result<Vec<SessionEventReplayEntry>, String> {
        let _lock = self.append_lock.lock().unwrap();
        Ok(self.read_file()?.entries)
    }

    fn read_file(&self) -> Result<ReplayFile, String> {
        let bytes = match fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(ReplayFile {
                    schema_version: REPLAY_SCHEMA_VERSION,
                    session_id: self.session_id.clone(),
                    entries: Vec::new(),
                });
            }
            Err(error) => return Err(format!("cannot read event replay: {error}")),
        };
        if bytes.len() > MAX_REPLAY_BYTES {
            return Err("Session event replay exceeds its 1 MiB limit".to_string());
        }
        let file: ReplayFile = serde_json::from_slice(&bytes)
            .map_err(|error| format!("cannot decode Session event replay: {error}"))?;
        if file.schema_version != REPLAY_SCHEMA_VERSION || file.session_id != self.session_id {
            return Err("Session event replay schema or identity is invalid".to_string());
        }
        if file.entries.len() > MAX_SESSION_EVENT_REPLAY_ENTRIES {
            return Err("Session event replay entry count exceeds its limit".to_string());
        }
        for entry in &file.entries {
            validate_entry(entry)?;
        }
        Ok(file)
    }

    fn write_file(&self, file: &ReplayFile) -> Result<(), String> {
        let bytes = serde_json::to_vec(file)
            .map_err(|error| format!("cannot encode Session event replay: {error}"))?;
        if bytes.len() > MAX_REPLAY_BYTES {
            return Err("Session event replay exceeds its 1 MiB limit".to_string());
        }
        let parent = self
            .path
            .parent()
            .ok_or_else(|| "Session event replay has no parent directory".to_string())?;
        let temp = parent.join(".turn_event_replay.tmp");
        fs::write(&temp, &bytes)
            .map_err(|error| format!("cannot write Session event replay: {error}"))?;
        OpenOptions::new()
            .write(true)
            .open(&temp)
            .and_then(|file| file.sync_all())
            .map_err(|error| format!("cannot sync Session event replay: {error}"))?;
        fs::rename(&temp, &self.path).map_err(|error| {
            let _ = fs::remove_file(&temp);
            format!("cannot replace Session event replay: {error}")
        })?;
        Ok(())
    }
}

fn validate_entry(entry: &SessionEventReplayEntry) -> Result<(), String> {
    if entry.sequence == 0
        || entry.event_type.len() > 64
        || entry
            .item_id
            .as_ref()
            .is_some_and(|value| value.len() > 128)
        || entry
            .tool_call_id
            .as_ref()
            .is_some_and(|value| value.len() > 128)
        || entry
            .tool_name
            .as_ref()
            .is_some_and(|value| value.len() > 128)
        || entry.context_sources.len() > 32
        || entry
            .context_sources
            .iter()
            .any(|source| source.source_id.len() > 128 || source.version_fingerprint.len() > 128)
    {
        return Err("Session event replay entry exceeds a fixed field limit".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{SessionRequest, SessionStore};
    use mini_agent_protocol::{Event, ThreadId, ToolCall, TurnId};
    use serde_json::json;

    #[test]
    fn compact_tool_events_omit_arguments_and_output_bodies() {
        let started = EventEnvelope::new(
            ThreadId::new("thread-1"),
            Some(TurnId::new("turn-1")),
            10,
            Event::ToolStarted {
                call: ToolCall {
                    id: "call-1".to_string(),
                    name: "shell".to_string(),
                    arguments: json!({"command":"secret-command"}),
                },
            },
        );
        let finished = EventEnvelope::new(
            ThreadId::new("thread-1"),
            Some(TurnId::new("turn-1")),
            11,
            Event::ToolFinished {
                call_id: "call-1".to_string(),
                name: "shell".to_string(),
                arguments: json!({"command":"secret-command"}),
                content: "secret-tool-output".to_string(),
                is_error: false,
                truncated: false,
                outcome: None,
            },
        );
        let encoded = serde_json::to_string(&[
            SessionEventReplayEntry::from_event(&started, 1).unwrap(),
            SessionEventReplayEntry::from_event(&finished, 2).unwrap(),
        ])
        .unwrap();

        assert!(encoded.contains("call-1"));
        assert!(encoded.contains("tool_started"));
        assert!(encoded.contains("tool_finished"));
        assert!(!encoded.contains("secret-command"));
        assert!(!encoded.contains("secret-tool-output"));
    }

    #[test]
    fn text_deltas_are_not_persistable_replay_events() {
        let event = EventEnvelope::new(
            ThreadId::new("thread-1"),
            Some(TurnId::new("turn-1")),
            1,
            Event::AssistantTextDelta {
                delta: "secret text".to_string(),
            },
        );

        assert!(SessionEventReplayEntry::from_event(&event, 1).is_none());
    }

    #[test]
    fn replay_ring_stays_bounded_after_session_reopen() {
        let root = crate::test_support::test_root();
        let opened = SessionStore::open(&root, SessionRequest::New).unwrap();
        let session_id = opened.store.session_id().to_string();
        let store = opened.store.event_replay_store();
        for sequence in 1..=MAX_SESSION_EVENT_REPLAY_ENTRIES as u64 + 1 {
            store
                .append(SessionEventReplayEntry {
                    thread_id: ThreadId::new(opened.store.thread_id()),
                    turn_id: Some(TurnId::new("turn-bounded-replay")),
                    sequence,
                    item_id: None,
                    turn_source: None,
                    event_type: "run_started".to_string(),
                    tool_call_id: None,
                    tool_name: None,
                    context_sources: Vec::new(),
                    recorded_at_ms: sequence,
                })
                .unwrap();
        }
        drop(store);
        drop(opened);

        let reopened = SessionStore::open(&root, SessionRequest::Resume(session_id)).unwrap();
        let entries = reopened.store.event_replay_store().entries().unwrap();
        assert_eq!(entries.len(), MAX_SESSION_EVENT_REPLAY_ENTRIES);
        assert_eq!(entries.first().unwrap().sequence, 2);
        assert_eq!(entries.last().unwrap().sequence, 513);

        drop(reopened);
        crate::test_support::remove_test_root(&root);
    }
}
