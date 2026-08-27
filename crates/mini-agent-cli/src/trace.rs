use mini_agent_core::Event;
use mini_agent_core::RunFailure;
use mini_agent_core::StopReason;
use mini_agent_core::ToolCall;
use serde::Serialize;
use serde_json::json;
use std::fs::File;
use std::io::BufRead;
use std::io::BufReader;
use std::io::IsTerminal;
use std::path::Path;
use std::process::ExitCode;

#[derive(Clone, Debug, Default, Serialize, PartialEq, Eq)]
pub struct TraceSummary {
    pub file: String,
    pub prompt: Option<String>,
    pub steps: usize,
    pub completed: bool,
    pub stop_reason: Option<String>,
    pub failure_reason: Option<String>,
    pub model_requests: usize,
    pub input_tokens: u64,
    pub cached_input_tokens: u64,
    pub output_tokens: u64,
    pub tool_calls_total: usize,
    pub tool_calls_success: usize,
    pub tool_calls_error: usize,
    pub tool_calls_truncated: usize,
    pub compactions: usize,
}

pub fn load_events(path: &Path) -> Result<Vec<Event>, String> {
    let file =
        File::open(path).map_err(|e| format!("cannot open trace file {}: {e}", path.display()))?;
    let reader = BufReader::new(file);
    let mut raw_lines = Vec::new();
    for (line_no, line) in reader.lines().enumerate() {
        let line = line.map_err(|e| format!("error reading line {}: {e}", line_no + 1))?;
        let trimmed = line.trim().to_string();
        if !trimmed.is_empty() {
            raw_lines.push(trimmed);
        }
    }

    if raw_lines.is_empty() {
        return Ok(Vec::new());
    }

    // Try parsing as native observation trace events
    let mut trace_events = Vec::new();
    let mut trace_err = None;
    for (line_no, line) in raw_lines.iter().enumerate() {
        match serde_json::from_str::<Event>(line) {
            Ok(event) => trace_events.push(event),
            Err(e) => {
                trace_err = Some(format!("error parsing event at line {}: {e}", line_no + 1));
                break;
            }
        }
    }

    if trace_err.is_none() {
        return Ok(trace_events);
    }

    // If native event parsing failed, delegate to durable session.jsonl adapter in session module
    if let Some(session_events) = crate::session::try_load_session_events(&raw_lines) {
        return Ok(session_events);
    }

    Err(trace_err.unwrap())
}

pub fn compute_summary(path: &Path, events: &[Event]) -> TraceSummary {
    let mut summary = TraceSummary {
        file: path.display().to_string(),
        ..Default::default()
    };

    for event in events {
        match event {
            Event::RunStarted { prompt } => {
                if summary.prompt.is_none() {
                    summary.prompt = Some(prompt.clone());
                }
            }
            Event::ModelStarted { step } => {
                summary.steps = summary.steps.max(*step);
            }
            Event::ModelResponded { usage, .. } => {
                summary.model_requests += 1;
                if let Some(usage) = usage {
                    summary.input_tokens = summary.input_tokens.saturating_add(usage.input_tokens);
                    summary.cached_input_tokens = summary
                        .cached_input_tokens
                        .saturating_add(usage.cached_input_tokens);
                    summary.output_tokens =
                        summary.output_tokens.saturating_add(usage.output_tokens);
                }
            }
            Event::ToolFinished {
                is_error,
                truncated,
                ..
            } => {
                summary.tool_calls_total += 1;
                if *is_error {
                    summary.tool_calls_error += 1;
                } else {
                    summary.tool_calls_success += 1;
                }
                if *truncated {
                    summary.tool_calls_truncated += 1;
                }
            }
            Event::ContextCompactionFinished { usage, .. } => {
                summary.compactions += 1;
                if let Some(usage) = usage {
                    summary.input_tokens = summary.input_tokens.saturating_add(usage.input_tokens);
                    summary.cached_input_tokens = summary
                        .cached_input_tokens
                        .saturating_add(usage.cached_input_tokens);
                    summary.output_tokens =
                        summary.output_tokens.saturating_add(usage.output_tokens);
                }
            }
            Event::RunFinished { stop_reason, steps } => {
                summary.steps = summary.steps.max(*steps);
                summary.completed = *stop_reason == StopReason::Completed;
                summary.stop_reason = Some(match stop_reason {
                    StopReason::Completed => "completed".to_string(),
                    StopReason::StepLimit => "step_limit".to_string(),
                });
            }
            Event::RunFailed { reason } => {
                summary.completed = false;
                summary.failure_reason = Some(match reason {
                    RunFailure::Model => "model_error".to_string(),
                    RunFailure::Compaction => "compaction_error".to_string(),
                    RunFailure::LimitExceeded(limit) => format!("limit_exceeded({limit})"),
                });
            }
            Event::AssistantReasoningDelta { .. }
            | Event::AssistantTextDelta { .. }
            | Event::ToolStarted { .. }
            | Event::ContextCompactionStarted { .. } => {}
        }
    }

    summary
}

