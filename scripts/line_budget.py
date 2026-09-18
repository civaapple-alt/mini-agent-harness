import argparse
import json
import re
import subprocess
import sys
from pathlib import Path
from pathlib import PurePosixPath


ROOT = Path(__file__).resolve().parents[1]
KERNEL_LIMIT = 6_000
# The release-source total includes production code and tests from the supported
# runtime packages. The experimental CLI/REPL is reported separately and is not
# part of this hard release gate.
PROJECT_LIMIT = 40_000
CONTROL_PLANE_LIMIT = 28_000

# The hard ceilings remain the emergency release boundary. The release-source
# operating limit leaves room for ordinary maintenance; the delta gate below
# permits bounded growth through the amber band and freezes positive growth in
# the red band. Runtime is reported for visibility but has no aggregate gate.
PROJECT_OPERATING_LIMIT = 39_000
PROJECT_RED_LIMIT = 39_500
PROJECT_NON_RED_DELTA_LIMIT = 300

# Keep the report aligned with the conceptual runtime layers. Capabilities are
# reported separately because they are provider implementations behind Host;
# protocol is reported separately so each external boundary stays visible.
CAPABILITY_PACKAGES = ("mini-agent-capabilities",)

LAYERS = (
    ("core", ("mini-agent-core",)),
    ("protocol", ("mini-agent-protocol",)),
    ("capabilities", CAPABILITY_PACKAGES),
    ("host", ("mini-agent-host",)),
    (
        "app-server",
        ("mini-agent-app-server", "mini-agent-app-server-protocol"),
    ),
    ("cli", ("mini-agent-cli",)),
)
RELEASE_PACKAGES = (
    "mini-agent-core",
    "mini-agent-protocol",
    "mini-agent-capabilities",
    "mini-agent-host",
    "mini-agent-app-server",
    "mini-agent-app-server-protocol",
)
RUNTIME_PACKAGES = (
    "mini-agent-core",
    "mini-agent-protocol",
    "mini-agent-host",
    "mini-agent-app-server",
    "mini-agent-app-server-protocol",
)
# Keep provider implementations outside the runtime gate even though they are
# included in the release-source total and shown as their own layer.
# The experimental CLI/REPL is intentionally outside both enforced totals.

KERNEL_PACKAGES = ("mini-agent-core", "mini-agent-protocol")
HOST_CONTROL_PLANE_PACKAGES = (
    "mini-agent-host",
    "mini-agent-app-server",
    "mini-agent-app-server-protocol",
)

# These are the Capabilities files whose responsibility is a stable control
# boundary rather than a concrete provider. The list is intentionally explicit
# and disjoint from the provider bucket so a file cannot be counted twice.
CAPABILITY_CONTROL_PLANE_PATHS = frozenset(
    {
        "crates/mini-agent-capabilities/src/path_policy.rs",
        "crates/mini-agent-capabilities/src/result_store.rs",
        "crates/mini-agent-capabilities/src/sandbox.rs",
        "crates/mini-agent-capabilities/src/security.rs",
        "crates/mini-agent-capabilities/src/session.rs",
        "crates/mini-agent-capabilities/src/session/storage.rs",
        "crates/mini-agent-capabilities/src/workspace.rs",
        "crates/mini-agent-capabilities/src/workspace/approval.rs",
        "crates/mini-agent-capabilities/src/workspace/files.rs",
        "crates/mini-agent-capabilities/src/workspace/patch.rs",
        "crates/mini-agent-capabilities/src/workspace/shell.rs",
        "crates/mini-agent-capabilities/src/workspace_tests.rs",
    }
)

CATEGORY_ORDER = (
    "execution-kernel",
    "host-control-plane",
    "capability-control-plane",
    "capability-provider",
    "cli",
)


