use mini_agent_core::Tool;
use mini_agent_core::ToolExecutionOutcome;
use mini_agent_core::ToolExecutionStatus;
use mini_agent_core::ToolSpec;
use serde_json::Value;

/// Wraps host tools with the policy-aware outcome projection used by core.
pub fn classify_tools(tools: Vec<Box<dyn Tool>>) -> Vec<Box<dyn Tool>> {
    tools
        .into_iter()
        .map(|tool| Box::new(ClassifiedTool(tool)) as Box<dyn Tool>)
        .collect()
}

struct ClassifiedTool(Box<dyn Tool>);

impl Tool for ClassifiedTool {
    fn spec(&self) -> ToolSpec {
        self.0.spec()
    }

    fn execute(&self, arguments: &Value) -> Result<String, mini_agent_core::ToolError> {
        self.0.execute(arguments)
    }

    fn execute_outcome(&self, arguments: &Value) -> ToolExecutionOutcome {
        classify_outcome(self.0.execute_outcome(arguments))
    }
}

fn classify_outcome(outcome: ToolExecutionOutcome) -> ToolExecutionOutcome {
    if outcome.status != ToolExecutionStatus::Failed {
        return outcome;
    }
    let status = if outcome.content.starts_with("user denied:")
        || outcome
            .content
            .starts_with("denied non-interactive action:")
    {
        ToolExecutionStatus::NeedsApproval
    } else if outcome
        .content
        .starts_with("workspace mutations locked in Plan Mode")
    {
        ToolExecutionStatus::Deferred
    } else if outcome.content.contains("circuit breaker is open")
        || outcome.content.contains("timed out")
    {
        ToolExecutionStatus::Retryable
    } else {
        return outcome;
    };
    ToolExecutionOutcome { status, ..outcome }
}

#[cfg(test)]
#[path = "tool_outcome_tests.rs"]
mod tests;
