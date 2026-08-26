# Source Code Line Budget and Minimalist Governance

Status: implemented

## Context

Software projects studying agent harnesses tend to bloat with speculative frameworks, unnecessary generic abstractions, and duplicate utilities, making comparative experimentation difficult and maintenance expensive.

## Decision

The workspace enforces strict hard line ceilings on Rust source code:

1. **Ceilings**:
   - `mini-agent-core`: Maximum **20,000** Rust lines (including tests).
   - Entire workspace: Maximum **30,000** Rust lines (including tests and CLI).
2. **Automated Enforcement**:
   - Ceilings are validated by `python scripts/line_budget.py` and run as part of CI.
3. **Design Principle**:
   - Removing a concept or abstraction is always preferred over compressing it behind shorthand macros or indirections.
   - Defaults are the product; configuration options are added only when two mutually incompatible behaviors must coexist.

## Consequences

- Codebase stays concise, readable, and tightly focused on harness behavioral research.
- Forces every new architectural proposal to prove its necessity against a strict complexity budget.
