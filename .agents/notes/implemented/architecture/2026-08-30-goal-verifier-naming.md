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

Only the `VERIFIER_OPENAI_*` names are supported. The product is still in
rapid iteration, so obsolete configuration names are not read as fallbacks and
do not create a second compatibility policy.

There is no `mini-agent-mentor` crate in the 0.4.0 workspace. Historical
Mentor command/design notes remain historical records; current user-facing
documentation describes only Goal verification.

## Consequences

- New code does not import an App Server `mentor` module or call a
  `mentor_provider_settings` method.
- Goal verifier turns continue to use an isolated, tool-free App Server path;
  the supported configuration namespace is now canonical and explicit.
- Configuration lookup has one canonical verifier namespace and no migration
  branch.

## Measured result

```text
runtime:          14,001 / 20,000 lines
all Rust source:  27,221 / 30,000 lines
```

## Verification

```text
cargo test -p mini-agent-host PASS (41 tests)
cargo test -p mini-agent-app-server PASS (20 tests)
cargo test -p mini-agent-cli PASS (14 unit + 11 integration tests)
cargo clippy -p mini-agent-capabilities -p mini-agent-host -p mini-agent-app-server -p mini-agent-cli --all-targets -- -D warnings PASS
cargo check --workspace --all-targets PASS
python scripts/line_budget.py PASS
```
