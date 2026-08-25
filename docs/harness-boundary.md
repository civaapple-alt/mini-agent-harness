# Harness boundary

## Question

What is the smallest harness that lets us compare good and bad agent behavior
without turning the experiment platform into the product being studied?

## WHAT

The model owns proposals:

- reasoning text, streamed separately from user-facing text;
- assistant text;
- zero or more named tool calls;
- arguments for each call.

The model does not execute calls, admit capabilities, truncate results, choose
retry policy, or decide whether a missing tool is fatal.

Tools own one capability each:

- a model-visible name, description, and input schema;
- execution of validated or rejected input;
- a result or an explicit error.

Tools do not append conversation state or call the model.

The provider adapter preserves reasoning and assistant text as distinct
bounded streams. The harness observes each delta without interpreting it and
retains settled reasoning with its assistant turn so a later Responses API
request can replay the same item order. This remains conversation mechanics,
not a second control lane or hidden scheduler.

The host may append a bounded typed context item for facts such as world state.
Core preserves its order and limit but does not discover the environment or
interpret the payload. Provider adapters decide the protocol role; the
Responses adapter maps it to a `developer` message.

## HOW

The harness owns mechanics:

- the system prompt and ordered conversation;
- an independently bounded context-item message shape;
- the model/tool/model loop;
- step and output limits;
- hard user-input, response, tool-call, and request-context bounds;
- opt-in model-generated context compaction between settled steps;
- unknown-tool and tool-failure projection;
- stop classification;
- an ordered observation trace.

The first implementation keeps this loop concrete. A policy interface would
make experiments look configurable while hiding the actual causal difference.
For now, a Good/Bad comparison is two small loop implementations or two direct
changes with their traces compared.

## Lessons retained from Pi v2

Pi v2's harness specification solves a larger problem: durable conversations
that resume after a crash without blindly repeating external effects. Its most
general lessons apply even before persistence exists.

### Effect boundary

Every external action has three conceptual moments:

```text
prepare intent -> perform uncertain effect -> settle outcome
```

The in-memory harness exposes these moments through ordered events. If a later
experiment adds persistence, intent and settlement are the two commit points;
the effect remains between them. Persistence must not be added as an unrelated
session feature.

### Explicit state

The next action should be selected from explicit current state. The first loop
has only messages, current step, and stop reason. It must not infer a tool's
success from the absence of an error event or infer completion from storage.

### Observation is not interception

Observers receive immutable events and return nothing. They cannot rewrite
messages, tool arguments, or outcomes. An interception mechanism will be added
only when a concrete experiment needs to compare one.

## Deliberate omissions

The default harness has no durable storage, tree, control lanes, queues, hooks,
memory, MCP, plugins, background work, telemetry framework, or schema
migration. It also has no generic scheduler or state-machine framework.
Reasoning display, the terminal input queue, result handles, and managed
processes stay in the CLI/provider host rather than becoming core scheduling or
persistence concepts. Context compaction is present as one direct loop branch
but remains disabled by default. When compaction runs, the latest typed context
item is retained so a summary cannot silently erase current execution
authority.

Each omission is reversible. Adding all of them preemptively is not.

## Admission test for a core feature

A proposed core feature must answer all four questions:

1. Which harness hypothesis does it let us test?
2. What observable trace distinguishes success from failure?
3. Why can it not live in the CLI or another host adapter?
4. Which existing concept can be removed or kept unchanged to pay for it?

If those answers are missing, the feature stays outside core.

The auto-copilot experiment admits compaction under this test: its hypothesis
is that bounded summaries let an unattended tool loop continue past raw-history
growth; `context_compaction_started` and `context_compaction_finished` make the
intervention observable; it must live in core because it replaces the exact
message sequence used by the next model request; and it adds no persistence,
queue, hook, or generic policy layer. Its auxiliary request keeps the normal
system prompt, tool catalog, and message prefix byte-stable, then appends the
compaction instruction as the final user message so prefix caches remain useful.
