"""
Demo 05: Management, Thread Settings, Goals, and Checkpoints
Demonstrates querying WorldState, thread lifecycle & forking, updating the
Codex-shaped collaboration mode, and managing a Thread-owned Goal.
"""

import asyncio

from mini_agent import AppServerError, MiniAgentClient


async def main():
    print("=== Demo 05: Workflows and Management APIs ===", flush=True)

    goal_turn_id: str | None = None
    goal_turn_started = asyncio.Event()

    async def observe_notification(notification: dict) -> None:
        nonlocal goal_turn_id
        if notification.get("method") != "thread/goal/updated":
            return
        data = notification.get("data", {})
        turn_id = data.get("turnId")
        if data.get("threadId") == "default" and turn_id:
            goal_turn_id = str(turn_id)
            goal_turn_started.set()

    async with MiniAgentClient(
        log_dir="logs", notification_handler=observe_notification
    ) as client:

        async def clear_goal_runtime(label: str) -> bool:
            nonlocal goal_turn_id
            current = await client.get_goal(thread_id="default")
            if current.goal is None:
                return False

            if current.goal.status == "active":
                if goal_turn_id is None:
                    try:
                        await asyncio.wait_for(goal_turn_started.wait(), timeout=10)
                    except asyncio.TimeoutError:
                        pass
                if goal_turn_id is not None:
                    try:
                        await client.interrupt_turn(goal_turn_id, thread_id="default")
                        await client.wait_for_turn(
                            goal_turn_id, timeout=30, poll_interval=0.2
                        )
                    except AppServerError as err:
                        message = str(err).lower()
                        if (
                            "no active turn" not in message
                            and "not active" not in message
                        ):
                            raise

            cleared = await client.clear_goal(thread_id="default")
            goal_turn_id = None
            goal_turn_started.clear()
            print(f"[{label}] Goal Cleared: {cleared.cleared}", flush=True)
            return cleared.cleared

        # 1. Initialize
        init_res = await client.initialize()
        print(
            f"[OK] Initialized, server: {init_res.get('serverName')} v{init_res.get('serverVersion')}",
            flush=True,
        )
        runtime_thread_id = await client.start_thread()
        await clear_goal_runtime("Startup cleanup")
        await client.start_thread("main-thread")

        # 2. Thread Lifecycle & Branching
        print("\n--- 1. Thread Lifecycle & Branching ---", flush=True)
        thread_list = await client.list_threads()
        print(f"Active Threads: {thread_list.data}", flush=True)

        fork_res = await client.fork_thread("main-thread", "experiment-branch")
        print(f"Forked Thread : {fork_res.thread_id}", flush=True)

        checkpoint = await client.read_thread("main-thread")
        print(
            f"Main Status   : {checkpoint.status}, Next Turn: {checkpoint.next_turn_number}",
            flush=True,
        )

        closed = await client.close_thread("experiment-branch")
        print(f"Closed Branch : {closed}", flush=True)

        # 3. Inspect Environment & World State
        print("\n--- 2. World State & Governance ---", flush=True)
        try:
            world = await client.get_world_state()
            print(
                f"OS/Arch Shell : {world.status.get('os')} {world.status.get('arch')} ({world.status.get('shell')})",
                flush=True,
            )
            print(
                f"Commands Avail: {len(world.status.get('available_commands', []))} tools detected",
                flush=True,
            )
        except AppServerError as err:
            print(f"(world/state error: {err})", flush=True)

        print("\n--- 2a. Independent Execution Controls ---", flush=True)
        try:
            execution = await client.set_world_execution(
                access="project", policy="interactive"
            )
            print(
                f"Execution Access: project, Policy: interactive, "
                f"Changed: {execution.changed}",
                flush=True,
            )
            print(
                "Trusted is a policy choice, not allow-all; high-risk actions "
                "still require approval.",
                flush=True,
            )
        except AppServerError as err:
            print(f"(world/set_execution error: {err})", flush=True)

        # 4. Inspect MCP Servers status
        print("\n--- 3. MCP Status ---", flush=True)
        try:
            mcp = await client.get_mcp_status()
            print(f"Enabled Servers : {mcp.enabled_servers}", flush=True)
            print(f"Available Tools : {mcp.tool_count}", flush=True)
        except AppServerError as err:
            print(f"(mcp/status error: {err})", flush=True)

        # Thread lifecycle management can inspect other threads, but the
        # current App Server process exposes runtime settings and Goal APIs
        # only for its bound runtime thread. Reattach it explicitly here.
        await client.start_thread(runtime_thread_id)
        print(f"Runtime Thread : {runtime_thread_id}", flush=True)

        # 5. Update collaboration mode (locks workspace mutations)
        print("\n--- 4. Enabling Plan Mode ---", flush=True)
        try:
            settings = await client.set_collaboration_mode("plan")
            print(f"Collaboration Mode: {settings.collaboration_mode.mode}", flush=True)
        except AppServerError as err:
            print(f"(thread/settings/update error: {err})", flush=True)

        # 6. Thread-owned Goal Runtime
        print("\n--- 5. Thread Goal Runtime ---", flush=True)
        try:
            goal_turn_id = None
            goal_turn_started.clear()
            goal_res = await client.set_goal(
                objective="Implement High-Performance Caching Layer",
                token_budget=4096,
            )
            goal = goal_res.goal
            print(f"Goal Thread   : {goal.thread_id}", flush=True)
            print(f"Goal Status   : {goal.status}", flush=True)
            print(
                f"Token Budget  : {goal.token_budget or 'unlimited'}",
                flush=True,
            )

            current = await client.get_goal()
            print(
                f"Goal Readback : {current.goal.status if current.goal else 'cleared'}",
                flush=True,
            )
        except AppServerError as err:
            print(f"(thread/goal error: {err})", flush=True)
        finally:
            await clear_goal_runtime("Shutdown cleanup")

        print("\n=== Demo 05 Completed Successfully ===", flush=True)


if __name__ == "__main__":
    asyncio.run(main())
