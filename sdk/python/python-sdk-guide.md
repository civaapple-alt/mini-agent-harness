# Python SDK guide

The `mini-agent` package is the zero-dependency asynchronous client for
`mini-agent-app-server`. It starts one local App Server process, exchanges
JSON-RPC over stdio, parses bounded projections, and forwards control requests.
The client does not implement an Agent Loop or keep Session history, approval
grants, or recovery state.

## Start and initialize a client

Use the context manager so the SDK always releases its child process. After
the process starts, negotiate the protocol and select a Thread before you
submit a Turn:

```python
import asyncio

from mini_agent import MiniAgentClient


async def main() -> None:
    async with MiniAgentClient() as client:
        await client.initialize()
        await client.start_thread()

        async for envelope in client.stream_turn("List files in this workspace."):
            if envelope.get("type") != "event":
                continue
            event = envelope["typed_event"]
            if event.event_type == "assistant_text_delta":
                print(event.delta, end="", flush=True)


asyncio.run(main())
```

The SDK requires Python 3.10 or later. It finds `mini-agent-app-server` on
`PATH`, or you can set `MINI_AGENT_APP_SERVER_PATH` or pass `executable=`.
When `start_thread()` is called without an ID, it attaches to the Thread
selected by the App Server for the active Session.

`restart()` reapplies the original client identity, provider selection, and
world execution access and policy. If the App Server reports a durable Session,
the SDK keeps its Session ID and resumes it after a process restart. With
`MINI_AGENT_SESSION_MODE=disabled`, the App Server has no durable Thread history
to recover; restarting creates an empty Thread with the same ID.

The constructor merges configuration in this order: explicit `env`, process
environment, then the first value found in `.env` under `cwd` or
`~/.mini-agent`. This matches the App Server's configuration-file locations;
the SDK never changes the parent process environment.

Logging is disabled by default. Pass `log_dir` or `log_file`, or set
`MINI_AGENT_LOG_DIR` or `MINI_AGENT_LOG_FILE`, to write SDK logs.

## Inspect execution recovery

`read_thread()` exposes an unsettled execution checkpoint as
`ThreadCheckpoint.execution_recovery`. `read_turn()` exposes the same App Server
record as `TurnReadResult.recovery`. Both fields use `ExecutionRecoveryInfo`.
The model includes the Turn ID, status, phase, checkpoint sequence, heartbeat,
progress timestamp, and recovery reason.

`recommended_action` is an informational hint. For `waiting_for_continue`, check
the Turn ID and checkpoint sequence before calling `resume_turn()`. For
`needs_reconciliation`, verify the tool's external effect before resuming. The
SDK never retries a tool from this hint. Unknown status and phase values map to
`UNKNOWN`; `raw` retains the original recovery record. Read enum values with
`recovery.status.value` and `recovery.phase.value`.

Use `replay_events()` after a short disconnect. If `TurnEventsResult.has_gap` is
true, read the Thread checkpoint and its bounded ThreadItems before treating
the replay page as complete. The SDK reports the gap and leaves reconciliation
to the caller.

```python
client = MiniAgentClient(
    executable="mini-agent-app-server",
    log_dir="logs",
    request_timeout=30.0,
)
```

## Call the public boundary

App Server owns Thread, Turn, Goal, Session, approval, and recovery semantics.
The SDK exposes its supported controls directly:

| Task | SDK method |
| --- | --- |
| Create or attach a Thread | `start_thread()` |
| List or read Threads | `list_threads()`, `read_thread()` |
| Submit a Turn | `start_turn()` or `stream_turn()` |
| Read or wait for settlement | `read_turn()` or `wait_for_turn()` |
| Steer or stop a running Turn | `steer_turn()` or `interrupt_turn()` |
| Read live or replayable state | `get_runtime_status()`, `replay_events()`, `list_thread_items()` |
| Create a logical Thread branch | `fork_thread()` |
| Create an independent persisted Session | `fork_session()` |
| Restore a serialized checkpoint | `resume_thread()` |
| Configure Plan or a Goal | `update_thread_settings()`, `set_goal()`, `get_goal()`, `clear_goal()` |
| Read or modify a Notebook | `read_notebook()`, `search_notebook()`, `write_notebook()`, `forget_notebook()` |
| Inspect execution environment | `get_world_state()`, `refresh_world()`, `set_world_execution()` |
| Inspect or retry MCP | `get_mcp_status()`, `retry_mcp()` |
| Inspect local tasks | `list_background_tasks()` and `list_scheduled_tasks()` |

`get_workflow_state()` is an SDK-only read-only convenience projection. It
combines cached Thread settings with `thread/goal/get`; it does not send the
removed `workflow/state` RPC.

`start_turn()` and `stream_turn()` accept `mode="start"` or
`mode="start_if_idle"`. Their optional `effort` is sent as the legacy
`input.reasoningEffort` fallback; an explicit Thread `reasoningSelection` takes
precedence.

`read_thread()` returns a `ThreadCheckpoint` that can be passed directly to
`resume_thread(new_thread_id, checkpoint)`. The SDK unwraps the `thread/read`
action envelope before sending the checkpoint to the App Server.

Protocol version 2 has no Notebook search RPC. The SDK's
`search_notebook()` helper reads the bounded Notebook projection and filters
entries locally; it does not send an unsupported method to the App Server.

