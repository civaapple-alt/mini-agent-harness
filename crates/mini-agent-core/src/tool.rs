use mini_agent_protocol::Tool;
use mini_agent_protocol::ToolError;
use mini_agent_protocol::ToolExecutionDelegate;
use mini_agent_protocol::ToolExecutionOutcome;
use mini_agent_protocol::ToolExecutionRequest;
use mini_agent_protocol::ToolSpec;
use serde_json::Value;
use std::sync::Arc;

pub struct ToolRouter {
    tools: Vec<Box<dyn Tool>>,
    executor: Arc<dyn ToolExecutionDelegate>,
    hidden_tools: Vec<String>,
}

struct DirectToolExecution;

impl ToolExecutionDelegate for DirectToolExecution {
    fn execute(&self, tool: &dyn Tool, request: &ToolExecutionRequest) -> ToolExecutionOutcome {
        tool.execute_outcome(&request.arguments)
    }
}

impl ToolRouter {
    pub fn new(tools: Vec<Box<dyn Tool>>) -> Self {
        Self::with_executor(tools, Arc::new(DirectToolExecution))
    }

    pub fn with_executor(
        tools: Vec<Box<dyn Tool>>,
        executor: Arc<dyn ToolExecutionDelegate>,
    ) -> Self {
        Self {
            tools,
            executor,
            hidden_tools: Vec::new(),
        }
    }

    pub fn specs(&self) -> Vec<ToolSpec> {
        self.tools
            .iter()
            .filter(|tool| !self.is_hidden(&tool.spec().name))
            .map(|tool| tool.spec())
            .collect()
    }

    pub fn extend(&mut self, tools: Vec<Box<dyn Tool>>) {
        self.tools.extend(tools);
    }

    /// Replaces the bounded model-visible tool filter without dropping tool
    /// implementations, so a Thread can widen or narrow its selection.
    pub fn set_hidden_tools(&mut self, names: Vec<String>) {
        self.hidden_tools = names;
    }

    pub fn execute(&self, name: &str, arguments: &Value) -> Result<String, ToolError> {
        let request = ToolExecutionRequest::new("legacy", name, arguments.clone());
        let outcome = self.execute_outcome(&request);
        if outcome.status == mini_agent_protocol::ToolExecutionStatus::Completed {
            Ok(outcome.content)
        } else {
            Err(ToolError(outcome.content))
        }
    }

    /// Routes one model call and preserves the host policy outcome.
    pub fn execute_outcome(&self, request: &ToolExecutionRequest) -> ToolExecutionOutcome {
        let Some(tool) = self.tools.iter().find(|tool| {
            let name = tool.spec().name;
            name == request.name && !self.is_hidden(&name)
        }) else {
            return ToolExecutionOutcome::failed(format!("unknown tool: {}", request.name));
        };
        self.executor.execute(tool.as_ref(), request)
    }

    fn is_hidden(&self, name: &str) -> bool {
        self.hidden_tools.iter().any(|hidden| hidden == name)
    }
}

impl Default for ToolRouter {
    fn default() -> Self {
        Self::new(Vec::new())
    }
}

/// Compatibility name retained while callers migrate to the routing boundary.
pub type ToolRegistry = ToolRouter;

#[cfg(test)]
#[path = "tool_tests.rs"]
mod tests;
