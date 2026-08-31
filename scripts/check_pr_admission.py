"""Check that a pull request contains completed change-admission answers."""

import os
import re
import sys


_QUESTION = re.compile(r"(?m)^\s*([1-6])\.\s+\*\*[^\n]*\*\*.*$")
_CHECKBOX = re.compile(r"(?im)^\s*-\s*\[([ x])\]\s+")
_PLACEHOLDER = "<!-- answer here -->"
_REQUIRED_SECTIONS = ("## 变更准入检查", "### 六项必答题", "### 准入确认")


def validate_body(body: str) -> list[str]:
    errors = []
    if not body.strip():
        return ["pull request body is empty"]

    for section in _REQUIRED_SECTIONS:
        if section not in body:
            errors.append(f"missing required section: {section}")

    if _PLACEHOLDER in body:
        errors.append("replace every '<!-- answer here -->' placeholder with an answer")

    questions = list(_QUESTION.finditer(body))
    numbers = [int(match.group(1)) for match in questions]
    if numbers != list(range(1, 7)):
        errors.append("six numbered admission questions (1 through 6) are required")
    else:
        confirmation_start = body.find("### 准入确认")
        for index, question in enumerate(questions):
            end = (
                questions[index + 1].start()
                if index < 5
                else confirmation_start if confirmation_start >= 0 else len(body)
            )
            answer = body[question.end() : end].strip()
            if not answer:
                errors.append(f"question {index + 1} has no answer")

    confirmation_start = body.find("### 准入确认")
    if confirmation_start >= 0:
        confirmation = body[confirmation_start + len("### 准入确认") :]
        next_heading = re.search(r"(?m)^#{1,3}\s+", confirmation)
        if next_heading:
            confirmation = confirmation[: next_heading.start()]
    else:
        confirmation = ""
    checkboxes = _CHECKBOX.findall(confirmation)
    if len(checkboxes) < 6 or any(mark.lower() != "x" for mark in checkboxes[:6]):
        errors.append("check all six admission confirmation boxes")
    return errors


def main() -> int:
    errors = validate_body(os.environ.get("PR_BODY", ""))
    if not errors:
        print("PR admission template is complete")
        return 0
    for error in errors:
        print(f"PR admission check: {error}", file=sys.stderr)
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
