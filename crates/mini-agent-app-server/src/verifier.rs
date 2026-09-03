//! Goal verifier turn orchestration through the App Server boundary.

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

const MAX_VERIFIER_HISTORY_MESSAGES: usize = 24;

fn verify_system_prompt() -> &'static str {
    include_str!("../builtin/prompts/system/verifier.md").trim_end()
}

/// Keeps verifier input bounded while preserving the newest settled history.
/// Core still applies its byte-level context limits when the isolated turn is
/// run; this window prevents a long Goal from paying for all prior turns.
fn bounded_verifier_history(messages: &[Message]) -> Vec<Message> {
    messages
        .iter()
        .skip(messages.len().saturating_sub(MAX_VERIFIER_HISTORY_MESSAGES))
        .cloned()
        .collect()
}

struct DiscardEvents;

impl EventSink for DiscardEvents {
    fn emit(&mut self, _event: EventEnvelope) {}
}

/// Runs one isolated Goal verifier turn against a settled checkpoint.
pub async fn verify_goal_checkpoint(
    runtime_config: &RuntimeConfig,
    messages: &[Message],
    criteria: &str,
) -> Result<(String, crate::goal_service::VerifierVerdict), String> {
    let provider = runtime_config.verifier_provider_settings()?;
    let model = OpenAiModel::new(
        provider.api_key,
        provider.model,
        provider.base_url,
        false,
        ImageStore::memory_only(),
    )
    .map_err(|error| error.to_string())?;
    let config = HarnessConfig {
        system_prompt: verify_system_prompt().to_string(),
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
        .restore_history(bounded_verifier_history(messages))
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
    let verdict = crate::goal_service::parse_verifier_verdict(&final_text);
    Ok((final_text, verdict))
}

#[cfg(test)]
#[path = "verifier_tests.rs"]
mod tests;

async fn run_harness_turn<M, S>(
    harness: Harness<M>,
    prompt: String,
    sink: &mut S,
) -> Result<TurnReadResult, String>
where
    M: mini_agent_protocol::Model + Send + 'static,
    S: EventSink + Send,
{
    let thread_id = ThreadId::new("goal-verifier");
    let server = AppServer::new(
        ThreadStart::new(thread_id.clone()),
        Thread::new(thread_id.clone(), harness),
    );
    let mut client = LocalAppServerClient::new(AppServerConnection::new(server));
    client
        .initialize("mini-agent-goal-verifier", env!("CARGO_PKG_VERSION"))
        .await
        .map_err(|error| error.message)?;
    let submission = client
        .start_turn(thread_id, TurnInput::new(TurnInputMode::Start, prompt))
        .await
        .map_err(|error| error.message)?;
    let turn_id = match submission {
        mini_agent_protocol::TurnSubmission::Started { turn_id } => turn_id,
        other => return Err(format!("goal verifier turn was not started: {other:?}")),
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
