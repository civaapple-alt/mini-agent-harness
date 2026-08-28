# Session as the Single Durable Runtime Store

Status: implemented

## Decision

`~/.mini-agent/sessions/<workspace>/<session-id>/session.jsonl` is the only durable runtime record. The append-only log stores session and thread lifecycle, context checkpoints, settled turns, derived mentor items, and `result_stored` records for large tool outputs.

`SessionStore` owns the append position and a shared append lock. `ResultStore` can bind to that store, reloads result handles from existing records on resume, and appends new handles under the same lock and sequence space. Persisted result content is bounded to 64 KiB so JSON escaping stays below the session record limit while retaining source byte and truncation metadata.

Interactive, `ask`, and `auto` CLI sessions always open a durable session. Persistence flags and the App Server `ephemeral` start option were removed. Internal test/demo paths may still use an unbound in-memory result store where no session is requested.

Live event envelopes remain an in-process/App Server stream for rendering and ACP/JSON-RPC notifications. They are not duplicated into an external trace file. The previous detailed trace replay implementation was prompt-weight-specific and is rejected as a mainline dependency.

## Verification

- `cargo check --workspace --all-targets` passed.
- `cargo test -p mini-agent-cli --test interactive -- --test-threads=1` passed: 32 tests.
- `cargo test -p mini-agent-host --lib result_store::tests::session_result_store_reloads_from_append_log -- --nocapture` passed.
- `cargo test -p mini-agent-host --lib result_store::tests::session_result_store -- --test-threads=1` passed: reload and persisted-content-bound tests.
- The result-store tests exercise append, drop, reopen, handle read, and the 64 KiB persisted-content bound.

## Follow-up boundaries

Running processes, queued input, approvals, and in-flight turns remain process-local and are not replayed after restart. Result handle persistence does not make an interrupted external effect replay-safe.
