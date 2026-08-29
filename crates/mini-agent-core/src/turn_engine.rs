use mini_agent_protocol::Event;
use mini_agent_protocol::ModelEvent;
use mini_agent_protocol::ModelEventSink;
use mini_agent_protocol::ModelResponse;
use mini_agent_protocol::Observer;

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
}

impl<O: Observer> ModelEventSink for ModelEventForwarder<'_, O> {
    fn emit(&mut self, event: ModelEvent) {
        match event {
            ModelEvent::ReasoningDelta(delta) => {
                self.emitted_bytes = self.emitted_bytes.saturating_add(delta.len());
                if self.emitted_bytes <= self.max_bytes {
                    self.observer
                        .observe(&Event::AssistantReasoningDelta { delta });
                }
            }
            ModelEvent::TextDelta(delta) => {
                self.emitted_bytes = self.emitted_bytes.saturating_add(delta.len());
                if self.emitted_bytes <= self.max_bytes {
                    self.observer.observe(&Event::AssistantTextDelta { delta });
                }
            }
        }
    }
}
