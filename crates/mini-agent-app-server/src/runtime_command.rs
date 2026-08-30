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
    WorkflowSetPlan {
        active: bool,
        prompt: Option<String>,
        reply: oneshot::Sender<ActionResult<()>>,
    },
    WorkflowInitGoal {
        objective: String,
        reply: oneshot::Sender<ActionResult<crate::workflows::GoalState>>,
    },
    WorkflowLoadGoal {
        reply: oneshot::Sender<ActionResult<Option<crate::workflows::GoalState>>>,
    },
    WorkflowCriteria {
        reply: oneshot::Sender<ActionResult<String>>,
    },
    WorkflowRecordVerdict {
        checkpoint_seq: u64,
        output: String,
        reply: oneshot::Sender<ActionResult<()>>,
    },
    WorkflowAdvance {
        verdict: Option<crate::workflows::VerifierVerdict>,
        reply: oneshot::Sender<ActionResult<crate::workflows::GoalState>>,
    },
    WorkflowPause {
        reply: oneshot::Sender<ActionResult<()>>,
    },
    WorkflowFail {
        reply: oneshot::Sender<ActionResult<crate::workflows::GoalState>>,
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
                | Self::WorkflowSetPlan { .. }
                | Self::WorkflowInitGoal { .. }
                | Self::WorkflowRecordVerdict { .. }
                | Self::WorkflowAdvance { .. }
                | Self::WorkflowPause { .. }
                | Self::WorkflowFail { .. }
        )
    }
}
