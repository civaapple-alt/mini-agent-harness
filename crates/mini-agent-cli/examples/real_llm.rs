#[allow(dead_code)]
#[path = "../src/env_file.rs"]
mod env_file;
#[allow(dead_code)]
#[path = "../src/image.rs"]
mod image;
#[path = "../src/openai/mod.rs"]
mod openai;
#[cfg(not(test))]
#[allow(dead_code, unused_imports)]
#[path = "../src/session.rs"]
mod session;

use env_file::Environment;
use image::DeepSeekFiles;
use image::FileUploader;
use image::ImageStore;
use mini_agent_core::ContextLimitBehavior;
use mini_agent_core::Event;
use mini_agent_core::Harness;
use mini_agent_core::HarnessConfig;
use mini_agent_core::HarnessError;
use mini_agent_core::Message;
use mini_agent_core::Model;
use mini_agent_core::ModelEventSink;
use mini_agent_core::ModelRequest;
use mini_agent_core::ModelResponse;
use mini_agent_core::Observer;
use mini_agent_core::StopReason;
use mini_agent_core::Tool;
use mini_agent_core::ToolCall;
use mini_agent_core::ToolError;
use mini_agent_core::ToolRegistry;
use mini_agent_core::ToolSpec;
use openai::OpenAiError;
use openai::OpenAiModel;
use serde_json::Value;
use serde_json::json;
use std::env;
use std::error::Error;
use std::fmt;
#[cfg(not(test))]
use std::fs;
use std::fs::OpenOptions;
use std::io;
use std::io::BufWriter;
use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::Duration;
#[cfg(not(test))]
use std::time::SystemTime;
#[cfg(not(test))]
use std::time::UNIX_EPOCH;
use tokio::time::timeout;

const DEFAULT_MAX_OUTPUT_TOKENS: usize = 256;
const MAX_OUTPUT_TOKENS: usize = 1024;
const MAX_REQUESTS: usize = 12;
const DEFAULT_TIMEOUT_SECONDS: u64 = 120;
const TEST_PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x04, 0x00, 0x00, 0x00, 0xB5, 0x1C, 0x0C,
    0x02, 0x00, 0x00, 0x00, 0x0B, 0x49, 0x44, 0x41, 0x54, 0x78, 0xDA, 0x63, 0x64, 0xF8, 0x0F, 0x00,
    0x01, 0x05, 0x01, 0x01, 0x27, 0x18, 0xE3, 0x66, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44,
    0xAE, 0x42, 0x60, 0x82,
];
const HELP: &str = "real-llm integration checks (network and provider billing are opt-in)\n\nUSAGE:\n    cargo run -p mini-agent-cli --example real_llm -- --allow-paid [OPTIONS]\n\nOPTIONS:\n    --allow-paid                 Required acknowledgement before contacting a provider\n    --scenario LIST               text, tool, conversation, persistence, vision, compaction, or all (default: text)\n    --max-requests N              Hard request budget, 1..12 (default: scenario budget)\n    --max-output-tokens N         Provider output cap, 16..1024 (default: 256)\n    --timeout-seconds N           Per-scenario wall-clock cap, 5..120 (default: 120)\n    --output PATH                 Create a JSONL evidence file instead of stdout\n\nThe runner never runs from cargo test or CI. Every scenario uses a short fixed\nprompt and reports model steps, provider requests, usage, and a deterministic\nverifier result.";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Scenario {
    Text,
    Tool,
    Conversation,
    Persistence,
    Vision,
    Compaction,
}

impl Scenario {
    fn parse(name: &str) -> Result<Self, String> {
        match name {
            "text" => Ok(Self::Text),
            "tool" => Ok(Self::Tool),
            "conversation" => Ok(Self::Conversation),
            "persistence" => Ok(Self::Persistence),
            "vision" => Ok(Self::Vision),
            "compaction" => Ok(Self::Compaction),
            other => Err(format!(
                "unknown scenario {other}; choose text, tool, conversation, persistence, vision, compaction, or all"
            )),
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Tool => "tool",
            Self::Conversation => "conversation",
            Self::Persistence => "persistence",
            Self::Vision => "vision",
            Self::Compaction => "compaction",
        }
    }