pub fn replay(path: &Path, json_output: bool) -> ExitCode {
    let events = match load_events(path) {
        Ok(events) => events,
        Err(error) => {
            eprintln!("error: {error}");
            return ExitCode::from(1);
        }
    };

    if json_output {
        let summary = compute_summary(path, &events);
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "summary": summary,
                "events_count": events.len()
            }))
            .unwrap_or_default()
        );
        return ExitCode::SUCCESS;
    }

    let color = std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none();
    println!(
        "--- trace replay: {} ({} events) ---",
        path.display(),
        events.len()
    );

    let mut in_reasoning = false;
    let mut in_assistant = false;

    for event in &events {
        match event {
            Event::RunStarted { prompt } => {
                println!(
                    "{} {}",
                    styled_tag("run[start]>", TagColor::Green, color),
                    prompt
                );
            }
            Event::ModelStarted { step } => {
                if in_reasoning || in_assistant {
                    println!();
                    in_reasoning = false;
                    in_assistant = false;
                }
                println!(
                    "{} step {step}",
                    styled_tag("model[start]>", TagColor::Cyan, color)
                );
            }
            Event::AssistantReasoningDelta { delta } => {
                if !in_reasoning {
                    if in_assistant {
                        println!();
                        in_assistant = false;
                    }
                    print!("{} ", styled_tag("thinking>", TagColor::Magenta, color));
                    in_reasoning = true;
                }
                print!("{delta}");
            }
            Event::AssistantTextDelta { delta } => {
                if !in_assistant {
                    if in_reasoning {
                        println!();
                        in_reasoning = false;
                    }
                    print!("{} ", styled_tag("assistant>", TagColor::Blue, color));
                    in_assistant = true;
                }
                print!("{delta}");
            }
            Event::ModelResponded {
                tool_calls, text, ..
            } => {
                if in_reasoning || in_assistant {
                    println!();
                    in_reasoning = false;
                    in_assistant = false;
                }
                if tool_calls.is_empty() && !text.is_empty() {
                    // Final response already streamed or captured
                }
            }
            Event::ToolStarted { call } => {
                if in_reasoning || in_assistant {
                    println!();
                    in_reasoning = false;
                    in_assistant = false;
                }
                println!("{}", format_tool_started(call, color));
            }
            Event::ToolFinished {
                name,
                content,
                is_error,
                truncated,
                ..
            } => {
                if in_reasoning || in_assistant {
                    println!();
                    in_reasoning = false;
                    in_assistant = false;
                }
                println!(
                    "{}",
                    format_tool_finished(name, content, *is_error, *truncated, color)
                );
            }
            Event::ContextCompactionStarted { before_bytes } => {
                if in_reasoning || in_assistant {
                    println!();
                    in_reasoning = false;
                    in_assistant = false;
                }
                println!(
                    "{} compacting {before_bytes} bytes",
                    styled_tag("context>", TagColor::Cyan, color)
                );
            }
            Event::ContextCompactionFinished {
                before_bytes,
                after_bytes,
                ..
            } => {
                println!(
                    "{} compacted {before_bytes} -> {after_bytes} bytes",
                    styled_tag("context>", TagColor::Cyan, color)
                );
            }
            Event::RunFinished { stop_reason, steps } => {
                if in_reasoning || in_assistant {
                    println!();
                    in_reasoning = false;
                    in_assistant = false;
                }
                println!(
                    "{} steps: {steps}, stop_reason: {stop_reason:?}",
                    styled_tag("run[finish]>", TagColor::Green, color)
                );
            }
            Event::RunFailed { reason } => {
                if in_reasoning || in_assistant {
                    println!();
                    in_reasoning = false;
                    in_assistant = false;
                }
                println!(
                    "{} {reason:?}",
                    styled_tag("run[failed]>", TagColor::Red, color)
                );
            }
        }
    }

    if in_reasoning || in_assistant {
        println!();
    }

    let summary = compute_summary(path, &events);
    print_summary_table(&summary, color);
    ExitCode::SUCCESS
}

pub fn summary(path: &Path, json_output: bool) -> ExitCode {
    let events = match load_events(path) {
        Ok(events) => events,
        Err(error) => {
            eprintln!("error: {error}");
            return ExitCode::from(1);
        }
    };

    let summary = compute_summary(path, &events);
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&summary).unwrap_or_default()
        );
    } else {
        let color = std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none();
        print_summary_table(&summary, color);
    }
    ExitCode::SUCCESS
}