def _scan_code(line: str, state: dict[str, object]) -> str:
    """Replace strings and comments so braces can be matched line by line."""
    result: list[str] = []
    index = 0
    block_comment = bool(state.get("block_comment"))
    quote = state.get("quote")
    raw_hashes = state.get("raw_hashes")
    escaped = False
    while index < len(line):
        if block_comment:
            end = line.find("*/", index)
            if end < 0:
                index = len(line)
                break
            block_comment = False
            index = end + 2
            continue
        if raw_hashes is not None:
            terminator = '"' + ("#" * int(raw_hashes))
            end = line.find(terminator, index)
            if end < 0:
                index = len(line)
                break
            raw_hashes = None
            index = end + len(terminator)
            continue
        if quote is not None:
            char = line[index]
            if escaped:
                escaped = False
            elif char == "\\":
                escaped = True
            elif char == quote:
                quote = None
            index += 1
            continue
        if line.startswith("//", index):
            break
        if line.startswith("/*", index):
            block_comment = True
            index += 2
            continue
        raw = re.match(r"b?r(#+)?\"", line[index:])
        if raw:
            raw_hashes = len(raw.group(1) or "")
            index += len(raw.group(0))
            continue
        char = line[index]
        if char == '"':
            quote = char
            index += 1
            continue
        # Do not treat Rust lifetimes such as `'a` as character literals.
        if char == "'" and index + 1 < len(line) and (
            line[index + 1] == "\\"
            or (index + 2 < len(line) and line[index + 2] == "'")
        ):
            quote = char
            index += 1
            continue
        result.append(char)
        index += 1
    state["block_comment"] = block_comment
    state["quote"] = quote
    state["raw_hashes"] = raw_hashes
    return "".join(result)


def _effective_code_line_indices(lines: list[str]) -> set[int]:
    """Return lines that contain Rust code after comments and whitespace."""
    state: dict[str, object] = {}
    return {
        index
        for index, line in enumerate(lines)
        if _scan_code(line, state).strip()
    }


def _test_ranges(lines: list[str]) -> set[int]:
    ranges: set[int] = set()
    index = 0
    while index < len(lines):
        if "#[cfg(test)]" not in lines[index]:
            index += 1
            continue
        start = index
        item = index + 1
        while item < len(lines) and lines[item].lstrip().startswith("#["):
            item += 1
        state: dict[str, object] = {}
        brace_depth = 0
        saw_brace = False
        end = item
        while end < len(lines):
            code = _scan_code(lines[end], state)
            for character in code:
                if character == "{":
                    saw_brace = True
                    brace_depth += 1
                elif character == "}" and saw_brace:
                    brace_depth -= 1
                    if brace_depth == 0:
                        break
                elif character == ";" and not saw_brace:
                    break
                elif character == "," and not saw_brace and ":" in code:
                    break
            if saw_brace and brace_depth == 0:
                break
            if not saw_brace and any(
                marker in code for marker in (";", ",")
            ):
                break
            end += 1
        ranges.update(range(start, min(end + 1, len(lines))))
        index = max(end + 1, index + 1)
    return ranges


def source_counts_for_text(path: str, text: str) -> tuple[int, int, int, int]:
    relative_path = PurePosixPath(path.replace("\\", "/"))
    lines = text.splitlines()
    effective_lines = _effective_code_line_indices(lines)
    total = len(effective_lines)
    relative_parts = relative_path.parts
    if "tests" in relative_parts:
        return total, 0, 0, total
    if relative_path.stem.endswith("_tests") or relative_path.stem == "tests":
        return total, 0, total, 0
    unit_lines = len(effective_lines & _test_ranges(lines))
    return total, total - unit_lines, unit_lines, 0


def source_counts(path: Path) -> tuple[int, int, int, int]:
    return source_counts_for_text(path.as_posix(), path.read_text(encoding="utf-8"))


def source_category(relative_path: str) -> str | None:
    """Return the disjoint architectural bucket for one Rust source path."""
    path = PurePosixPath(relative_path.replace("\\", "/"))
    try:
        crates_index = path.parts.index("crates")
        package = path.parts[crates_index + 1]
    except (ValueError, IndexError):
        return None

    if package in KERNEL_PACKAGES:
        return "execution-kernel"
    if package in HOST_CONTROL_PLANE_PACKAGES:
        return "host-control-plane"
    if package == "mini-agent-capabilities":
        normalized = "/".join(path.parts)
        if normalized in CAPABILITY_CONTROL_PLANE_PATHS:
            return "capability-control-plane"
        return "capability-provider"
    if package == "mini-agent-cli":
        return "cli"
    return None


def _add_counts(
    target: list[int], value: tuple[int, int, int, int]
) -> None:
    for index, item in enumerate(value):
        target[index] += item


