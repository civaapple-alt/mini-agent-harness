# Model Provider Factory Seam

Status: implemented

## Decision

Host runtime assembly is generic over the Core `Model` contract. The Host now
exposes `ModelProviderFactory<M>` and
`prepare_harness_with_model_factory`, while the App Server exposes
`AppServerRuntime::<M>::start_with_model_factory`.

The capability registry records stable model provider descriptors separately
from construction. An embedding application registers its model ID with
`CapabilityRegistry::with_model_provider` and supplies the implementation
factory. Profile validation therefore remains deterministic and does not load
code from profile files.

The built-in OpenAI-compatible startup methods remain source-compatible and
use the same generic path with the built-in factory. Tools, policy, extensions,
world state, session persistence, and workflow management remain assembled by
Host and served through the App Server.

## Example

`crates/mini-agent-app-server/examples/external_model_provider.rs` contains a
compile-checked `EchoModel` provider and demonstrates the complete registration
and startup call without contacting a network provider.

## Verification

```text
cargo check -p mini-agent-capabilities -p mini-agent-host -p mini-agent-app-server --all-targets --locked PASS
cargo check -p mini-agent-app-server --examples --locked PASS
cargo test -p mini-agent-capabilities registry --all-targets --locked PASS (4 tests)
cargo test -p mini-agent-app-server --all-targets --locked PASS (25 tests)
```
