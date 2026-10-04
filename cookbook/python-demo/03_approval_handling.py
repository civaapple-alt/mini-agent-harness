"""
Demo 03: Sensitive Tool Approvals
Demonstrates intercepting approval/request notifications when the agent
attempts sensitive actions (e.g., executing shell commands or writing files),
and programmatically or interactively prompting the user.
"""

import asyncio

from mini_agent import MiniAgentClient, ToolFinishedEvent


async def interactive_approval_handler(request: dict) -> dict:
    """
    Custom approval callback.
    Can be connected to a GUI modal, CLI prompt, or security policy engine.
    """
    print("\n" + "=" * 60, flush=True)
    print("[SECURITY APPROVAL REQUIRED]", flush=True)
    print(f"Request ID : {request['requestId']}", flush=True)
    print(f"Action     : {request['actionSummary']}", flush=True)
    print("=" * 60, flush=True)

    loop = asyncio.get_running_loop()
    user_choice = await loop.run_in_executor(
        None,
        lambda: input("Allow agent to execute this action? (y/N): ").strip().lower(),
    )
    decision = "approve" if user_choice in ("y", "yes") else "deny"
    print(f"-> Decision: {decision.upper()}\n", flush=True)
    return {
        "decision": decision,
        "grantScope": "once" if decision == "approve" else None,
    }


async def main():
    print("=== Demo 03: Sensitive Tool Approvals ===", flush=True)

    # Initialize client with our custom approval handler and log_dir='logs'
    async with MiniAgentClient(
        approval_handler=interactive_approval_handler,
        log_dir="logs",
    ) as client:
        await client.initialize()
        await client.start_thread()

        prompt = (
            "Please run a shell command to check current git status and list files."
        )
        print(f"[Prompt]: {prompt}\n", flush=True)

        async for item in client.stream_turn(prompt):
            if item["type"] == "event":
                event = item["event"]
                event_type = event.get("type")

                if event_type == "assistant_text_delta":
                    print(event.get("delta", ""), end="", flush=True)

                elif event_type == "tool_started":
                    call = event.get("call", {})
                    print(
                        f"\n[Tool Started]: {call.get('name')}({call.get('arguments')})",
                        flush=True,
                    )

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
                            f"[Tool Finished]: {typed_event.name} "
                            f"status={lifecycle} outcome={outcome}",
                            flush=True,
                        )

                elif event_type == "turn_finished":
                    print(
                        f"\n\n[Turn Finished Status]: {event.get('status')}", flush=True
                    )


if __name__ == "__main__":
    asyncio.run(main())
