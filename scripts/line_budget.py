from pathlib import Path
import sys


ROOT = Path(__file__).resolve().parents[1]
CORE = ROOT / "crates" / "mini-codex-core"
CORE_LIMIT = 20_000
PROJECT_LIMIT = 30_000


def rust_lines(path: Path) -> int:
    return sum(
        len(source.read_text(encoding="utf-8").splitlines())
        for source in path.rglob("*.rs")
    )


core_lines = rust_lines(CORE)
project_lines = rust_lines(ROOT / "crates")

print(f"mini-codex-core: {core_lines}/{CORE_LIMIT} lines")
print(f"all Rust source: {project_lines}/{PROJECT_LIMIT} lines")

if core_lines > CORE_LIMIT or project_lines > PROJECT_LIMIT:
    sys.exit("line budget exceeded")
