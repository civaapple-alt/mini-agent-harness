use mini_agent_protocol::{Tool, ToolError, ToolHandler, ToolRuntime, ToolSpec};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub const NOTEBOOK_FILE_NAME: &str = "notebook.json";
pub const MAX_NOTEBOOK_BYTES: usize = 64 * 1024;
const MAX_NOTEBOOK_ENTRIES: usize = 32;
const MAX_NOTEBOOK_KEY_BYTES: usize = 96;
const MAX_NOTEBOOK_ENTRY_BYTES: usize = 4 * 1024;

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NotebookEntry {
    pub key: String,
    pub content: String,
    pub updated_at_ms: u64,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NotebookSnapshot {
    pub version: u64,
    pub revision: u64,
    pub updated_at_ms: u64,
    pub entries: Vec<NotebookEntry>,
}

impl NotebookSnapshot {
    pub fn summary(&self, max_bytes: usize) -> String {
        let full = self
            .entries
            .iter()
            .map(|entry| format!("{}: {}", entry.key, entry.content))
            .collect::<Vec<_>>()
            .join("\n");
        if full.len() <= max_bytes {
            return full;
        }
        if max_bytes < '…'.len_utf8() {
            return full
                .chars()
                .take_while(|character| character.len_utf8() <= max_bytes)
                .collect();
        }
        let mut summary = String::new();
        for character in full.chars() {
            if summary.len() + character.len_utf8() + '…'.len_utf8() > max_bytes {
                break;
            }
            summary.push(character);
        }
        summary.push('…');
        summary
    }
}

pub fn read_notebook(path: &Path) -> Result<NotebookSnapshot, String> {
    if !path.is_file() {
        return Ok(empty_snapshot());
    }
    let bytes = fs::read(path).map_err(|error| format!("cannot read notebook: {error}"))?;
    if bytes.len() > MAX_NOTEBOOK_BYTES {
        return Err(format!("notebook exceeds {MAX_NOTEBOOK_BYTES} byte limit"));
    }
    let snapshot: NotebookSnapshot =
        serde_json::from_slice(&bytes).map_err(|error| format!("invalid notebook: {error}"))?;
    validate_snapshot(&snapshot)?;
    Ok(snapshot)
}

pub fn upsert_notebook(
    path: &Path,
    key: &str,
    content: &str,
    append: bool,
) -> Result<NotebookSnapshot, String> {
    validate_key(key)?;
    validate_content(content)?;
    let mut snapshot = read_notebook(path)?;
    let now = timestamp_ms();
    if let Some(entry) = snapshot.entries.iter_mut().find(|entry| entry.key == key) {
        if append && !entry.content.is_empty() {
            entry.content.push('\n');
        }
        if append {
            entry.content.push_str(content);
        } else {
            entry.content = content.to_string();
        }
        validate_content(&entry.content)?;
        entry.updated_at_ms = now;
    } else {
        if snapshot.entries.len() >= MAX_NOTEBOOK_ENTRIES {
            return Err(format!("notebook exceeds {MAX_NOTEBOOK_ENTRIES} entries"));
        }
        snapshot.entries.push(NotebookEntry {
            key: key.to_string(),
            content: content.to_string(),
            updated_at_ms: now,
        });
    }
    snapshot.version = 1;
    snapshot.revision = snapshot.revision.saturating_add(1);
    snapshot.updated_at_ms = now;
    validate_snapshot(&snapshot)?;
    write_snapshot(path, &snapshot)?;
    Ok(snapshot)
}

fn empty_snapshot() -> NotebookSnapshot {
    NotebookSnapshot {
        version: 1,
        ..NotebookSnapshot::default()
    }
}

fn validate_snapshot(snapshot: &NotebookSnapshot) -> Result<(), String> {
    if snapshot.version != 1 {
        return Err("unsupported notebook version".to_string());
    }
    if snapshot.entries.len() > MAX_NOTEBOOK_ENTRIES {
        return Err(format!("notebook exceeds {MAX_NOTEBOOK_ENTRIES} entries"));
    }
    for entry in &snapshot.entries {
        validate_key(&entry.key)?;
        validate_content(&entry.content)?;
    }
    let encoded = serde_json::to_vec(snapshot).map_err(|error| error.to_string())?;
    if encoded.len() > MAX_NOTEBOOK_BYTES {
        return Err(format!("notebook exceeds {MAX_NOTEBOOK_BYTES} byte limit"));
    }
    Ok(())
}

fn validate_key(key: &str) -> Result<(), String> {
    if key.trim().is_empty() || key.len() > MAX_NOTEBOOK_KEY_BYTES {
        return Err(format!(
            "notebook key must be 1..={MAX_NOTEBOOK_KEY_BYTES} bytes"
        ));
    }
    Ok(())
}

fn validate_content(content: &str) -> Result<(), String> {
    if content.len() > MAX_NOTEBOOK_ENTRY_BYTES {
        return Err(format!(
            "notebook entry exceeds {MAX_NOTEBOOK_ENTRY_BYTES} bytes"
        ));
    }
    Ok(())
}

fn write_snapshot(path: &Path, snapshot: &NotebookSnapshot) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "notebook has no parent directory".to_string())?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("cannot create notebook directory: {error}"))?;
    let encoded = serde_json::to_vec_pretty(snapshot).map_err(|error| error.to_string())?;
    let temp = parent.join(format!(".notebook-{}.tmp", timestamp_ms()));
    fs::write(&temp, encoded).map_err(|error| format!("cannot write notebook: {error}"))?;
    if let Err(error) = fs::rename(&temp, path) {
        let _ = fs::remove_file(&temp);
        return Err(format!("cannot commit notebook: {error}"));
    }
    Ok(())
}

