use super::*;

pub(super) struct PromptContext<'a> {
    pub(super) prompt: String,
    pub(super) run_control: &'a RunControl,
    pub(super) runtime: &'a mut AppServerRuntime,
    pub(super) model_runtime: &'a tokio::runtime::Runtime,
    pub(super) events: &'a mpsc::SyncSender<ReplEvent>,
}

pub(super) fn run_prompt(context: PromptContext<'_>) {
    let PromptContext {
        prompt,
        run_control,
        runtime,
        model_runtime,
        events,
    } = context;
    run_control.clear_steer();
    let mut observer = ChannelObserver(events.clone());
    let batch =
        match model_runtime.block_on(runtime.client_mut().run_turn_batch(prompt, &mut observer)) {
            Ok(batch) => batch,
            Err(error) => {
                report_run_error(events, &error);
                return;
            }
        };
    let Some(outcome) = batch.turns.last() else {
        let _ = events.send(ReplEvent::Warning(
            "error: app server returned an empty turn batch".to_string(),
        ));
        return;
    };
    let steered = batch
        .turns
        .iter()
        .any(|turn| turn.stop_reason == Some(StopReason::Steered));
    match (steered, outcome.status == TurnStatus::StepLimit) {
        (true, _) => {
            let _ = events.send(ReplEvent::Notice(format!(
                "steer> checkpoint saved after {} model step(s); continuing with the new message",
                outcome.steps
            )));
        }
        (_, true) => {
            let _ = events.send(ReplEvent::Warning(format!(
                "warning: stopped after {} model steps",
                outcome.steps
            )));
        }
        _ => {}
    }
}
