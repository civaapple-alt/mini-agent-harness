"""
Demo 01: Basic Turn Execution
Demonstrates connecting to mini-agent-app-server, initializing, and executing a single turn.
"""

import asyncio

from mini_agent import MiniAgentClient, ToolFinishedEvent


async def main():
    print("=== Demo 01: Basic Turn Execution ===", flush=True)

    # 1. Start client with detailed logging enabled into logs/ directory
    async with MiniAgentClient(log_dir="logs") as client:
        # 2. Negotiate protocol and initialize
        await client.initialize(client_name="python-basic-demo")
        print("[OK] App Server initialized", flush=True)

        # 3. Start default execution thread
        thread_id = await client.start_thread()
        print(f"[OK] Thread started: {thread_id}", flush=True)

        # 4. Stream and process turn events
        prompt = "Hello! Please summarize what tools you have available in 2 concise sentences."
        print(f"\n[User Prompt]: {prompt}\n", flush=True)

        printed_reasoning_header = False
        printed_assistant_header = False

        async for item in client.stream_turn(prompt):
            if item["type"] == "_turn_submission":
                sub = item.get("data")
                turn_id = getattr(sub, "turn_id", None) or (
                    sub.get("turn_id") if isinstance(sub, dict) else "started"
                )
                print(f"[Turn Started] Turn ID: {turn_id}", flush=True)

            elif item["type"] == "event":
                event = item["event"]
                event_type = event.get("type")

                # Stream reasoning tokens
                if event_type == "assistant_reasoning_delta":
                    if not printed_reasoning_header:
                        print("\n[Thinking]:\n", end="", flush=True)
                        printed_reasoning_header = True
                    print(event.get("delta", ""), end="", flush=True)

                # Stream assistant text tokens
                elif event_type == "assistant_text_delta":
                    if not printed_assistant_header:
                        print("\n\n[Assistant Response]:\n", end="", flush=True)
                        printed_assistant_header = True
                    print(event.get("delta", ""), end="", flush=True)

                # Tool started
                elif event_type == "tool_started":
                    call = event.get("call", {})
                    print(
                        f"\n -> [Tool Calling]: {call.get('name')}({call.get('arguments')})",
                        flush=True,
                    )

                # Tool finished
                elif event_type == "tool_finished":
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
                        print(
                            f" <- [Tool Finished] name={typed_event.name} "
                            f"status={lifecycle} outcome={outcome}",
                            flush=True,
                        )

                # Turn completed
                elif event_type == "turn_finished":
                    status = event.get("status")
                    print(f"\n\n[Turn Finished Status]: {status}", flush=True)

                # Run failed error
                elif event_type == "run_failed":
                    reason = event.get("reason", "unknown error")
                    print(f"\n\n[Run Failed Reason]: {reason}", flush=True)


if __name__ == "__main__":
    asyncio.run(main())
