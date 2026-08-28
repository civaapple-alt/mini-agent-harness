use serde::Deserialize;
use serde::Serialize;

/// Identifies the kind of input submitted to a running conversation.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnInputMode {
    Start,
    StartIfIdle,
    Steer,
    FollowUp,
}

/// User input handed to the Thread runtime.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct TurnInput {
    pub mode: TurnInputMode,
    pub text: String,
}

impl TurnInput {
    pub fn new(mode: TurnInputMode, text: impl Into<String>) -> Self {
        Self {
            mode,
            text: text.into(),
        }
    }
}

/// Lifecycle state of a long-lived conversation.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ThreadStatus {
    Idle,
    Running,
    AwaitingInput,
    Failed,
    Closed,
}

/// Lifecycle state of one submitted Turn.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnStatus {
    InProgress,
    Completed,
    Steered,
    Cancelled,
    Failed,
}

/// Result of submitting input to a Thread.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum TurnSubmission {
    Started { turn_id: String },
    Steered { turn_id: String },
    Queued,
    NotSubmitted { reason: String },
}
