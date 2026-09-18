#!/usr/bin/env python3
"""Check local Markdown links in selected files and directories."""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path


LINK = re.compile(r"!?\[[^]]*\]\((?:<)?([^)> ]+)(?:>)?\)")
SKIP_PREFIXES = ("#", "http:", "https:", "mailto:", "codex:", "app:")
SKIP_DIRS = {".git", "node_modules", "dist", "target", ".venv", "__pycache__"}


def markdown_files(path: Path) -> list[Path]:
    if path.is_file():
        return [path] if path.suffix == ".md" else []
    return [
        candidate
        for candidate in path.rglob("*.md")
        if not any(part in SKIP_DIRS for part in candidate.parts)
    ]


def check_file(path: Path) -> list[str]:
    errors: list[str] = []
    for line_number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        for raw_target in LINK.findall(line):
            target = raw_target.split("#", 1)[0]
            if not target or target.startswith(SKIP_PREFIXES):
                continue
            if not (path.parent / target).resolve().exists():
                errors.append(f"{path}:{line_number}: missing local link target {raw_target}")
    return errors


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("paths", nargs="+", type=Path)
    args = parser.parse_args()
    errors = [error for path in args.paths for file in markdown_files(path) for error in check_file(file)]
    if errors:
        print("\n".join(errors))
        return 1
    print("docs-links: PASS")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
