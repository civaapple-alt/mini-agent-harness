import importlib.util
import pathlib
import unittest


SCRIPT = pathlib.Path(__file__).with_name("check_iteration_note.py")
SPEC = importlib.util.spec_from_file_location("check_iteration_note", SCRIPT)
assert SPEC and SPEC.loader
check_iteration_note = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(check_iteration_note)


class IterationNoteTests(unittest.TestCase):
    def test_accepts_note_with_all_evidence_sections(self) -> None:
        text = "\n".join(check_iteration_note.REQUIRED_MARKERS)
        self.assertEqual(check_iteration_note.validate_note(text), [])

    def test_reports_missing_evidence_sections(self) -> None:
        errors = check_iteration_note.validate_note("状态：implemented\n## Verification")
        self.assertIn(
            "missing required marker: ## Harness hypothesis",
            errors,
        )

    def test_rejects_empty_note(self) -> None:
        self.assertEqual(check_iteration_note.validate_note(""), ["note is empty"])


if __name__ == "__main__":
    unittest.main()
