use crate::AppServerError;
use crate::action::RuntimeRevision;
use crate::action::{ActionFailure, ActionResponse, ActionResult};
use crate::worker::Command;
use mini_agent_capabilities::{ApprovalController, ApprovalMode};
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
        approval: ApprovalMode,
        copilot: bool,
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
    RuntimeState {
        reply: oneshot::Sender<ActionResult<crate::runtime_state::RuntimeStateSnapshot>>,
    },
    ThreadSettingsUpdate {
        active: bool,
        builtin_tools: Option<mini_agent_host::BuiltinToolSelection>,
        reply: oneshot::Sender<ActionResult<Vec<String>>>,
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