    fn request_budget(self) -> usize {
        match self {
            Self::Text => 1,
            Self::Tool => 2,
            Self::Conversation => 2,
            Self::Persistence => 2,
            Self::Vision => 3,
            Self::Compaction => 2,
        }
    }
}

#[derive(Debug)]
struct Args {
    allow_paid: bool,
    scenarios: Vec<Scenario>,
    max_requests: Option<usize>,
    max_output_tokens: usize,
    timeout_seconds: u64,
    output: Option<PathBuf>,
}

#[derive(Default)]
struct EvalObserver {
    events: Vec<Event>,
    input_tokens: u64,
    cached_input_tokens: u64,
    output_tokens: u64,
}

impl EvalObserver {
    fn requests(&self) -> usize {
        self.events
            .iter()
            .filter(|event| matches!(event, Event::ModelStarted { .. }))
            .count()
    }

    fn compactions(&self) -> usize {
        self.events
            .iter()
            .filter(|event| matches!(event, Event::ContextCompactionFinished { .. }))
            .count()
    }

    fn usage_value(&self) -> Value {
        json!({
            "input_tokens": self.input_tokens,
            "cached_input_tokens": self.cached_input_tokens,
            "output_tokens": self.output_tokens,
        })
    }
}

impl Observer for EvalObserver {
    fn observe(&mut self, event: &Event) {
        if let Event::ModelResponded {
            usage: Some(usage), ..
        } = event
        {
            self.input_tokens += usage.input_tokens;
            self.cached_input_tokens += usage.cached_input_tokens;
            self.output_tokens += usage.output_tokens;
        }
        if let Event::ContextCompactionFinished {
            usage: Some(usage), ..
        } = event
        {
            self.input_tokens += usage.input_tokens;
            self.cached_input_tokens += usage.cached_input_tokens;
            self.output_tokens += usage.output_tokens;
        }
        self.events.push(event.clone());
    }
}

#[derive(Debug)]
enum BudgetError {
    Exhausted,
    Provider(OpenAiError),
}

impl fmt::Display for BudgetError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Exhausted => formatter.write_str("real-llm request budget exhausted"),
            Self::Provider(error) => error.fmt(formatter),
        }
    }
}

impl Error for BudgetError {}

fn reserve_request(used: &Arc<AtomicUsize>, max_requests: usize) -> bool {
    used.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |used| {
        (used < max_requests).then_some(used + 1)
    })
    .is_ok()
}

struct BudgetedModel {
    inner: OpenAiModel,
    used: Arc<AtomicUsize>,
    max_requests: usize,
}

impl Model for BudgetedModel {
    type Error = BudgetError;

    fn respond<'a>(
        &'a mut self,
        request: ModelRequest<'a>,
        events: &'a mut (dyn ModelEventSink + Send),
    ) -> impl std::future::Future<Output = Result<ModelResponse, Self::Error>> + Send + 'a {
        let reserved = reserve_request(&self.used, self.max_requests);
        async move {
            if !reserved {
                return Err(BudgetError::Exhausted);
            }
            self.inner
                .respond(request, events)
                .await
                .map_err(BudgetError::Provider)
        }
    }
}

struct BudgetedUploader {
    inner: DeepSeekFiles,
    used: Arc<AtomicUsize>,
    max_requests: usize,
}

impl FileUploader for BudgetedUploader {
    fn upload(&self, filename: &str, media_type: &str, bytes: &[u8]) -> Result<String, ToolError> {
        if !reserve_request(&self.used, self.max_requests) {
            return Err(ToolError("real-llm request budget exhausted".to_string()));
        }
        self.inner.upload(filename, media_type, bytes)
    }
}

struct Lookup;

impl Tool for Lookup {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "lookup".to_string(),
            description: "Return the exact value for the supplied test key.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "key": {
                        "type": "string",
                        "enum": ["alpha"]
                    }
                },
                "required": ["key"],
                "additionalProperties": false
            }),
        }
    }

    fn execute(&self, arguments: &Value) -> Result<String, ToolError> {
        match arguments.get("key").and_then(Value::as_str) {
            Some("alpha") => Ok("ALPHA-42".to_string()),
            Some(key) => Err(ToolError(format!("unexpected key: {key}"))),
            None => Err(ToolError("key must be a string".to_string())),
        }
    }
}

