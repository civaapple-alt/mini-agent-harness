use serde::Deserialize;
use serde::Serialize;

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct ThreadId(pub String);

impl ThreadId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct TurnId(pub String);

impl TurnId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Identifies the kind of input submitted to a running conversation.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnInputMode {
    Start,
    StartIfIdle,
    Steer,
    FollowUp,
}

/// A workflow activated for one turn without changing project configuration.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnWorkflow {
    pub kind: TurnWorkflowKind,
    pub id: String,
    pub mode: TurnWorkflowMode,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnWorkflowKind {
    SkillGroup,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnWorkflowMode {
    Auto,
}

/// User input handed to the Thread runtime.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnInput {
    pub mode: TurnInputMode,
    pub text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub selected_skills: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow: Option<TurnWorkflow>,
}

/// Starts a protocol-visible Thread with a stable identity.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct ThreadStart {
    pub thread_id: ThreadId,
}

impl ThreadStart {
    pub fn new(thread_id: ThreadId) -> Self {
        Self { thread_id }
    }
}

/// Starts one Turn on an existing Thread.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct TurnStart {
    pub input: TurnInput,
    /// Optional Host-owned operation identity for a detached/background turn.
    /// Core carries this boundary metadata but does not schedule or interpret it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_attempt: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_group_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_sequence: Option<u32>,
}

impl TurnStart {
    pub fn new(input: TurnInput) -> Self {
        Self {
            input,
            operation_id: None,
            operation_attempt: None,
            operation_group_id: None,
            execution_mode: None,
            group_sequence: None,
        }
    }
}

/// Requests cooperative cancellation of one active turn.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct TurnCancel {
    pub turn_id: TurnId,
}

impl TurnCancel {
    pub fn new(turn_id: TurnId) -> Self {
        Self { turn_id }
    }
}

impl TurnInput {
    pub fn new(mode: TurnInputMode, text: impl Into<String>) -> Self {
        Self {
            mode,
            text: text.into(),
            selected_skills: Vec::new(),
            workflow: None,
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
    StepLimit,
    Steered,
    Cancelled,
    Failed,
}

/// Result of submitting input to a Thread.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum TurnSubmission {
    Started { turn_id: TurnId },
    Steered { turn_id: TurnId },
    Queued,
    NotSubmitted { reason: String },
}

#[cfg(test)]
#[path = "turn_tests.rs"]
mod tests;
