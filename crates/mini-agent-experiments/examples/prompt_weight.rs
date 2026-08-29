use mini_agent_capabilities::openai::OpenAiModel;
use mini_agent_core::Harness;
use mini_agent_core::HarnessConfig;
use mini_agent_core::ToolRegistry;
use mini_agent_host::env_file::Environment;
use mini_agent_protocol::Event;
use mini_agent_protocol::Observer;
use mini_agent_protocol::StopReason;
use mini_agent_protocol::Tool;
use mini_agent_protocol::ToolError;
use mini_agent_protocol::ToolSpec;
use serde_json::Value;
use serde_json::json;
use std::env;
use std::error::Error;
use std::fs::OpenOptions;
use std::io;
use std::io::BufWriter;
use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

const MINIMAL_PROMPT: &str = include_str!(
    "../../../.agents/notes/rejected/feature/resources/prompt-weight/prompt-weight-minimal.txt"
);
const EXPANDED_PROMPT: &str = include_str!(
    "../../../.agents/notes/rejected/feature/resources/prompt-weight/prompt-weight-expanded.txt"
);
const HELP: &str = "prompt-weight experiment\n\nUSAGE:\n    cargo run -p mini-agent-experiments --example prompt_weight -- [--runs N] [--output PATH]\n\nEach repetition normally issues 12 and may issue up to 18 model responses. Output paths must not already exist.";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Treatment {
    Minimal,
    Expanded,
}

impl Treatment {
    fn name(self) -> &'static str {
        match self {
            Self::Minimal => "minimal",
            Self::Expanded => "expanded",
        }
    }

    fn prompt(self) -> &'static str {
        match self {
            Self::Minimal => MINIMAL_PROMPT,
            Self::Expanded => EXPANDED_PROMPT,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
struct Args {
    runs: usize,
    output: Option<PathBuf>,
}

#[derive(Clone, Copy)]
struct Task {
    id: &'static str,
    key: &'static str,
    expected: &'static str,
}

const TASKS: [Task; 3] = [
    Task {
        id: "ascii",
        key: "alpha",
        expected: "ALPHA-42",
    },
    Task {
        id: "unicode",
        key: "beta",
        expected: "BETA-蓝",
    },
    Task {
        id: "punctuation",
        key: "gamma",
        expected: "gamma/value:v3",
    },
];

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), Box<dyn Error>> {
    let raw_args = env::args().skip(1).collect::<Vec<_>>();
    if matches!(raw_args.as_slice(), [argument] if matches!(argument.as_str(), "--help" | "-h")) {
        println!("{HELP}");
        return Ok(());
    }
    let args = parse_args(raw_args)?;
    let environment = Environment::load(".env")?;
    let api_key = environment
        .resolve("OPENAI_API_KEY")
        .ok_or("OPENAI_API_KEY is required")?;
    let model_name = environment
        .resolve("OPENAI_MODEL")
        .ok_or("OPENAI_MODEL is required")?;
    let base_url = environment.resolve("OPENAI_BASE_URL");
    let _sources = (api_key.source, model_name.source);
    let api_key = api_key.value;
    let model_name = model_name.value;
    let base_url = base_url
        .map(|value| value.value)
        .unwrap_or_else(|| "https://api.openai.com/v1".to_string());
    let mut output: Box<dyn Write> = match args.output {
        Some(path) => Box::new(BufWriter::new(
            OpenOptions::new().write(true).create_new(true).open(path)?,
        )),
        None => Box::new(BufWriter::new(io::stdout())),
    };
    let mut records = Vec::new();

    for repetition in 1..=args.runs {
        for (task_index, task) in TASKS.iter().enumerate() {
            let treatments = if (repetition + task_index) % 2 == 0 {
                [Treatment::Minimal, Treatment::Expanded]
            } else {
                [Treatment::Expanded, Treatment::Minimal]
            };
            for treatment in treatments {
                let record = run_case(
                    repetition,
                    *task,
                    treatment,
                    &api_key,
                    &model_name,
                    &base_url,
                )
                .await;
                write_json_line(&mut output, &record)?;
                records.push(record);
            }
        }
    }

    let summary = summarize(&model_name, args.runs, &records);
    write_json_line(&mut output, &summary)?;
    output.flush()?;
    Ok(())
}

async fn run_case(
    repetition: usize,
    task: Task,
    treatment: Treatment,
    api_key: &str,
    model_name: &str,
    base_url: &str,
) -> Value {
    let started = Instant::now();
    let model = match OpenAiModel::new(
        api_key.to_string(),
        model_name.to_string(),
        base_url.to_string(),
        false,
        mini_agent_capabilities::image::ImageStore::memory_only(),
    ) {
        Ok(model) => model,
        Err(error) => return error_record(repetition, task, treatment, error.to_string()),
    };
    let tools = ToolRegistry::new(vec![Box::new(Lookup)]);
    let config = HarnessConfig {
        system_prompt: treatment.prompt().trim().to_string(),
        max_steps: 3,
        ..HarnessConfig::default()
    };
    let mut harness = Harness::new(model, tools, config);
    let mut observer = EvalObserver::default();
    let prompt = format!(
        "Call lookup exactly once with key `{}`. Then reply with exactly the tool result and nothing else.",
        task.key
    );
    let outcome = match harness.run(prompt, &mut observer).await {
        Ok(outcome) => outcome,
        Err(error) => return error_record(repetition, task, treatment, error.to_string()),
    };
    let lookup_calls = observer
        .events
        .iter()
        .filter_map(|event| match event {
            Event::ToolStarted { call } if call.name == "lookup" => Some(call),
            _ => None,
        })
        .collect::<Vec<_>>();
    let argument_correct = lookup_calls.len() == 1
        && lookup_calls[0].arguments.get("key").and_then(Value::as_str) == Some(task.key);
    let tool_errors = observer
        .events
        .iter()
        .filter(|event| matches!(event, Event::ToolFinished { is_error: true, .. }))
        .count();

    json!({
        "type": "run",
        "model": model_name,
        "repetition": repetition,
        "task": task.id,
        "treatment": treatment.name(),
        "system_prompt_bytes": treatment.prompt().trim().len(),
        "completed": outcome.stop_reason == StopReason::Completed,
        "model_steps": outcome.steps,
        "tool_calls": lookup_calls.len(),
        "tool_errors": tool_errors,
        "argument_correct": argument_correct,
        "verifier_passed": outcome.final_text.trim() == task.expected,
        "final_text": outcome.final_text,
        "latency_ms": started.elapsed().as_millis(),
        "usage_responses": observer.usage_responses,
        "input_tokens": observer.usage().map(|usage| usage.0),
        "cached_input_tokens": observer.usage().map(|usage| usage.1),
        "output_tokens": observer.usage().map(|usage| usage.2),
        "events": observer.events,
    })
}