def category_counts(root: Path) -> dict[str, tuple[int, int, int, int]]:
    counts = {name: [0, 0, 0, 0] for name in CATEGORY_ORDER}
    unclassified = []
    for source in (root / "crates").rglob("*.rs"):
        relative_path = source.relative_to(root).as_posix()
        category = source_category(relative_path)
        if category is None:
            unclassified.append(relative_path)
            continue
        _add_counts(counts[category], source_counts(source))
    if unclassified:
        paths = ", ".join(sorted(unclassified))
        raise RuntimeError(f"unclassified Rust source paths: {paths}")
    return {name: tuple(counts[name]) for name in CATEGORY_ORDER}


def package_counts(root: Path, package: str) -> tuple[int, int, int, int]:
    counts = [0, 0, 0, 0]
    for source in (root / "crates" / package).rglob("*.rs"):
        for index, value in enumerate(source_counts(source)):
            counts[index] += value
    return tuple(counts)


def layer_counts(root: Path, packages: tuple[str, ...]) -> tuple[int, int, int, int]:
    counts = [0, 0, 0, 0]
    for package in packages:
        for index, value in enumerate(package_counts(root, package)):
            counts[index] += value
    return tuple(counts)


def layer_lines(root: Path, packages: tuple[str, ...]) -> int:
    return layer_counts(root, packages)[0]


def _report_from_categories(
    categories: dict[str, tuple[int, int, int, int]],
) -> dict[str, object]:
    runtime = categories["execution-kernel"][0] + categories[
        "host-control-plane"
    ][0]
    release = runtime + categories["capability-control-plane"][0] + categories[
        "capability-provider"
    ][0]
    kernel = categories["execution-kernel"][0]
    control_plane = categories["host-control-plane"][0] + categories[
        "capability-control-plane"
    ][0]
    return {
        "categories": categories,
        "kernel": kernel,
        "runtime": runtime,
        "release": release,
        "control_plane": control_plane,
    }


def build_report(root: Path = ROOT) -> dict[str, object]:
    layers = {}
    for name, packages in LAYERS:
        layers[name] = layer_counts(root, packages)
    categories = category_counts(root)
    report = _report_from_categories(categories)
    report["layers"] = layers
    expected_release = layer_lines(root, RELEASE_PACKAGES)
    expected_runtime = layer_lines(root, RUNTIME_PACKAGES)
    assert report["release"] == expected_release
    assert report["runtime"] == expected_runtime
    return report


def _git_source_texts(root: Path, ref: str):
    listing = subprocess.run(
        ["git", "ls-tree", "-r", "--name-only", ref, "--", "crates"],
        cwd=root,
        check=False,
        capture_output=True,
        text=True,
        encoding="utf-8",
    )
    if listing.returncode != 0:
        raise RuntimeError(listing.stderr.strip() or f"cannot read git ref {ref}")
    for relative_path in listing.stdout.splitlines():
        if not relative_path.endswith(".rs"):
            continue
        if source_category(relative_path) is None:
            continue
        source = subprocess.run(
            ["git", "show", f"{ref}:{relative_path}"],
            cwd=root,
            check=False,
            capture_output=True,
            text=True,
            encoding="utf-8",
        )
        if source.returncode != 0:
            raise RuntimeError(
                source.stderr.strip() or f"cannot read {relative_path} at {ref}"
            )
        yield relative_path, source.stdout


def build_git_report(root: Path, ref: str) -> dict[str, object]:
    counts = {name: [0, 0, 0, 0] for name in CATEGORY_ORDER}
    for relative_path, text in _git_source_texts(root, ref):
        category = source_category(relative_path)
        assert category is not None
        _add_counts(counts[category], source_counts_for_text(relative_path, text))
    return _report_from_categories(
        {name: tuple(counts[name]) for name in CATEGORY_ORDER}
    )


def _budget_band(total: int, operating: int, red: int, hard: int) -> str:
    if total > hard:
        return "fail"
    if total > red:
        return "red"
    if total > operating:
        return "amber"
    return "green"


def _status(report: dict[str, object]) -> dict[str, str]:
    return {
        "kernel": "fail" if int(report["kernel"]) > KERNEL_LIMIT else "green",
        "control_plane": (
            "fail"
            if int(report["control_plane"]) > CONTROL_PLANE_LIMIT
            else "green"
        ),
        "release": _budget_band(
            int(report["release"]),
            PROJECT_OPERATING_LIMIT,
            PROJECT_RED_LIMIT,
            PROJECT_LIMIT,
        ),
    }


