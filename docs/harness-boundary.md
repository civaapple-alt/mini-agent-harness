# Harness boundary

## Question

What is the smallest harness that lets us compare good and bad agent behavior
without turning the experiment platform into the product being studied?

## WHAT

The model owns proposals:

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

## HOW

The harness owns mechanics:

- the system prompt and ordered conversation;
- the model/tool/model loop;
- step and output limits;
- hard user-input, response, tool-call, and request-context bounds;
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

The default harness has no durable storage, tree, lanes, queues, hooks,
compaction, memory, MCP, plugins, background work, telemetry framework, or
schema migration. It also has no generic scheduler or state-machine framework.

Each omission is reversible. Adding all of them preemptively is not.

## Admission test for a core feature

A proposed core feature must answer all four questions:

1. Which harness hypothesis does it let us test?
2. What observable trace distinguishes success from failure?
3. Why can it not live in the CLI or another host adapter?
4. Which existing concept can be removed or kept unchanged to pay for it?

If those answers are missing, the feature stays outside core.
