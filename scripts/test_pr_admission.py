import importlib.util
import pathlib
import unittest


SCRIPT = pathlib.Path(__file__).with_name("check_pr_admission.py")
SPEC = importlib.util.spec_from_file_location("check_pr_admission", SCRIPT)
assert SPEC and SPEC.loader
check_pr_admission = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(check_pr_admission)


def valid_body() -> str:
    questions = "\n".join(
        f"{index}. **Question {index}**\n\nanswer {index}"
        for index in range(1, 7)
    )
    confirmations = "\n".join(
        f"- [x] {label} (confirmed)"
        for label in check_pr_admission._CONFIRMATION_LABELS
    )
    return f"## 变更准入检查\n\n### 六项必答题\n\n{questions}\n\n### 准入确认\n{confirmations}"


class PrAdmissionTests(unittest.TestCase):
    def test_accepts_completed_template(self) -> None:
        self.assertEqual(check_pr_admission.validate_body(valid_body()), [])

    def test_rejects_unanswered_template(self) -> None:
        errors = check_pr_admission.validate_body(
            valid_body().replace("answer 3", "<!-- answer here -->")
        )
        self.assertIn(
            "replace every '<!-- answer here -->' placeholder with an answer", errors
        )

    def test_rejects_blank_answer(self) -> None:
        errors = check_pr_admission.validate_body(valid_body().replace("answer 3", ""))
        self.assertIn("question 3 has no answer", errors)

    def test_rejects_missing_confirmation(self) -> None:
        body = valid_body().replace("- [x]", "- [ ]", 1)
        self.assertIn("check all six admission confirmation boxes", check_pr_admission.validate_body(body))

    def test_ignores_unrelated_checked_boxes_outside_confirmation(self) -> None:
        body = valid_body().replace("- [x]", "- [ ]", 1)
        body += "\n\n- [x] unrelated checklist item"
        self.assertIn("check all six admission confirmation boxes", check_pr_admission.validate_body(body))

    def test_rejects_unrelated_checked_boxes_inside_confirmation(self) -> None:
        body = valid_body()
        confirmation = "### 准入确认\n- [x] unrelated checklist item\n"
        body = body.replace("### 准入确认\n", confirmation)
        body = body.replace(
            f"- [x] {check_pr_admission._CONFIRMATION_LABELS[0]}",
            f"- [ ] {check_pr_admission._CONFIRMATION_LABELS[0]}",
            1,
        )
        self.assertIn("check all six admission confirmation boxes", check_pr_admission.validate_body(body))


if __name__ == "__main__":
    unittest.main()