fn error_record(repetition: usize, task: Task, treatment: Treatment, error: String) -> Value {
    json!({
        "type": "run",
        "repetition": repetition,
        "task": task.id,
        "treatment": treatment.name(),
        "completed": false,
        "verifier_passed": false,
        "error": error,
    })
}

fn summarize(model: &str, runs: usize, records: &[Value]) -> Value {
    let treatment_summary = |treatment: Treatment| {
        let matching = records
            .iter()
            .filter(|record| record["treatment"] == treatment.name())
            .collect::<Vec<_>>();
        let passed = matching
            .iter()
            .filter(|record| record["verifier_passed"] == true)
            .count();
        let input_tokens = matching
            .iter()
            .filter_map(|record| record["input_tokens"].as_u64())
            .sum::<u64>();
        let output_tokens = matching
            .iter()
            .filter_map(|record| record["output_tokens"].as_u64())
            .sum::<u64>();
        json!({
            "attempted": matching.len(),
            "passed": passed,
            "pass_rate": passed as f64 / matching.len() as f64,
            "reported_input_tokens": input_tokens,
            "reported_output_tokens": output_tokens,
        })
    };

    json!({
        "type": "summary",
        "model": model,
        "repetitions": runs,
        "minimal": treatment_summary(Treatment::Minimal),
        "expanded": treatment_summary(Treatment::Expanded),
    })
}

#[derive(Default)]
struct EvalObserver {
    events: Vec<Event>,
    usage_responses: usize,
    input_tokens: u64,
    cached_input_tokens: u64,
    output_tokens: u64,
}

impl EvalObserver {
    fn usage(&self) -> Option<(u64, u64, u64)> {
        (self.usage_responses > 0).then_some((
            self.input_tokens,
            self.cached_input_tokens,
            self.output_tokens,
        ))
    }
}

impl Observer for EvalObserver {
    fn observe(&mut self, event: &Event) {
        if let Event::ModelResponded {
            usage: Some(usage), ..
        } = event
        {
            self.usage_responses += 1;
            self.input_tokens += usage.input_tokens;
            self.cached_input_tokens += usage.cached_input_tokens;
            self.output_tokens += usage.output_tokens;
        }
        self.events.push(event.clone());
    }
}

struct Lookup;

impl Tool for Lookup {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "lookup".to_string(),
            description: "Return the exact value associated with a test key".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {"key": {"type": "string"}},
                "required": ["key"],
                "additionalProperties": false
            }),
        }
    }

    fn execute(&self, arguments: &Value) -> Result<String, ToolError> {
        match arguments.get("key").and_then(Value::as_str) {
            Some("alpha") => Ok("ALPHA-42".to_string()),
            Some("beta") => Ok("BETA-蓝".to_string()),
            Some("gamma") => Ok("gamma/value:v3".to_string()),
            Some(key) => Err(ToolError(format!("unknown key: {key}"))),
            None => Err(ToolError("key must be a string".to_string())),
        }
    }
}

fn parse_args(args: Vec<String>) -> Result<Args, String> {
    let mut args = args.into_iter();
    let mut runs = 1;
    let mut output = None;
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--runs" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--runs requires a number".to_string())?;
                runs = value
                    .parse::<usize>()
                    .map_err(|_| "--runs must be an integer".to_string())?;
                if !(1..=20).contains(&runs) {
                    return Err("--runs must be between 1 and 20".to_string());
                }
            }
            "--output" => {
                if output.is_some() {
                    return Err("--output may be provided only once".to_string());
                }
                output = Some(PathBuf::from(
                    args.next()
                        .ok_or_else(|| "--output requires a path".to_string())?,
                ));
            }
            _ => return Err(format!("unknown option: {argument}")),
        }
    }
    Ok(Args { runs, output })
}

fn write_json_line(output: &mut dyn Write, value: &Value) -> Result<(), Box<dyn Error>> {
    serde_json::to_writer(&mut *output, value)?;
    writeln!(output)?;
    output.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_one_repetition() {
        assert_eq!(
            parse_args(Vec::new()).unwrap(),
            Args {
                runs: 1,
                output: None,
            }
        );
    }

    #[test]
    fn parses_runs_and_output() {
        assert_eq!(
            parse_args(vec![
                "--runs".to_string(),
                "3".to_string(),
                "--output".to_string(),
                "result.jsonl".to_string(),
            ])
            .unwrap(),
            Args {
                runs: 3,
                output: Some(PathBuf::from("result.jsonl")),
            }
        );
    }

    #[test]
    fn lookup_fixture_is_exact() {
        assert_eq!(Lookup.execute(&json!({"key": "beta"})).unwrap(), "BETA-蓝");
    }
}
