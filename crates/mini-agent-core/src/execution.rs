use mini_agent_protocol::Message;
use mini_agent_protocol::ToolCall;
use mini_agent_protocol::ToolExecutionContext;
use mini_agent_protocol::ToolExecutionOutcome;
use mini_agent_protocol::TurnId;
use mini_agent_protocol::TurnInput;
use serde::Deserialize;
use serde::Serialize;

/// A model-safe boundary from which the same logical Turn can continue.
///
/// It is written before a model request and after a complete tool batch. It
/// never contains an unfinished assistant/tool group.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionCheckpoint {
    pub turn_id: TurnId,
    pub input: TurnInput,
    pub messages: Vec<Message>,
    /// The next model step to execute. Step numbering starts at one.
    pub next_model_step: usize,
    pub final_text: String,
    pub phase: ExecutionPhase,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub applied_steer_request_ids: Vec<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionPhase {
    ModelRequest,
    ToolBatch,
}

/// One durable tool batch associated with a completed model response.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolBatchIntent {
    pub turn_id: TurnId,
    pub step: usize,
    pub reasoning: String,
    pub text: String,
    pub calls: Vec<ToolCall>,
}

/// One call's durable state inside a tool batch.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionToolCall {
    pub call: ToolCall,
    pub started: bool,
    pub outcome: Option<ToolExecutionOutcome>,
}

/// A tool batch that can be reconstructed without asking the model again.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionToolBatch {
    pub intent: ToolBatchIntent,
    pub calls: Vec<ExecutionToolCall>,
}

/// Identity carried from Thread into Core so checkpoints retain the complete
/// user input and the original logical Turn ID.
#[derive(Clone, Debug, PartialEq)]
pub struct ExecutionRunContext {
    pub turn_id: TurnId,
    pub input: TurnInput,
    pub applied_steer_request_ids: Vec<String>,
}

/// Optional Host-owned state supplied when Core continues one logical Turn.
#[derive(Default)]
pub struct ExecutionRunOptions<'a> {
    pub tool_context: Option<ToolExecutionContext>,
    pub execution_context: Option<ExecutionRunContext>,
    pub journal: Option<&'a mut dyn ExecutionJournalSink>,
    pub resume: Option<(ExecutionCheckpoint, Option<ExecutionToolBatch>)>,
}

/// Host-owned boundaries and recovery state for a Thread Turn.
pub struct TurnExecutionOptions<'p, 'j> {
    pub prelude: &'p [mini_agent_protocol::Event],
    pub preflight_error: Option<&'p str>,
    pub journal: Option<&'j mut dyn ExecutionJournalSink>,
    pub resume: Option<(ExecutionCheckpoint, Option<ExecutionToolBatch>)>,
    pub applied_steer_request_ids: Vec<String>,
}

/// A bounded journal update emitted by the Core loop to its persistence owner.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ExecutionJournalEntry {
    Checkpoint {
        checkpoint: ExecutionCheckpoint,
    },
    SteerAccepted {
        turn_id: TurnId,
        request_id: String,
        text: String,
    },
    SteerUnapplied {
        turn_id: TurnId,
        request_id: String,
        reason: String,
    },
    ToolBatchStarted {
        batch: ToolBatchIntent,
    },
    ToolCallStarted {
        turn_id: TurnId,
        step: usize,
        call_id: String,
    },
    ToolCallFinished {
        turn_id: TurnId,
        step: usize,
        call_id: String,
        outcome: ToolExecutionOutcome,
    },
    ToolBatchSettled {
        turn_id: TurnId,
        step: usize,
    },
    WaitingForContinue {
        turn_id: TurnId,
        reason: String,
    },
    Resumed {
        turn_id: TurnId,
        request_id: String,
        checkpoint_seq: u64,
    },
    Heartbeat {
        turn_id: TurnId,
        phase: ExecutionPhase,
        at_ms: u64,
        last_progress_ms: u64,
    },
    NeedsReconciliation {
        turn_id: TurnId,
        reason: String,
    },
    Settled {
        turn_id: TurnId,
    },
}

/// Persistence hook for recoverable execution progress.
///
/// This is an active execution boundary: a failed write stops the loop before
/// it begins another model step or side-effecting tool call.
pub trait ExecutionJournalSink: Send {
    fn append(&mut self, entry: ExecutionJournalEntry) -> Result<u64, String>;
}

pub(crate) fn append_if_present(
    journal: &mut Option<&mut dyn ExecutionJournalSink>,
    entry: ExecutionJournalEntry,
) -> Result<Option<u64>, String> {
    match journal {
        Some(journal) => journal.append(entry).map(Some),
        None => Ok(None),
    }
}
