use crate::AppServerError;
use crate::action::RuntimeRevision;
use crate::action::{ActionFailure, ActionResponse, ActionResult};
use crate::worker::Command;
use mini_agent_capabilities::{ApprovalController, ApprovalPolicy, SecurityPreset};
use mini_agent_core::ThreadCheckpoint;
use mini_agent_protocol::ThreadId;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use tokio::sync::mpsc;
use tokio::sync::oneshot;

pub(super) struct RuntimeRequest {
    pub(super) expected_revision: RuntimeRevision,
    pub(super) command: RuntimeCommand,
}

/// Runtime management and Thread commands handled by the App Server actor.
pub(super) enum RuntimeCommand {
    SessionInfo {
        reply: oneshot::Sender<ActionResult<Option<crate::RuntimeSessionInfo>>>,
    },
    PrepareSessionFork {
        source_thread_id: ThreadId,
        new_thread_id: ThreadId,
        context_policy: mini_agent_app_server_protocol::ForkContextPolicy,
        operation_id: Option<String>,
        operation_attempt: Option<u32>,
        operation_prompt: Option<String>,
        operation_group_id: Option<String>,
        execution_mode: Option<String>,
        group_sequence: Option<u32>,
        reply: oneshot::Sender<ActionResult<mini_agent_app_server_protocol::SessionForkResult>>,
    },
    ReadNotebook {
        scope: String,
        reply: oneshot::Sender<ActionResult<serde_json::Value>>,
    },
    WriteNotebook {
        key: String,
        content: String,
        append: bool,
        importance: String,
        reply: oneshot::Sender<ActionResult<serde_json::Value>>,
    },
    ForgetNotebook {
        key: String,
        reply: oneshot::Sender<ActionResult<serde_json::Value>>,
    },
    CheckpointSeq {
        reply: oneshot::Sender<ActionResult<Option<u64>>>,
    },
    ThreadId {
        reply: oneshot::Sender<ActionResult<ThreadId>>,
    },
    World {
        reply: oneshot::Sender<ActionResult<mini_agent_host::WorldState>>,
    },
    RefreshWorld {
        reply: oneshot::Sender<ActionResult<bool>>,
    },
    SetExecution {
        access: SecurityPreset,
        policy: ApprovalPolicy,
        reply: oneshot::Sender<ActionResult<bool>>,
    },
    UpdateThread {
        update: crate::ThreadUpdate,
        reply: oneshot::Sender<ActionResult<()>>,
    },
    McpStatus {
        reply: oneshot::Sender<ActionResult<crate::management::McpRuntimeSnapshot>>,
    },
    RetryMcp {
        approval: ApprovalController,
        reply: oneshot::Sender<ActionResult<crate::McpRetryResult>>,
    },
    ReadCheckpoint {
        reply: oneshot::Sender<ActionResult<ThreadCheckpoint>>,
    },
    StartNewThread {
        reply: oneshot::Sender<ActionResult<()>>,
    },
    ThreadSettingsUpdate {
        active: bool,
        builtin_tools: Option<mini_agent_host::BuiltinToolSelection>,
        continuation_mode: Option<mini_agent_app_server_protocol::ContinuationMode>,
        reply: oneshot::Sender<ActionResult<crate::management::ThreadSettingsRuntimeSnapshot>>,
    },
    ThreadGoalSet {
        objective: Option<String>,
        status: Option<mini_agent_app_server_protocol::ThreadGoalStatus>,
        token_budget: Option<Option<i64>>,
        reply: oneshot::Sender<ActionResult<crate::goal_service::GoalState>>,
    },
    ThreadGoalGet {
        reply: oneshot::Sender<ActionResult<Option<crate::goal_service::GoalState>>>,
    },
    ThreadGoalClear {
        reply: oneshot::Sender<ActionResult<bool>>,
    },
}

impl RuntimeCommand {
    pub(super) fn is_mutation(&self) -> bool {
        matches!(
            self,
            Self::RefreshWorld { .. }
                | Self::SetExecution { .. }
                | Self::UpdateThread { .. }
                | Self::RetryMcp { .. }
                | Self::StartNewThread { .. }
                | Self::ThreadSettingsUpdate { .. }
                | Self::ThreadGoalSet { .. }
                | Self::ThreadGoalClear { .. }
                | Self::PrepareSessionFork { .. }
                | Self::WriteNotebook { .. }
                | Self::ForgetNotebook { .. }
        )
    }
}

/// Internal command client shared by the Thread settings and Goal request
/// processors. It carries no domain state; all state remains owned by the
/// App Server runtime actor.
#[derive(Clone)]
pub(crate) struct RuntimeCommandClient {
    commands: mpsc::Sender<Command>,
    revision: Arc<AtomicU64>,
}

impl RuntimeCommandClient {
    pub(crate) fn new(commands: mpsc::Sender<Command>, revision: Arc<AtomicU64>) -> Self {
        Self { commands, revision }
    }

    pub(crate) async fn request_action<T, F>(
        &self,
        build: F,
    ) -> Result<ActionResponse<T>, ActionFailure>
    where
        F: FnOnce(oneshot::Sender<ActionResult<T>>) -> RuntimeCommand,
    {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(Command::Runtime(RuntimeRequest {
                expected_revision: self.revision.load(Ordering::SeqCst).into(),
                command: build(reply),
            }))
            .await
            .map_err(|_| ActionFailure::without_receipt(AppServerError::Disconnected))?;
        response
            .await
            .map_err(|_| ActionFailure::without_receipt(AppServerError::Disconnected))?
    }
}
