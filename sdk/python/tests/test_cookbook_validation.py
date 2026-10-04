"""No-token validation for the published Cookbook examples."""

from __future__ import annotations

import runpy
from pathlib import Path

import pytest

COOKBOOK_DIR = Path(__file__).parents[3] / "cookbook" / "python-demo"
COOKBOOK_SCRIPTS = sorted(COOKBOOK_DIR.glob("*.py"))


@pytest.mark.parametrize("script", COOKBOOK_SCRIPTS, ids=lambda path: path.name)
def test_cookbook_scripts_compile(script: Path):
    compile(script.read_text(encoding="utf-8"), str(script), "exec")


def test_protocol_compatibility_cookbook_runs_without_provider():
    runpy.run_path(
        str(COOKBOOK_DIR / "06_protocol_compatibility.py"), run_name="__main__"
    )


def test_recovery_and_projection_cookbook_runs_without_provider():
    runpy.run_path(
        str(COOKBOOK_DIR / "07_recovery_and_projection.py"), run_name="__main__"
    )
