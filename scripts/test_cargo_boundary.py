import importlib.util
import unittest
from pathlib import Path


SCRIPT = Path(__file__).with_name("cargo_boundary.py")
SPEC = importlib.util.spec_from_file_location("cargo_boundary", SCRIPT)
assert SPEC and SPEC.loader
cargo_boundary = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(cargo_boundary)


def metadata(*packages: tuple[str, list[tuple[str, str | None]]]) -> dict:
    return {
        "packages": [
            {
                "name": name,
                "dependencies": [
                    {"name": dependency_name, "source": source}
                    for dependency_name, source in dependencies
                ],
            }
            for name, dependencies in packages
        ]
    }


class CargoBoundaryTests(unittest.TestCase):
    def test_current_direction_passes_and_exposes_review_edge(self) -> None:
        result = cargo_boundary.report(
            metadata(
                ("mini-agent-protocol", []),
                ("mini-agent-core", [("mini-agent-protocol", None)]),
                (
                    "mini-agent-capabilities",
                    [("mini-agent-core", None), ("mini-agent-protocol", None)],
                ),
                (
                    "mini-agent-host",
                    [("mini-agent-capabilities", None), ("mini-agent-core", None)],
                ),
                (
                    "mini-agent-app-server",
                    [("mini-agent-capabilities", None), ("mini-agent-host", None)],
                ),
                ("mini-agent-cli", [("mini-agent-app-server", None)]),
            )
        )
        self.assertEqual(result["status"], "pass")
        self.assertEqual(
            [(finding["from"], finding["to"]) for finding in result["review_edges"]],
            [("mini-agent-app-server", "mini-agent-capabilities")],
        )

    def test_lower_layer_dependency_fails(self) -> None:
        result = cargo_boundary.report(
            metadata(
                ("mini-agent-protocol", []),
                ("mini-agent-core", [("mini-agent-protocol", None), ("mini-agent-host", None)]),
                ("mini-agent-host", []),
            )
        )
        self.assertEqual(result["status"], "fail")
        self.assertIn(
            "unexpected workspace dependency: mini-agent-core -> mini-agent-host",
            result["violations"],
        )

    def test_external_dependencies_are_not_boundary_edges(self) -> None:
        result = cargo_boundary.report(
            metadata(
                ("mini-agent-protocol", [("serde", "registry+https://example.invalid")]),
            )
        )
        self.assertEqual(result["status"], "pass")
        self.assertEqual(result["workspace_dependencies"], {"mini-agent-protocol": []})

    def test_new_workspace_package_requires_explicit_configuration(self) -> None:
        result = cargo_boundary.report(
            metadata(
                ("mini-agent-protocol", []),
                ("mini-agent-experiment", []),
            )
        )
        self.assertEqual(result["status"], "fail")
        self.assertIn(
            "unconfigured workspace package: mini-agent-experiment",
            result["violations"],
        )


if __name__ == "__main__":
    unittest.main()
