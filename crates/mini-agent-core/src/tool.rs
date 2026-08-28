use mini_agent_protocol::Tool;
use mini_agent_protocol::ToolError;
use mini_agent_protocol::ToolExecutionOutcome;
use mini_agent_protocol::ToolExecutionRequest;
use mini_agent_protocol::ToolSpec;
use serde_json::Value;

#[derive(Default)]
pub struct ToolRouter {
    tools: Vec<Box<dyn Tool>>,
}

impl ToolRouter {
    pub fn new(tools: Vec<Box<dyn Tool>>) -> Self {
        Self { tools }
    }

    pub fn specs(&self) -> Vec<ToolSpec> {
        self.tools.iter().map(|tool| tool.spec()).collect()
    }

    pub fn extend(&mut self, tools: Vec<Box<dyn Tool>>) {
        self.tools.extend(tools);
    }

    pub fn execute(&self, name: &str, arguments: &Value) -> Result<String, ToolError> {
        let tool = self
            .tools
            .iter()
            .find(|tool| tool.spec().name == name)
            .ok_or_else(|| ToolError(format!("unknown tool: {name}")))?;
        tool.execute(arguments)
    }

    /// Routes one model call and preserves the host policy outcome.
    pub fn execute_outcome(&self, request: &ToolExecutionRequest) -> ToolExecutionOutcome {
        let Some(tool) = self
            .tools
            .iter()
            .find(|tool| tool.spec().name == request.name)
        else {
            return ToolExecutionOutcome::failed(format!("unknown tool: {}", request.name));
        };
        tool.execute_outcome(&request.arguments)
    }
}

/// Compatibility name retained while callers migrate to the routing boundary.
pub type ToolRegistry = ToolRouter;