def _delta_gate_violations(
    current: dict[str, object], base: dict[str, object]
) -> tuple[list[str], dict[str, int]]:
    violations = []
    deltas = {
        "kernel": int(current["kernel"]) - int(base["kernel"]),
        "release": int(current["release"]) - int(base["release"]),
        "control_plane": int(current["control_plane"])
        - int(base["control_plane"]),
    }
    policies = (
        (
            "release",
            PROJECT_OPERATING_LIMIT,
            PROJECT_RED_LIMIT,
            PROJECT_LIMIT,
            PROJECT_NON_RED_DELTA_LIMIT,
        ),
    )
    for name, operating, red, hard, non_red_delta in policies:
        total = int(current[name])
        delta = deltas[name]
        if total > hard:
            violations.append(f"{name} exceeds hard limit ({total}/{hard})")
        elif total <= red and delta > non_red_delta:
            violations.append(
                f"{name} grew by {delta} lines, above non-red limit {non_red_delta}"
            )
        if total > red and delta > 0:
            violations.append(f"{name} is in red band and cannot grow")
    kernel_total = int(current["kernel"])
    if kernel_total > KERNEL_LIMIT:
        violations.append(
            f"core+protocol exceeds hard limit ({kernel_total}/{KERNEL_LIMIT})"
        )
    control_plane_total = int(current["control_plane"])
    if control_plane_total > CONTROL_PLANE_LIMIT:
        violations.append(
            "control-plane exceeds hard limit "
            f"({control_plane_total}/{CONTROL_PLANE_LIMIT})"
        )
    return violations, deltas


def _json_counts(counts: tuple[int, int, int, int]) -> dict[str, int]:
    total, production, unit, integration = counts
    return {
        "total": total,
        "production": production,
        "unit": unit,
        "integration": integration,
    }


def _json_report(report: dict[str, object]) -> dict[str, object]:
    return {
        "kernel": report["kernel"],
        "runtime": report["runtime"],
        "release": report["release"],
        "control_plane": report["control_plane"],
        "categories": {
            name: _json_counts(report["categories"][name])
            for name in CATEGORY_ORDER
        },
        "layers": {
            name: _json_counts(counts)
            for name, counts in report.get("layers", {}).items()
        },
        "status": _status(report),
    }


def _percentage(total: int, limit: int) -> str:
    if limit == 0:
        return "100.0%" if total else "0.0%"
    return f"{total * 100 / limit:.1f}%"


def _compact_status(report: dict[str, object]) -> dict[str, str]:
    statuses = _status(report)
    return {
        "kernel": "FAIL" if statuses["kernel"] == "fail" else "PASS",
        "control_plane": "FAIL"
        if statuses["control_plane"] == "fail"
        else "PASS",
        "release": {
            "green": "PASS",
            "amber": "WARN",
            "red": "FREEZE",
            "fail": "FAIL",
        }[statuses["release"]],
    }


def _print_report(
    report: dict[str, object], root: Path = ROOT, verbose: bool = False
) -> None:
    if verbose:
        for name, packages in LAYERS:
            package_list = ", ".join(packages)
            total, production, unit, integration = report["layers"][name]
            print(
                f"{name}: {total} effective code lines "
                f"(production {production}, unit {unit}, integration {integration}) "
                f"[{package_list}]"
            )
            if len(packages) > 1:
                for package in packages:
                    package_total, package_production, package_unit, package_integration = (
                        package_counts(root, package)
                    )
                    print(
                        f"  {package}: {package_total} effective code lines "
                        f"(production {package_production}, unit {package_unit}, "
                        f"integration {package_integration})"
                    )
        for name in CATEGORY_ORDER:
            total, production, unit, integration = report["categories"][name]
            print(
                f"  category/{name}: {total} effective code lines "
                f"(production {production}, unit {unit}, integration {integration})"
            )
        runtime_total = int(report["runtime"])
        runtime_counts = report["categories"]["execution-kernel"]
        host_counts = report["categories"]["host-control-plane"]
        print(
            f"runtime (core + protocol + host + app-server): "
            f"{runtime_total} effective code lines "
            "(informational; no aggregate hard limit) "
            f"(production {runtime_counts[1] + host_counts[1]}, "
            f"unit {runtime_counts[2] + host_counts[2]}, "
            f"integration {runtime_counts[3] + host_counts[3]})"
        )
        release_counts = [0, 0, 0, 0]
        for name in CATEGORY_ORDER[:-1]:
            _add_counts(release_counts, report["categories"][name])
        print(
            "release Rust source (excluding experimental CLI/REPL): "
            f"{report['release']}/{PROJECT_LIMIT} effective code lines "
            f"(production {release_counts[1]}, unit {release_counts[2]}, "
            f"integration {release_counts[3]})"
        )

    statuses = _compact_status(report)
    metrics = (
        ("core+protocol", int(report["kernel"]), KERNEL_LIMIT, statuses["kernel"]),
        (
            "control-plane",
            int(report["control_plane"]),
            CONTROL_PLANE_LIMIT,
            statuses["control_plane"],
        ),
        ("release", int(report["release"]), PROJECT_LIMIT, statuses["release"]),
    )
    for name, total, limit, status in metrics:
        remaining = max(limit - total, 0)
        print(
            f"{name:<15} {total:>5}/{limit:<5} "
            f"{_percentage(total, limit):>6} "
            f"remain {remaining:>5} {status}"
        )


