//! Minimal host-embedded provider example.
//!
//! Run with `cargo run -p mini-agent-capabilities --example external_tool_provider`.

use mini_agent_capabilities::CapabilityDescriptor;
use mini_agent_capabilities::CapabilityKind;
use mini_agent_capabilities::CapabilityRegistry;
use mini_agent_capabilities::ToolBuildRequest;
use mini_agent_capabilities::ToolProvider;
use mini_agent_protocol::Tool;
use mini_agent_protocol::ToolError;
use mini_agent_protocol::ToolHandler;
use mini_agent_protocol::ToolRuntime;
use mini_agent_protocol::ToolSpec;
use serde_json::Value;
use serde_json::json;
use std::sync::Arc;

struct EchoTool;

impl ToolHandler for EchoTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "external_echo".to_string(),
            description: "Echo a JSON value from an external provider".to_string(),
            parameters: json!({
                "type": "object",
                "properties": { "value": {} },
                "required": ["value"]
            }),
        }
    }
}

impl ToolRuntime for EchoTool {
    fn execute(&self, arguments: &Value) -> Result<String, ToolError> {
        arguments
            .get("value")
            .map(Value::to_string)
            .ok_or_else(|| ToolError("missing `value`".to_string()))
    }
}

struct ExampleToolProvider;

impl ToolProvider for ExampleToolProvider {
    fn descriptor(&self) -> CapabilityDescriptor {
        CapabilityDescriptor {
            id: "example-echo",
            kind: CapabilityKind::Tool,
            description: "Example host-embedded echo tool provider",
        }
    }

    fn build_tools(&self, _request: ToolBuildRequest) -> Result<Vec<Box<dyn Tool>>, ToolError> {
        Ok(vec![Box::new(EchoTool)])
    }
}

fn main() {
    let registry = CapabilityRegistry::builtin().with_tool_provider(Arc::new(ExampleToolProvider));
    println!("registered providers:");
    for descriptor in registry.descriptors() {
        println!(
            "- {:?}: {} ({})",
            descriptor.kind, descriptor.id, descriptor.description
        );
    }
    println!("select `example-echo` in a RuntimeProfile to expose external_echo");
}
