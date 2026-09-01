mod event;
mod model;
mod tool;
mod turn;

pub use event::Event;
pub use event::EventEnvelope;
pub use event::EventSink;
pub use event::Observer;
pub use event::RunFailure;
pub use model::Message;
pub use model::Model;
pub use model::ModelEvent;
pub use model::ModelEventSink;
pub use model::ModelRequest;
pub use model::ModelResponse;
pub use model::ModelUsage;
pub use model::ToolCall;
pub use tool::Tool;
pub use tool::ToolError;
pub use tool::ToolExecutionDelegate;
pub use tool::ToolExecutionOutcome;
pub use tool::ToolExecutionRequest;
pub use tool::ToolExecutionStatus;
pub use tool::ToolSpec;
pub use turn::ThreadId;
pub use turn::ThreadStart;
pub use turn::ThreadStatus;
pub use turn::TurnCancel;
pub use turn::TurnId;
pub use turn::TurnInput;
pub use turn::TurnInputMode;
pub use turn::TurnStart;
pub use turn::TurnStatus;
pub use turn::TurnSubmission;

/// Returns a deterministic non-cryptographic digest for bounded diagnostics.
///
/// Callers must not use this value for secrets, authentication, or integrity
/// protection. It is intended only for comparing redacted harness records.
pub fn stable_digest(bytes: &[u8]) -> String {
    let mut hash = 14_695_981_039_346_656_037u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(1_099_511_628_211);
    }
    format!("fnv1a64-{hash:016x}")
}

pub use run::LimitExceeded;
pub use run::LimitKind;
pub use run::StopReason;

mod run {
    use serde::Deserialize;
    use serde::Serialize;
    use std::fmt;

    #[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
    #[serde(rename_all = "snake_case")]
    pub enum LimitKind {
        ContextItemBytes,
        UserInputBytes,
        ModelResponseBytes,
        ToolCallsPerStep,
        ToolOutputBytes,
        ContextBytes,
    }

    impl fmt::Display for LimitKind {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str(match self {
                Self::ContextItemBytes => "context item bytes",
                Self::UserInputBytes => "user input bytes",
                Self::ModelResponseBytes => "model response bytes",
                Self::ToolCallsPerStep => "tool calls per step",
                Self::ToolOutputBytes => "tool output bytes",
                Self::ContextBytes => "context bytes",
            })
        }
    }

    #[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
    pub struct LimitExceeded {
        pub kind: LimitKind,
        pub limit: usize,
        pub actual: usize,
    }

    impl fmt::Display for LimitExceeded {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(
                formatter,
                "{} limit exceeded: {} > {}",
                self.kind, self.actual, self.limit
            )
        }
    }

    #[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
    #[serde(rename_all = "snake_case")]
    pub enum StopReason {
        Completed,
        StepLimit,
        Steered,
        Cancelled,
    }
}
