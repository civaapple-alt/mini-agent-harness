# Harness experiments

## Unit of comparison

A harness experiment changes one HOW decision while holding WHAT inputs fixed.

Fixed inputs should include:

- model and provider version;
- system prompt unless it is the treatment;
- user prompt and workspace fixture;
- tool names, schemas, and implementations;
- maximum steps and tool-output budget;
- randomness or repeated-run count.

The treatment is one explicit harness behavior, such as:

- return an unknown-tool error to the model versus ending the run;
- truncate tool output versus reject oversized output;
- expose one precise edit tool versus a general write tool;
- add a retry versus expose the first provider failure;
- preserve every prior message versus compact context.

Do not compare two bundles of unrelated features and call the result a harness
finding.

## Evidence

Each run records ordered JSONL events with `--trace PATH`. A comparison should
report at least:

- stop reason and number of model steps;
- requested and completed tool calls;
- tool errors and output truncation;
- final task evidence, preferably a deterministic verifier;
- latency and token/cost data when a provider exposes them;
- unexpected human approvals or interventions.

The trace captures causal harness events and portable input, cached-input, and
output token counts when the provider reports them. Pricing and
provider-specific accounting remain outside core.

## Good and bad

Good and bad are outcomes, not architectural styles.

A smaller harness is bad when it hides a failure, repeats a dangerous effect,
or prevents the model from recovering from useful tool feedback. A larger
harness is bad when its added context, policies, retries, or abstractions make
the same model less predictable without improving task evidence.

The preferred harness is the smallest treatment that improves repeatable task
outcomes under the fixed comparison.

## Initial experiment queue

1. Unknown tool: project the error back into context versus stop immediately.
   [First deterministic result](experiments/unknown-tool.md): projection lets a
   recovery-capable model pass the verifier in two steps; immediate stop does
   not.
2. Edit surface: exact unique replacement versus unrestricted file rewrite.
   [Deterministic result](experiments/edit-surface.md): both reach the target
   in three steps, but whole-file rewrite loses unrelated content. The default
   therefore edits existing files precisely and creates new files separately.
3. Tool output: head truncation versus head-plus-tail truncation.
   [Deterministic result](experiments/tool-output-retention.md): with the same
   byte budget, head-plus-tail preserves both orientation and a decisive final
   verdict, so it becomes the direct default.
4. Prompt weight: minimal tool-use instruction versus a long policy prompt.
   The [real-model evaluation](experiments/prompt-weight.md) is ready and keeps
   full event, verifier, latency, and token evidence. It remains intentionally
   unrun until provider use is explicitly authorized.
5. Persistence: restart a read-only tool call versus restart a write-capable
   call; use this to decide whether an effect intent journal is justified.
   [Crash-boundary simulation](experiments/effect-recovery.md): reads can be
   replayed, non-idempotent writes cannot, and intent alone detects uncertainty
   but cannot settle it. Generic persistence therefore stays out of core.
