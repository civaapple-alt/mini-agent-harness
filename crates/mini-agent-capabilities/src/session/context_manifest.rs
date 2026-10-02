use mini_agent_protocol::{ContextInjectionKind, ContextInjectionRecord, TurnId};
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub const MAX_CONTEXT_MANIFEST_ENTRIES: usize = 512;
const MANIFEST_SCHEMA_VERSION: u64 = 1;
const MAX_MANIFEST_BYTES: usize = 1024 * 1024;
const MAX_METADATA_FIELD_BYTES: usize = 512;

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionContextManifestEntry {
    pub thread_id: String,
    pub turn_id: Option<TurnId>,
    pub source_id: String,
    pub source_name: String,
    pub kind: ContextInjectionKind,
    pub version_fingerprint: String,
    pub workspace: Option<String>,
    pub path: Option<String>,
    pub applies_to: String,
    pub permission_basis: String,
    pub injection_reason: String,
    pub bytes: u64,
    pub reused: bool,
    pub injected_at_ms: u64,
}

#[derive(Clone)]
pub struct SessionContextManifestStore {
    session_id: String,
    path: PathBuf,
    append_lock: Arc<Mutex<()>>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ManifestFile {
    schema_version: u64,
    session_id: String,
    entries: Vec<SessionContextManifestEntry>,
}

impl SessionContextManifestStore {
    pub(super) fn new(
        session_dir: &Path,
        session_id: impl Into<String>,
        append_lock: Arc<Mutex<()>>,
    ) -> Self {
        Self {
            session_id: session_id.into(),
            path: session_dir.join("context_manifest.json"),
            append_lock,
        }
    }

    pub fn append(
        &self,
        thread_id: &str,
        turn_id: Option<&TurnId>,
        records: &[ContextInjectionRecord],
    ) -> Result<(), String> {
        if records.len() > 32 {
            return Err("context manifest update exceeds 32 entries".to_string());
        }
        let _lock = self.append_lock.lock().unwrap();
        let mut file = self.read_file()?;
        let now = super::timestamp_ms();
        for record in records {
            let entry = manifest_entry(thread_id, turn_id, record, now);
            validate_entry(&entry)?;
            let identity = (
                entry.thread_id.as_str(),
                entry.turn_id.as_ref().map(TurnId::as_str),
                entry.source_id.as_str(),
                entry.version_fingerprint.as_str(),
            );
            if let Some(existing) = file.entries.iter_mut().find(|existing| {
                (
                    existing.thread_id.as_str(),
                    existing.turn_id.as_ref().map(TurnId::as_str),
                    existing.source_id.as_str(),
                    existing.version_fingerprint.as_str(),
                ) == identity
            }) {
                existing.reused |= entry.reused;
                existing.injected_at_ms = entry.injected_at_ms;
            } else {
                file.entries.push(entry);
            }
        }
        if file.entries.len() > MAX_CONTEXT_MANIFEST_ENTRIES {
            let remove = file.entries.len() - MAX_CONTEXT_MANIFEST_ENTRIES;
            file.entries.drain(..remove);
        }
        self.write_file(&file)
    }

    pub fn entries(&self) -> Result<Vec<SessionContextManifestEntry>, String> {
        let _lock = self.append_lock.lock().unwrap();
        Ok(self.read_file()?.entries)
    }

