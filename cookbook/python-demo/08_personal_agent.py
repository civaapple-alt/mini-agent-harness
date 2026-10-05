"""Demonstrate SDK session interaction and recovery, not a domain personal assistant."""

from __future__ import annotations

import asyncio
import os

from mini_agent import (
    ExecutionRecoveryStatus,
    MiniAgentClient,
    TurnTimeoutError,
    UserQuestion,
    UserQuestionAnswer,
    UserQuestionInteraction,
    UserQuestionNotification,
)


async def read_line(prompt: str) -> str:
    return await asyncio.to_thread(input, prompt)


async def choose_answer(question: UserQuestion) -> UserQuestionAnswer:
    print(f"\nAgent asks: {question.prompt}")
    for index, option in enumerate(question.options, start=1):
        recommendation = " (recommended)" if option.recommended else ""
        print(f"  {index}. {option.label}{recommendation}")
        if option.description:
            print(f"     {option.description}")
        if option.recommendation_reason:
            print(f"     Reason: {option.recommendation_reason}")

    while True:
        value = await read_line("Answer: ")
        if question.allow_skip and value.strip().lower() == "skip":
            return UserQuestionAnswer.skipped()
        if value.isdigit():
            index = int(value) - 1
            if 0 <= index < len(question.options):
                return UserQuestionAnswer.for_option(question.options[index].id)
        if question.allow_free_text and value.strip():
            return UserQuestionAnswer.free_text(value)
        choices = f"1 to {len(question.options)}"
        if question.allow_free_text:
            choices += ", or enter text"
        if question.allow_skip:
            choices += ", or type 'skip'"
        print(f"Choose {choices}.")


async def answer_interaction(
    client: MiniAgentClient,
    interaction: UserQuestionInteraction,
    responding: set[tuple[str, int]],
) -> None:
    question = interaction.current_question
    if question is None:
        return
    if (
        interaction.current_index < len(interaction.answers)
        and interaction.answers[interaction.current_index] is not None
    ):
        return

    key = (interaction.interaction_id, interaction.current_index)
    if key in responding:
        return
    responding.add(key)
    try:
        answer = await choose_answer(question)
        await client.respond_user_question(
            interaction_id=interaction.interaction_id,
            thread_id=interaction.thread_id,
            turn_id=interaction.turn_id,
            call_id=interaction.call_id,
            question_id=question.id,
            answer=answer,
        )
    finally:
        responding.discard(key)


async def wait_for_settlement(client: MiniAgentClient, turn_id: str) -> None:
    while True:
        try:
            result = await client.wait_for_turn(turn_id, timeout=60)
            if result.final_text:
                print(f"\nAgent: {result.final_text}")
            print(f"[Recovered turn {result.status}]")
            return
        except TurnTimeoutError:
            print(
                "The earlier turn is still running; waiting before accepting a new prompt."
            )


async def main() -> None:
    responding: set[tuple[str, int]] = set()
    client: MiniAgentClient

    async def handle_notification(notification: dict) -> None:
        typed = notification.get("typed_user_question")
        if not isinstance(typed, UserQuestionNotification):
            return
        if typed.phase not in {"requested", "updated"}:
            return
        await answer_interaction(client, typed.interaction, responding)

    client = MiniAgentClient(
        env={
            "MINI_AGENT_SESSION_MODE": "named",
            "MINI_AGENT_SESSION_ID": os.environ.get(
                "MINI_AGENT_SESSION_ID", "personal-agent"
            ),
        },
        user_questions=True,
        notification_handler=handle_notification,
    )

    async with client:
        await client.initialize(client_name="python-personal-agent")
        thread_id = await client.start_thread()
        checkpoint = await client.read_thread(thread_id)
        session = await client.get_session_info()
        print(
            f"Connected to Session {session.session_id if session else '(disabled)'} "
            f"and Thread {thread_id}; stored messages: {len(checkpoint.messages)}."
        )

        pending = checkpoint.pending_user_question
        if pending is not None:
            interaction = UserQuestionInteraction.from_dict(pending)
            await answer_interaction(client, interaction, responding)
            await wait_for_settlement(client, interaction.turn_id)
        elif (
            checkpoint.execution_recovery is not None
            and checkpoint.execution_recovery.status != ExecutionRecoveryStatus.SETTLED
        ):
            recovery = checkpoint.execution_recovery
            print(
                "An earlier turn needs explicit recovery before new prompts: "
                f"status={recovery.status.value}, turn={recovery.turn_id}, "
                f"checkpoint={recovery.checkpoint_seq}."
            )
            return

        print("Type /quit to exit. App Server keeps the conversation in this Session.")
        while True:
            try:
                prompt = await read_line("\nYou: ")
            except EOFError:
                break
            if prompt.strip() == "/quit":
                break
            if not prompt.strip():
                continue

            async for envelope in client.stream_turn(prompt, thread_id=thread_id):
                if envelope.get("type") != "event":
                    continue
                event = envelope.get("event", {})
                event_type = event.get("type")
                if event_type == "assistant_text_delta":
                    print(event.get("delta", ""), end="", flush=True)
                elif event_type == "turn_finished":
                    print(f"\n[Turn {event.get('status')}]")
                elif event_type == "run_failed":
                    print(f"\n[Run failed: {event.get('reason')}]")


if __name__ == "__main__":
    asyncio.run(main())
