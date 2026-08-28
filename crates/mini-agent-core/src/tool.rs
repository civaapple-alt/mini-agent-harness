use mini_agent_protocol::Tool;
use mini_agent_protocol::ToolError;
use mini_agent_protocol::ToolSpec;
use serde_json::Value;

#[derive(Default)]
pub struct ToolRegistry {
    tools: Vec<Box<dyn Tool>>,
}

impl ToolRegistry {
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
}
