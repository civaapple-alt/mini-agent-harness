use mini_agent_protocol::{Tool, ToolError, ToolHandler, ToolRuntime, ToolSpec};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub const NOTEBOOK_FILE_NAME: &str = "notebook.json";
pub const MAX_NOTEBOOK_BYTES: usize = 64 * 1024;
pub const MAX_NOTEBOOK_ENTRIES: usize = 64;
pub const MAX_NOTEBOOK_KEYWORDS: usize = 12;
pub const MAX_NOTEBOOK_EVIDENCE: usize = 8;
pub const MAX_EVIDENCE_SUBJECT_CHARS: usize = 160;
const MAX_NOTEBOOK_KEY_BYTES: usize = 96;
const MAX_NOTEBOOK_ENTRY_BYTES: usize = 4 * 1024;
const MAX_NOTEBOOK_KEYWORD_BYTES: usize = 64;
const MAX_EVIDENCE_FIELD_BYTES: usize = 256;
const MAX_EVIDENCE_SUBJECT_BYTES: usize = 1024;
const NOTEBOOK_METADATA_BUDGET_BYTES: usize = 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NotebookLimits {
    pub max_entries: usize,
    pub max_entry_bytes: usize,
    pub max_bytes: usize,
}

impl Default for NotebookLimits {
    fn default() -> Self {
        Self {
            max_entries: MAX_NOTEBOOK_ENTRIES,
            max_entry_bytes: MAX_NOTEBOOK_ENTRY_BYTES,
            max_bytes: MAX_NOTEBOOK_BYTES,
        }
    }
}

impl NotebookLimits {
    pub fn from_env() -> Self {
        let max_entries = env_limit("MINI_AGENT_NOTEBOOK_MAX_ENTRIES", 1, MAX_NOTEBOOK_ENTRIES)
            .unwrap_or(MAX_NOTEBOOK_ENTRIES);
        let max_entry_bytes = env_limit(
            "MINI_AGENT_NOTEBOOK_MAX_ENTRY_BYTES",
            256,
            MAX_NOTEBOOK_ENTRY_BYTES,
        )
        .unwrap_or(MAX_NOTEBOOK_ENTRY_BYTES);
        Self {
            max_entries,
            max_entry_bytes,
            max_bytes: (max_entries * (max_entry_bytes + NOTEBOOK_METADATA_BUDGET_BYTES))
                .min(MAX_NOTEBOOK_BYTES),
        }
    }
}

