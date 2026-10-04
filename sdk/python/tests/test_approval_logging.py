"""Approval log records must not expose free-form tool arguments."""

import logging

import pytest
from mini_agent.approval_logging import approval_log_fields
from mini_agent.client import MiniAgentClient


def test_approval_log_fields_keep_only_patch_counts():
    assert approval_log_fields(
        {
            "toolName": "apply_patch",
            "actionSummary": "apply_patch · 新增 1 个文件 · 修改 2 个文件 · 删除 1 个文件",
        }
    ) == ("apply_patch", "新增=1 修改=2 删除=1")


def test_approval_log_fields_redact_shell_action():
    assert approval_log_fields(
        {
            "toolName": "shell",
            "actionSummary": "shell command `git reset --soft HEAD~1\ncat secret.txt`",
        }
    ) == ("shell", "redacted")


@pytest.mark.asyncio
async def test_sdk_approval_log_does_not_include_shell_command(caplog):
    client = MiniAgentClient()
    command = "shell command `git reset --soft HEAD~1\ncat secret.txt`"

    with caplog.at_level(logging.INFO, logger="mini_agent"):
        await client._publish_approval(
            {
                "requestId": "approval-1",
                "toolName": "shell",
                "actionSummary": command,
            },
            "requested",
        )

    assert "tool=shell" in caplog.text
    assert "summary=redacted" in caplog.text
    assert "request_id=approval-1" in caplog.text
    assert command not in caplog.text
