use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use std::error::Error;
use std::fmt;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

/// A model-requested invocation handed to a host tool executor.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ToolExecutionRequest {
    pub call_id: String,
    pub name: String,
    pub arguments: Value,
}

impl ToolExecutionRequest {
    pub fn new(call_id: impl Into<String>, name: impl Into<String>, arguments: Value) -> Self {
        Self {
            call_id: call_id.into(),
            name: name.into(),
            arguments,
        }
    }
}

impl From<crate::ToolCall> for ToolExecutionRequest {
    fn from(call: crate::ToolCall) -> Self {
        Self::new(call.id, call.name, call.arguments)
    }
}

/// The settled policy/execution state of one tool invocation.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolExecutionStatus {
    Completed,
    Failed,
    NeedsApproval,
    Deferred,
    Retryable,
}

impl ToolExecutionStatus {
    pub fn is_error(self) -> bool {
        !matches!(self, Self::Completed)
    }
}

/// A host-facing tool result with an explicit policy and retry classification.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ToolExecutionOutcome {
    pub status: ToolExecutionStatus,
    pub content: String,
}

impl ToolExecutionOutcome {
    pub fn completed(content: impl Into<String>) -> Self {
        Self {
            status: ToolExecutionStatus::Completed,
            content: content.into(),
        }
    }

    pub fn failed(content: impl Into<String>) -> Self {
        Self {
            status: ToolExecutionStatus::Failed,
            content: content.into(),
        }
    }
}

/// Describes the admission work required before a tool can cause side effects.
///
/// `Legacy` preserves the existing tool-owned lifecycle during incremental
/// migration. `ApprovalRequired` moves the approval decision to the host
/// execution delegate while leaving tool-specific validation with the tool.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolAdmission {
    Legacy,
    ApprovalRequired { action: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolError(pub String);

impl fmt::Display for ToolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Error for ToolError {}

/// A capability that the execution kernel may expose to a model.
pub trait Tool: Send + Sync {
    fn spec(&self) -> ToolSpec;
    fn execute(&self, arguments: &Value) -> Result<String, ToolError>;

    /// Describes host admission for one model-requested call.
    fn admission(&self, _request: &ToolExecutionRequest) -> Result<ToolAdmission, ToolError> {
        Ok(ToolAdmission::Legacy)
    }

    /// Executes a call after the host has completed its typed admission.
    /// Implementations returning `ApprovalRequired` must override this method
    /// so the legacy approval path is not invoked a second time.
    fn execute_after_admission(&self, request: &ToolExecutionRequest) -> ToolExecutionOutcome {
        match self.execute(&request.arguments) {
            Ok(content) => ToolExecutionOutcome::completed(content),
            Err(error) => ToolExecutionOutcome::failed(error.to_string()),
        }
    }

    /// Returns a structured result while preserving the legacy `execute` API.
    /// Host tools may override this to report approval, deferral, or retryable
    /// failures without encoding policy in a plain error string.
    fn execute_outcome(&self, arguments: &Value) -> ToolExecutionOutcome {
        match self.execute(arguments) {
            Ok(content) => ToolExecutionOutcome::completed(content),
            Err(error) => ToolExecutionOutcome::failed(error.to_string()),
        }
    }
}

/// Delegates one resolved tool call to the owner of its execution lifecycle.
///
/// Core uses this boundary after `ToolRouter` resolves a tool. A host can use
/// it to add admission, approval, sandbox, retry, and outcome classification
/// without moving those concerns into the portable tool contract. The
/// delegate must return a bounded outcome and must not mutate Core session
/// history; Core remains responsible for recording the result.
pub trait ToolExecutionDelegate: Send + Sync {
    fn execute(&self, tool: &dyn Tool, request: &ToolExecutionRequest) -> ToolExecutionOutcome;
}

#[cfg(test)]
#[path = "tool_tests.rs"]
mod tests;