fn env_limit(name: &str, minimum: usize, maximum: usize) -> Option<usize> {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .map(|value: usize| value.clamp(minimum, maximum))
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum NotebookImportance {
    Critical,
    High,
    #[default]
    Normal,
    Temporary,
}

impl NotebookImportance {
    fn rank(&self) -> u8 {
        match self {
            Self::Critical => 4,
            Self::High => 3,
            Self::Normal => 2,
            Self::Temporary => 1,
        }
    }

    pub fn parse(value: Option<&str>) -> Result<Self, String> {
        match value.unwrap_or("normal") {
            "critical" => Ok(Self::Critical),
            "high" => Ok(Self::High),
            "normal" => Ok(Self::Normal),
            "temporary" => Ok(Self::Temporary),
            _ => {
                Err("notebook importance must be critical, high, normal, or temporary".to_string())
            }
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NotebookEntry {
    pub key: String,
    pub content: String,
    #[serde(default)]
    pub importance: NotebookImportance,
    pub updated_at_ms: u64,
    #[serde(default)]
    pub keywords: Vec<String>,
    #[serde(default)]
    pub evidence: Vec<Value>,
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
        let mut entries = self.entries.clone();
        entries.sort_by(|left, right| {
            right
                .importance
                .rank()
                .cmp(&left.importance.rank())
                .then_with(|| right.updated_at_ms.cmp(&left.updated_at_ms))
                .then_with(|| left.key.cmp(&right.key))
        });
        let full = entries
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
    let limits = NotebookLimits::from_env();
    if !path.is_file() {
        return Ok(empty_snapshot());
    }
    let bytes = fs::read(path).map_err(|error| format!("cannot read notebook: {error}"))?;
    if bytes.len() > limits.max_bytes {
        return Err(format!("notebook exceeds {} byte limit", limits.max_bytes));
    }
    let snapshot: NotebookSnapshot =
        serde_json::from_slice(&bytes).map_err(|error| format!("invalid notebook: {error}"))?;
    validate_snapshot_with_limits(&snapshot, limits)?;
    Ok(snapshot)
}

pub fn read_notebook_scope(session_dir: &Path, scope: &str) -> Result<NotebookSnapshot, String> {
    let path = match scope {
        "self" => session_dir.join(NOTEBOOK_FILE_NAME),
        "parent" => parent_notebook_path(session_dir)
            .ok_or_else(|| "parent notebook scope is unavailable".to_string())?,
        _ => return Err("notebook scope must be self or parent".to_string()),
    };
    read_notebook(&path)
}

pub fn upsert_notebook(
    path: &Path,
    key: &str,
    content: &str,
    append: bool,
) -> Result<NotebookSnapshot, String> {
    upsert_notebook_with_importance(path, key, content, append, NotebookImportance::Normal)
}

pub fn upsert_notebook_with_importance(
    path: &Path,
    key: &str,
    content: &str,
    append: bool,
    importance: NotebookImportance,
) -> Result<NotebookSnapshot, String> {
    upsert_notebook_with_metadata(path, key, content, append, importance, None, None)
}

pub fn upsert_notebook_with_metadata(
    path: &Path,
    key: &str,
    content: &str,
    append: bool,
    importance: NotebookImportance,
    keywords: Option<Vec<String>>,
    evidence: Option<Vec<Value>>,
) -> Result<NotebookSnapshot, String> {
    let limits = NotebookLimits::from_env();
    validate_key(key)?;
    validate_content(content, limits.max_entry_bytes)?;
    let keywords = keywords.map(validate_keywords).transpose()?;
    let evidence = evidence.map(validate_evidence).transpose()?;
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
        validate_content(&entry.content, limits.max_entry_bytes)?;
        entry.importance = importance;
        entry.updated_at_ms = now;
        if let Some(keywords) = keywords {
            entry.keywords = keywords;
        }
        if let Some(evidence) = evidence {
            entry.evidence = evidence;
        }
    } else {
        if snapshot.entries.len() >= limits.max_entries {
            return Err(format!("notebook exceeds {} entries", limits.max_entries));
        }
        snapshot.entries.push(NotebookEntry {
            key: key.to_string(),
            content: content.to_string(),
            importance,
            updated_at_ms: now,
            keywords: keywords.unwrap_or_default(),
            evidence: evidence.unwrap_or_default(),
        });
    }
    snapshot.version = 1;
    snapshot.revision = snapshot.revision.saturating_add(1);
    snapshot.updated_at_ms = now;
    validate_snapshot_with_limits(&snapshot, limits)?;
    write_snapshot(path, &snapshot)?;
    Ok(snapshot)
}

pub fn forget_notebook(path: &Path, key: &str) -> Result<NotebookSnapshot, String> {
    let limits = NotebookLimits::from_env();
    validate_key(key)?;
    let mut snapshot = read_notebook(path)?;
    snapshot.entries.retain(|entry| entry.key != key);
    snapshot.version = 1;
    snapshot.revision = snapshot.revision.saturating_add(1);
    snapshot.updated_at_ms = timestamp_ms();
    validate_snapshot_with_limits(&snapshot, limits)?;
    write_snapshot(path, &snapshot)?;
    Ok(snapshot)
}

fn empty_snapshot() -> NotebookSnapshot {
    NotebookSnapshot {
        version: 1,
        ..NotebookSnapshot::default()
    }
}

fn validate_snapshot_with_limits(
    snapshot: &NotebookSnapshot,
    limits: NotebookLimits,
) -> Result<(), String> {
    if snapshot.version != 1 {
        return Err("unsupported notebook version".to_string());
    }
    if snapshot.entries.len() > limits.max_entries {
        return Err(format!("notebook exceeds {} entries", limits.max_entries));
    }
    for entry in &snapshot.entries {
        validate_key(&entry.key)?;
        validate_content(&entry.content, limits.max_entry_bytes)?;
        validate_keywords(entry.keywords.clone())?;
        validate_evidence(entry.evidence.clone())?;
    }
    let encoded = serde_json::to_vec(snapshot).map_err(|error| error.to_string())?;
    if encoded.len() > limits.max_bytes {
        return Err(format!("notebook exceeds {} byte limit", limits.max_bytes));
    }
    Ok(())
}

fn validate_keywords(keywords: Vec<String>) -> Result<Vec<String>, String> {
    if keywords.len() > MAX_NOTEBOOK_KEYWORDS {
        return Err(format!(
            "notebook entry exceeds {MAX_NOTEBOOK_KEYWORDS} keywords"
        ));
    }
    let mut normalized = Vec::with_capacity(keywords.len());
    for keyword in keywords {
        let keyword = keyword.trim();
        if keyword.is_empty() || keyword.len() > MAX_NOTEBOOK_KEYWORD_BYTES {
            return Err(format!(
                "notebook keyword must be 1..={MAX_NOTEBOOK_KEYWORD_BYTES} bytes"
            ));
        }
        if !normalized.iter().any(|existing| existing == keyword) {
            normalized.push(keyword.to_string());
        }
    }
    Ok(normalized)
}

fn validate_evidence(evidence: Vec<Value>) -> Result<Vec<Value>, String> {
    if evidence.len() > MAX_NOTEBOOK_EVIDENCE {
        return Err(format!(
            "notebook entry exceeds {MAX_NOTEBOOK_EVIDENCE} evidence records"
        ));
    }
    let mut evidence = evidence;
    for item in &mut evidence {
        let object = item
            .as_object_mut()
            .ok_or_else(|| "notebook evidence must be an object".to_string())?;
        let kind = object
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if kind != "commit" && kind != "file" {
            return Err("notebook evidence kind must be commit or file".to_string());
        }
        for name in [
            "project",
            "commit",
            "path",
            "authorAt",
            "committedAt",
            "contentSha256",
        ] {
            if let Some(field) = object.get(name).and_then(Value::as_str)
                && (field.trim().is_empty() || field.len() > MAX_EVIDENCE_FIELD_BYTES)
            {
                return Err(format!(
                    "notebook evidence fields must be 1..={MAX_EVIDENCE_FIELD_BYTES} bytes"
                ));
            }
        }
        if object
            .get("lines")
            .and_then(Value::as_array)
            .is_some_and(|lines| lines.len() > 2)
        {
            return Err("notebook evidence lines must contain at most two values".to_string());
        }
        if let Some(subject) = object
            .remove("subject")
            .and_then(|value| value.as_str().map(str::to_string))
        {
            let normalized_subject = subject.split_whitespace().collect::<Vec<_>>().join(" ");
            let mut chars = normalized_subject.chars();
            let mut bounded = chars
                .by_ref()
                .take(MAX_EVIDENCE_SUBJECT_CHARS)
                .collect::<String>();
            object.insert(
                "subjectTruncated".to_string(),
                Value::Bool(chars.next().is_some()),
            );
            while bounded.len() > MAX_EVIDENCE_SUBJECT_BYTES {
                bounded.pop();
            }
            if !bounded.is_empty() {
                object.insert("subject".to_string(), Value::String(bounded));
            }
        }
        if object
            .get("recordedAtMs")
            .and_then(Value::as_u64)
            .unwrap_or_default()
            == 0
        {
            object.insert("recordedAtMs".to_string(), json!(timestamp_ms()));
        }
    }
    Ok(evidence)
}

fn validate_key(key: &str) -> Result<(), String> {
    if key.trim().is_empty() || key.len() > MAX_NOTEBOOK_KEY_BYTES {
        return Err(format!(
            "notebook key must be 1..={MAX_NOTEBOOK_KEY_BYTES} bytes"
        ));
    }
    Ok(())
}

fn validate_content(content: &str, max_bytes: usize) -> Result<(), String> {
    if content.len() > max_bytes {
        return Err(format!("notebook entry exceeds {max_bytes} bytes"));
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
    Forget,
}

struct NotebookTool {
    path: PathBuf,
    parent_path: Option<PathBuf>,
    mode: NotebookMode,
    lock: Arc<Mutex<()>>,
}

impl ToolHandler for NotebookTool {
    fn spec(&self) -> ToolSpec {
        match self.mode {
            NotebookMode::Read => ToolSpec {
                name: "notebook_read".to_string(),
                description: "Read the bounded Session notebook. Use scope=parent only when a Child needs a verified, read-only fact from its parent Session. Omit key to read all entries.".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "key": {"type": "string"},
                        "scope": {"type": "string", "enum": ["self", "parent"]}
                    },
                    "additionalProperties": false
                }),
            },
            NotebookMode::Write => ToolSpec {
                name: "notebook_write".to_string(),
                description: "Write one bounded fact to this Session notebook. Use a stable key; set append=true to add to an existing entry. Importance controls future summary ordering.".to_string(),
                parameters: json!({
                    "type": "object",
                    "required": ["key", "content"],
                    "properties": {
                        "key": {"type": "string"},
                        "content": {"type": "string"},
                        "append": {"type": "boolean"},
                        "importance": {"type": "string", "enum": ["critical", "high", "normal", "temporary"]},
                        "keywords": {"type": "array", "maxItems": 12, "items": {"type": "string"}},
                        "evidence": {"type": "array", "maxItems": 8, "items": {"type": "object"}}
                    },
                    "additionalProperties": false
                }),
            },
            NotebookMode::Forget => ToolSpec {
                name: "notebook_forget".to_string(),
                description: "Forget one key from this Session notebook. This changes future memory projections but does not erase Checkpoint history.".to_string(),
                parameters: json!({
                    "type": "object",
                    "required": ["key"],
                    "properties": {"key": {"type": "string"}},
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
                let scope = arguments
                    .get("scope")
                    .and_then(Value::as_str)
                    .unwrap_or("self");
                let path = match scope {
                    "self" => &self.path,
                    "parent" => self.parent_path.as_ref().ok_or_else(|| {
                        ToolError("parent notebook scope is unavailable".to_string())
                    })?,
                    _ => {
                        return Err(ToolError(
                            "notebook_read scope must be self or parent".to_string(),
                        ));
                    }
                };
                let snapshot = read_notebook(path).map_err(ToolError)?;
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
                let importance =
                    NotebookImportance::parse(arguments.get("importance").and_then(Value::as_str))
                        .map_err(ToolError)?;
                let keywords = arguments
                    .get("keywords")
                    .map(|value| {
                        value
                            .as_array()
                            .ok_or_else(|| "notebook_write keywords must be an array".to_string())
                            .and_then(|items| {
                                items
                                    .iter()
                                    .map(|item| {
                                        item.as_str().map(str::to_string).ok_or_else(|| {
                                            "notebook keyword must be a string".to_string()
                                        })
                                    })
                                    .collect()
                            })
                    })
                    .transpose()
                    .map_err(ToolError)?;
                let evidence = arguments
                    .get("evidence")
                    .map(|value| {
                        serde_json::from_value::<Vec<Value>>(value.clone())
                            .map_err(|error| format!("notebook evidence is invalid: {error}"))
                    })
                    .transpose()
                    .map_err(ToolError)?;
                let snapshot = upsert_notebook_with_metadata(
                    &self.path, key, content, append, importance, keywords, evidence,
                )
                .map_err(ToolError)?;
                serde_json::to_string(&json!({
                    "key": key,
                    "revision": snapshot.revision,
                    "entries": snapshot.entries.len()
                }))
                .map_err(|error| ToolError(error.to_string()))
            }
            NotebookMode::Forget => {
                let key = arguments
                    .get("key")
                    .and_then(Value::as_str)
                    .ok_or_else(|| ToolError("notebook_forget requires key".to_string()))?;
                let snapshot = forget_notebook(&self.path, key).map_err(ToolError)?;
                serde_json::to_string(&json!({
                    "key": key,
                    "revision": snapshot.revision,
                    "entries": snapshot.entries.len(),
                }))
                .map_err(|error| ToolError(error.to_string()))
            }
        }
    }
}

pub fn notebook_tools(session_dir: PathBuf) -> Vec<Box<dyn Tool>> {
    let path = session_dir.join(NOTEBOOK_FILE_NAME);
    let parent_path = parent_notebook_path(&session_dir);
    let lock = Arc::new(Mutex::new(()));
    vec![
        Box::new(NotebookTool {
            path: path.clone(),
            parent_path: parent_path.clone(),
            mode: NotebookMode::Read,
            lock: Arc::clone(&lock),
        }),
        Box::new(NotebookTool {
            path: path.clone(),
            parent_path: None,
            mode: NotebookMode::Write,
            lock: Arc::clone(&lock),
        }),
        Box::new(NotebookTool {
            path: path.clone(),
            parent_path: None,
            mode: NotebookMode::Forget,
            lock,
        }),
    ]
}

fn parent_notebook_path(session_dir: &Path) -> Option<PathBuf> {
    let session_path = session_dir.join("session.jsonl");
    let first_line = fs::read_to_string(session_path)
        .ok()?
        .lines()
        .next()?
        .to_string();
    let header: Value = serde_json::from_str(&first_line).ok()?;
    if header.get("kind").and_then(Value::as_str) != Some("session_created") {
        return None;
    }
    let parent_id = header
        .get("forked_from")
        .and_then(|value| value.get("parent_session_id"))
        .and_then(Value::as_str)?;
    if parent_id.is_empty()
        || parent_id.len() > 128
        || !parent_id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        return None;
    }
    let sessions_root = session_dir.parent()?.canonicalize().ok()?;
    let parent_dir = sessions_root.join(parent_id).canonicalize().ok()?;
    if !parent_dir.starts_with(&sessions_root) || !parent_dir.is_dir() {
        return None;
    }
    Some(parent_dir.join(NOTEBOOK_FILE_NAME))
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

    #[test]
    fn importance_is_persisted_and_controls_summary_order() {
        let root = temp_path();
        let path = root.join(NOTEBOOK_FILE_NAME);
        upsert_notebook_with_importance(
            &path,
            "normal-fact",
            "normal",
            false,
            NotebookImportance::Normal,
        )
        .unwrap();
        upsert_notebook_with_importance(
            &path,
            "critical-fact",
            "critical",
            false,
            NotebookImportance::Critical,
        )
        .unwrap();
        let snapshot = read_notebook(&path).unwrap();
        assert_eq!(snapshot.entries[1].importance, NotebookImportance::Critical);
        assert!(
            snapshot
                .summary(1024)
                .starts_with("critical-fact: critical")
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn forgetting_a_key_keeps_checkpoint_independent_revision() {
        let root = temp_path();
        let path = root.join(NOTEBOOK_FILE_NAME);
        let first = upsert_notebook(&path, "facts", "one", false).unwrap();
        let second = forget_notebook(&path, "facts").unwrap();
        assert_eq!(second.revision, first.revision + 1);
        assert!(second.entries.is_empty());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn child_notebook_scope_resolves_only_verified_parent_lineage() {
        let root = temp_path();
        let parent_dir = root.join("parent");
        let child_dir = root.join("child");
        fs::create_dir_all(&parent_dir).unwrap();
        fs::create_dir_all(&child_dir).unwrap();
        fs::write(
            parent_dir.join(NOTEBOOK_FILE_NAME),
            serde_json::to_vec(
                &upsert_notebook(&parent_dir.join(NOTEBOOK_FILE_NAME), "fact", "one", false)
                    .unwrap(),
            )
            .unwrap(),
        )
        .unwrap();
        fs::write(
            child_dir.join("session.jsonl"),
            serde_json::json!({
                "kind": "session_created",
                "forked_from": {"parent_session_id": "parent"}
            })
            .to_string(),
        )
        .unwrap();
        assert_eq!(
            parent_notebook_path(&child_dir),
            Some(parent_dir.canonicalize().unwrap().join(NOTEBOOK_FILE_NAME))
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn evidence_normalizes_subject_and_persists_keywords() {
        let root = temp_path();
        let path = root.join(NOTEBOOK_FILE_NAME);
        let subject = format!("  feat: add   cached commit metadata {}", "x".repeat(180));
        let snapshot = upsert_notebook_with_metadata(
            &path,
            "architecture",
            "remember the session boundary",
            false,
            NotebookImportance::High,
            Some(vec!["checkpoint".to_string(), "memory".to_string()]),
            Some(vec![json!({
                "kind": "commit",
                "project": "mini-codex",
                "commit": "3941fcc",
                "subject": subject,
                "committedAt": "2026-09-18T10:00:00Z"
            })]),
        )
        .unwrap();
        let evidence = &snapshot.entries[0].evidence[0];
        assert_eq!(evidence["subjectTruncated"], true);
        assert!(evidence["subject"].as_str().unwrap().len() <= MAX_EVIDENCE_SUBJECT_BYTES);
        assert_eq!(snapshot.entries[0].keywords, ["checkpoint", "memory"]);
        assert_eq!(evidence["commit"], "3941fcc");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn old_notebook_entries_default_new_metadata() {
        let snapshot: NotebookSnapshot = serde_json::from_value(serde_json::json!({
            "version": 1,
            "revision": 1,
            "updatedAtMs": 1,
            "entries": [{"key": "fact", "content": "old", "updatedAtMs": 1}]
        }))
        .unwrap();
        assert!(snapshot.entries[0].keywords.is_empty());
        assert!(snapshot.entries[0].evidence.is_empty());
    }
}
