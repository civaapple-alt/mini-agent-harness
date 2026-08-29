# Source Code Line Budget and Minimalist Governance

Status: implemented

## Context

Software projects studying agent harnesses tend to bloat with speculative frameworks, unnecessary generic abstractions, and duplicate utilities, making comparative experimentation difficult and maintenance expensive.

## Decision

The workspace enforces strict hard line ceilings on Rust source code:

1. **Ceilings**:
   - Runtime layers (`core` + `protocol` + `host` + `app-server`): Maximum
     **20,000** Rust lines (including tests). The provider implementation group
     `mini-agent-capabilities` is excluded from this runtime limit but remains
     part of the workspace total.
   - Entire workspace: Maximum **30,000** Rust lines (including tests). Both
     the runtime and workspace ceilings fail the release gate for 0.4.0.
2. **Automated Enforcement**:
   - Both ceilings are validated by `python scripts/line_budget.py` and run as
     part of CI.
   - The report also shows `core`, `protocol`, `capabilities`, `host`,
     `app-server`, and `cli` separately so architectural growth is
     visible before the workspace ceiling is reached.
   - Each layer and the workspace total are split into production, unit-test,
     and integration-test lines. Inline `#[cfg(test)]` items and `*_tests.rs`
     files count as unit tests; files under a `tests/` directory count as
     integration tests. The workspace total and runtime total are both hard
     gates.
3. **Design Principle**:
   - Removing a concept or abstraction is always preferred over compressing it behind shorthand macros or indirections.
   - Defaults are the product; configuration options are added only when two mutually incompatible behaviors must coexist.

## Consequences

- Codebase stays concise, readable, and tightly focused on harness behavioral research.
- Forces every new architectural proposal to prove its necessity against a strict complexity budget.