struct ReadImageFixture {
    images: ImageStore,
    id: String,
}

impl Tool for ReadImageFixture {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "read_image".to_string(),
            description: "Load the fixed image fixture for this integration check.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "enum": ["fixture.png"]
                    }
                },
                "required": ["path"],
                "additionalProperties": false
            }),
        }
    }

    fn execute(&self, arguments: &Value) -> Result<String, ToolError> {
        if arguments.get("path").and_then(Value::as_str) != Some("fixture.png") {
            return Err(ToolError("path must be fixture.png".to_string()));
        }
        let stored = self
            .images
            .get(&self.id)
            .ok_or_else(|| ToolError("image fixture is no longer available".to_string()))?;
        Ok(image::format_envelope(&stored))
    }
}

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
    if matches!(raw_args.as_slice(), [arg] if matches!(arg.as_str(), "--help" | "-h")) {
        println!("{HELP}");
        return Ok(());
    }
    let args = parse_args(raw_args)?;
    if !args.allow_paid {
        return Err(
            "--allow-paid is required; this runner makes real provider requests and may incur charges"
                .into(),
        );
    }

    let environment = Environment::load(".env")?;
    let api_key = environment
        .resolve("OPENAI_API_KEY")
        .ok_or("OPENAI_API_KEY is required")?
        .value;
    let model_name = environment
        .resolve("OPENAI_MODEL")
        .ok_or("OPENAI_MODEL is required")?
        .value;
    let base_url = environment
        .resolve("OPENAI_BASE_URL")
        .map(|value| value.value)
        .unwrap_or_else(|| "https://api.openai.com/v1".to_string());
    let chat_base_url = environment
        .resolve("OPENAI_CHAT_BASE_URL")
        .map(|value| value.value);
    let required_requests = args
        .scenarios
        .iter()
        .map(|scenario| scenario.request_budget())
        .sum::<usize>();
    let max_requests = args.max_requests.unwrap_or(required_requests);
    if required_requests > max_requests {
        return Err(format!(
            "selected scenarios require {required_requests} requests, but --max-requests is {max_requests}"
        )
        .into());
    }

    let used = Arc::new(AtomicUsize::new(0));
    let mut output: Box<dyn Write> = match args.output {
        Some(path) => Box::new(BufWriter::new(
            OpenOptions::new().write(true).create_new(true).open(path)?,
        )),
        None => Box::new(BufWriter::new(io::stdout())),
    };
    let mut records = Vec::new();

    for scenario in &args.scenarios {
        let before_requests = used.load(Ordering::SeqCst);
        let mut record = match timeout(
            Duration::from_secs(args.timeout_seconds),
            run_scenario(
                *scenario,
                &api_key,
                &model_name,
                &base_url,
                chat_base_url.as_deref(),
                args.max_output_tokens,
                Arc::clone(&used),
                max_requests,
            ),
        )
        .await
        {
            Ok(record) => record,
            Err(_) => json!({
                "type": "scenario",
                "scenario": scenario.name(),
                "passed": false,
                "error": format!("scenario timed out after {} seconds", args.timeout_seconds),
                "requests_used": used.load(Ordering::SeqCst),
            }),
        };
        let requests_used = used.load(Ordering::SeqCst).saturating_sub(before_requests);
        if let Some(object) = record.as_object_mut() {
            object.insert("requests_used".to_string(), json!(requests_used));
        }
        write_json_line(&mut output, &record)?;
        records.push(record);
    }

    let passed = records
        .iter()
        .all(|record| record.get("passed").and_then(Value::as_bool) == Some(true));
    let summary = json!({
        "type": "summary",
        "scenarios": args.scenarios.iter().map(|scenario| scenario.name()).collect::<Vec<_>>(),
        "model": model_name,
        "max_requests": max_requests,
        "requests_used": used.load(Ordering::SeqCst),
        "max_output_tokens": args.max_output_tokens,
        "timeout_seconds": args.timeout_seconds,
        "passed": passed,
    });
    write_json_line(&mut output, &summary)?;
    output.flush()?;

    if passed {
        Ok(())
    } else {
        Err("one or more real-llm scenarios failed; inspect the JSONL evidence".into())
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_scenario(
    scenario: Scenario,
    api_key: &str,
    model_name: &str,
    base_url: &str,
    chat_base_url: Option<&str>,
    max_output_tokens: usize,
    used: Arc<AtomicUsize>,
    max_requests: usize,
) -> Value {
    match scenario {
        Scenario::Text => {
            run_text(
                api_key,
                model_name,
                base_url,
                max_output_tokens,
                used,
                max_requests,
            )
            .await
        }
        Scenario::Tool => {
            run_tool(
                api_key,
                model_name,
                base_url,
                max_output_tokens,
                used,
                max_requests,
            )
            .await
        }
        Scenario::Conversation => {
            run_conversation(
                api_key,
                model_name,
                base_url,
                max_output_tokens,
                used,
                max_requests,
            )
            .await
        }
        Scenario::Persistence => {
            #[cfg(not(test))]
            {
                run_persistence(
                    api_key,
                    model_name,
                    base_url,
                    max_output_tokens,
                    used,
                    max_requests,
                )
                .await
            }
            #[cfg(test)]
            {
                error_record(
                    "persistence",
                    "persistence scenario is only available outside cargo test".to_string(),
                )
            }
        }
        Scenario::Vision => {
            run_vision(
                api_key,
                model_name,
                base_url,
                chat_base_url,
                max_output_tokens,
                used,
                max_requests,
            )
            .await
        }
        Scenario::Compaction => {
            run_compaction(
                api_key,
                model_name,
                base_url,
                max_output_tokens,
                used,
                max_requests,
            )
            .await
        }
    }
}

fn model(
    api_key: &str,
    model_name: &str,
    base_url: &str,
    max_output_tokens: usize,
    used: Arc<AtomicUsize>,
    max_requests: usize,
) -> Result<BudgetedModel, String> {
    model_with_images(
        api_key,
        model_name,
        base_url,
        None,
        max_output_tokens,
        used,
        max_requests,
        ImageStore::memory_only(),
    )
}

#[allow(clippy::too_many_arguments)]
fn model_with_images(
    api_key: &str,
    model_name: &str,
    base_url: &str,
    chat_base_url: Option<&str>,
    max_output_tokens: usize,
    used: Arc<AtomicUsize>,
    max_requests: usize,
    images: ImageStore,
) -> Result<BudgetedModel, String> {
    let inner = OpenAiModel::new(
        api_key.to_string(),
        model_name.to_string(),
        base_url.to_string(),
        chat_base_url.map(str::to_string),
        false,
        images,
    )
    .map_err(|error| error.to_string())?
    .with_max_output_tokens(max_output_tokens);
    Ok(BudgetedModel {
        inner,
        used,
        max_requests,
    })
}

fn config(max_steps: usize) -> HarnessConfig {
    HarnessConfig {
        system_prompt:
            "You are a real-provider integration test. Follow the user instruction exactly and keep the answer short."
                .to_string(),
        max_steps,
        ..HarnessConfig::default()
    }
}

async fn run_text(
    api_key: &str,
    model_name: &str,
    base_url: &str,
    max_output_tokens: usize,
    used: Arc<AtomicUsize>,
    max_requests: usize,
) -> Value {
    let model = match model(
        api_key,
        model_name,
        base_url,
        max_output_tokens,
        used,
        max_requests,
    ) {
        Ok(model) => model,
        Err(error) => return error_record("text", error),
    };
    let mut harness = Harness::new(model, ToolRegistry::default(), config(1));
    let mut observer = EvalObserver::default();
    let result = harness
        .run(
            "Reply with exactly REAL-LLM-OK and no other words.",
            &mut observer,
        )
        .await;
    match result {
        Ok(outcome) => report(
            "text",
            outcome.stop_reason == StopReason::Completed
                && outcome.final_text.trim() == "REAL-LLM-OK",
            &observer,
            json!({"final_text": outcome.final_text, "steps": outcome.steps}),
        ),
        Err(error) => harness_error_record("text", error, &observer),
    }
}

async fn run_tool(
    api_key: &str,
    model_name: &str,
    base_url: &str,
    max_output_tokens: usize,
    used: Arc<AtomicUsize>,
    max_requests: usize,
) -> Value {
    let model = match model(
        api_key,
        model_name,
        base_url,
        max_output_tokens,
        used,
        max_requests,
    ) {
        Ok(model) => model,
        Err(error) => return error_record("tool", error),
    };
    let mut harness = Harness::new(model, ToolRegistry::new(vec![Box::new(Lookup)]), config(2));
    let mut observer = EvalObserver::default();
    let result = harness
        .run(
            "Call lookup exactly once with key alpha. Then reply with exactly the returned value and no other words.",
            &mut observer,
        )
        .await;
    let call = observer.events.iter().find_map(|event| match event {
        Event::ToolStarted { call } => Some(call),
        _ => None,
    });
    match result {
        Ok(outcome) => {
            let argument_ok = call
                .map(|call| {
                    call.name == "lookup"
                        && call.arguments.get("key").and_then(Value::as_str) == Some("alpha")
                })
                .unwrap_or(false);
            report(
                "tool",
                outcome.stop_reason == StopReason::Completed
                    && argument_ok
                    && outcome.final_text.trim() == "ALPHA-42",
                &observer,
                json!({
                    "final_text": outcome.final_text,
                    "steps": outcome.steps,
                    "tool_call": call.map(tool_call_value),
                }),
            )
        }
        Err(error) => harness_error_record("tool", error, &observer),
    }
}

async fn run_conversation(
    api_key: &str,
    model_name: &str,
    base_url: &str,
    max_output_tokens: usize,
    used: Arc<AtomicUsize>,
    max_requests: usize,
) -> Value {
    let model = match model(
        api_key,
        model_name,
        base_url,
        max_output_tokens,
        used,
        max_requests,
    ) {
        Ok(model) => model,
        Err(error) => return error_record("conversation", error),
    };
    let mut harness = Harness::new(model, ToolRegistry::default(), config(1));
    let mut observer = EvalObserver::default();
    let first = harness
        .run(
            "Remember the codeword exactly: CONTEXT-42. Reply only with ACK.",
            &mut observer,
        )
        .await;
    if let Err(error) = first {
        return harness_error_record("conversation", error, &observer);
    }
    let second = harness
        .run(
            "What exact codeword did I ask you to remember? Reply with only the codeword.",
            &mut observer,
        )
        .await;
    match second {
        Ok(outcome) => report(
            "conversation",
            outcome.stop_reason == StopReason::Completed
                && outcome.final_text.trim() == "CONTEXT-42",
            &observer,
            json!({"final_text": outcome.final_text, "steps": outcome.steps}),
        ),
        Err(error) => harness_error_record("conversation", error, &observer),
    }
}

#[cfg(not(test))]
async fn run_persistence(
    api_key: &str,
    model_name: &str,
    base_url: &str,
    max_output_tokens: usize,
    used: Arc<AtomicUsize>,
    max_requests: usize,
) -> Value {
    let first_model = match model(
        api_key,
        model_name,
        base_url,
        max_output_tokens,
        Arc::clone(&used),
        max_requests,
    ) {
        Ok(model) => model,
        Err(error) => return error_record("persistence", error),
    };
    let workspace = match env::current_dir() {
        Ok(workspace) => workspace,
        Err(error) => return error_record("persistence", error.to_string()),
    };
    let session_root = match session::session_directory(&workspace) {
        Ok(path) => path,
        Err(error) => return error_record("persistence", error),
    };
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let session_id = format!("real-llm-{timestamp}-{}", used.load(Ordering::SeqCst));
    let session_path = session_root.join(&session_id);
    let cleanup = || {
        let _ = fs::remove_dir_all(&session_path);
    };
    let mut opened = match session::SessionStore::open(
        &workspace,
        session::SessionRequest::Named(session_id.clone()),
    ) {
        Ok(opened) => opened,
        Err(error) => return error_record("persistence", error),
    };
    let first_prompt =
        "Remember the persisted codeword exactly: PERSIST-42. Reply exactly PERSIST-ACK.";
    let mut harness = Harness::new(first_model, ToolRegistry::default(), config(1));
    let mut observer = EvalObserver::default();
    let first = match harness.run(first_prompt, &mut observer).await {
        Ok(outcome) => outcome,
        Err(error) => {
            let record = harness_error_record("persistence", error, &observer);
            drop(opened);
            cleanup();
            return record;
        }
    };
    if first.stop_reason != StopReason::Completed {
        let record = report(
            "persistence",
            false,
            &observer,
            json!({
                "phase": "initial_persisted_turn",
                "final_text": first.final_text,
                "steps": first.steps,
            }),
        );
        drop(opened);
        cleanup();
        return record;
    }
    if let Err(error) = opened.store.record_turn(session::TurnCommit {
        started_at_ms: timestamp as u64,
        prompt: first_prompt,
        status: session::TurnStatus::Completed,
        steps: first.steps,
        error: None,
        messages: harness.messages(),
        checkpoint: harness.messages(),
    }) {
        drop(opened);
        cleanup();
        return error_record("persistence", error);
    }
    drop(opened);

    let mut resumed = match session::SessionStore::open(
        &workspace,
        session::SessionRequest::Resume(session_id.clone()),
    ) {
        Ok(opened) => opened,
        Err(error) => {
            cleanup();
            return error_record("persistence", error);
        }
    };
    let restored_messages = resumed.messages.len();
    let model = match model(
        api_key,
        model_name,
        base_url,
        max_output_tokens,
        used,
        max_requests,
    ) {
        Ok(model) => model,
        Err(error) => {
            drop(resumed);
            cleanup();
            return error_record("persistence", error);
        }
    };
    let mut resumed_harness = Harness::new(model, ToolRegistry::default(), config(1));
    if let Err(error) = resumed_harness.restore_history(std::mem::take(&mut resumed.messages)) {
        drop(resumed);
        cleanup();
        return error_record("persistence", error.to_string());
    }
    let second = resumed_harness
        .run(
            "What exact codeword was persisted in the previous process? Reply with only the codeword.",
            &mut observer,
        )
        .await;
    let record = match second {
        Ok(outcome) => report(
            "persistence",
            outcome.stop_reason == StopReason::Completed
                && outcome.final_text.trim() == "PERSIST-42"
                && restored_messages >= 2,
            &observer,
            json!({
                "session_id": session_id,
                "restored_messages": restored_messages,
                "final_text": outcome.final_text,
                "steps": outcome.steps,
            }),
        ),
        Err(error) => harness_error_record("persistence", error, &observer),
    };
    drop(resumed);
    cleanup();
    record
}

async fn run_vision(
    api_key: &str,
    model_name: &str,
    base_url: &str,
    chat_base_url: Option<&str>,
    max_output_tokens: usize,
    used: Arc<AtomicUsize>,
    max_requests: usize,
) -> Value {
    let uses_files = image::uses_deepseek_files(base_url);
    let images = if uses_files {
        ImageStore::with_uploader(Arc::new(BudgetedUploader {
            inner: DeepSeekFiles::new(api_key.to_string(), base_url),
            used: Arc::clone(&used),
            max_requests,
        }))
    } else {
        ImageStore::memory_only()
    };
    let stored = match images.save("fixture.png", "image/png", TEST_PNG.to_vec()) {
        Ok(stored) => stored,
        Err(error) => return error_record("vision", error.to_string()),
    };
    if uses_files && stored.file_id.is_none() {
        return error_record(
            "vision",
            "DeepSeek image fixture upload did not return a file_id".to_string(),
        );
    }
    let model = match model_with_images(
        api_key,
        model_name,
        base_url,
        chat_base_url,
        max_output_tokens,
        used,
        max_requests,
        images.clone(),
    ) {
        Ok(model) => model,
        Err(error) => return error_record("vision", error),
    };
    let tool = ReadImageFixture {
        images,
        id: stored.id,
    };
    let mut harness = Harness::new(model, ToolRegistry::new(vec![Box::new(tool)]), config(2));
    let mut observer = EvalObserver::default();
    let result = harness
        .run(
            "Call read_image exactly once with path fixture.png. Then reply exactly VISION-LLM-OK and no other words.",
            &mut observer,
        )
        .await;
    let call = observer.events.iter().find_map(|event| match event {
        Event::ToolStarted { call } => Some(call),
        _ => None,
    });
    match result {
        Ok(outcome) => {
            let argument_ok = call
                .map(|call| {
                    call.name == "read_image"
                        && call.arguments.get("path").and_then(Value::as_str) == Some("fixture.png")
                })
                .unwrap_or(false);
            report(
                "vision",
                outcome.stop_reason == StopReason::Completed
                    && argument_ok
                    && outcome.final_text.trim() == "VISION-LLM-OK",
                &observer,
                json!({
                    "final_text": outcome.final_text,
                    "steps": outcome.steps,
                    "image_transport": if uses_files { "files_api" } else { "inline" },
                    "tool_call": call.map(tool_call_value),
                }),
            )
        }
        Err(error) => harness_error_record("vision", error, &observer),
    }
}

async fn run_compaction(
    api_key: &str,
    model_name: &str,
    base_url: &str,
    max_output_tokens: usize,
    used: Arc<AtomicUsize>,
    max_requests: usize,
) -> Value {
    let model = match model(
        api_key,
        model_name,
        base_url,
        max_output_tokens,
        used,
        max_requests,
    ) {
        Ok(model) => model,
        Err(error) => return error_record("compaction", error),
    };
    let test_config = HarnessConfig {
        system_prompt: "You summarize short integration-test state.".to_string(),
        max_steps: 1,
        max_user_input_bytes: 512,
        max_model_response_bytes: 16 * 1024,
        max_context_bytes: 700,
        context_limit_behavior: ContextLimitBehavior::Compact,
        ..HarnessConfig::default()
    };
    let mut harness = Harness::new(model, ToolRegistry::default(), test_config);
    let history = vec![
        Message::User {
            text: "checkpoint one: owner=agent; status=ready; checksum=42.".to_string(),
        },
        Message::Assistant {
            reasoning: String::new(),
            text: "acknowledged checkpoint one".to_string(),
            tool_calls: Vec::new(),
        },
        Message::User {
            text: "checkpoint two: milestone=blue; queue=clear.".to_string(),
        },
        Message::Assistant {
            reasoning: String::new(),
            text: "acknowledged checkpoint two".to_string(),
            tool_calls: Vec::new(),
        },
        Message::User {
            text: "checkpoint three: next=verify; checksum=42.".to_string(),
        },
        Message::Assistant {
            reasoning: String::new(),
            text: "acknowledged checkpoint three".to_string(),
            tool_calls: Vec::new(),
        },
    ];
    if let Err(error) = harness.restore_history(history) {
        return error_record("compaction", error.to_string());
    }
    let mut observer = EvalObserver::default();
    let result = harness
        .run(
            "Continue from the recorded state. Reply exactly COMPACTION-OK.",
            &mut observer,
        )
        .await;
    match result {
        Ok(outcome) => report(
            "compaction",
            outcome.stop_reason == StopReason::Completed
                && outcome.final_text.trim() == "COMPACTION-OK"
                && observer.compactions() > 0,
            &observer,
            json!({
                "final_text": outcome.final_text,
                "steps": outcome.steps,
            "compactions": observer.compactions(),
            }),
        ),
        Err(error) => harness_error_record("compaction", error, &observer),
    }
}

fn tool_call_value(call: &ToolCall) -> Value {
    json!({"name": call.name, "arguments": call.arguments})
}

fn report(scenario: &str, passed: bool, observer: &EvalObserver, details: Value) -> Value {
    json!({
        "type": "scenario",
        "scenario": scenario,
        "passed": passed,
        "model_steps": observer.requests(),
        "compactions": observer.compactions(),
        "usage": observer.usage_value(),
        "details": details,
    })
}

fn error_record(scenario: &str, error: String) -> Value {
    json!({
        "type": "scenario",
        "scenario": scenario,
        "passed": false,
        "model_steps": 0,
        "error": error,
    })
}

fn harness_error_record<E: fmt::Display>(
    scenario: &str,
    error: HarnessError<E>,
    observer: &EvalObserver,
) -> Value {
    report(
        scenario,
        false,
        observer,
        json!({"error": error.to_string()}),
    )
}

fn write_json_line(output: &mut dyn Write, value: &Value) -> Result<(), Box<dyn Error>> {
    serde_json::to_writer(&mut *output, value)?;
    writeln!(output)?;
    output.flush()?;
    Ok(())
}

fn parse_args(raw: Vec<String>) -> Result<Args, String> {
    let mut allow_paid = false;
    let mut scenarios = Vec::new();
    let mut max_requests = None;
    let mut max_output_tokens = DEFAULT_MAX_OUTPUT_TOKENS;
    let mut timeout_seconds = DEFAULT_TIMEOUT_SECONDS;
    let mut output = None;
    let mut arguments = raw.into_iter();

    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--allow-paid" => allow_paid = true,
            "--scenario" => {
                let value = arguments
                    .next()
                    .ok_or_else(|| "--scenario requires a value".to_string())?;
                for name in value
                    .split(',')
                    .map(str::trim)
                    .filter(|name| !name.is_empty())
                {
                    if name == "all" {
                        scenarios = vec![
                            Scenario::Text,
                            Scenario::Tool,
                            Scenario::Conversation,
                            Scenario::Persistence,
                            Scenario::Vision,
                            Scenario::Compaction,
                        ];
                        break;
                    }
                    let scenario = Scenario::parse(name)?;
                    if !scenarios.contains(&scenario) {
                        scenarios.push(scenario);
                    }
                }
            }
            "--max-requests" => {
                let value = arguments
                    .next()
                    .ok_or_else(|| "--max-requests requires a number".to_string())?;
                let parsed = value
                    .parse::<usize>()
                    .map_err(|_| "--max-requests must be an integer".to_string())?;
                if !(1..=MAX_REQUESTS).contains(&parsed) {
                    return Err(format!(
                        "--max-requests must be between 1 and {MAX_REQUESTS}"
                    ));
                }
                max_requests = Some(parsed);
            }
            "--max-output-tokens" => {
                let value = arguments
                    .next()
                    .ok_or_else(|| "--max-output-tokens requires a number".to_string())?;
                let parsed = value
                    .parse::<usize>()
                    .map_err(|_| "--max-output-tokens must be an integer".to_string())?;
                if !(16..=MAX_OUTPUT_TOKENS).contains(&parsed) {
                    return Err(format!(
                        "--max-output-tokens must be between 16 and {MAX_OUTPUT_TOKENS}"
                    ));
                }
                max_output_tokens = parsed;
            }
            "--timeout-seconds" => {
                let value = arguments
                    .next()
                    .ok_or_else(|| "--timeout-seconds requires a number".to_string())?;
                let parsed = value
                    .parse::<u64>()
                    .map_err(|_| "--timeout-seconds must be an integer".to_string())?;
                if !(5..=DEFAULT_TIMEOUT_SECONDS).contains(&parsed) {
                    return Err(format!(
                        "--timeout-seconds must be between 5 and {DEFAULT_TIMEOUT_SECONDS}"
                    ));
                }
                timeout_seconds = parsed;
            }
            "--output" => {
                if output.is_some() {
                    return Err("--output may be provided only once".to_string());
                }
                output = Some(PathBuf::from(
                    arguments
                        .next()
                        .ok_or_else(|| "--output requires a path".to_string())?,
                ));
            }
            other => return Err(format!("unknown option: {other}")),
        }
    }

    if scenarios.is_empty() {
        scenarios.push(Scenario::Text);
    }
    Ok(Args {
        allow_paid,
        scenarios,
        max_requests,
        max_output_tokens,
        timeout_seconds,
        output,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_one_text_request() {
        let args = parse_args(Vec::new()).unwrap();
        assert_eq!(args.scenarios, vec![Scenario::Text]);
        assert_eq!(args.max_requests, None);
        assert_eq!(args.max_output_tokens, DEFAULT_MAX_OUTPUT_TOKENS);
    }

    #[test]
    fn all_scenarios_have_a_bounded_budget() {
        let args = parse_args(vec![
            "--scenario".to_string(),
            "all".to_string(),
            "--max-requests".to_string(),
            "12".to_string(),
            "--max-output-tokens".to_string(),
            "64".to_string(),
        ])
        .unwrap();
        assert_eq!(args.scenarios.len(), 6);
        assert_eq!(
            args.scenarios
                .iter()
                .map(|scenario| scenario.request_budget())
                .sum::<usize>(),
            12
        );
    }

    #[test]
    fn paid_acknowledgement_is_separate_from_argument_parsing() {
        assert!(!parse_args(Vec::new()).unwrap().allow_paid);
        assert!(
            parse_args(vec!["--allow-paid".to_string()])
                .unwrap()
                .allow_paid
        );
    }
}