def check(
    root: Path = ROOT,
    base: str | None = None,
    enforce_delta: bool = False,
    json_output: bool = False,
    verbose: bool = False,
) -> int:
    if enforce_delta and base is None:
        print("--check-delta requires --base", file=sys.stderr)
        return 2
    try:
        current = build_report(root)
        baseline = build_git_report(root, base) if base else None
    except RuntimeError as error:
        print(str(error), file=sys.stderr)
        return 2

    violations = []
    if int(current["kernel"]) > KERNEL_LIMIT:
        violations.append(
            f"core+protocol exceeds hard limit ({current['kernel']}/{KERNEL_LIMIT})"
        )
    if int(current["release"]) > PROJECT_LIMIT:
        violations.append(
            f"release exceeds hard limit ({current['release']}/{PROJECT_LIMIT})"
        )
    if int(current["control_plane"]) > CONTROL_PLANE_LIMIT:
        violations.append(
            "control-plane exceeds hard limit "
            f"({current['control_plane']}/{CONTROL_PLANE_LIMIT})"
        )
    deltas = None
    if baseline is not None:
        delta_violations, deltas = _delta_gate_violations(current, baseline)
        if enforce_delta:
            violations.extend(delta_violations)

    if json_output:
        payload = {
            "limits": {
                "core_protocol_hard": KERNEL_LIMIT,
                "release_hard": PROJECT_LIMIT,
                "control_plane_hard": CONTROL_PLANE_LIMIT,
                "release_operating": PROJECT_OPERATING_LIMIT,
                "release_red": PROJECT_RED_LIMIT,
                "release_non_red_delta": PROJECT_NON_RED_DELTA_LIMIT,
            },
            "current": _json_report(current),
            "base": _json_report(baseline) if baseline else None,
            "delta": deltas,
            "delta_enforced": enforce_delta,
            "violations": violations,
        }
        print(json.dumps(payload, ensure_ascii=False, indent=2))
    else:
        print(f"line-budget: {'FAIL' if violations else 'PASS'}")
        _print_report(current, root, verbose=verbose)
        if deltas is not None:
            print(
                f"delta: core+protocol {deltas['kernel']:+d}, "
                f"release {deltas['release']:+d}, "
                f"control-plane {deltas['control_plane']:+d}"
            )
        if violations:
            print("line budget gate failed:", file=sys.stderr)
            for violation in violations:
                print(f"- {violation}", file=sys.stderr)

    return 1 if violations else 0


def main() -> int:
    parser = argparse.ArgumentParser(description="Report and enforce Rust line budgets")
    parser.add_argument(
        "--base", help="git revision used as the baseline for delta reporting"
    )
    parser.add_argument(
        "--check-delta",
        action="store_true",
        help="enforce the operating-band and per-PR delta policy",
    )
    parser.add_argument(
        "--json", action="store_true", help="emit machine-readable JSON"
    )
    parser.add_argument(
        "--verbose", action="store_true", help="include layer and category details"
    )
    args = parser.parse_args()
    return check(
        base=args.base,
        enforce_delta=args.check_delta,
        json_output=args.json,
        verbose=args.verbose,
    )


if __name__ == "__main__":
    raise SystemExit(main())
