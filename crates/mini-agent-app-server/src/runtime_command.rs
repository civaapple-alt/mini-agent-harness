use crate::action::ActionResult;
use crate::action::RuntimeRevision;
use mini_agent_capabilities::{ApprovalController, ApprovalMode};
use mini_agent_core::ThreadCheckpoint;
use mini_agent_protocol::ThreadId;
use tokio::sync::oneshot;

pub(super) struct RuntimeRequest {
    pub(super) expected_revision: RuntimeRevision,
    pub(super) command: RuntimeCommand,
}

/// Runtime management and workflow commands handled by the App Server actor.
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
    WorkflowState {
        reply: oneshot::Sender<ActionResult<(bool, Option<crate::workflows::GoalState>)>>,
    },
    SetCollaborationMode {
        active: bool,
        builtin_tools: Option<mini_agent_host::BuiltinToolSelection>,
        reply: oneshot::Sender<ActionResult<Vec<String>>>,
    },
    GoalSet {
        objective: Option<String>,
        status: Option<mini_agent_app_server_protocol::ThreadGoalStatus>,
        token_budget: Option<Option<i64>>,
        reply: oneshot::Sender<ActionResult<crate::workflows::GoalState>>,
    },
    GoalGet {
        reply: oneshot::Sender<ActionResult<Option<crate::workflows::GoalState>>>,
    },
    GoalClear {
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
                | Self::SetCollaborationMode { .. }
                | Self::GoalSet { .. }
                | Self::GoalClear { .. }
        )
    }
}