fn print_summary_table(summary: &TraceSummary, color: bool) {
    println!(
        "\n{}",
        styled_tag("--- Execution Summary ---", TagColor::Cyan, color)
    );
    println!("File:            {}", summary.file);
    if let Some(prompt) = &summary.prompt {
        println!("Prompt:          {}", prompt);
    }
    println!("Total Steps:     {}", summary.steps);
    println!(
        "Status:          {}",
        if summary.completed {
            "Completed"
        } else {
            "Incomplete/Failed"
        }
    );
    if let Some(reason) = &summary.stop_reason {
        println!("Stop Reason:     {}", reason);
    }
    if let Some(failure) = &summary.failure_reason {
        println!("Failure:         {}", failure);
    }
    println!("Model Requests:  {}", summary.model_requests);
    println!(
        "Tokens (In/Out): {} input ({} cached) / {} output",
        summary.input_tokens, summary.cached_input_tokens, summary.output_tokens
    );
    println!(
        "Tool Calls:      {} total ({} ok, {} err, {} truncated)",
        summary.tool_calls_total,
        summary.tool_calls_success,
        summary.tool_calls_error,
        summary.tool_calls_truncated
    );
    if summary.compactions > 0 {
        println!("Compactions:     {}", summary.compactions);
    }
}

#[derive(Clone, Copy)]
enum TagColor {
    Red,
    Green,
    Yellow,
    Blue,
    Magenta,
    Cyan,
}

fn styled_tag(tag: &str, color: TagColor, enabled: bool) -> String {
    if !enabled {
        return tag.to_string();
    }
    let code = match color {
        TagColor::Red => 31,
        TagColor::Green => 32,
        TagColor::Yellow => 33,
        TagColor::Blue => 34,
        TagColor::Magenta => 35,
        TagColor::Cyan => 36,
    };
    format!("\u{1b}[{code}m{tag}\u{1b}[0m")
}

fn format_tool_started(call: &ToolCall, color: bool) -> String {
    let tag = styled_tag("tool>", TagColor::Yellow, color);
    format!("{tag} {} — {}", call.name, call.arguments)
}

fn format_tool_finished(
    name: &str,
    content: &str,
    is_error: bool,
    truncated: bool,
    color: bool,
) -> String {
    let (tag, tag_color) = if is_error {
        ("tool[error]>", TagColor::Red)
    } else {
        ("tool[ok]>", TagColor::Green)
    };
    let tag = styled_tag(tag, tag_color, color);
    let trunc_str = if truncated { " [truncated]" } else { "" };
    let preview = if content.len() > 120 {
        format!("{}...{}", &content[..60], &content[content.len() - 40..])
    } else {
        content.to_string()
    };
    format!("{tag} {name}{trunc_str}: {preview}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use mini_agent_core::ModelUsage;

    #[test]
    fn calculates_metrics_from_events() {
        let events = vec![
            Event::RunStarted {
                prompt: "test task".to_string(),
            },
            Event::ModelStarted { step: 1 },
            Event::ModelResponded {
                reasoning: "thinking...".to_string(),
                text: "hello".to_string(),
                tool_calls: vec![],
                usage: Some(ModelUsage {
                    input_tokens: 100,
                    cached_input_tokens: 20,
                    output_tokens: 50,
                }),
            },
            Event::RunFinished {
                stop_reason: StopReason::Completed,
                steps: 1,
            },
        ];

        let summary = compute_summary(Path::new("dummy.jsonl"), &events);
        assert_eq!(summary.steps, 1);
        assert!(summary.completed);
        assert_eq!(summary.model_requests, 1);
        assert_eq!(summary.input_tokens, 100);
        assert_eq!(summary.cached_input_tokens, 20);
        assert_eq!(summary.output_tokens, 50);
        assert_eq!(summary.prompt, Some("test task".to_string()));
    }

    #[test]
    fn parses_session_jsonl_records() {
        let lines = vec![
            json!({"seq": 1, "kind": "session_created", "session_id": "s-123"}).to_string(),
            json!({"seq": 2, "kind": "turn_started", "prompt": "hello agent"}).to_string(),
            json!({
                "seq": 3,
                "kind": "item",
                "message": {
                    "role": "assistant",
                    "reasoning": "let me think",
                    "text": "here is my answer",
                    "tool_calls": []
                }
            })
            .to_string(),
            json!({
                "seq": 4,
                "kind": "turn_settled",
                "status": "completed",
                "steps": 1
            })
            .to_string(),
        ];

        let events = crate::session::try_load_session_events(&lines).unwrap();
        assert_eq!(events.len(), 6);
        assert!(matches!(&events[0], Event::RunStarted { prompt } if prompt == "hello agent"));
        assert!(matches!(&events[1], Event::ModelStarted { step: 1 }));
        assert!(
            matches!(&events[2], Event::AssistantReasoningDelta { delta } if delta == "let me think")
        );
        assert!(
            matches!(&events[3], Event::AssistantTextDelta { delta } if delta == "here is my answer")
        );
        assert!(
            matches!(&events[4], Event::ModelResponded { reasoning, text, .. } if reasoning == "let me think" && text == "here is my answer")
        );
        assert!(matches!(
            &events[5],
            Event::RunFinished {
                stop_reason: StopReason::Completed,
                steps: 1
            }
        ));

        let summary = compute_summary(std::path::Path::new("session.jsonl"), &events);
        assert_eq!(summary.prompt, Some("hello agent".to_string()));
        assert_eq!(summary.steps, 1);
        assert_eq!(summary.model_requests, 1);
        assert!(summary.completed);
    }
}
