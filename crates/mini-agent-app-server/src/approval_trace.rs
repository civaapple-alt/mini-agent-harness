use crate::{ApprovalRequest, ApprovalResolution};
use mini_agent_protocol::stable_digest;
use serde_json::json;
use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

const FILE_NAME: &str = "approval-evidence.jsonl";
const MAX_TOTAL: usize = 256 * 1024;

#[derive(Clone, Default)]
pub(crate) struct ApprovalTrace(Arc<Mutex<HashMap<String, PathBuf>>>);

impl ApprovalTrace {
    pub(crate) fn bind_thread_file(&self, thread_id: String, session_file: &Path) {
        self.0
            .lock()
            .unwrap()
            .insert(thread_id, session_file.with_file_name(FILE_NAME));
    }

    pub(crate) fn requested(&self, request: &ApprovalRequest) {
        let Some(thread_id) = request.thread_id.as_ref().map(|id| id.as_str()) else {
            return;
        };
        let _ = self.record(
            thread_id,
            json!({
                "event": "approval_requested",
                "request_id": request.request_id,
                "project_id": request.project_id,
                "thread_id": thread_id,
                "turn_id": request.turn_id.as_ref().map(|id| id.as_str()),
                "call_id": request.call_id,
                "session_item_id": request.call_id,
                "tool": request.tool_name,
                "action_summary": safe_action(&request.action),
                "action_hash": action_hash(request),
                "policy": request.policy,
                "access": request.access
            }),
        );
    }

    pub(crate) fn resolved(&self, request: &ApprovalRequest, result: &ApprovalResolution) {
        let Some(thread_id) = result.thread_id.as_ref().map(|id| id.as_str()) else {
            return;
        };
        let _ = self.record(
            thread_id,
            json!({
                "event": "approval_resolved",
                "request_id": result.request_id,
                "project_id": result.project_id,
                "thread_id": thread_id,
                "turn_id": result.turn_id.as_ref().map(|id| id.as_str()),
                "call_id": result.call_id,
                "session_item_id": result.call_id,
                "tool": result.tool_name,
                "action_summary": safe_action(&result.action),
                "action_hash": action_hash(request),
                "policy": request.policy,
                "access": request.access,
                "outcome": result.outcome,
                "grant_scope": result.grant_scope
            }),
        );
    }

    fn record(&self, thread_id: &str, value: serde_json::Value) -> Option<()> {
        let path = self.0.lock().unwrap().get(thread_id).cloned()?;
        let mut line = serde_json::to_vec(&value).ok()?;
        line.push(b'\n');
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .ok()?;
        let size = file.metadata().ok()?.len() as usize;
        if size.saturating_add(line.len()) > MAX_TOTAL {
            return None;
        }
        file.write_all(&line).ok()
    }
}

fn action_hash(request: &ApprovalRequest) -> String {
    let bytes = request
        .action_key
        .as_ref()
        .and_then(|key| serde_json::to_vec(key).ok())
        .unwrap_or_else(|| request.action.as_bytes().to_vec());
    stable_digest(&bytes)
}

fn safe_action(action: &str) -> String {
    action
        .split_whitespace()
        .next()
        .unwrap_or("unknown")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_one_thread_sidecar() {
        let root = std::env::temp_dir().join(format!("mini-agent-approval-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let trace = ApprovalTrace::default();
        trace.bind_thread_file("thread".to_string(), &root.join("session.jsonl"));
        let _ = trace.record("thread", json!({"event": "approval_resolved"}));
        assert_eq!(
            std::fs::read_to_string(root.join(FILE_NAME)).unwrap(),
            "{\"event\":\"approval_resolved\"}\n"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn records_session_join_key_without_copying_full_action() {
        let root =
            std::env::temp_dir().join(format!("mini-agent-approval-action-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let trace = ApprovalTrace::default();
        trace.bind_thread_file("thread".to_string(), &root.join("session.jsonl"));
        let request = ApprovalRequest {
            request_id: "approval-1".to_string(),
            action: "shell command `rg -n \"install|plugin\" README.md`".to_string(),
            action_summary: "shell command `rg -n \"install|plugin\" README.md`".to_string(),
            project_id: Some("project".to_string()),
            workspace_id: Some("workspace".to_string()),
            workspace_revision: Some(1),
            session_id: Some("session".to_string()),
            call_id: Some("call-1".to_string()),
            thread_id: Some(mini_agent_protocol::ThreadId::new("thread")),
            turn_id: Some(mini_agent_protocol::TurnId::new("turn-1")),
            tool_name: Some("shell".to_string()),
            action_class: "shell".to_string(),
            action_key: None,
            access: mini_agent_app_server_protocol::AccessScope::Project,
            policy: mini_agent_app_server_protocol::ApprovalPolicy::Trusted,
            allowed_grant_scopes: vec![mini_agent_app_server_protocol::ActionGrantScope::Once],
            target_paths: Vec::new(),
        };

        trace.requested(&request);

        let output = std::fs::read_to_string(root.join(FILE_NAME)).unwrap();
        let value: serde_json::Value = serde_json::from_str(output.trim()).unwrap();
        assert_eq!(value["tool"], "shell");
        assert_eq!(value["call_id"], "call-1");
        assert_eq!(value["session_item_id"], "call-1");
        assert!(value.get("action").is_none());
        let _ = std::fs::remove_dir_all(root);
    }
}
