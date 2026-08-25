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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolError(pub String);

impl fmt::Display for ToolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Error for ToolError {}

/// A capability that the harness may expose to a model.
///
/// Tools are synchronous on purpose. The first harness executes one action at
/// a time; concurrency should be introduced only by an experiment that needs
/// it.
pub trait Tool: Send + Sync {
    fn spec(&self) -> ToolSpec;
    fn execute(&self, arguments: &Value) -> Result<String, ToolError>;
}

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