    fn read_file(&self) -> Result<ManifestFile, String> {
        let bytes = match fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(ManifestFile {
                    schema_version: MANIFEST_SCHEMA_VERSION,
                    session_id: self.session_id.clone(),
                    entries: Vec::new(),
                });
            }
            Err(error) => return Err(format!("cannot read context manifest: {error}")),
        };
        if bytes.len() > MAX_MANIFEST_BYTES {
            return Err("context manifest exceeds its 1 MiB limit".to_string());
        }
        let file: ManifestFile = serde_json::from_slice(&bytes)
            .map_err(|error| format!("cannot read context manifest: {error}"))?;
        if file.schema_version != MANIFEST_SCHEMA_VERSION || file.session_id != self.session_id {
            return Err("context manifest schema or Session identity is invalid".to_string());
        }
        if file.entries.len() > MAX_CONTEXT_MANIFEST_ENTRIES {
            return Err("context manifest entry count exceeds its limit".to_string());
        }
        for entry in &file.entries {
            validate_entry(entry)?;
        }
        Ok(file)
    }

    fn write_file(&self, file: &ManifestFile) -> Result<(), String> {
        let bytes = serde_json::to_vec(file)
            .map_err(|error| format!("cannot encode context manifest: {error}"))?;
        if bytes.len() > MAX_MANIFEST_BYTES {
            return Err("context manifest exceeds its 1 MiB limit".to_string());
        }
        let parent = self
            .path
            .parent()
            .ok_or_else(|| "context manifest has no parent directory".to_string())?;
        let temp = parent.join(".context_manifest.tmp");
        fs::write(&temp, &bytes)
            .map_err(|error| format!("cannot write context manifest: {error}"))?;
        File::open(&temp)
            .and_then(|file| file.sync_all())
            .map_err(|error| format!("cannot sync context manifest: {error}"))?;
        fs::rename(&temp, &self.path).map_err(|error| {
            let _ = fs::remove_file(&temp);
            format!("cannot replace context manifest: {error}")
        })?;
        Ok(())
    }
}

fn manifest_entry(
    thread_id: &str,
    turn_id: Option<&TurnId>,
    record: &ContextInjectionRecord,
    injected_at_ms: u64,
) -> SessionContextManifestEntry {
    let (permission_basis, injection_reason) = match record.kind {
        ContextInjectionKind::ProjectInstructions => (
            "workspace instruction policy",
            "include applicable project instructions",
        ),
        ContextInjectionKind::Skill if record.id == "available_extensions" => (
            "Host skill discovery policy",
            "show the bounded skill catalog available to this Turn",
        ),
        ContextInjectionKind::Skill => (
            "explicit Turn skill activation",
            "include the selected Skill definition",
        ),
        ContextInjectionKind::WorkspaceState => (
            "App Server runtime control plane",
            "provide the current workspace runtime state",
        ),
        ContextInjectionKind::Other => (
            "Session-owned runtime context",
            "include a bounded Session context summary",
        ),
    };
    SessionContextManifestEntry {
        thread_id: thread_id.to_string(),
        turn_id: turn_id.cloned(),
        source_id: record.id.clone(),
        source_name: record.source.clone(),
        kind: record.kind,
        version_fingerprint: record.fingerprint.clone(),
        workspace: record.workspace.clone(),
        path: record.path.clone(),
        applies_to: record.scope.clone(),
        permission_basis: permission_basis.to_string(),
        injection_reason: injection_reason.to_string(),
        bytes: record.bytes,
        reused: record.reused,
        injected_at_ms,
    }
}

fn validate_entry(entry: &SessionContextManifestEntry) -> Result<(), String> {
    for (label, value) in [
        ("Thread id", entry.thread_id.as_str()),
        ("source id", entry.source_id.as_str()),
        ("source name", entry.source_name.as_str()),
        ("fingerprint", entry.version_fingerprint.as_str()),
        ("scope", entry.applies_to.as_str()),
        ("permission basis", entry.permission_basis.as_str()),
        ("injection reason", entry.injection_reason.as_str()),
    ] {
        if value.len() > MAX_METADATA_FIELD_BYTES {
            return Err(format!("context manifest {label} exceeds its size limit"));
        }
    }
    for value in [&entry.workspace, &entry.path].into_iter().flatten() {
        if value.len() > MAX_METADATA_FIELD_BYTES {
            return Err("context manifest source path exceeds its size limit".to_string());
        }
    }
    Ok(())
}
