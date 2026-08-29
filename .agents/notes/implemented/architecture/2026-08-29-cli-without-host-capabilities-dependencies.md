# CLI Without Direct Host and Capabilities Dependencies

Status: implemented

## Decision

`mini-agent-cli` is now a frontend of `mini-agent-app-server` at the Cargo
dependency boundary. Its direct dependencies are limited to:

```text
mini-agent-app-server
serde_json
tokio
```

The App Server exposes a documented `frontend` facade for the small set of
launch and presentation contracts that a CLI needs: sandbox and security
values, approval control, profile/config resolution, run observation, core
event/input types, and skill discovery. Concrete implementations remain in
Host and Capabilities, but the CLI no longer compiles against those crates
directly.

Provider-specific `prompt_weight` and `real_llm` programs were moved from the
CLI package to `mini-agent-experiments`. They remain runnable and tested, while
the executable package stays focused on user-facing CLI behavior.

The dependency shape is now:

```text
mini-agent-cli
    -> mini-agent-app-server::frontend
    -> mini-agent-app-server::LocalAppServerClient
        -> mini-agent-host -> mini-agent-capabilities -> mini-agent-core
```

## Consequences

- Removing or replacing the Host/Capabilities implementations no longer
  changes the CLI manifest or source imports unless the App Server frontend
  facade changes.
- CLI-specific startup flags still exist, but their value types and runtime
  adapters are exposed from one App Server boundary.
- The facade intentionally re-exports stable value and adapter contracts; it
  does not expose Host runtime assembly or capability registries to the CLI.
- Provider experiments are explicitly separated from the executable package
  and therefore do not pull provider implementation dependencies into the CLI
  target.

## Verification

```text
cargo tree -p mini-agent-cli --depth 1 --locked
  mini-agent-app-server
  serde_json
  tokio
rg mini_agent_(capabilities|host|core) crates/mini-agent-cli --glob '*.rs' --glob '*.toml'
  no matches
cargo test -p mini-agent-cli --all-targets --locked PASS (51 tests)
cargo test -p mini-agent-experiments --all-targets --locked PASS (6 tests)
cargo clippy -p mini-agent-app-server -p mini-agent-app-server-protocol -p mini-agent-cli -p mini-agent-acp -p mini-agent-experiments --all-targets --locked -- -D warnings PASS
```

The checks are local Windows evidence. The runtime line budget is currently
`15,952/20,000`; the overall workspace total is `38,881/30,000` because the
provider and experiment crates remain visible in the project total.
