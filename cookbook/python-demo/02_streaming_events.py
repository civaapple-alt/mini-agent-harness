"""
Demo 02: Deep Event Stream Inspection
Demonstrates inspecting granular event types including model step lifecycle,
token streaming, tool invocations, UTF-8 output truncation, and usage metrics.
"""

import asyncio
import json

from mini_agent import MiniAgentClient, ToolFinishedEvent


async def main():
    print("=== Demo 02: Deep Event Stream Inspection ===", flush=True)

    async with MiniAgentClient(log_dir="logs") as client:
        await client.initialize()
        await client.start_thread()

        prompt = "Inspect the current directory, check files, and tell me the project structure."
        print(f"[Prompt]: {prompt}\n", flush=True)

        current_stream_mode: str | None = None

        async for item in client.stream_turn(prompt):
            if item["type"] == "approval":
                approval = item.get("approval", {})
                phase = approval.get("phase", "unknown")
                outcome = approval.get("outcome", "pending")
                print(
                    f"[Approval {phase}] {approval.get('actionSummary', '')} "
                    f"(id={approval.get('requestId', '')}, outcome={outcome})",
                    flush=True,
                )
                continue

            if item["type"] != "event":
                continue

            event = item["event"]
            seq = item.get("sequence", 0)
            event_type = event.get("type", "unknown")

            # 1. Model inference step started
            if event_type == "model_started":
                if current_stream_mode is not None:
                    print("\n", flush=True)
                    current_stream_mode = None
                print(
                    f"\n[Seq #{seq:02d}] [Model Step {event.get('step')} Started]",
                    flush=True,
                )

            # 2. Streaming reasoning tokens
            elif event_type == "assistant_reasoning_delta":
                if current_stream_mode != "reasoning":
                    print(f"\n[Seq #{seq:02d}] [Thinking]: ", end="", flush=True)
                    current_stream_mode = "reasoning"
                print(event.get("delta", ""), end="", flush=True)

            # 3. Streaming response text tokens
            elif event_type == "assistant_text_delta":
                if current_stream_mode != "text":
                    print(f"\n\n[Seq #{seq:02d}] [Assistant]: ", end="", flush=True)
                    current_stream_mode = "text"
                print(event.get("delta", ""), end="", flush=True)

            # 4. Model responded step summary
            elif event_type == "model_responded":
                if current_stream_mode is not None:
                    print("\n", flush=True)
                    current_stream_mode = None

                usage = event.get("usage")
                if usage:
                    inp = usage.get("input_tokens", usage.get("prompt_tokens", 0))
                    cached = usage.get(
                        "cached_input_tokens", usage.get("cache_read_tokens", 0)
                    )
                    out = usage.get("output_tokens", usage.get("completion_tokens", 0))
                    tot = usage.get("total_tokens", inp + out)
                    print(
                        f"[Seq #{seq:02d}] [Token Usage]: "
                        f"input={inp}, cached={cached}, output={out}, total={tot}",
                        flush=True,
                    )
                tool_calls = event.get("tool_calls", [])
                if tool_calls:
                    names = [t.get("name") for t in tool_calls]
                    print(f"[Seq #{seq:02d}] [Tools Proposed]: {names}", flush=True)

            # 5. Tool invocation started
            elif event_type == "tool_started":
                if current_stream_mode is not None:
                    print("\n", flush=True)
                    current_stream_mode = None
                call = event.get("call", {})
                args = call.get("arguments", "")
                args_str = (
                    json.dumps(args, ensure_ascii=False)
                    if isinstance(args, (dict, list))
                    else str(args)
                )
                if len(args_str) > 100:
                    args_str = args_str[:100] + "..."
                print(
                    f"[Seq #{seq:02d}] [Tool Started]: {call.get('name')}({args_str})",
                    flush=True,
                )

            # 6. Tool finished
            elif event_type == "tool_finished":
                if current_stream_mode is not None:
                    print("\n", flush=True)
                    current_stream_mode = None
                typed_event = item["typed_event"]
                if isinstance(typed_event, ToolFinishedEvent):
                    tool_item = next(
                        (
                            candidate
                            for candidate in item.get("typed_items", [])
                            if candidate.id == typed_event.call_id
                        ),
                        None,
                    )
                    lifecycle = tool_item.status if tool_item else "unknown"
                    outcome = typed_event.outcome or "unknown"
                    truncated = " (truncated)" if typed_event.truncated else ""
                    print(
                        f"[Seq #{seq:02d}] [Tool Finished]: "
                        f"{typed_event.name} status={lifecycle} "
                        f"outcome={outcome}{truncated}",
                        flush=True,
                    )
                    preview = typed_event.content[:150].replace("\n", " ")
                    if preview:
                        print(f"       Output Preview: {preview}...", flush=True)

            # 8. Bounded context compaction lifecycle
            elif event_type == "context_compaction_started":
                print(
                    f"[Seq #{seq:02d}] [Compaction Started]: "
                    f"before={event.get('before_bytes', 0)} bytes",
                    flush=True,
                )

            elif event_type == "context_compaction_finished":
                print(
                    f"[Seq #{seq:02d}] [Compaction Finished]: "
                    f"{event.get('before_bytes', 0)} -> {event.get('after_bytes', 0)} bytes",
                    flush=True,
                )

            # 9. Run-level stop outcome (turn_finished follows it)
            elif event_type == "run_finished":
                print(
                    f"[Seq #{seq:02d}] [Run Finished]: "
                    f"reason={event.get('stop_reason')}, steps={event.get('steps', 0)}",
                    flush=True,
                )

            # 7. Turn finished or failed
            elif event_type == "turn_finished":
                if current_stream_mode is not None:
                    print("\n", flush=True)
                    current_stream_mode = None
                print(
                    f"\n[Seq #{seq:02d}] [Turn Finished]: status={event.get('status')}",
                    flush=True,
                )

            elif event_type == "run_failed":
                if current_stream_mode is not None:
                    print("\n", flush=True)
                    current_stream_mode = None
                print(
                    f"\n[Seq #{seq:02d}] [Run Failed]: reason={event.get('reason')}",
                    flush=True,
                )


if __name__ == "__main__":
    asyncio.run(main())
