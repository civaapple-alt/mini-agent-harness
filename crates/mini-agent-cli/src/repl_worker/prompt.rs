use super::*;

pub(super) fn run_prompt(
    prompt: String,
    run_control: &RunControl,
    runtime: &mut AppServerRuntime,
    model_runtime: &tokio::runtime::Runtime,
    events: &mpsc::SyncSender<ReplEvent>,
) {
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
        send_event(
            events,
            ReplEvent::Warning("error: app server returned an empty turn batch".to_string()),
        );
        return;
    };
    let steered = batch
        .turns
        .iter()
        .any(|turn| turn.stop_reason == Some(StopReason::Steered));
    match (steered, outcome.status == TurnStatus::StepLimit) {
        (true, _) => {
            send_event(
                events,
                ReplEvent::Notice(format!(
                    "steer> checkpoint saved after {} model step(s); continuing with the new message",
                    outcome.steps
                )),
            );
        }
        (_, true) => {
            send_event(
                events,
                ReplEvent::Warning(
                    "warning: runtime protection triggered; inspect the settled result or retry"
                        .to_string(),
                ),
            );
        }
        _ => {}
    }
}
