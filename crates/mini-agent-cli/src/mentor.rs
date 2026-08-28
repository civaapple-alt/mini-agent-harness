use crate::config::RuntimeConfig;
use crate::observer::RunObserver;
use crate::observer::ScriptFormat;
use crate::observer::print_final_answer;
use crate::openai::OpenAiModel;
use crate::session::DerivedItem;
use crate::session::SessionRequest;
use crate::session::SessionStore;
use mini_agent_core::ContextLimitBehavior;
use mini_agent_core::Harness;
use mini_agent_core::HarnessConfig;
use mini_agent_core::Message;
use mini_agent_core::StopReason;
use mini_agent_core::ToolRegistry;
use serde_json::json;
use std::path::PathBuf;
use std::process::ExitCode;

const MAX_CRITERIA_BYTES: usize = 32 * 1024;
const INSIGHT_SYSTEM_PROMPT: &str = "You are an independent mentor reviewing a settled coding-agent session. Analyze only the supplied session evidence. Identify important patterns, missed opportunities, incorrect assumptions, risks, and the highest-value next actions. Distinguish observations from inferences. Do not claim to have run tools or inspected anything outside the session. Answer in the language used by the user unless the session clearly requests another language.";
const VERIFY_SYSTEM_PROMPT: &str = "You are an independent verifier reviewing a settled coding-agent session against explicit criteria. Use only the supplied session evidence. For every criterion, state pass, fail, or insufficient evidence and cite concrete session evidence. Do not treat claims of completion as proof when verification evidence is absent. End with an overall verdict and unresolved checks. Do not claim to have run tools or inspected anything outside the session. Answer in the language used by the user unless the criteria request another language.";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    Insight,
    Verify,
}

#[derive(Debug)]
struct Request {
    action: Action,
    session_id: String,
    criteria: Option<String>,
}

pub(crate) async fn run(arguments: String, trace: Option<PathBuf>, json_output: bool) -> ExitCode {
    let request = match parse_request(&arguments) {
        Ok(request) => request,
        Err(error) => return preflight_error(json_output, &error),
    };
    let runtime_config = match RuntimeConfig::load() {
        Ok(config) => config,
        Err(error) => return preflight_error(json_output, &error),
    };
    let provider = match runtime_config.mentor_provider_settings() {
        Ok(provider) => provider,
        Err(error) => return preflight_error(json_output, &error),
    };
    let mut opened = match SessionStore::open(
        &runtime_config.workspace(),
        SessionRequest::Resume(request.session_id.clone()),
    ) {
        Ok(opened) => opened,
        Err(error) => return preflight_error(json_output, &error),
    };
    let source_checkpoint_seq = opened.store.checkpoint_seq();
    let source_fingerprint = match fingerprint(&opened.messages) {
        Ok(fingerprint) => fingerprint,
        Err(error) => return preflight_error(json_output, &error),
    };
    let model = match OpenAiModel::new(
        provider.api_key,
        provider.model.clone(),
        provider.base_url,
        provider.chat_base_url,
        provider.web_search,
        crate::image::ImageStore::memory_only(),
    ) {
        Ok(model) => model,
        Err(error) => return preflight_error(json_output, &error.to_string()),
    };
    let config = HarnessConfig {
        system_prompt: request.action.system_prompt().to_string(),
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
    if let Err(error) = harness.restore_history(std::mem::take(&mut opened.messages)) {
        return preflight_error(
            json_output,
            &format!("cannot restore mentor source: {error}"),
        );
    }
    harness.replace_config(config);
    let format = if json_output {
        ScriptFormat::Json
    } else {
        ScriptFormat::Text
    };
    let mut observer = match RunObserver::for_script(trace, format) {
        Ok(observer) => observer,
        Err(error) => {
            return preflight_error(json_output, &format!("cannot create trace: {error}"));
        }
    };
    let prompt = request.analysis_prompt();
    let outcome = match harness.run(prompt, &mut observer).await {
        Ok(outcome) if outcome.stop_reason == StopReason::Completed => outcome,
        Ok(outcome) => {
            observer.finish();
            return run_error(
                json_output,
                &provider.model,
                &format!(
                    "mentor stopped after {} model steps without completing",
                    outcome.steps
                ),
                &observer,
            );
        }
        Err(error) => {
            observer.finish();
            return run_error(json_output, &provider.model, &error.to_string(), &observer);
        }
    };
    observer.finish();
    if let Err(error) = opened.store.record_derived(DerivedItem {
        item_kind: request.action.item_kind(),
        provider: "openai_responses",
        model: &provider.model,
        source_checkpoint_seq,
        source_fingerprint: &source_fingerprint,
        criteria: request.criteria.as_deref(),
        output: &outcome.final_text,
    }) {
        return run_error(json_output, &provider.model, &error, &observer);
    }

    if json_output {
        println!(
            "{}",
            json!({
                "output": outcome.final_text,
                "exit_code": 0,
                "role": "mentor",
                "action": request.action.name(),
                "model": provider.model,
                "session_id": opened.store.session_id(),
                "thread_id": opened.store.thread_id(),
                "source_checkpoint_seq": source_checkpoint_seq,
                "source_fingerprint": source_fingerprint,
                "usage": observer.stats_json(),
                "tool_calls": observer.tool_calls_json(),
            })
        );
    } else if !observer.assistant_displayed() {
        print_final_answer(&outcome.final_text);
    }
    ExitCode::SUCCESS
}

pub(crate) async fn verify_checkpoint(
    runtime_config: &RuntimeConfig,
    messages: &[Message],
    criteria: &str,
) -> Result<(String, crate::goal::VerifierVerdict), String> {
    let provider = runtime_config.mentor_provider_settings()?;
    let model = OpenAiModel::new(
        provider.api_key,
        provider.model,
        provider.base_url,
        provider.chat_base_url,
        false,
        crate::image::ImageStore::memory_only(),
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
    let outcome = harness
        .run(prompt, &mut ())
        .await
        .map_err(|error| format!("goal verifier failed: {error}"))?;
    if outcome.stop_reason != StopReason::Completed {
        return Err(format!(
            "goal verifier stopped after {} model steps without completing",
            outcome.steps
        ));
    }
    let verdict = crate::goal::parse_verifier_verdict(&outcome.final_text);
    Ok((outcome.final_text, verdict))
}

impl Request {
    fn analysis_prompt(&self) -> String {
        match &self.criteria {
            Some(criteria) => {
                format!("Verify the settled session above against these criteria:\n\n{criteria}")
            }
            None => "Produce an insight review of the settled session above.".to_string(),
        }
    }
}

impl Action {
    fn name(self) -> &'static str {
        match self {
            Self::Insight => "insight",
            Self::Verify => "verify",
        }
    }

    fn item_kind(self) -> &'static str {
        match self {
            Self::Insight => "mentor_insight",
            Self::Verify => "mentor_verification",
        }
    }

    fn system_prompt(self) -> &'static str {
        match self {
            Self::Insight => INSIGHT_SYSTEM_PROMPT,
            Self::Verify => VERIFY_SYSTEM_PROMPT,
        }
    }
}

