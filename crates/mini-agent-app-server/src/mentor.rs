use crate::AppServer;
use crate::AppServerConnection;
use crate::LocalAppServerClient;
use mini_agent_app_server_protocol::TurnReadResult;
use mini_agent_capabilities::ImageStore;
use mini_agent_capabilities::OpenAiModel;
use mini_agent_core::ContextLimitBehavior;
use mini_agent_core::Harness;
use mini_agent_core::HarnessConfig;
use mini_agent_core::Thread;
use mini_agent_core::ToolRegistry;
use mini_agent_host::config::RuntimeConfig;
use mini_agent_protocol::Event;
use mini_agent_protocol::EventEnvelope;
use mini_agent_protocol::EventSink;
use mini_agent_protocol::Message;
use mini_agent_protocol::StopReason;
use mini_agent_protocol::ThreadId;
use mini_agent_protocol::ThreadStart;
use mini_agent_protocol::TurnInput;
use mini_agent_protocol::TurnInputMode;

const VERIFY_SYSTEM_PROMPT: &str = "You are an independent verifier reviewing a settled coding-agent session against explicit criteria. Use only the supplied session evidence. For every criterion, state pass, fail, or insufficient evidence and cite concrete session evidence. Do not treat claims of completion as proof when verification evidence is absent. End with an overall verdict and unresolved checks. Do not claim to have run tools or inspected anything outside the session. Answer in the language used by the user unless the criteria request another language.";

struct DiscardEvents;

impl EventSink for DiscardEvents {
    fn emit(&mut self, _event: EventEnvelope) {}
}

pub async fn verify_checkpoint(
    runtime_config: &RuntimeConfig,
    messages: &[Message],
    criteria: &str,
) -> Result<(String, crate::workflows::VerifierVerdict), String> {
    let provider = runtime_config.mentor_provider_settings()?;
    let model = OpenAiModel::new(
        provider.api_key,
        provider.model,
        provider.base_url,
        false,
        ImageStore::memory_only(),
    )
    .map_err(|error| error.to_string())?;
    let config = HarnessConfig {
        system_prompt: VERIFY_SYSTEM_PROMPT.to_string(),
        max_steps: 1,
        max_tool_calls_per_step: 0,
        context_limit_behavior: ContextLimitBehavior::Reject,
        ..HarnessConfig::default()
    };
    let mut harness = Harness::new(
        model,
        ToolRegistry::new(Vec::new()),
        HarnessConfig::default(),
    );
    harness
        .restore_history(messages.to_vec())
        .map_err(|error| format!("cannot restore goal verifier source: {error}"))?;
    harness.replace_config(config);
    let prompt = format!(
        "Verify the settled goal milestone against the following acceptance plan.\n\n{criteria}"
    );
    let mut sink = DiscardEvents;
    let outcome = run_harness_turn(harness, prompt, &mut sink)
        .await
        .map_err(|error| format!("goal verifier failed: {error}"))?;
    if outcome.stop_reason != Some(StopReason::Completed) {
        return Err(format!(
            "goal verifier stopped after {} model steps without completing",
            outcome.steps
        ));
    }
    let final_text = outcome.final_text.unwrap_or_default();
    let verdict = crate::workflows::parse_verifier_verdict(&final_text);
    Ok((final_text, verdict))
}

async fn run_harness_turn<M, S>(
    harness: Harness<M>,
    prompt: String,
    sink: &mut S,
) -> Result<TurnReadResult, String>
where
    M: mini_agent_protocol::Model + Send + 'static,
    S: EventSink + Send,
{
    let thread_id = ThreadId::new("mentor");
    let server = AppServer::new(
        ThreadStart::new(thread_id.clone()),
        Thread::new(thread_id.clone(), harness),
    );
    let mut client = LocalAppServerClient::new(AppServerConnection::new(server));
    client
        .initialize("mini-agent-mentor", env!("CARGO_PKG_VERSION"))
        .await
        .map_err(|error| error.message)?;
    let submission = client
        .start_turn(thread_id, TurnInput::new(TurnInputMode::Start, prompt))
        .await
        .map_err(|error| error.message)?;
    let turn_id = match submission {
        mini_agent_protocol::TurnSubmission::Started { turn_id } => turn_id,
        other => return Err(format!("mentor turn was not started: {other:?}")),
    };
    loop {
        let event = client.next_event().await.map_err(|error| error.message)?;
        let finished = event.turn_id.as_ref() == Some(&turn_id)
            && matches!(event.event, Event::TurnFinished { .. });
        sink.emit(event);
        if finished {
            break;
        }
    }
    client
        .read_turn(turn_id)
        .await
        .map_err(|error| error.message)
}