fn timestamp_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

enum NotebookMode {
    Read,
    Write,
}

struct NotebookTool {
    path: PathBuf,
    mode: NotebookMode,
    lock: Arc<Mutex<()>>,
}

impl ToolHandler for NotebookTool {
    fn spec(&self) -> ToolSpec {
        match self.mode {
            NotebookMode::Read => ToolSpec {
                name: "notebook_read".to_string(),
                description: "Read the bounded Session notebook. Use it for durable facts across turns and context compaction; omit key to read all entries.".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {"key": {"type": "string"}},
                    "additionalProperties": false
                }),
            },
            NotebookMode::Write => ToolSpec {
                name: "notebook_write".to_string(),
                description: "Write one bounded fact to the Session notebook. Use a stable key; set append=true to add to an existing entry. This is durable Session state, not a workspace file.".to_string(),
                parameters: json!({
                    "type": "object",
                    "required": ["key", "content"],
                    "properties": {
                        "key": {"type": "string"},
                        "content": {"type": "string"},
                        "append": {"type": "boolean"}
                    },
                    "additionalProperties": false
                }),
            },
        }
    }
}

impl ToolRuntime for NotebookTool {
    fn execute(&self, arguments: &Value) -> Result<String, ToolError> {
        let _guard = self.lock.lock().unwrap();
        match self.mode {
            NotebookMode::Read => {
                let snapshot = read_notebook(&self.path).map_err(ToolError)?;
                let key = arguments.get("key").and_then(Value::as_str);
                if let Some(key) = key {
                    let entry = snapshot.entries.iter().find(|entry| entry.key == key);
                    serde_json::to_string(&entry).map_err(|error| ToolError(error.to_string()))
                } else {
                    serde_json::to_string(&snapshot).map_err(|error| ToolError(error.to_string()))
                }
            }
            NotebookMode::Write => {
                let key = arguments
                    .get("key")
                    .and_then(Value::as_str)
                    .ok_or_else(|| ToolError("notebook_write requires key".to_string()))?;
                let content = arguments
                    .get("content")
                    .and_then(Value::as_str)
                    .ok_or_else(|| ToolError("notebook_write requires content".to_string()))?;
                let append = arguments
                    .get("append")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let snapshot =
                    upsert_notebook(&self.path, key, content, append).map_err(ToolError)?;
                serde_json::to_string(&json!({
                    "key": key,
                    "revision": snapshot.revision,
                    "entries": snapshot.entries.len()
                }))
                .map_err(|error| ToolError(error.to_string()))
            }
        }
    }
}

pub fn notebook_tools(session_dir: PathBuf) -> Vec<Box<dyn Tool>> {
    let path = session_dir.join(NOTEBOOK_FILE_NAME);
    let lock = Arc::new(Mutex::new(()));
    vec![
        Box::new(NotebookTool {
            path: path.clone(),
            mode: NotebookMode::Read,
            lock: Arc::clone(&lock),
        }),
        Box::new(NotebookTool {
            path,
            mode: NotebookMode::Write,
            lock,
        }),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_path() -> PathBuf {
        std::env::temp_dir().join(format!(
            "mini-agent-notebook-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn notebook_upsert_is_bounded_and_durable() {
        let root = temp_path();
        let path = root.join(NOTEBOOK_FILE_NAME);
        let first = upsert_notebook(&path, "facts", "one", false).unwrap();
        assert_eq!(first.revision, 1);
        let second = upsert_notebook(&path, "facts", "two", true).unwrap();
        assert_eq!(second.entries[0].content, "one\ntwo");
        assert_eq!(read_notebook(&path).unwrap(), second);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn notebook_rejects_oversized_entry() {
        let root = temp_path();
        let path = root.join(NOTEBOOK_FILE_NAME);
        let content = "x".repeat(MAX_NOTEBOOK_ENTRY_BYTES + 1);
        assert!(upsert_notebook(&path, "facts", &content, false).is_err());
        let _ = fs::remove_dir_all(root);
    }
}
