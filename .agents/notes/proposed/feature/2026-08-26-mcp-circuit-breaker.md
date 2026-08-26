# Circuit Breaking and Graceful Degradation for Remote HTTP MCP Tools

Status: proposed

## Context

Streamable HTTP MCP tools depend on external network stability. When a remote server encounters high latency, intermittent 500 errors, or connection drops during a multi-turn run, consecutive failing tool calls degrade the model's performance and burn step budgets.

## Proposal

Implement a host-level circuit breaker in `mini-agent-cli/src/mcp.rs`:
1. Track consecutive transport timeouts and failure thresholds for HTTP MCP endpoints.
2. Trip into an `Open` state after $N$ consecutive failures, temporarily deregistering the affected tool specifications from the prompt context.
3. Automatically probe health in the background and restore tools upon successful heartbeat.
4. Notify the user and model with an explicit tool unavailability notice instead of hanging.

## Acceptance Criteria

- A hanging or failing HTTP MCP server fails fast and does not block independent local tools or other responsive MCP servers.
- Trace events log state transitions (`McpServerDegraded`, `McpServerRestored`).

## Risks

- Adding dynamic tool deregistration must not cause context hash thrashing or violate the core tool-spec stability contract.
