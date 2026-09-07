#!/usr/bin/env python3
"""Check the workspace Cargo dependency direction.

This is intentionally a small, explicit boundary check. It does not infer
architecture from crate names and it does not rewrite Cargo manifests. A new
workspace edge must be reviewed and added to the allowlist deliberately.
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
from pathlib import Path
from typing import Any


ROOT = Path(__file__).resolve().parents[1]

# These are the currently admitted workspace edges. External dependencies are
# ignored; this check is about ownership between the harness crates.
ALLOWED_DEPENDENCIES: dict[str, frozenset[str]] = {
    "mini-agent-protocol": frozenset(),
    "mini-agent-core": frozenset({"mini-agent-protocol"}),
    "mini-agent-capabilities": frozenset({"mini-agent-core", "mini-agent-protocol"}),
    "mini-agent-host": frozenset(
        {"mini-agent-capabilities", "mini-agent-core", "mini-agent-protocol"}
    ),
    "mini-agent-app-server-protocol": frozenset({"mini-agent-protocol"}),
    "mini-agent-app-server": frozenset(
        {
            "mini-agent-app-server-protocol",
            "mini-agent-capabilities",
            "mini-agent-core",
            "mini-agent-host",
            "mini-agent-protocol",
        }
    ),
    "mini-agent-cli": frozenset({"mini-agent-app-server", "mini-agent-protocol"}),
}

# Existing edges that are admitted for now but should remain visible during
# ownership review. They are warnings, not a second authority.
REVIEW_EDGES: dict[tuple[str, str], str] = {
    (
        "mini-agent-app-server",
        "mini-agent-capabilities",
    ): "App Server directly consumes provider/control-plane APIs; keep this edge under review before extracting or moving ownership.",
}


def workspace_dependency_graph(metadata: dict[str, Any]) -> dict[str, set[str]]:
    """Return only path dependencies between workspace packages."""

    packages = metadata.get("packages", [])
    workspace_names = {package["name"] for package in packages}
    graph = {name: set() for name in workspace_names}

    for package in packages:
        package_name = package["name"]
        for dependency in package.get("dependencies", []):
            dependency_name = dependency["name"]
            if dependency.get("source") is None and dependency_name in workspace_names:
                graph[package_name].add(dependency_name)

    return graph


def violations(graph: dict[str, set[str]]) -> list[str]:
    """Return unexpected packages or workspace edges in stable order."""

    problems: list[str] = []
    unknown_packages = sorted(set(graph) - set(ALLOWED_DEPENDENCIES))
    problems.extend(f"unconfigured workspace package: {name}" for name in unknown_packages)

    for package_name in sorted(graph):
        allowed = ALLOWED_DEPENDENCIES.get(package_name, frozenset())
        for dependency_name in sorted(graph[package_name] - allowed):
            problems.append(f"unexpected workspace dependency: {package_name} -> {dependency_name}")

    return problems


def review_edges(graph: dict[str, set[str]]) -> list[dict[str, str]]:
    findings = []
    for (package_name, dependency_name), reason in sorted(REVIEW_EDGES.items()):
        if dependency_name in graph.get(package_name, set()):
            findings.append(
                {
                    "from": package_name,
                    "to": dependency_name,
                    "reason": reason,
                }
            )
    return findings


def load_metadata(metadata_file: Path | None) -> dict[str, Any]:
    if metadata_file is not None:
        return json.loads(metadata_file.read_text(encoding="utf-8"))

    completed = subprocess.run(
        ["cargo", "metadata", "--no-deps", "--format-version", "1"],
        cwd=ROOT,
        check=False,
        capture_output=True,
        text=True,
    )
    if completed.returncode != 0:
        raise RuntimeError(completed.stderr.strip() or "cargo metadata failed")
    return json.loads(completed.stdout)


def report(metadata: dict[str, Any]) -> dict[str, Any]:
    graph = workspace_dependency_graph(metadata)
    problems = violations(graph)
    findings = review_edges(graph)
    return {
        "status": "fail" if problems else "pass",
        "workspace_dependencies": {
            package_name: sorted(dependencies)
            for package_name, dependencies in sorted(graph.items())
        },
        "review_edges": findings,
        "violations": problems,
    }


def print_text(result: dict[str, Any]) -> None:
    for package_name, dependencies in result["workspace_dependencies"].items():
        rendered = ", ".join(dependencies) if dependencies else "(none)"
        print(f"{package_name}: {rendered}")

    for finding in result["review_edges"]:
        print(f"review: {finding['from']} -> {finding['to']}: {finding['reason']}")

    if result["violations"]:
        for problem in result["violations"]:
            print(f"violation: {problem}", file=sys.stderr)
    else:
        print("Cargo boundary: pass")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--metadata-file",
        type=Path,
        help="read a cargo metadata JSON document instead of invoking Cargo",
    )
    parser.add_argument("--json", action="store_true", help="emit a machine-readable report")
    args = parser.parse_args()

    try:
        result = report(load_metadata(args.metadata_file))
    except (OSError, json.JSONDecodeError, RuntimeError) as error:
        print(f"cargo boundary check failed: {error}", file=sys.stderr)
        return 2

    if args.json:
        print(json.dumps(result, ensure_ascii=False, indent=2, sort_keys=True))
    else:
        print_text(result)
    return 1 if result["violations"] else 0


if __name__ == "__main__":
    raise SystemExit(main())
