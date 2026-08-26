# Model Context Protocol (MCP) and Skills Discovery

Status: implemented

## Context

Agents must interact with external tools and domain instructions defined by open ecosystem standards (Agent Skills, MCP) without compromising safety, startup latency, or bounded tool cardinality.

## Decision

1. **Protocol Implementation**:
   - The CLI leverages `rmcp` to support both `stdio` child processes and `Streamable HTTP` transports.
   - Project-level MCP configuration resides in `.agents/mcp/` or `.agents/mcp.json`.
2. **Skills Discovery**:
   - Discovers project-scoped `SKILL.md` collections under `.agents/skills/` and `.agents/skillsets/`.
   - Reads only bounded YAML frontmatter headers on discovery, allowing progressive disclosure when tools call `read_file`.
3. **Non-Blocking Startup & Approval**:
   - Interactive startup does not freeze if an MCP server requires approval; servers remain marked inactive until connection approval succeeds.
   - Interactive `/mcp` command enables retrying connection-failed or denied servers dynamically without wiping conversation history.

## Consequences

- Standardizes tool interoperability with external tools and plugin ecosystems.
- Isolates flaky external network services from the primary CLI startup and conversation lifecycle.
