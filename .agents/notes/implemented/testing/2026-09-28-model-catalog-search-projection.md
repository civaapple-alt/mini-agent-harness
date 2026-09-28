# Model catalog search projection evidence

## Decision

The App Server provider view carries Host-computed `webSearchSupport` and
`webSearchEnabled` alongside the provider's editable `webSearch` preference.
This keeps endpoint support and effective enablement available to clients.

## Bounded scenario

| Field | Evidence |
| --- | --- |
| Hypothesis | Protocol projection preserves Host-computed search support and effective enablement. |
| Public path | App Server model catalog projection used by `model/catalog/manage`. |
| Setup | Host provider view marks search as supported, enabled, and configured; no credential value or provider request. |
| Stimulus | Project the Host view into the public App Server catalog DTO. |
| Trace | Protocol output retains `webSearchSupport: supported` and `webSearchEnabled: true`. |
| Settlement | Clients receive the same computed capability state that Host resolved. |
| Failure case | Missing or defaulted fields would discard Host's endpoint decision. |
| Command | `cargo test -p mini-agent-app-server provider_search_support_survives_protocol_projection` |
| Gap | Tests DTO projection directly; it does not call the machine catalog store or Web Studio. |

## Verification

`cargo test -p mini-agent-app-server` passed (81 library tests and one binary
test); `cargo test -p mini-agent-app-server-protocol` passed (21 tests).
Targeted Clippy, fmt, and line budget passed.
