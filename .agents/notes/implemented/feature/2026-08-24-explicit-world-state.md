# Explicit World State Context Injection

Status: implemented

## Context

Coding agents often hallucinate platform commands (e.g. running Unix `bash` syntax on Windows without PowerShell, or attempting forbidden network requests) when host platform facts are implicit or absent from context.

## Decision

1. **Detection**:
   - The CLI detects host OS, architecture, active shell (`pwsh` on Windows, `sh` on Unix), workspace directory, approval mode, and discovered command catalog.
2. **Context Injection**:
   - The captured facts are formatted into a deterministic JSON object (`WorldState`) and injected as a typed `Message::Context` developer message.
3. **Inspection & Refresh**:
   - Exposed to users via `mini-agent status` and interactive commands `/world` and `/world refresh`.
   - Preserved across context compaction cycles to guarantee unbroken environmental awareness.

## Consequences

- Significantly reduces syntax errors and invalid command invocations across diverse OS environments.
- Ensures runtime environment facts are explicitly tracked in trace logs rather than being hidden in implicit system prompts.
