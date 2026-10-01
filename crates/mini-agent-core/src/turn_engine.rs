use mini_agent_protocol::Event;
use mini_agent_protocol::ModelEvent;
use mini_agent_protocol::ModelEventSink;
use mini_agent_protocol::ModelResponse;
use mini_agent_protocol::ModelTiming;
use mini_agent_protocol::Observer;
use std::time::Instant;

pub(super) fn model_response_bytes(response: &ModelResponse) -> usize {
    response.reasoning.len()
        + response.text.len()
        + serde_json::to_vec(&response.tool_calls)
            .expect("tool calls must serialize")
            .len()
}

pub(super) struct SilentModelEvents;

impl ModelEventSink for SilentModelEvents {
    fn emit(&mut self, _event: ModelEvent) {}
}

pub(super) struct ModelEventForwarder<'a, O> {
    pub(super) observer: &'a mut O,
    pub(super) emitted_bytes: usize,
    pub(super) max_bytes: usize,
    started_at: Instant,
    first_output_ms: Option<u64>,
}

impl<'a, O> ModelEventForwarder<'a, O> {
    pub(super) fn new(observer: &'a mut O, max_bytes: usize) -> Self {
        Self {
            observer,
            emitted_bytes: 0,
            max_bytes,
            started_at: Instant::now(),
            first_output_ms: None,
        }
    }

    pub(super) fn timing(&self) -> ModelTiming {
        ModelTiming {
            ttft_ms: self.first_output_ms,
            response_ms: elapsed_ms(self.started_at.elapsed()),
        }
    }
}

impl<O: Observer> ModelEventSink for ModelEventForwarder<'_, O> {
    fn emit(&mut self, event: ModelEvent) {
        match event {
            ModelEvent::ReasoningDelta(delta) => {
                self.record_first_output(&delta);
                self.emitted_bytes = self.emitted_bytes.saturating_add(delta.len());
                if self.emitted_bytes <= self.max_bytes {
                    self.observer
                        .observe(&Event::AssistantReasoningDelta { delta });
                }
            }
            ModelEvent::TextDelta(delta) => {
                self.record_first_output(&delta);
                self.emitted_bytes = self.emitted_bytes.saturating_add(delta.len());
                if self.emitted_bytes <= self.max_bytes {
                    self.observer.observe(&Event::AssistantTextDelta { delta });
                }
            }
        }
    }
}

impl<O> ModelEventForwarder<'_, O> {
    fn record_first_output(&mut self, delta: &str) {
        if !delta.is_empty() && self.first_output_ms.is_none() {
            self.first_output_ms = Some(elapsed_ms(self.started_at.elapsed()));
        }
    }
}

fn elapsed_ms(duration: std::time::Duration) -> u64 {
    duration.as_millis().min(u64::MAX as u128) as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use mini_agent_protocol::ModelEvent;

    struct RecordingObserver;

    impl Observer for RecordingObserver {
        fn observe(&mut self, _event: &Event) {}
    }

    #[test]
    fn timing_ignores_empty_deltas_and_measures_first_nonempty_output() {
        let mut observer = RecordingObserver;
        let mut forwarder = ModelEventForwarder::new(&mut observer, 1024);
        forwarder.emit(ModelEvent::ReasoningDelta(String::new()));
        assert_eq!(forwarder.timing().ttft_ms, None);

        forwarder.emit(ModelEvent::TextDelta("answer".to_string()));
        let timing = forwarder.timing();
        assert!(timing.ttft_ms.is_some());
        assert!(timing.response_ms >= timing.ttft_ms.unwrap());
    }
}
