import contextlib
import importlib.util
import io
import tempfile
import unittest
from unittest import mock
from pathlib import Path

SCRIPT = Path(__file__).with_name("line_budget.py")
SPEC = importlib.util.spec_from_file_location("line_budget", SCRIPT)
assert SPEC and SPEC.loader
line_budget = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(line_budget)


class LineBudgetTests(unittest.TestCase):
    def test_source_counts_separates_production_unit_and_integration(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            package_root = root / "crates" / "mini-agent-core" / "src"
            package_root.mkdir(parents=True)
            inline_unit = package_root / "lib.rs"
            inline_unit.write_text(
                "pub fn run() { let value = \"}\"; }\n"
                "#[cfg(test)]\n"
                "mod tests {\n"
                "    #[test]\n"
                "    fn braces_in_strings_are_ignored() {\n"
                "        assert_eq!(\"{\", \"{\");\n"
                "    }\n"
                "}\n",
                encoding="utf-8",
            )
            dedicated_unit = package_root / "skills_tests.rs"
            dedicated_unit.write_text("#[test]\nfn dedicated() {}\n", encoding="utf-8")
            integration = (
                root / "crates" / "mini-agent-core" / "tests" / "runtime.rs"
            )
            integration.parent.mkdir(parents=True)
            integration.write_text("#[test]\nfn integration() {}\n", encoding="utf-8")

            self.assertEqual(line_budget.source_counts(inline_unit), (8, 1, 7, 0))
            self.assertEqual(line_budget.source_counts(dedicated_unit), (2, 0, 2, 0))
            self.assertEqual(line_budget.source_counts(integration), (2, 0, 0, 2))

    def test_source_counts_excludes_blank_and_comment_only_lines(self):
        text = (
            "// module comment\n"
            "\n"
            "pub fn run() {} // trailing comment still has code\n"
            "/* block comment\n"
            " * continuation\n"
            " */\n"
            "#[cfg(test)]\n"
            "mod tests {\n"
            "    // test comment\n"
            "    #[test]\n"
            "    fn check() {}\n"
            "}\n"
        )

        self.assertEqual(
            line_budget.source_counts_for_text("src/lib.rs", text),
            (6, 1, 5, 0),
        )

    def test_layer_lines_sums_selected_crates(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for package, source in {
                "mini-agent-core": "fn core() {}\n",
                "mini-agent-protocol": "fn protocol() {}\nfn event() {}\n",
                "mini-agent-host": "fn host() {}\n",
                "mini-agent-app-server": "fn server() {}\n",
                "mini-agent-app-server-protocol": "fn wire() {}\n",
                "mini-agent-cli": "fn cli() {}\nfn repl() {}\n",
            }.items():
                package_root = root / "crates" / package / "src"
                package_root.mkdir(parents=True)
                (package_root / "lib.rs").write_text(source, encoding="utf-8")

            self.assertEqual(line_budget.layer_lines(root, ("mini-agent-core",)), 1)
            self.assertEqual(line_budget.layer_lines(root, ("mini-agent-host",)), 1)
            self.assertEqual(
                line_budget.layer_lines(
                    root,
                    (
                        "mini-agent-app-server",
                        "mini-agent-app-server-protocol",
                    ),
                ),
                2,
            )
            self.assertEqual(line_budget.layer_lines(root, ("mini-agent-cli",)), 2)

    def test_source_categories_keep_control_plane_paths_explicit(self):
        self.assertEqual(
            line_budget.source_category(
                "crates/mini-agent-capabilities/src/security.rs"
            ),
            "capability-control-plane",
        )
        self.assertEqual(
            line_budget.source_category(
                "crates/mini-agent-capabilities/src/mcp.rs"
            ),
            "capability-provider",
        )
        self.assertEqual(
            line_budget.source_category("crates/mini-agent-host/src/world.rs"),
            "host-control-plane",
        )
        self.assertEqual(
            line_budget.source_category("crates/mini-agent-cli/src/main.rs"),
            "cli",
        )

    def test_unclassified_rust_source_fails_closed(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "crates" / "mini-agent-new" / "src" / "lib.rs"
            source.parent.mkdir(parents=True)
            source.write_text("fn future_crate() {}\n", encoding="utf-8")

            with self.assertRaisesRegex(RuntimeError, "unclassified Rust source"):
                line_budget.category_counts(root)

    def test_delta_gate_allows_growth_at_per_pr_limit(self):
        current = {"kernel": 5_200, "runtime": 24_200, "release": 40_000, "control_plane": 1}
        base = {"kernel": 5_000, "runtime": 24_000, "release": 39_000, "control_plane": 1}

        violations, deltas = line_budget._delta_gate_violations(current, base)

        self.assertEqual(
            deltas, {"kernel": 200, "release": 1_000, "control_plane": 0}
        )
        self.assertEqual(violations, [])

    def test_delta_above_guidance_is_not_a_hard_gate(self):
        current = {"kernel": 5_201, "runtime": 24_201, "release": 40_001, "control_plane": 1}
        base = {"kernel": 5_000, "runtime": 24_000, "release": 39_000, "control_plane": 1}

        violations, deltas = line_budget._delta_gate_violations(current, base)

        self.assertEqual(
            deltas, {"kernel": 201, "release": 1_001, "control_plane": 0}
        )
        self.assertEqual(violations, [])

    def test_delta_gate_allows_growth_above_old_red_band(self):
        current = {"kernel": 5_501, "runtime": 24_501, "release": 40_001, "control_plane": 1}
        base = {"kernel": 5_500, "runtime": 24_500, "release": 40_000, "control_plane": 1}

        violations, deltas = line_budget._delta_gate_violations(current, base)

        self.assertEqual(
            deltas, {"kernel": 1, "release": 1, "control_plane": 0}
        )
        self.assertEqual(violations, [])

    def test_delta_gate_allows_zero_growth_at_hard_limit(self):
        current = {"kernel": 6_887, "runtime": 25_887, "release": 65_000, "control_plane": 1}
        base = {"kernel": 6_887, "runtime": 25_887, "release": 65_000, "control_plane": 1}

        violations, deltas = line_budget._delta_gate_violations(current, base)

        self.assertEqual(deltas, {"kernel": 0, "release": 0, "control_plane": 0})
        self.assertEqual(violations, [])

    def test_check_reports_success_for_a_small_workspace(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            package_root = root / "crates" / "mini-agent-core" / "src"
            package_root.mkdir(parents=True)
            (package_root / "lib.rs").write_text("fn core() {}\n", encoding="utf-8")
            output = io.StringIO()
            with contextlib.redirect_stdout(output):
                self.assertEqual(line_budget.check(root), 0)
            self.assertIn("line-budget: PASS", output.getvalue())
            self.assertIn("core+protocol", output.getvalue())
            self.assertIn("1/7000", output.getvalue())
            self.assertIn("control-plane", output.getvalue())
            self.assertIn("0/45000", output.getvalue())
            self.assertIn(
                "release             1/65000",
                output.getvalue(),
            )
            self.assertNotIn("runtime (core + protocol + host + app-server)", output.getvalue())
            self.assertIn(
                "1/7000",
                output.getvalue(),
            )

    def test_capabilities_are_reported_but_excluded_from_runtime_gate(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            core_root = root / "crates" / "mini-agent-core" / "src"
            core_root.mkdir(parents=True)
            (core_root / "lib.rs").write_text("fn core() {}\n", encoding="utf-8")
            capabilities_root = (
                root / "crates" / "mini-agent-capabilities" / "src"
            )
            capabilities_root.mkdir(parents=True)
            (capabilities_root / "lib.rs").write_text(
                "fn tool() {}\nfn model() {}\nfn policy() {}\n", encoding="utf-8"
            )

            output = io.StringIO()
            with contextlib.redirect_stdout(output):
                self.assertEqual(line_budget.check(root, verbose=True), 0)

            self.assertIn("capabilities: 3 effective code lines", output.getvalue())
            self.assertIn(
                "category/capability-provider: 3 effective code lines", output.getvalue()
            )
            self.assertIn(
                "runtime (core + protocol + host + app-server): "
                "1 effective code lines (informational; no aggregate hard limit)",
                output.getvalue(),
            )

    def test_control_plane_hard_limit_is_enforced(self):
        current = {"kernel": 1, "runtime": 1, "release": 1, "control_plane": 45_001}
        base = {"kernel": 1, "runtime": 1, "release": 1, "control_plane": 45_000}

        violations, deltas = line_budget._delta_gate_violations(current, base)

        self.assertEqual(deltas["control_plane"], 1)
        self.assertIn(
            "control-plane exceeds hard limit (45001/45000)", violations
        )

    def test_delta_report_enforces_core_protocol_hard_limit(self):
        current = {"kernel": 7_001, "runtime": 1, "release": 1, "control_plane": 1}
        base = {"kernel": 7_001, "runtime": 1, "release": 1, "control_plane": 1}

        violations, _ = line_budget._delta_gate_violations(current, base)

        self.assertIn("core+protocol exceeds hard limit (7001/7000)", violations)

    def test_release_hard_limit_is_enforced(self):
        current = {"kernel": 1, "runtime": 1, "release": 65_001, "control_plane": 1}
        base = {"kernel": 1, "runtime": 1, "release": 65_001, "control_plane": 1}

        violations, _ = line_budget._delta_gate_violations(current, base)

        self.assertIn("release exceeds hard limit (65001/65000)", violations)

    def test_core_protocol_hard_limit_is_enforced(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            package_root = root / "crates" / "mini-agent-core" / "src"
            package_root.mkdir(parents=True)
            (package_root / "lib.rs").write_text("fn core() {}\n", encoding="utf-8")

            output = io.StringIO()
            with mock.patch.object(line_budget, "KERNEL_LIMIT", 0):
                with contextlib.redirect_stdout(output):
                    with contextlib.redirect_stderr(io.StringIO()):
                        self.assertEqual(line_budget.check(root), 1)
            self.assertIn("core+protocol", output.getvalue())
            self.assertIn("1/0", output.getvalue())

    def test_experimental_cli_is_reported_but_excluded_from_release_gate(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            core_root = root / "crates" / "mini-agent-core" / "src"
            core_root.mkdir(parents=True)
            (core_root / "lib.rs").write_text("fn core() {}\n", encoding="utf-8")
            cli_root = root / "crates" / "mini-agent-cli" / "src"
            cli_root.mkdir(parents=True)
            (cli_root / "lib.rs").write_text("fn cli() {}\n" * 20, encoding="utf-8")

            output = io.StringIO()
            with mock.patch.object(line_budget, "PROJECT_LIMIT", 1):
                with contextlib.redirect_stdout(output):
                    self.assertEqual(line_budget.check(root, verbose=True), 0)
            self.assertIn("cli: 20 effective code lines", output.getvalue())
            self.assertIn(
                "release Rust source (excluding experimental CLI/REPL): "
                "1/1 effective code lines",
                output.getvalue(),
            )

    def test_release_source_total_is_release_gate(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            package_root = root / "crates" / "mini-agent-core" / "src"
            package_root.mkdir(parents=True)
            (package_root / "lib.rs").write_text("fn core() {}\n", encoding="utf-8")

            output = io.StringIO()
            with mock.patch.object(line_budget, "PROJECT_LIMIT", 0):
                with contextlib.redirect_stdout(output):
                    with contextlib.redirect_stderr(io.StringIO()):
                        self.assertEqual(line_budget.check(root), 1)
            self.assertIn("line-budget: FAIL", output.getvalue())
            self.assertIn(
                "release             1/0",
                output.getvalue(),
            )


if __name__ == "__main__":
    unittest.main()
