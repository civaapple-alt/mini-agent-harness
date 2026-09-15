"""Check that an implemented iteration note contains replayable evidence."""

from __future__ import annotations

import argparse
import sys
from pathlib import Path


REQUIRED_MARKERS = (
    "状态：implemented",
    "## Harness hypothesis",
    "## Ownership and boundaries",
    "## Cross-repository contract",
    "## Verification",
    "## Remaining risks",
)


def validate_note(text: str) -> list[str]:
    """Return missing evidence sections for one implemented note."""
    if not text.strip():
        return ["note is empty"]
    return [f"missing required marker: {marker}" for marker in REQUIRED_MARKERS if marker not in text]


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("note", type=Path)
    args = parser.parse_args()
    try:
        text = args.note.read_text(encoding="utf-8")
    except OSError as error:
        print(f"iteration note check: cannot read {args.note}: {error}", file=sys.stderr)
        return 2
    errors = validate_note(text)
    if errors:
        for error in errors:
            print(f"iteration note check: {error}", file=sys.stderr)
        return 1
    print(f"iteration note is complete: {args.note}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
