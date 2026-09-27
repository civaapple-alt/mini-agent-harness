"""Run the Session Doctor maintenance commands against an isolated fixture."""

from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path
from urllib.parse import quote


def _record(seq: int, kind: str, **fields: object) -> bytes:
    return (json.dumps({"seq": seq, "kind": kind, **fields}) + "\n").encode()


def _session_store(home: Path, workspace: Path) -> Path:
    workspace_key = quote(
        str(workspace.resolve()),
        safe="-_.abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789",
    )
    return home / ".mini-agent" / "sessions" / workspace_key


def _make_session(
    base: Path, session_id: str, suffix: bytes = b""
) -> tuple[Path, bytes]:
    session_dir = base / session_id
    session_dir.mkdir(parents=True)
    prefix = b"".join(
        [
            _record(
                1,
                "session_created",
                session_id=session_id,
                schema_version=1,
            ),
            _record(2, "checkpoint", thread_id="default", messages=[]),
        ]
    )
    path = session_dir / "session.jsonl"
    path.write_bytes(prefix + suffix)
    return path, prefix


def _run(
    app_server: Path, workspace: Path, home: Path, args: list[str]
) -> subprocess.CompletedProcess[str]:
    env = os.environ.copy()
    env["HOME"] = str(home)
    env["USERPROFILE"] = str(home)
    env["MINI_AGENT_SESSION_MODE"] = "doctor-scenario-invalid"
    for key in (
        "OPENAI_API_KEY",
        "OPENAI_MODEL",
        "OPENAI_BASE_URL",
        "VERIFIER_OPENAI_API_KEY",
        "VERIFIER_OPENAI_MODEL",
        "VERIFIER_OPENAI_BASE_URL",
    ):
        env.pop(key, None)
    return subprocess.run(
        [str(app_server), *args],
        cwd=workspace,
        env=env,
        stdin=subprocess.DEVNULL,
        capture_output=True,
        text=True,
        timeout=15,
        check=False,
    )


def _report(app_server: Path, workspace: Path, home: Path) -> dict[str, object]:
    result = _run(app_server, workspace, home, ["doctor", "--json"])
    if result.returncode != 0:
        raise RuntimeError("doctor scan command failed")
    payload = json.loads(result.stdout)
    if not isinstance(payload, dict) or payload.get("schema_version") != 1:
        raise RuntimeError("doctor scan returned an unsupported report")
    return payload


def run(app_server: Path) -> None:
    if not app_server.is_file():
        raise FileNotFoundError(f"App Server binary does not exist: {app_server}")
    with tempfile.TemporaryDirectory(prefix="mini-agent-session-doctor-") as temporary:
        root = Path(temporary)
        home = root / "home"
        workspace = root / "workspace"
        home.mkdir()
        workspace.mkdir()
        base = _session_store(home, workspace)
        tail_path, valid_prefix = _make_session(
            base,
            "scenario-tail",
            b'{"seq":3,"kind":"item"',
        )
        original_tail = tail_path.read_bytes()
        gap_path, _ = _make_session(base, "scenario-gap", _record(4, "future_kind"))
        original_gap = gap_path.read_bytes()
        _make_session(base, "scenario-locked")
        (base / "scenario-locked.lock").write_text("pid=1\n", encoding="utf-8")

        report = _report(app_server, workspace, home)
        findings = {finding["session_id"]: finding for finding in report["findings"]}
        if findings["scenario-tail"]["repair_available"] is not True:
            raise AssertionError("incomplete final record was not offered for repair")
        if findings["scenario-gap"]["issue_code"] != "sequence_gap":
            raise AssertionError("sequence gap was not reported")
        if findings["scenario-locked"]["inspection"] != "locked_unverified":
            raise AssertionError("locked Session was treated as inspected")

        repair = _run(
            app_server,
            workspace,
            home,
            ["doctor", "repair", "--session-id", "scenario-tail", "--json"],
        )
        if repair.returncode != 0:
            raise RuntimeError("doctor tail repair command failed")
        repaired = json.loads(repair.stdout)
        if (
            not isinstance(repaired, dict)
            or repaired.get("session_id") != "scenario-tail"
        ):
            raise RuntimeError("doctor repair returned an unsupported result")
        backup_path = repaired.get("backup_path")
        if not isinstance(backup_path, str) or Path(backup_path).is_absolute():
            raise AssertionError("repair did not return a relative backup path")
        if (home / ".mini-agent" / backup_path).read_bytes() != original_tail:
            raise AssertionError("backup does not match the original Session log")
        if tail_path.read_bytes() != valid_prefix:
            raise AssertionError("repair changed bytes before the incomplete tail")

        for session_id, unchanged_path, original in [
            ("scenario-gap", gap_path, original_gap),
            ("scenario-locked", base / "scenario-locked" / "session.jsonl", None),
        ]:
            rejected = _run(
                app_server,
                workspace,
                home,
                ["doctor", "repair", "--session-id", session_id, "--json"],
            )
            if rejected.returncode == 0:
                raise AssertionError(f"doctor repaired ineligible Session {session_id}")
            if original is not None and unchanged_path.read_bytes() != original:
                raise AssertionError(f"rejected repair changed Session {session_id}")

        repaired_report = _report(app_server, workspace, home)
        final_tail = next(
            finding
            for finding in repaired_report["findings"]
            if finding["session_id"] == "scenario-tail"
        )
        if final_tail["incomplete_tail"] or final_tail["repair_available"]:
            raise AssertionError("repaired Session still reports an incomplete tail")
        print(
            "Session Doctor scenario passed: scan, backup, safe repair, and rejection paths."
        )


if __name__ == "__main__":
    default_binary = Path("target/debug/mini-agent-app-server")
    if os.name == "nt":
        default_binary = default_binary.with_suffix(".exe")
    binary = Path(sys.argv[1]) if len(sys.argv) > 1 else default_binary
    run(binary.resolve())
