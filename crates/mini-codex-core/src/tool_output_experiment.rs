use super::*;
use serde::Serialize;

const MAX_BYTES: usize = 96;
const HEADER: &str = "build started\n";
const TAIL_EVIDENCE: &str = "\nVERDICT=SAFE_TO_PROCEED";

#[derive(Debug, PartialEq, Serialize)]
struct ExperimentResult {
    treatment: &'static str,
    model_steps: usize,
    retained_bytes: usize,
    header_visible: bool,
    tail_evidence_visible: bool,
    verifier_passed: bool,
}

fn raw_tool_output() -> String {
    format!("{HEADER}{}{TAIL_EVIDENCE}", "重复的构建噪声\n".repeat(30))
}

fn truncate_head_only(mut content: String, max_bytes: usize) -> String {
    if content.len() <= max_bytes {
        return content;
    }
    let end = floor_char_boundary(&content, max_bytes);
    content.truncate(end);
    content
}

fn model_accepts(output: &str) -> bool {
    output.contains(TAIL_EVIDENCE.trim())
}

fn assess(treatment: &'static str, output: String) -> ExperimentResult {
    ExperimentResult {
        treatment,
        model_steps: 2,
        retained_bytes: output.len(),
        header_visible: output.starts_with(HEADER),
        tail_evidence_visible: output.contains(TAIL_EVIDENCE.trim()),
        verifier_passed: model_accepts(&output),
    }
}

#[test]
fn compares_head_only_with_head_and_tail() {
    let raw = raw_tool_output();
    let head_only = assess("head_only", truncate_head_only(raw.clone(), MAX_BYTES));
    let head_and_tail = assess("head_and_tail", truncate_utf8(raw, MAX_BYTES));

    println!(
        "{}",
        serde_json::to_string_pretty(&[&head_only, &head_and_tail]).unwrap()
    );
    assert_eq!(
        head_only,
        ExperimentResult {
            treatment: "head_only",
            model_steps: 2,
            retained_bytes: 95,
            header_visible: true,
            tail_evidence_visible: false,
            verifier_passed: false,
        }
    );
    assert_eq!(
        head_and_tail,
        ExperimentResult {
            treatment: "head_and_tail",
            model_steps: 2,
            retained_bytes: 94,
            header_visible: true,
            tail_evidence_visible: true,
            verifier_passed: true,
        }
    );
}
