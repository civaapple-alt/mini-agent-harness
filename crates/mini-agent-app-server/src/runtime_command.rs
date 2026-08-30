use crate::RuntimeTurnResult;
use crate::action::ActionResult;
use mini_agent_capabilities::{ApprovalController, ApprovalMode};
use mini_agent_core::ThreadCheckpoint;
use mini_agent_protocol::{Message, ThreadId};
use tokio::sync::oneshot;

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
    UpdateWorld {
        updated: mini_agent_host::WorldState,
        reply: oneshot::Sender<ActionResult<bool>>,
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
    RecordContext {
        checkpoint: ThreadCheckpoint,
        reply: oneshot::Sender<ActionResult<()>>,
    },
    RecordTurn {
        started_at_ms: u64,
        prompt: String,
        result: RuntimeTurnResult,
        messages: Vec<Message>,
        checkpoint: Vec<Message>,
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
