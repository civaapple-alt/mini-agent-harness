import contextlib
import importlib.util
import io
import tempfile
import unittest
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

    def test_layer_lines_sums_selected_crates(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for package, source in {
                "mini-agent-core": "fn core() {}\n",
                "mini-agent-protocol": "fn protocol() {}\nfn event() {}\n",
                "mini-agent-host": "fn host() {}\n",
                "mini-agent-app-server": "fn server() {}\n",
                "mini-agent-app-server-protocol": "fn wire() {}\n",
                "mini-agent-acp": "fn acp() {}\n",
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
                        "mini-agent-acp",
                    ),
                ),
                3,
            )
            self.assertEqual(line_budget.layer_lines(root, ("mini-agent-cli",)), 2)

    def test_check_reports_success_for_a_small_workspace(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            package_root = root / "crates" / "mini-agent-core" / "src"
            package_root.mkdir(parents=True)
            (package_root / "lib.rs").write_text("fn core() {}\n", encoding="utf-8")
            acp_root = root / "crates" / "mini-agent-acp" / "src"
            acp_root.mkdir(parents=True)
            (acp_root / "lib.rs").write_text(
                "fn acp() {}\nfn edge() {}\n", encoding="utf-8"
            )

            output = io.StringIO()
            with contextlib.redirect_stdout(output):
                self.assertEqual(line_budget.check(root), 0)
            self.assertIn("production 1, unit 0, integration 0", output.getvalue())
            self.assertIn(
                "runtime (core + protocol + host + app-server): 1/20000 lines",
                output.getvalue(),
            )
            self.assertIn("acp: 2 lines", output.getvalue())
            self.assertIn("all Rust source: 3/30000 lines", output.getvalue())
            self.assertIn(
                "acp: 2 lines (production 2, unit 0, integration 0) [mini-agent-acp]",
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
                self.assertEqual(line_budget.check(root), 0)

            self.assertIn("capabilities: 3 lines", output.getvalue())
            self.assertIn(
                "runtime (core + protocol + host + app-server): 1/20000 lines",
                output.getvalue(),
            )


if __name__ == "__main__":
    unittest.main()
