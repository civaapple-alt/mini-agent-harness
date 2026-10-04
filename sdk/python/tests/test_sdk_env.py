"""Environment discovery contract for the Python SDK."""

from __future__ import annotations

from pathlib import Path

from mini_agent.client import _env_search_dirs, _find_and_load_env


def test_sdk_searches_only_workspace_and_user_config() -> None:
    search_dirs = [
        Path(directory).resolve()
        for directory in _env_search_dirs("D:/external/project")
    ]
    user_config = Path.home() / ".mini-agent"

    assert search_dirs == [Path("D:/external/project").resolve(), user_config]


def test_sdk_env_precedence_matches_runtime_config_files(tmp_path, monkeypatch) -> None:
    workspace = tmp_path / "workspace"
    workspace.mkdir()
    parent = tmp_path
    user_config = tmp_path / "user" / ".mini-agent"
    user_config.mkdir(parents=True)
    monkeypatch.setattr(
        "mini_agent.client.os.path.expanduser",
        lambda path: str(user_config) if path == "~/.mini-agent" else path,
    )
    (parent / ".env").write_text("SHOULD_NOT_LOAD=parent\n", encoding="utf-8")
    (workspace / ".env").write_text("VALUE=workspace\n", encoding="utf-8")
    (user_config / ".env").write_text(
        "VALUE=user\nUSER_ONLY=loaded\n", encoding="utf-8"
    )

    loaded = _find_and_load_env(str(workspace))

    assert loaded == {"VALUE": "workspace", "USER_ONLY": "loaded"}
