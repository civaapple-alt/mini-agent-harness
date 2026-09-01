use super::*;
use mini_agent_protocol::ToolError;
use mini_agent_protocol::ToolHandler;
use mini_agent_protocol::ToolRuntime;
use mini_agent_protocol::ToolSpec;
use serde_json::Value;

struct NamedTool(&'static str);

impl ToolHandler for NamedTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.0.to_string(),
            description: self.0.to_string(),
            parameters: serde_json::json!({"type": "object"}),
        }
    }
}

impl ToolRuntime for NamedTool {
    fn execute(&self, _arguments: &Value) -> Result<String, ToolError> {
        Ok(self.0.to_string())
    }
}

#[test]
fn default_catalog_is_explicit_and_typed() {
    assert_eq!(
        default_builtin_catalog(),
        &[
            ToolCatalogEntry {
                name: "read_file",
                provider: mini_agent_capabilities::BUILTIN_TOOL_PROVIDER,
                origin: ToolOrigin::Builtin,
                exposure: ToolExposure::Visible,
                admission: ToolAdmissionClass::ReadOnly,
            },
            ToolCatalogEntry {
                name: "edit_file",
                provider: mini_agent_capabilities::BUILTIN_TOOL_PROVIDER,
                origin: ToolOrigin::Builtin,
                exposure: ToolExposure::Visible,
                admission: ToolAdmissionClass::ApprovalRequired,
            },
            ToolCatalogEntry {
                name: "write_file",
                provider: mini_agent_capabilities::BUILTIN_TOOL_PROVIDER,
                origin: ToolOrigin::Builtin,
                exposure: ToolExposure::Visible,
                admission: ToolAdmissionClass::ApprovalRequired,
            },
            ToolCatalogEntry {
                name: "shell",
                provider: mini_agent_capabilities::BUILTIN_TOOL_PROVIDER,
                origin: ToolOrigin::Builtin,
                exposure: ToolExposure::Visible,
                admission: ToolAdmissionClass::ApprovalRequired,
            },
            ToolCatalogEntry {
                name: "web_fetch",
                provider: mini_agent_capabilities::BUILTIN_TOOL_PROVIDER,
                origin: ToolOrigin::Builtin,
                exposure: ToolExposure::Visible,
                admission: ToolAdmissionClass::ReadOnly,
            },
            ToolCatalogEntry {
                name: "read_image",
                provider: mini_agent_capabilities::BUILTIN_TOOL_PROVIDER,
                origin: ToolOrigin::Builtin,
                exposure: ToolExposure::Visible,
                admission: ToolAdmissionClass::ReadOnly,
            },
        ]
    );
}

#[test]
fn host_filter_drops_unlisted_builtin_tools() {
    let tools = retain_default_builtin_tools(vec![
        Box::new(NamedTool("read_file")),
        Box::new(NamedTool("unlisted_tool")),
        Box::new(NamedTool("disabled_tool")),
        Box::new(NamedTool("shell")),
    ]);

    assert_eq!(
        tools
            .iter()
            .map(|tool| tool.spec().name)
            .collect::<Vec<_>>(),
        vec!["read_file", "shell"]
    );
}

#[test]
fn builtin_selection_is_bounded_and_reversible() {
    let selection =
        BuiltinToolSelection::from_names(vec!["shell".to_string(), "read_file".to_string()])
            .unwrap();
    assert_eq!(selection.names(), &["shell", "read_file"]);
    assert_eq!(
        selection.hidden_names(),
        vec!["edit_file", "write_file", "web_fetch", "read_image"]
    );
    assert!(BuiltinToolSelection::from_names(vec!["unknown".to_string()]).is_err());
    assert!(
        BuiltinToolSelection::from_names(vec!["shell".to_string(), "shell".to_string(),]).is_err()
    );
}
