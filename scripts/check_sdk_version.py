#!/usr/bin/env python3
"""Check that the published Python SDK matches the Harness release contract."""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def text(path: str) -> str:
    return (ROOT / path).read_text(encoding="utf-8")


def require_match(label: str, source: str, pattern: str) -> str:
    match = re.search(pattern, source, re.MULTILINE | re.DOTALL)
    if not match:
        raise ValueError(f"could not read {label}")
    return match.group(1)


def toml_version(path: str, section_name: str) -> str:
    section = require_match(
        f"[{section_name}] in {path}",
        text(path),
        rf"^\[{re.escape(section_name)}\]\s*(.*?)(?=^\[|\Z)",
    )
    return require_match("version", section, r'^version\s*=\s*"([^\"]+)"')


def main() -> int:
    try:
        sdk_init = text("sdk/python/src/mini_agent/__init__.py")
        client = text("sdk/python/src/mini_agent/client.py")
        rust_protocol = text("crates/mini-agent-app-server-protocol/src/lib.rs")
        readme = text("sdk/python/README.md")

        versions = {
            "Cargo workspace": toml_version("Cargo.toml", "workspace.package"),
            "SDK package": toml_version("sdk/python/pyproject.toml", "project"),
            "SDK __version__": require_match(
                "SDK __version__", sdk_init, r'^__version__\s*=\s*"([^\"]+)"'
            ),
            "SDK client identity": require_match(
                "SDK client identity",
                client,
                r'^\s*self\._client_version\s*=\s*"([^\"]+)"',
            ),
            "SDK initialize default": require_match(
                "SDK initialize default",
                client,
                r'^\s*client_version:\s*str\s*=\s*"([^\"]+)"',
            ),
        }
        protocols = {
            "Rust App Server": require_match(
                "Rust App Server protocol version",
                rust_protocol,
                r"pub const PROTOCOL_VERSION: u32 = (\d+);",
            ),
            "Python SDK": require_match(
                "Python SDK protocol version",
                client,
                r"^APP_SERVER_PROTOCOL_VERSION = (\d+)$",
            ),
            "SDK README": require_match(
                "SDK README protocol version", readme, r"当前协议版本为 `([0-9]+)`"
            ),
        }
    except (OSError, ValueError) as error:
        print(f"[ERROR] {error}", file=sys.stderr)
        return 1

    if len(set(versions.values())) != 1:
        print("[ERROR] SDK release version drift:", file=sys.stderr)
        for label, version in versions.items():
            print(f"  - {label}: {version}", file=sys.stderr)
        return 1
    if len(set(protocols.values())) != 1:
        print("[ERROR] App Server protocol version drift:", file=sys.stderr)
        for label, version in protocols.items():
            print(f"  - {label}: {version}", file=sys.stderr)
        return 1

    print(
        f"[OK] SDK release version {next(iter(versions.values()))}; "
        f"App Server protocol version {next(iter(protocols.values()))}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
