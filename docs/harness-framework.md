# Runtime architecture

Mini Agent Harness is a deliverable Agent runtime. Its architecture keeps the
Agent Loop small and puts long-lived execution concerns in the Control Plane.
The split is an ownership boundary. It is not a license for either side to
accumulate unrelated responsibilities.

## Ownership

| Layer | Owns | Does not own |
| --- | --- | --- |
| Protocol | model, tool, message, event, stop, and limit contracts | provider connections or side effects |
| Core | bounded turn loop, context control, stop classification, observation events, and conversation writeback | files, processes, approval UI, persistence, or terminal output |
| Capabilities | providers, workspace tools, process execution, sandboxing, MCP, Skills, and concrete side effects | the Core turn loop or a second Session history |
| Host | prompt and rule composition, tool admission, approval ordering, and runtime assembly | public Thread lifecycle or a browser-facing state store |
| App Server | Thread, Turn, Goal, Session, Actor/CAS control, runtime status, recovery, and JSON-RPC projection | a second model/tool loop |
| SDK, Gateway, and Web Studio | process connection, protocol mapping, Project metadata, control requests, and bounded UI projections | execution authority, approval grants, or canonical Session history |

`mini-agent` and Web Studio are clients of the same App Server runtime. The
CLI is useful for local runs, scripts, and lower-level boundary checks. Web
Studio is the main control and observation interface for long-running work.
The Rust REPL and Python TUI are experimental clients. They do not define a
separate runtime model.

## Runtime path

```text
model request or tool call
        ↓
Core executes one bounded turn
        ↓
Host admits the action and composes runtime state
        ↓
Capabilities perform the approved side effect
        ↓
App Server persists and projects Thread, Turn, Session, and events
        ↓
Python SDK → FastAPI Gateway → Web Studio
```

Core runs one Thread at a time. It validates a model response, executes a
bounded tool batch, appends the result to the conversation, and either requests
another model step or settles the Turn. Cancellation and steering are observed
at safe boundaries between model steps and complete tool batches. A settled
checkpoint is written before a later Turn resumes that Session.

Child work does not add a scheduler to Core. Host and App Server create an
independent child Session from an exact settled checkpoint, then run its own
runtime. The Session store records the operation lifecycle and App Server
projects it to clients. A child has its own history, approval flow, runtime
status, and event stream.

## Long-running control plane

The Control Plane makes a running task observable and recoverable without
expanding the model loop:

- App Server owns the canonical Thread, Turn, Goal, Session, checkpoint, and
  operation state.
- Host and Capabilities own tool admission, path policy, sandboxing, approval,
  and side effects.
- The Session store keeps settled conversation checkpoints, operation records,
  and the bounded Notebook. It does not make an interrupted external effect
  safe to replay.
- Gateway and Web Studio project runtime state, replay bounded events after a
  reconnect, and reconcile from canonical Thread and item data when the event
  cursor has a gap.

For exact public methods and fields, see [App Server](app-server.md). For the
change-admission rules and non-negotiable boundaries, see [Harness
boundaries](harness-boundaries.md).

## Maintenance

Update this document when an ownership boundary or the runtime path changes.
Record the implementation decision, alternatives, and verification evidence in
`.agents/notes/`. Do not turn this page into a batch log or a comparison of
other agent products.