fn parse_request(arguments: &str) -> Result<Request, String> {
    let mut parts = arguments.splitn(3, char::is_whitespace);
    let action = match parts.next().filter(|value| !value.is_empty()) {
        Some("insight") => Action::Insight,
        Some("verify") => Action::Verify,
        Some(other) => return Err(format!("unknown mentor action: {other}")),
        None => return Err("mentor requires `insight` or `verify`".to_string()),
    };
    let session_id = parts
        .next()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("mentor {} requires SESSION_ID", action.name()))?
        .to_string();
    let criteria = parts
        .next()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    match action {
        Action::Insight if criteria.is_some() => {
            Err("mentor insight does not accept criteria".to_string())
        }
        Action::Verify if criteria.is_none() => Err("mentor verify requires CRITERIA".to_string()),
        Action::Verify
            if criteria
                .as_ref()
                .is_some_and(|criteria| criteria.len() > MAX_CRITERIA_BYTES) =>
        {
            Err(format!(
                "mentor criteria exceeds {MAX_CRITERIA_BYTES} byte limit"
            ))
        }
        Action::Insight | Action::Verify => Ok(Request {
            action,
            session_id,
            criteria,
        }),
    }
}

fn fingerprint(messages: &[Message]) -> Result<String, String> {
    let encoded = serde_json::to_vec(messages)
        .map_err(|error| format!("cannot fingerprint mentor source: {error}"))?;
    let hash = encoded.iter().fold(0xcbf29ce484222325_u64, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
    });
    Ok(format!("fnv1a64:{hash:016x}"))
}

fn preflight_error(json_output: bool, error: &str) -> ExitCode {
    eprintln!("error: {error}");
    if json_output {
        println!(
            "{}",
            json!({
                "output": "",
                "exit_code": 2,
                "role": "mentor",
                "error": error,
            })
        );
    }
    ExitCode::from(2)
}

fn run_error(json_output: bool, model: &str, error: &str, observer: &RunObserver) -> ExitCode {
    eprintln!("error: {error}");
    if json_output {
        println!(
            "{}",
            json!({
                "output": "",
                "exit_code": 1,
                "role": "mentor",
                "model": model,
                "usage": observer.stats_json(),
                "tool_calls": observer.tool_calls_json(),
                "error": error,
            })
        );
    }
    ExitCode::FAILURE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_insight_and_verify_requests() {
        let insight = parse_request("insight s-123").unwrap();
        let verify = parse_request("verify s-123 tests pass and diff is clean").unwrap();

        assert_eq!(insight.action, Action::Insight);
        assert_eq!(insight.session_id, "s-123");
        assert_eq!(insight.criteria, None);
        assert_eq!(verify.action, Action::Verify);
        assert_eq!(verify.session_id, "s-123");
        assert_eq!(
            verify.criteria.as_deref(),
            Some("tests pass and diff is clean")
        );
    }

    #[test]
    fn rejects_missing_or_mismatched_arguments() {
        assert_eq!(
            parse_request("verify s-123").unwrap_err(),
            "mentor verify requires CRITERIA"
        );
        assert_eq!(
            parse_request("insight s-123 extra").unwrap_err(),
            "mentor insight does not accept criteria"
        );
        assert_eq!(
            parse_request("review s-123").unwrap_err(),
            "unknown mentor action: review"
        );
    }

    #[test]
    fn fingerprints_exact_message_content() {
        let first = fingerprint(&[Message::User {
            text: "one".to_string(),
        }])
        .unwrap();
        let same = fingerprint(&[Message::User {
            text: "one".to_string(),
        }])
        .unwrap();
        let second = fingerprint(&[Message::User {
            text: "two".to_string(),
        }])
        .unwrap();

        assert_eq!(first, same);
        assert_ne!(first, second);
        assert!(first.starts_with("fnv1a64:"));
    }
}
