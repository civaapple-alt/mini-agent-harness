use super::*;
use mini_agent_protocol::ToolCall;
use serde_json::json;

#[test]
fn colors_only_the_terminal_tag() {
    assert_eq!(
        styled_tag("thinking>", TagColor::Magenta, true),
        "\u{1b}[35mthinking>\u{1b}[0m"
    );
    assert_eq!(
        styled_tag("tool[ok]>", TagColor::Green, true),
        "\u{1b}[32mtool[ok]>\u{1b}[0m"
    );
}

#[test]
fn leaves_tags_plain_when_color_is_disabled() {
    assert_eq!(
        styled_tag("tool[error]>", TagColor::Red, false),
        "tool[error]>"
    );
}

#[test]
fn terminal_final_answer_has_one_assistant_tag() {
    assert_eq!(
        format_final_answer("finished", true, true),
        "\u{1b}[34massistant>\u{1b}[0m finished"
    );
    assert_eq!(
        format_final_answer("finished", true, false),
        "assistant> finished"
    );
}

#[test]
fn redirected_final_answer_remains_script_friendly() {
    assert_eq!(format_final_answer("finished", false, false), "finished");
}

#[test]
fn script_output_hides_assistant_stream_when_redirected_or_json() {
    assert!(matches!(
        script_assistant_display(ScriptFormat::Text, true, true),
        AssistantDisplay::Stream {
            target: OutputTarget::Stdout,
            color: true
        }
    ));
    assert!(matches!(
        script_assistant_display(ScriptFormat::Text, false, false),
        AssistantDisplay::Hidden
    ));
    assert!(matches!(
        script_assistant_display(ScriptFormat::Json, true, true),
        AssistantDisplay::Hidden
    ));
}

#[test]
fn tool_start_details_are_bounded_and_redacted() {
    let call = ToolCall {
        id: "call-1".to_string(),
        name: "shell".to_string(),
        arguments: json!({"command": "Get-ChildItem\n-Force"}),
    };
    assert_eq!(
        format_tool_started(&call, false),
        "tool> shell — Get-ChildItem\\n-Force"
    );
    let long_call = ToolCall {
        arguments: json!({"command": "x".repeat(MAX_TOOL_DETAIL_BYTES + 1)}),
        ..call
    };
    let detail = tool_detail(&long_call).unwrap();
    assert!(detail.ends_with('…'));
    assert!(detail.len() <= MAX_TOOL_DETAIL_BYTES);

    let file_call = ToolCall {
        name: "write_file".to_string(),
        arguments: json!({"path": "README.md", "content": "secret"}),
        ..long_call
    };
    assert_eq!(
        format_tool_started(&file_call, false),
        "tool> write_file — README.md"
    );
    assert!(!format_tool_started(&file_call, false).contains("secret"));
}

#[test]
fn tool_finished_bounds_non_shell_output() {
    assert_eq!(
        format_tool_finished("read_file", "MAKE THIS LOUD", false, false),
        "tool[ok]> MAKE THIS LOUD"
    );
    let long = "x".repeat(MAX_TOOL_DETAIL_BYTES + 8);
    let line = format_tool_finished("read_file", &long, false, false);
    assert!(line.ends_with('…'));
    assert!(line.len() <= "tool[ok]> ".len() + MAX_TOOL_DETAIL_BYTES);
}

#[test]
fn shell_tool_finished_preserves_full_stdout() {
    let output = "ok: workspace\nerror: credential\n";
    assert_eq!(
        format_tool_finished("shell", output, false, false),
        "tool[ok]> ok: workspace\nerror: credential\n"
    );
    let long = "x".repeat(MAX_TOOL_DETAIL_BYTES + 8);
    assert_eq!(
        format_tool_finished("shell", &long, false, false),
        format!("tool[ok]> {long}")
    );
}

#[test]
fn unknown_tool_start_does_not_display_arbitrary_arguments() {
    let call = ToolCall {
        id: "call-1".to_string(),
        name: "project/mcp_tool".to_string(),
        arguments: json!({"token": "secret"}),
    };
    assert_eq!(format_tool_started(&call, false), "tool> project/mcp_tool");
}
