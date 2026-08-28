use super::SessionStore;
use super::new_id;
use super::timestamp_ms;
use serde_json::json;

pub struct DerivedItem<'a> {
    pub item_kind: &'a str,
    pub provider: &'a str,
    pub model: &'a str,
    pub source_checkpoint_seq: u64,
    pub source_fingerprint: &'a str,
    pub criteria: Option<&'a str>,
    pub output: &'a str,
}

impl SessionStore {
    pub fn checkpoint_seq(&self) -> u64 {
        self.checkpoint_seq
    }

    pub fn record_derived(&mut self, item: DerivedItem<'_>) -> Result<(), String> {
        self.append_records(vec![json!({
            "kind": "derived_item",
            "item_id": new_id("i"),
            "item_kind": item.item_kind,
            "thread_id": self.thread_id,
            "timestamp_ms": timestamp_ms(),
            "source": {
                "session_id": self.session_id,
                "thread_id": self.thread_id,
                "checkpoint_seq": item.source_checkpoint_seq,
                "fingerprint": item.source_fingerprint,
            },
            "producer": {
                "role": "mentor",
                "provider": item.provider,
                "model": item.model,
            },
            "criteria": item.criteria,
            "output": item.output,
        })])
    }
}
