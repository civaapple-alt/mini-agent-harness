use super::storage::{SessionLoadIssue, acquire_lock_without_reclaim, mini_agent_home};
use super::*;
use std::fs::{self, File, OpenOptions};
use std::io::Write;

pub const SESSION_DOCTOR_SCHEMA_VERSION: u32 = 1;
const MAX_DIAGNOSTIC_SESSIONS: usize = 4096;
const MAX_DIAGNOSTIC_ENTRIES: usize = 8192;
const MAX_DIAGNOSTIC_FINDINGS: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionInspectionStatus {
    Inspected,
    LockedUnverified,
    Unreadable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionIntegrityStatus {
    Complete,
    HistoryIncomplete,
    Invalid,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionRecoveryStatus {
    Resumable,
    Unavailable,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionDiagnosticIssue {
    Healthy,
    IncompleteTail,
    RecoveryGap,
    IncompleteTailAndRecoveryGap,
    SequenceGap,
    MissingCheckpoint,
    InvalidRecord,
    Locked,
    Unreadable,
    InvalidSessionDirectory,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SessionDiagnosticFinding {
    pub session_id: String,
    pub issue_code: SessionDiagnosticIssue,
    pub inspection: SessionInspectionStatus,
    pub integrity: SessionIntegrityStatus,
    pub recovery: SessionRecoveryStatus,
    pub incomplete_tail: bool,
    pub repair_available: bool,
    pub byte_offset: Option<u64>,
    pub line: Option<u64>,
    pub expected_seq: Option<u64>,
    pub found_seq: Option<u64>,
    pub missing_seq: Option<u64>,
    pub recommendation: &'static str,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct SessionInspectionCounts {
    pub inspected: usize,
    pub locked_unverified: usize,
    pub unreadable: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct SessionIntegrityCounts {
    pub complete: usize,
    pub history_incomplete: usize,
    pub invalid: usize,
    pub unknown: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct SessionRecoveryCounts {
    pub resumable: usize,
    pub unavailable: usize,
    pub unknown: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct SessionDiagnosticCounts {
    pub inspection: SessionInspectionCounts,
    pub integrity: SessionIntegrityCounts,
    pub recovery: SessionRecoveryCounts,
    pub repairable_tails: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SessionDiagnosticReport {
    pub schema_version: u32,
    pub scanned_sessions: usize,
    pub sessions_truncated: bool,
    pub findings_truncated: bool,
    pub counts: SessionDiagnosticCounts,
    pub findings: Vec<SessionDiagnosticFinding>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SessionRepairResult {
    pub session_id: String,
    pub backup_path: String,
    pub finding: SessionDiagnosticFinding,
}

impl SessionDiagnosticReport {
    fn new() -> Self {
        Self {
            schema_version: SESSION_DOCTOR_SCHEMA_VERSION,
            scanned_sessions: 0,
            sessions_truncated: false,
            findings_truncated: false,
            counts: SessionDiagnosticCounts::default(),
            findings: Vec::new(),
        }
    }

    fn record(&mut self, finding: SessionDiagnosticFinding) {
        self.scanned_sessions = self.scanned_sessions.saturating_add(1);
        match finding.inspection {
            SessionInspectionStatus::Inspected => {
                self.counts.inspection.inspected += 1;
            }
            SessionInspectionStatus::LockedUnverified => {
                self.counts.inspection.locked_unverified += 1;
            }
            SessionInspectionStatus::Unreadable => {
                self.counts.inspection.unreadable += 1;
            }
        }
        match finding.integrity {
            SessionIntegrityStatus::Complete => self.counts.integrity.complete += 1,
            SessionIntegrityStatus::HistoryIncomplete => {
                self.counts.integrity.history_incomplete += 1;
            }
            SessionIntegrityStatus::Invalid => self.counts.integrity.invalid += 1,
            SessionIntegrityStatus::Unknown => self.counts.integrity.unknown += 1,
        }
        match finding.recovery {
            SessionRecoveryStatus::Resumable => self.counts.recovery.resumable += 1,
            SessionRecoveryStatus::Unavailable => self.counts.recovery.unavailable += 1,
            SessionRecoveryStatus::Unknown => self.counts.recovery.unknown += 1,
        }
        if finding.repair_available {
            self.counts.repairable_tails += 1;
        }
        if self.findings.len() < MAX_DIAGNOSTIC_FINDINGS {
            self.findings.push(finding);
        } else {
            self.findings_truncated = true;
        }
    }
}

impl SessionStore {
    /// Inspect bounded SessionStore logs without opening a Session writer.
    pub fn inspect_workspace(workspace: &Path) -> Result<SessionDiagnosticReport, String> {
        let base_dir = session_directory(workspace)?;
        match fs::symlink_metadata(&base_dir) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err("Session storage directory is a symbolic link".to_string());
            }
            Ok(metadata) if !metadata.is_dir() => {
                return Err("Session storage path is not a directory".to_string());
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(SessionDiagnosticReport::new());
            }
            Err(error) => return Err(format!("cannot inspect Session directory: {error}")),
        }
        let entries = match fs::read_dir(&base_dir) {
            Ok(entries) => entries,
            Err(error) => return Err(format!("cannot list Session directory: {error}")),
        };

        let mut sessions = Vec::new();
        let mut report = SessionDiagnosticReport::new();
        let mut candidate_count = 0usize;
        for (entry_count, entry) in entries.enumerate() {
            if entry_count == MAX_DIAGNOSTIC_ENTRIES {
                report.sessions_truncated = true;
                break;
            }
            let entry = entry.map_err(|error| format!("cannot list Session entry: {error}"))?;
            let file_type = entry
                .file_type()
                .map_err(|error| format!("cannot inspect Session entry: {error}"))?;
            if !file_type.is_dir() && !file_type.is_symlink() {
                continue;
            }
            if candidate_count == MAX_DIAGNOSTIC_SESSIONS {
                report.sessions_truncated = true;
                break;
            }
            candidate_count += 1;
            let session_id = entry.file_name().to_string_lossy().into_owned();
            if validate_session_id(&session_id).is_err() {
                report.record(unreadable_finding(
                    bounded_session_id(&session_id),
                    SessionDiagnosticIssue::InvalidSessionDirectory,
                    "目录名不是有效的 Session ID。请检查 Session 存储目录。",
                ));
                continue;
            }
            sessions.push((session_id, entry.path(), file_type.is_symlink()));
        }
        sessions.sort_by(|left, right| left.0.cmp(&right.0));
        for (session_id, session_dir, is_symlink) in sessions {
            let finding = if is_symlink {
                unreadable_finding(
                    session_id,
                    SessionDiagnosticIssue::Unreadable,
                    "Session 目录是符号链接，未跟随读取。请检查目录后重试。",
                )
            } else {
                inspect_session(&base_dir, &session_id, &session_dir)
            };
            report.record(finding);
        }
        Ok(report)
    }

    /// Back up and remove only a torn final record after exclusive revalidation.
    pub fn repair_incomplete_tail(
        workspace: &Path,
        session_id: &str,
    ) -> Result<SessionRepairResult, String> {
        validate_session_id(session_id)?;
        let base_dir = session_directory(workspace)?;
        ensure_regular_directory(&base_dir)?;
        let _lock = acquire_lock_without_reclaim(&base_dir, session_id)?;
        let session_dir = base_dir.join(session_id);
        ensure_regular_directory(&session_dir)?;
        let session_path = session_dir.join(SESSION_FILE_NAME);
        ensure_regular_file(&session_path)?;
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&session_path)
            .map_err(|error| format!("cannot open Session log: {error}"))?;
        let metadata = file
            .metadata()
            .map_err(|error| format!("cannot inspect Session log: {error}"))?;
        if metadata.len() > MAX_SESSION_BYTES {
            return Err(format!("Session exceeds {MAX_SESSION_BYTES} byte limit"));
        }
        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        std::io::Read::read_to_end(&mut file, &mut bytes)
            .map_err(|error| format!("cannot read Session log: {error}"))?;
        if bytes.len() as u64 > MAX_SESSION_BYTES {
            return Err(format!("Session exceeds {MAX_SESSION_BYTES} byte limit"));
        }
        let loaded = load_records(session_id, &bytes).map_err(String::from)?;
        if loaded.valid_bytes >= bytes.len() {
            return Err("Session log has no repairable incomplete tail".to_string());
        }
        let backup_path = write_recovery_backup(workspace, session_id, &bytes)?;
        if let Err(error) = file.set_len(loaded.valid_bytes as u64) {
            return Err(format!(
                "cannot truncate Session log; original backup is {backup_path}: {error}"
            ));
        }
        if let Err(error) = file.sync_all() {
            let restore = restore_original(&session_path, &bytes);
            return Err(match restore {
                Ok(()) => format!(
                    "cannot sync repaired Session log; original restored from backup {backup_path}: {error}"
                ),
                Err(restore_error) => format!(
                    "cannot sync repaired Session log ({error}); restore failed ({restore_error}); original backup is {backup_path}"
                ),
            });
        }

        let finding = finding_from_loaded(session_id, &bytes[..loaded.valid_bytes], &loaded, false);
        Ok(SessionRepairResult {
            session_id: session_id.to_string(),
            backup_path,
            finding,
        })
    }
}

fn inspect_session(
    base_dir: &Path,
    session_id: &str,
    session_dir: &Path,
) -> SessionDiagnosticFinding {
    let lock_path = base_dir.join(format!("{session_id}.lock"));
    match fs::symlink_metadata(&lock_path) {
        Ok(_) => {
            return locked_finding(
                session_id,
                "Session 存在活动或未确认的锁，未检查日志。稍后重试。",
            );
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => {
            return unreadable_finding(
                session_id.to_string(),
                SessionDiagnosticIssue::Unreadable,
                "无法检查 Session 锁状态。请确认本机文件权限后重试。",
            );
        }
    }
    if let Err(error) = ensure_regular_directory(session_dir) {
        return unreadable_finding(
            session_id.to_string(),
            SessionDiagnosticIssue::Unreadable,
            if error == "Session directory is a symbolic link" {
                "Session 目录是符号链接，未跟随读取。请检查目录后重试。"
            } else {
                "无法读取 Session 目录。请检查目录后重试。"
            },
        );
    }
    let session_path = session_dir.join(SESSION_FILE_NAME);
    if let Err(error) = ensure_regular_file(&session_path) {
        let recommendation = if error == "Session log is missing" {
            "Session 日志不存在。请恢复备份或创建新 Session。"
        } else if error == "Session log is a symbolic link" {
            "Session 日志是符号链接，未跟随读取。请检查文件后重试。"
        } else {
            "无法读取 Session 日志。请检查文件权限后重试。"
        };
        return unreadable_finding(
            session_id.to_string(),
            SessionDiagnosticIssue::Unreadable,
            recommendation,
        );
    }
    let metadata = match fs::metadata(&session_path) {
        Ok(metadata) => metadata,
        Err(_) => {
            return unreadable_finding(
                session_id.to_string(),
                SessionDiagnosticIssue::Unreadable,
                "无法读取 Session 日志。请检查文件权限后重试。",
            );
        }
    };
    if metadata.len() > MAX_SESSION_BYTES {
        return unreadable_finding(
            session_id.to_string(),
            SessionDiagnosticIssue::Unreadable,
            "Session 日志超过检查限制，未读取内容。请检查文件大小。",
        );
    }
    let bytes = match fs::read(&session_path) {
        Ok(bytes) if bytes.len() as u64 <= MAX_SESSION_BYTES => bytes,
        _ => {
            return unreadable_finding(
                session_id.to_string(),
                SessionDiagnosticIssue::Unreadable,
                "无法在大小限制内读取 Session 日志。请检查文件后重试。",
            );
        }
    };
    match fs::symlink_metadata(&lock_path) {
        Ok(_) => {
            return locked_finding(
                session_id,
                "Session 在检查期间获得了锁，未确认日志状态。稍后重试。",
            );
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => {
            return unreadable_finding(
                session_id.to_string(),
                SessionDiagnosticIssue::Unreadable,
                "无法确认 Session 锁状态。请确认本机文件权限后重试。",
            );
        }
    }
    match load_records(session_id, &bytes) {
        Ok(loaded) => finding_from_loaded(session_id, &bytes, &loaded, true),
        Err(error) => finding_from_error(session_id, &bytes, error),
    }
}

fn finding_from_loaded(
    session_id: &str,
    bytes: &[u8],
    loaded: &storage::LoadedRecords,
    allow_repair: bool,
) -> SessionDiagnosticFinding {
    let incomplete_tail = loaded.valid_bytes < bytes.len();
    let missing_seq = loaded.recovery_gap.flatten();
    let has_recovery_gap = loaded.recovery_gap.is_some();
    let integrity = if has_recovery_gap {
        SessionIntegrityStatus::HistoryIncomplete
    } else {
        SessionIntegrityStatus::Complete
    };
    let issue_code = match (incomplete_tail, has_recovery_gap) {
        (false, false) => SessionDiagnosticIssue::Healthy,
        (true, false) => SessionDiagnosticIssue::IncompleteTail,
        (false, true) => SessionDiagnosticIssue::RecoveryGap,
        (true, true) => SessionDiagnosticIssue::IncompleteTailAndRecoveryGap,
    };
    let recommendation = match (incomplete_tail, has_recovery_gap) {
        (false, false) => "日志结构完整，Session 可恢复。",
        (true, false) => "末尾记录未完整写入。可先备份，再截去不完整尾部。",
        (false, true) => {
            "日志标记存在历史缺口。可恢复性不代表历史完整，请检查备份或创建新 Session。"
        }
        (true, true) => {
            "可备份并截去不完整尾部；日志仍标记存在历史缺口，请检查备份或创建新 Session。"
        }
    };
    let offset = incomplete_tail.then_some(loaded.valid_bytes as u64);
    SessionDiagnosticFinding {
        session_id: session_id.to_string(),
        issue_code,
        inspection: SessionInspectionStatus::Inspected,
        integrity,
        recovery: SessionRecoveryStatus::Resumable,
        incomplete_tail,
        repair_available: allow_repair && incomplete_tail,
        byte_offset: offset,
        line: offset.map(|offset| line_for_offset(bytes, offset as usize)),
        expected_seq: None,
        found_seq: None,
        missing_seq,
        recommendation,
    }
}

fn finding_from_error(
    session_id: &str,
    bytes: &[u8],
    error: storage::SessionLoadError,
) -> SessionDiagnosticFinding {
    let issue_code = match error.issue {
        SessionLoadIssue::SequenceGap => SessionDiagnosticIssue::SequenceGap,
        SessionLoadIssue::MissingCheckpoint => SessionDiagnosticIssue::MissingCheckpoint,
        SessionLoadIssue::InvalidRecord => SessionDiagnosticIssue::InvalidRecord,
    };
    let recommendation = match error.issue {
        SessionLoadIssue::SequenceGap => {
            "日志记录序号不连续，不能自动补造。请恢复备份或创建新 Session。"
        }
        SessionLoadIssue::MissingCheckpoint => {
            "没有可恢复的已结算 checkpoint。请恢复备份或创建新 Session。"
        }
        SessionLoadIssue::InvalidRecord => {
            "日志记录或 checkpoint 无效，不能自动修复。请恢复备份或创建新 Session。"
        }
    };
    let line = error
        .byte_offset
        .map(|offset| line_for_offset(bytes, offset));
    SessionDiagnosticFinding {
        session_id: session_id.to_string(),
        issue_code,
        inspection: SessionInspectionStatus::Inspected,
        integrity: SessionIntegrityStatus::Invalid,
        recovery: SessionRecoveryStatus::Unavailable,
        incomplete_tail: false,
        repair_available: false,
        byte_offset: error.byte_offset.map(|offset| offset as u64),
        line,
        expected_seq: error.expected_seq,
        found_seq: error.found_seq,
        missing_seq: None,
        recommendation,
    }
}

fn unreadable_finding(
    session_id: String,
    issue_code: SessionDiagnosticIssue,
    recommendation: &'static str,
) -> SessionDiagnosticFinding {
    SessionDiagnosticFinding {
        session_id,
        issue_code,
        inspection: SessionInspectionStatus::Unreadable,
        integrity: SessionIntegrityStatus::Unknown,
        recovery: SessionRecoveryStatus::Unknown,
        incomplete_tail: false,
        repair_available: false,
        byte_offset: None,
        line: None,
        expected_seq: None,
        found_seq: None,
        missing_seq: None,
        recommendation,
    }
}

fn locked_finding(session_id: &str, recommendation: &'static str) -> SessionDiagnosticFinding {
    SessionDiagnosticFinding {
        session_id: session_id.to_string(),
        issue_code: SessionDiagnosticIssue::Locked,
        inspection: SessionInspectionStatus::LockedUnverified,
        integrity: SessionIntegrityStatus::Unknown,
        recovery: SessionRecoveryStatus::Unknown,
        incomplete_tail: false,
        repair_available: false,
        byte_offset: None,
        line: None,
        expected_seq: None,
        found_seq: None,
        missing_seq: None,
        recommendation,
    }
}

fn ensure_regular_directory(path: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            "Session directory is missing".to_string()
        } else {
            format!("cannot inspect Session directory: {error}")
        }
    })?;
    if metadata.file_type().is_symlink() {
        return Err("Session directory is a symbolic link".to_string());
    }
    if !metadata.is_dir() {
        return Err("Session directory is not a directory".to_string());
    }
    Ok(())
}

fn ensure_regular_file(path: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            "Session log is missing".to_string()
        } else {
            format!("cannot inspect Session log: {error}")
        }
    })?;
    if metadata.file_type().is_symlink() {
        return Err("Session log is a symbolic link".to_string());
    }
    if !metadata.is_file() {
        return Err("Session log is not a regular file".to_string());
    }
    Ok(())
}

fn write_recovery_backup(
    workspace: &Path,
    session_id: &str,
    bytes: &[u8],
) -> Result<String, String> {
    let home = mini_agent_home()
        .ok_or_else(|| "cannot resolve mini-agent home for recovery backup".to_string())?;
    let session_base = session_directory(workspace)?;
    let workspace_key = session_base
        .file_name()
        .ok_or_else(|| "cannot resolve workspace key for recovery backup".to_string())?;
    let backup_dir = home
        .join("recovery-backups")
        .join(workspace_key)
        .join(session_id);
    let backup_root = home.join("recovery-backups");
    let workspace_backup_dir = backup_root.join(workspace_key);
    for directory in [&backup_root, &workspace_backup_dir, &backup_dir] {
        ensure_recovery_directory(directory)?;
    }
    for suffix in 0..16u8 {
        let filename = if suffix == 0 {
            format!("{}.jsonl", timestamp_ms())
        } else {
            format!("{}-{suffix}.jsonl", timestamp_ms())
        };
        let path = backup_dir.join(filename);
        let mut backup = match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(format!("cannot create recovery backup: {error}")),
        };
        if let Err(error) = backup.write_all(bytes).and_then(|()| backup.sync_all()) {
            let _ = fs::remove_file(&path);
            return Err(format!("cannot sync recovery backup: {error}"));
        }
        sync_directory(&backup_dir)?;
        return path
            .strip_prefix(&home)
            .map(|relative| relative.to_string_lossy().replace('\\', "/"))
            .map_err(|_| "recovery backup path escaped mini-agent home".to_string());
    }
    Err("cannot allocate a unique recovery backup name".to_string())
}

fn sync_directory(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| format!("cannot sync recovery backup directory: {error}"))?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

fn ensure_recovery_directory(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => Ok(()),
        Ok(_) => Err("recovery backup path is not a regular directory".to_string()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            match fs::create_dir(path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(_) => return Err("cannot create recovery backup directory".to_string()),
            }
            if let Some(parent) = path.parent() {
                sync_directory(parent)?;
            }
            let metadata = fs::symlink_metadata(path)
                .map_err(|_| "cannot inspect recovery backup directory".to_string())?;
            if metadata.is_dir() && !metadata.file_type().is_symlink() {
                Ok(())
            } else {
                Err("recovery backup path is not a regular directory".to_string())
            }
        }
        Err(_) => Err("cannot inspect recovery backup directory".to_string()),
    }
}

