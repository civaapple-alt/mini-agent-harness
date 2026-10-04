"""
Demo 04: Steering and Cooperative Interruption
Demonstrates how to steer an active turn with corrective guidance
and cooperatively cancel/interrupt a long-running execution.
"""

import asyncio

from mini_agent import MiniAgentClient
from mini_agent.errors import AppServerError, ServerProcessError


async def report_disconnect(client: MiniAgentClient, thread_id: str) -> None:
    """Show the authoritative reads to use after the stream reports EOF."""
    print(
        "[App Server EOF] Stream ended without proving that the Turn completed.",
        flush=True,
    )
    try:
        runtime = await client.get_runtime_status(thread_id=thread_id)
        items = await client.list_thread_items(thread_id=thread_id, limit=128)
        checkpoint = await client.read_thread(thread_id=thread_id)
        print(
            f"[Recovery Projection] phase={runtime.phase} "
            f"turn={runtime.turn_id} items={len(items.data)} "
            f"checkpoint={checkpoint.status}",
            flush=True,
        )
    except (AppServerError, ServerProcessError) as recovery_error:
        print(f"[Recovery Unavailable] {recovery_error}", flush=True)


async def main():
    print("=== Demo 04: Steering and Cooperative Interruption ===", flush=True)

    async with MiniAgentClient(log_dir="logs") as client:
        await client.initialize()
        await client.start_thread()

        # ---------------------------------------------------------------------
        # Part 1: Mid-Turn Steering
        # ---------------------------------------------------------------------
        prompt1 = (
            "Write a comprehensive 1000-word tutorial on Python asyncio concurrency."
        )
        print("\n--- Part 1: Steering Active Turn ---")
        print(f"[Initial Prompt]: {prompt1}\n", flush=True)

        stream1 = client.stream_turn(prompt1)
        submission1 = await anext(stream1)
        turn_id1 = submission1["submission"].turn_id or "turn-1"
        print(f"[Turn 1 Started]: {turn_id1}", flush=True)

        # Inject runtime steering instruction
        steer_text = "Stop writing the full tutorial. Just give me a 3-bullet-point executive summary instead."
        print(f"\n[Steering Instruction Injected]: {steer_text}", flush=True)
        try:
            steer_resp = await client.steer_turn(turn_id1, steer_text)
            print(
                f"[Steer Acknowledged]: actionId={steer_resp.get('actionId')}\n",
                flush=True,
            )
        except ServerProcessError:
            await report_disconnect(client, "default")
        except AppServerError as err:
            if "no active turn" not in str(err).lower():
                raise
            print("[Steer Window Closed]: Turn settled before steering arrived.\n")

        # Wait until turn settles and read result
        print("[Waiting for steered turn to settle...]", flush=True)
        try:
            async for _ in stream1:
                pass
            result1 = await client.wait_for_turn(turn_id1)
        except ServerProcessError:
            await report_disconnect(client, "default")
        else:
            print("[Turn 1 Settled]:", flush=True)
            print(f"Status     : {result1.status}", flush=True)
            print(f"Stop Reason: {result1.stop_reason}", flush=True)
            if result1.final_text:
                print(
                    f"\n[Output Preview]:\n{result1.final_text[:300]}...\n",
                    flush=True,
                )

        # ---------------------------------------------------------------------
        # Part 2: Cooperative Turn Interruption (Cancel)
        # ---------------------------------------------------------------------
        prompt2 = "Count from 1 to 1000 and write a paragraph explaining prime factorization for each."
        print("\n--- Part 2: Cooperative Turn Interruption ---")
        print(f"[Initial Prompt]: {prompt2}\n", flush=True)

        stream2 = client.stream_turn(prompt2)
        submission2 = await anext(stream2)
        turn_id2 = submission2["submission"].turn_id or "turn-2"
        print(f"[Turn 2 Started]: {turn_id2}", flush=True)

        print("[Interrupting Turn 2...]", flush=True)
        try:
            await client.interrupt_turn(turn_id2)
            print("[Interrupt Acknowledged]", flush=True)
        except ServerProcessError:
            await report_disconnect(client, "default")
        except AppServerError as err:
            if "no active turn" not in str(err).lower():
                raise
            print(
                "[Interrupt Window Closed]: Turn settled before interruption arrived."
            )

        print("[Waiting for turn cancellation checkpoint...]", flush=True)
        try:
            async for _ in stream2:
                pass
            result2 = await client.wait_for_turn(turn_id2)
        except ServerProcessError:
            await report_disconnect(client, "default")
        else:
            print("[Turn 2 Settled]:", flush=True)
            print(f"Status     : {result2.status}", flush=True)
            print(f"Stop Reason: {result2.stop_reason}", flush=True)
        print("\n=== Demo 04 Completed Successfully ===", flush=True)


if __name__ == "__main__":
    asyncio.run(main())
