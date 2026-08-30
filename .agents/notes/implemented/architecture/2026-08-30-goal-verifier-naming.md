# Goal Verifier Naming Boundary

Status: implemented

## Decision

The feature formerly carrying the Mentor name is a Goal verifier, not a
separate product path. The current code and API use `verifier` terminology:

```text
mini-agent-app-server/src/verifier.rs
mini_agent_app_server::verifier::verify_goal_checkpoint
ThreadId("goal-verifier")
mini-agent-goal-verifier (internal App Server client name)
RuntimeConfig::verifier_provider_settings
```

The canonical configuration names are:

```text
VERIFIER_OPENAI_MODEL
VERIFIER_OPENAI_API_KEY
VERIFIER_OPENAI_BASE_URL
```

Existing installations remain compatible because the legacy
`MENTOR_OPENAI_MODEL`, `MENTOR_OPENAI_API_KEY`, and `MENTOR_OPENAI_BASE_URL`
names are read as fallbacks. When both names are present, the `VERIFIER_*`
value wins at the same source level and across process, workspace, and user
configuration resolution.

There is no `mini-agent-mentor` crate in the 0.4.0 workspace. Historical
Mentor command/design notes remain historical records; current user-facing
documentation describes only Goal verification and mentions the legacy names
only as compatibility aliases.

## Consequences

- New code does not import an App Server `mentor` module or call a
  `mentor_provider_settings` method.
- Goal verifier turns continue to use an isolated, tool-free App Server path;
  only naming and configuration precedence changed.
- Existing `.env` files using `MENTOR_OPENAI_*` continue to work without
  migration.
- The legacy environment names can be removed in a later breaking release
  after an explicit deprecation window.

## Measured result

```text
runtime:          14,055 / 20,000 lines
all Rust source:  27,269 / 30,000 lines
```

## Verification

```text
cargo test -p mini-agent-host PASS (42 tests)
cargo test -p mini-agent-app-server PASS (20 tests)
cargo test -p mini-agent-cli PASS (14 unit + 11 integration tests)
cargo clippy -p mini-agent-capabilities -p mini-agent-host -p mini-agent-app-server -p mini-agent-cli --all-targets -- -D warnings PASS
cargo check --workspace --all-targets PASS
python scripts/line_budget.py PASS
```