fn restore_original(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .write(true)
        .open(path)
        .map_err(|error| error.to_string())?;
    file.set_len(0).map_err(|error| error.to_string())?;
    file.write_all(bytes).map_err(|error| error.to_string())?;
    file.sync_all().map_err(|error| error.to_string())
}

fn line_for_offset(bytes: &[u8], offset: usize) -> u64 {
    bytes[..offset.min(bytes.len())]
        .iter()
        .filter(|byte| **byte == b'\n')
        .count() as u64
        + 1
}

fn bounded_session_id(session_id: &str) -> String {
    session_id.chars().take(64).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{remove_test_root, test_root};

    fn settled_session() -> (PathBuf, String, PathBuf, u64) {
        let root = test_root();
        let mut opened = SessionStore::open(&root, SessionRequest::New).unwrap();
        let session_id = opened.store.session_id().to_string();
        let messages = vec![Message::User {
            text: "private prompt that must not appear in diagnostics".to_string(),
        }];
        opened
            .store
            .record_turn_with_id(
                "turn-doctor",
                TurnCommit {
                    started_at_ms: timestamp_ms(),
                    prompt: "private prompt that must not appear in diagnostics",
                    status: TurnStatus::Completed,
                    steps: 1,
                    error: None,
                    messages: &messages,
                    tool_arguments: &[],
                    presentation: None,
                    checkpoint: &messages,
                },
            )
            .unwrap();
        let path = opened.store.path().to_path_buf();
        let next_seq = opened.store.next_seq;
        drop(opened);
        (root, session_id, path, next_seq)
    }

    fn cleanup(root: &Path, session_id: &str) {
        if let Ok(base) = session_directory(root)
            && let Some(workspace_key) = base.file_name()
            && let Some(home) = mini_agent_home()
        {
            let backups = home
                .join("recovery-backups")
                .join(workspace_key)
                .join(session_id);
            let _ = fs::remove_dir_all(backups);
        }
        remove_test_root(root);
    }

    #[test]
    fn inspection_is_read_only_and_separates_history_from_recovery() {
        let (root, session_id, path, next_seq) = settled_session();
        let mut file = OpenOptions::new().append(true).open(&path).unwrap();
        writeln!(
            file,
            "{{\"seq\":{next_seq},\"kind\":\"recovery_gap\",\"missing_seq\":7}}"
        )
        .unwrap();
        drop(file);
        let before = fs::read(&path).unwrap();

        let report = SessionStore::inspect_workspace(&root).unwrap();
        let finding = report
            .findings
            .iter()
            .find(|finding| finding.session_id == session_id)
            .unwrap();
        assert_eq!(finding.integrity, SessionIntegrityStatus::HistoryIncomplete);
        assert_eq!(finding.recovery, SessionRecoveryStatus::Resumable);
        assert_eq!(finding.missing_seq, Some(7));
        assert!(!finding.repair_available);
        assert!(
            !serde_json::to_string(&report)
                .unwrap()
                .contains("private prompt")
        );
        assert_eq!(fs::read(&path).unwrap(), before);
        cleanup(&root, &session_id);
    }

    #[test]
    fn repair_backups_before_truncating_only_a_valid_incomplete_tail() {
        let (root, session_id, path, next_seq) = settled_session();
        let valid_prefix = fs::read(&path).unwrap();
        let mut file = OpenOptions::new().append(true).open(&path).unwrap();
        write!(file, "{{\"seq\":{next_seq},\"kind\":\"item\"").unwrap();
        drop(file);
        let original = fs::read(&path).unwrap();

        let report = SessionStore::inspect_workspace(&root).unwrap();
        let finding = report
            .findings
            .iter()
            .find(|finding| finding.session_id == session_id)
            .unwrap();
        assert!(finding.repair_available);
        assert!(finding.incomplete_tail);
        assert!(!finding.recommendation.contains("private prompt"));

        let repaired = SessionStore::repair_incomplete_tail(&root, &session_id).unwrap();
        let backup = mini_agent_home().unwrap().join(&repaired.backup_path);
        assert_eq!(fs::read(backup).unwrap(), original);
        assert_eq!(fs::read(&path).unwrap(), valid_prefix);
        assert!(!repaired.finding.incomplete_tail);
        assert!(!repaired.finding.repair_available);
        assert!(!Path::new(&repaired.backup_path).is_absolute());
        cleanup(&root, &session_id);
    }

    #[test]
    fn sequence_gaps_and_locks_are_never_repaired() {
        let (root, session_id, path, _) = settled_session();
        let bytes = fs::read(&path).unwrap();
        let mut records = bytes
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
            .map(serde_json::from_slice::<Value>)
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        let expected = records.last().unwrap()["seq"].as_u64().unwrap();
        records.last_mut().unwrap()["seq"] = json!(expected + 1);
        let mut corrupted = Vec::new();
        for record in &records {
            corrupted.extend(serde_json::to_vec(record).unwrap());
            corrupted.push(b'\n');
        }
        fs::write(&path, &corrupted).unwrap();

        let report = SessionStore::inspect_workspace(&root).unwrap();
        let finding = report
            .findings
            .iter()
            .find(|finding| finding.session_id == session_id)
            .unwrap();
        assert_eq!(finding.issue_code, SessionDiagnosticIssue::SequenceGap);
        assert_eq!(finding.expected_seq, Some(expected));
        assert_eq!(finding.found_seq, Some(expected + 1));
        assert!(!finding.repair_available);
        assert!(SessionStore::repair_incomplete_tail(&root, &session_id).is_err());
        assert_eq!(fs::read(&path).unwrap(), corrupted);

        fs::write(
            session_directory(&root)
                .unwrap()
                .join(format!("{session_id}.lock")),
            "pid=1\n",
        )
        .unwrap();
        let locked = SessionStore::inspect_workspace(&root).unwrap();
        let finding = locked
            .findings
            .iter()
            .find(|finding| finding.session_id == session_id)
            .unwrap();
        assert_eq!(
            finding.inspection,
            SessionInspectionStatus::LockedUnverified
        );
        assert_eq!(finding.integrity, SessionIntegrityStatus::Unknown);
        assert!(SessionStore::repair_incomplete_tail(&root, &session_id).is_err());
        cleanup(&root, &session_id);
    }
}