## Stream events without losing identity

`stream_turn()` submits a Turn and yields its correlated envelopes until that
Turn settles. The first envelope has type `_turn_submission`; subsequent
envelopes include `event`, `approval`, and lifecycle notifications. Unknown
Core event types remain `GenericEvent`, so a newer App Server event does not
break an older SDK consumer.

```python
async for envelope in client.stream_turn("Inspect the workspace"):
    if envelope.get("type") != "event":
        continue
    for item in envelope.get("typed_items", []):
        if item.type == "toolCall":
            print(item.id, item.name, item.status, item.outcome)
```

`ThreadItem.status` describes item lifecycle. `ThreadItem.outcome` describes
the structured tool result. Do not infer approval, retry, or success from tool
output text.

For a reconnect, use the latest per-Thread event sequence with
`replay_events()`. If the page has `has_gap=True`, load canonical history
before accepting replay as complete:

```python
page = await client.replay_events("default", after_sequence=last_sequence, limit=128)
if page.has_gap:
    checkpoint = await client.read_thread("default")
    items = await client.list_thread_items("default", limit=128)
```

## Receive notifications and approvals

`notification_handler` receives `turn/event`, item lifecycle, runtime, and
transport-error notifications. The SDK awaits the handler for `turn/event`,
so put slow work on your own queue rather than block the reader loop.

Each `stream_turn()` has a 512-message and 2 MiB buffer; streams on one client
share an 8 MiB byte budget. If a consumer falls behind, `StreamEventOverflowError`
ends that stream and identifies its Thread. Use the last received sequence with
`replay_events()` and reconcile canonical history if the replay page reports a
gap. Stream dictionaries retain `itemId` and `turnSource`, and the typed item
lifecycle and Thread-item projections expose `turn_source`.

```python
async def approve_once(request: dict) -> dict:
    return {"decision": "approve", "grantScope": "once"}


async def observe(notification: dict) -> None:
    print(notification["type"])


client = MiniAgentClient(
    approval_handler=approve_once,
    notification_handler=observe,
)
```

`approval_handler` receives only requests that the runtime has not already
resolved. It must return a decision and, for approval, a grant scope allowed
by the request. Without a handler, the SDK denies the request. The SDK reports
the same approval record through `stream_turn()`.

To let the Host expose its `ask_user` tool, construct the client with
`user_questions=True`. On `user-question/request` and
`user-question/updated`, the notification envelope includes a typed
`UserQuestionNotification` in `typed_user_question`. Its interaction identifies
the Thread, Turn, tool call, current question, and prior answers. Send one answer with
`respond_user_question()`, using `UserQuestionAnswer.for_option()`,
`UserQuestionAnswer.free_text()`, or `UserQuestionAnswer.skipped()` according
to the question's allowed choices. The SDK never picks a recommended option
for the user. If the client reconnects while a question is open,
`read_thread()` returns the pending interaction so the application can present
it again. See [`08_personal_agent.py`](../../cookbook/python-demo/08_personal_agent.py)
for a persistent interactive example.

## Set execution and lifecycle controls

Access scope and approval policy are independent. Host and Capabilities
enforce both controls, not the SDK:

```python
await client.set_world_execution(access="full_machine", policy="interactive")
await client.update_thread_settings(
    "plan",
    thread_id="default",
    continuation_mode="manual",
)
await client.set_goal("Review the repository and report verified findings.")
```

`full_machine` expands candidate filesystem scope. It does not bypass Deny
rules, Plan locks, tool availability, sandbox checks, or high-risk approval.
An accepted `interrupt_turn()` request also does not mean that the Turn is
settled. Continue streaming or call `read_turn()` until the terminal result.
Core cancels its pending model future immediately when the interrupt reaches
the run loop; a provider may continue remote work after the local request is
closed. `MiniAgentClient.stop(force=True)` first tries the App Server's normal
EOF shutdown, then sends termination signals to the isolated POSIX process
group if it does not exit. Its Boolean result reports confirmed process exit,
not just that shutdown was requested.

`fork_thread()` creates an in-process logical branch. `fork_session()` creates
an independent Session from a complete checkpoint. Neither copy an in-flight
Turn or approval wait. Notebook entries, background Shell tasks, and scheduled
delay markers remain App Server control-plane state. Read their bounded
projections through the SDK, but do not keep a second operation history or
scheduler in a client. Scheduled tasks are delay markers only; they do not
resume a Thread or start a later Turn.

## Handle timeouts and protocol changes

The default request timeout is 30 seconds. A positive `request_timeout`
changes that value for one client. On timeout, the SDK removes the pending
request and raises `AppServerRequestTimeoutError`, which records the method and
timeout. A process exit or transport failure raises `ServerProcessError`.
`AppServerError` retains the JSON-RPC error `code`, `message`, and `data`.
`session/fork` identity or context-policy conflicts use
`SESSION_FORK_CONFLICT_CODE` (`-32001`).
`wait_for_turn()` retries timed-out `turn/read` requests and successful
`in_progress` results. Other App Server errors, including an unknown Turn ID,
are returned immediately.

When event models, public types, or wire fields change, run:

```bash
uv run --project sdk/python python cookbook/python-demo/06_protocol_compatibility.py
uv run --project sdk/python pytest sdk/python/tests/test_sdk_events.py sdk/python/tests/test_cookbook_validation.py -q
```
