# Capabilities API Boundaries

Status: implemented

## Decision

The `mini-agent-capabilities` root remains a single crate facade, but its
exports now distinguish three roles:

```text
stable contracts
  approval/security, sandbox selection, sessions, result storage,
  provider registry, prompt roles

Host/App Server composition seams
  model factories, OpenAI provider types, image store/upload seam,
  MCP loading, skill discovery, workspace tool assembly

crate-internal implementation
  process sandbox guard, shell command execution/output, workspace state,
  argument parsing, image detection/envelopes/projection/wire types,
  path aliases and timestamp generation
```

The audit covered all root re-exports and all Rust call sites in the
workspace. The following symbols had no crate-external call sites and were
removed from the root facade and narrowed to `pub(crate)` or private:

- `ProcessSandbox` and the Windows job-object module;
- `run_sandboxed_command`, `CommandOutput`, `shell_command`, and `string_arg`;
- image implementation types and helpers (`DeepSeekFiles`, `ProjectedImage`,
  `StoredImage`, envelopes, media detection, projection, and wire helpers);
- `timestamp_ms` and path-policy helpers other than `normalize_path`.

The remaining MCP and workspace assembly exports are intentionally retained:
Host and App Server still call them to compose a runtime. They are embedding
seams, not low-level user-facing workflow APIs, and should move behind a Host
facade in a later compatibility-conscious step.

Path-policy tests were moved from Host into the Capabilities crate so tests do
not force implementation helpers to remain public across crate boundaries.

## Consequences

- The root facade no longer exposes process handles, shell execution details,
  image wire representations, or generic JSON argument parsing.
- `ImageStore` and `FileUploader` remain available for provider composition;
  image persistence and request projection are internal to the capability
  implementation.
- `SandboxKind`, approval/security types, session types, result storage, and
  provider registry types remain available to Host/App Server as stable
  runtime contracts.
- Existing runtime behavior and wire formats are unchanged. This is a
  visibility and ownership cleanup, not a capability removal.

## Measured result

```text
capabilities:      9,511 lines
runtime:          13,977 / 20,000 lines
all Rust source:  27,191 / 30,000 lines
```

## Verification

```text
cargo fmt --all PASS
cargo test -p mini-agent-capabilities PASS (63 tests)
cargo test -p mini-agent-host PASS (40 tests)
cargo test -p mini-agent-app-server PASS (20 tests)
cargo check --workspace --all-targets PASS
cargo clippy -p mini-agent-capabilities -p mini-agent-host -p mini-agent-app-server -p mini-agent-cli --all-targets -- -D warnings PASS
python scripts/line_budget.py PASS
```
