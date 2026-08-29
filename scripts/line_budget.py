import re
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
RUNTIME_LIMIT = 20_000
# The workspace total includes production code and tests and is a hard release
# gate for the 0.4.0 release.
PROJECT_LIMIT = 30_000

# Keep the report aligned with the conceptual runtime layers. Capabilities are
# reported separately because they are provider implementations behind Host;
# protocol and ACP are also reported separately so each external boundary stays
# visible.
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
    ("acp", ("mini-agent-acp",)),
    ("cli", ("mini-agent-cli",)),
    ("experiments", ("mini-agent-experiments",)),
)
RUNTIME_PACKAGES = (
    "mini-agent-core",
    "mini-agent-protocol",
    "mini-agent-host",
    "mini-agent-app-server",
    "mini-agent-app-server-protocol",
)
# Keep provider implementations outside the runtime gate even though they are
# included in the all-Rust workspace total and shown as their own layer.


def rust_lines(path: Path) -> int:
    return sum(
        len(source.read_text(encoding="utf-8").splitlines())
        for source in path.rglob("*.rs")
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
                return "".join(result)
            block_comment = False
            index = end + 2
            continue
        if raw_hashes is not None:
            terminator = '"' + ("#" * int(raw_hashes))
            end = line.find(terminator, index)
            if end < 0:
                return "".join(result)
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


def source_counts(path: Path) -> tuple[int, int, int, int]:
    lines = path.read_text(encoding="utf-8").splitlines()
    total = len(lines)
    relative_parts = path.parts
    if "tests" in relative_parts:
        return total, 0, 0, total
    if path.stem.endswith("_tests") or path.stem == "tests":
        return total, 0, total, 0
    unit_lines = len(_test_ranges(lines))
    return total, total - unit_lines, unit_lines, 0


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


def project_counts(root: Path) -> tuple[int, int, int, int]:
    counts = [0, 0, 0, 0]
    for source in (root / "crates").rglob("*.rs"):
        for index, value in enumerate(source_counts(source)):
            counts[index] += value
    return tuple(counts)


def layer_lines(root: Path, packages: tuple[str, ...]) -> int:
    return layer_counts(root, packages)[0]


def check(root: Path = ROOT) -> int:
    crates_root = root / "crates"
    project_lines = rust_lines(crates_root)

    for name, packages in LAYERS:
        package_list = ", ".join(packages)
        total, production, unit, integration = layer_counts(root, packages)
        print(
            f"{name}: {total} lines "
            f"(production {production}, unit {unit}, integration {integration}) "
            f"[{package_list}]"
        )
        if len(packages) > 1:
            for package in packages:
                package_total, package_production, package_unit, package_integration = (
                    package_counts(root, package)
                )
                print(
                    f"  {package}: {package_total} lines "
                    f"(production {package_production}, unit {package_unit}, "
                    f"integration {package_integration})"
                )
    total, production, unit, integration = project_counts(root)
    assert total == project_lines
    runtime_total, runtime_production, runtime_unit, runtime_integration = (
        layer_counts(root, RUNTIME_PACKAGES)
    )
    print(
        f"runtime (core + protocol + host + app-server): "
        f"{runtime_total}/{RUNTIME_LIMIT} lines "
        f"(production {runtime_production}, unit {runtime_unit}, "
        f"integration {runtime_integration})"
    )
    print(
        f"all Rust source: {project_lines}/{PROJECT_LIMIT} lines "
        f"(production {production}, unit {unit}, integration {integration})"
    )

    if runtime_total > RUNTIME_LIMIT or project_lines > PROJECT_LIMIT:
        print("line budget exceeded", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(check())
