mod event;
mod model;
mod tool;
mod turn;

pub use event::Event;
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
pub use tool::ToolSpec;
pub use turn::ThreadStatus;
pub use turn::TurnInput;
pub use turn::TurnInputMode;
pub use turn::TurnStatus;
pub use turn::TurnSubmission;

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
    }
}
