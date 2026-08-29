use super::*;

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
fn plain_terminal_ask_streams_assistant_to_stdout() {
    assert!(matches!(
        script_assistant_display(ScriptFormat::Text, true, true),
        AssistantDisplay::Stream {
            target: OutputTarget::Stdout,
            color: true
        }
    ));
}

#[test]
fn redirected_and_json_ask_hold_the_final_answer() {
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
fn shell_tool_start_includes_a_bounded_single_line_command() {
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
}

#[test]
fn web_fetch_start_displays_the_url() {
    let call = ToolCall {
        id: "call-1".to_string(),
        name: "web_fetch".to_string(),
        arguments: json!({"url": "https://example.com/docs"}),
    };
    assert_eq!(
        format_tool_started(&call, false),
        "tool> web_fetch — https://example.com/docs"
    );
}

#[test]
fn file_tool_start_only_displays_the_path() {
    let call = ToolCall {
        id: "call-1".to_string(),
        name: "write_file".to_string(),
        arguments: json!({"path": "README.md", "content": "secret"}),
    };

    assert_eq!(
        format_tool_started(&call, false),
        "tool> write_file — README.md"
    );
    assert!(!format_tool_started(&call, false).contains("secret"));
}

#[test]
fn tool_finished_stays_on_one_bounded_line() {
    assert_eq!(
        format_tool_finished("read_file", "MAKE THIS LOUD", false, false),
        "tool[ok]> MAKE THIS LOUD"
    );
    assert_eq!(
        format_tool_finished(
            "read_file",
            "use crate::config::RuntimeConfig;\nuse crate::observer::RunObserver;",
            false,
            false
        ),
        "tool[ok]> use crate::config::RuntimeConfig;\\nuse crate::observer::RunObserver;"
    );

    let long = "x".repeat(MAX_TOOL_DETAIL_BYTES + 8);
    let line = format_tool_finished("read_file", &long, false, false);
    assert!(line.starts_with("tool[ok]> "));
    assert!(line.ends_with('…'));
    assert!(line.len() <= "tool[ok]> ".len() + MAX_TOOL_DETAIL_BYTES);
}

#[test]
fn shell_tool_finished_prints_full_stdout() {
    let output = "ok: workspace\nerror: credential\n";
    assert_eq!(
        format_tool_finished("shell", output, false, false),
        "tool[ok]> ok: workspace\nerror: credential\n"
    );
    let long = "x".repeat(MAX_TOOL_DETAIL_BYTES + 8);
    let line = format_tool_finished("shell", &long, false, false);
    assert_eq!(line, format!("tool[ok]> {long}"));
    assert!(!line.contains('…'));
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
