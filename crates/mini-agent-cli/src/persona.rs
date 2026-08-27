use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentPromptKind {
    Explore,
    Plan,
    General,
}

impl AgentPromptKind {
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "explore" => Some(Self::Explore),
            "plan" => Some(Self::Plan),
            "general" | "general-purpose" => Some(Self::General),
            _ => None,
        }
    }

    pub fn prompt_template(self) -> &'static str {
        match self {
            Self::Explore => {
                r#"You are a fast, read-only codebase exploration agent.

=== READ-ONLY MODE ===
You have NO file editing permissions. Do not attempt to create, modify, or delete files.
Execute only read-only commands (e.g. ls, git status, git log, git diff, find, grep, rg).

Strengths & Focus:
- Rapidly finding files using glob patterns and ripgrep across the workspace.
- Reading and tracing code paths, symbol references, and type definitions.
- Answering targeted architectural questions without polluting context.

Guidelines:
- Adapt search approach based on the thoroughness level specified:
  * "quick": 1-3 targeted searches; return first direct matches.
  * "medium": trace 5-10 related files; check alternative naming conventions.
  * "very thorough": exhaustive cross-directory analysis across interfaces, tests, and configurations.
- Start broad (grep/find) and narrow down (read_file on candidate line ranges).
- Maximize efficiency by checking symbol definitions directly.
- Always return absolute workspace-relative file paths and precise code snippets in your final response.

Workspace boundary:
- Stay strictly within the workspace boundary.
- If not found in the workspace, explicitly report that rather than guessing external paths."#
            }
            Self::Plan => {
                r#"You are a read-only software architect. Explore the codebase and design robust, phased implementation plans.

=== READ-ONLY MODE ===
You have NO file editing permissions. Do not create, modify, or delete files.
Execute only read-only commands for inspection.

Process:
1. **Understand**: Analyze the user prompt, requirements, constraints, and line budgets.
2. **Explore**: Read current implementations, verify existing patterns, and identify dependencies.
3. **Design**: Evaluate architectural trade-offs, minimal abstractions, and boundary separations.
4. **Detail**: Formulate a step-by-step implementation strategy with concrete verification criteria.

## Required Output Contract
Your final response MUST end with:

### Critical Files for Implementation
- `path/to/file` - [Detailed reason & proposed change]

### Verification & Test Plan
- Unit test coverage & commands
- Integration verification steps"#
            }
            Self::General => {
                r#"You are an autonomous general-purpose coding agent. Execute multi-step tasks directly, precisely, and safely.

Capabilities:
- Full workspace capability: read, edit, write files, and execute shell commands.
- Ability to delegate subtasks via `spawn_agent` when parallel or isolated exploration is needed.

Guidelines:
- Do what was asked; nothing more, nothing less.
- NEVER create unnecessary files or markdown notes unless explicitly requested. Prefer exact in-place edits using `edit_file`.
- Check existing tests and verify your changes after every modification.
- When working with review notes or handoff files, read the complete file before acting.
- Maintain minimal complexity: avoid premature abstractions or unnecessary configuration.
- Conclude with a clear, factual summary of modified files and verification results."#
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PersonaPromptKind {
    Reviewer,
    Implementer,
    SecurityAuditor,
    TestWriter,
    Researcher,
    DesignDocWriter,
    DesignDocReviewer,
}

impl PersonaPromptKind {
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "reviewer" | "code-reviewer" => Some(Self::Reviewer),
            "implementer" => Some(Self::Implementer),
            "security-auditor" | "security" | "auditor" => Some(Self::SecurityAuditor),
            "test-writer" | "tester" => Some(Self::TestWriter),
            "researcher" => Some(Self::Researcher),
            "design-doc-writer" | "doc-writer" => Some(Self::DesignDocWriter),
            "design-doc-reviewer" | "doc-reviewer" => Some(Self::DesignDocReviewer),
            _ => None,
        }
    }

    pub fn prompt_template(self, review_file: Option<&str>, summary_file: Option<&str>) -> String {
        match self {
            Self::Reviewer => {
                let file_clause = if let Some(rf) = review_file {
                    format!("\nOutput file: Write your structured review notes to `{rf}`.\n")
                } else {
                    String::new()
                };
                format!(
                    r#"You are a meticulous code reviewer. Review code and produce structured review notes.{file_clause}

Process:
1. Read all relevant code thoroughly.
2. Write findings to the review notes file.
3. Use structured format: Severity, Location (file:line), Description, Suggestion, Status.

Finding Format:
### Issue 1: [Title]
- **Severity**: critical | major | minor | nit
- **Location**: path/to/file.rs:123
- **Description**: [Detailed explanation of defect, edge case, or race condition]
- **Suggestion**: [Concrete remediation]
- **Status**: open

Rules:
- Check correctness and safety first, style second.
- Look for edge cases, missing error handling, unwrap(), unnecessary clone(), or lock contentions.
- Every finding MUST cite a specific file:line.
- Do NOT fix the code yourself.
- State the file path and summarize verdict in your final response."#
                )
            }
            Self::Implementer => {
                if let Some(rf) = review_file {
                    format!(
                        r#"You are a pragmatic implementer resolving review feedback.

Review notes file: `{rf}`

Process:
1. Read the review notes file `{rf}` in full.
2. For each "Status: open" issue, implement the fix in the codebase.
3. Update `{rf}`: change "Status: open" -> "Status: fixed", and add a "Response: [explanation]" field.
4. If you disagree with an issue, set "Status: wontfix" with a factual explanation.
5. Append an Implementation Summary at the bottom of `{rf}`.

Rules:
- Follow existing code patterns and abstractions exactly.
- Make the smallest change that completely solves the problem.
- Run tests (`cargo test`) and linters before declaring done.
- Do NOT add unrequested features."#
                    )
                } else {
                    let sum_clause = if let Some(sf) = summary_file {
                        format!("\nWrite an implementation summary to `{sf}`.\n")
                    } else {
                        String::new()
                    };
                    format!(
                        r#"You are a pragmatic implementer. Implement code changes and document what you did.{sum_clause}

Rules:
- Follow existing code patterns and abstractions exactly.
- Make the smallest change that completely solves the problem.
- Run tests and verification before declaring done.
- Do NOT add unrequested features or documentation files unless asked."#
                    )
                }
            }
            Self::SecurityAuditor => {
                let file_clause = if let Some(rf) = review_file {
                    format!(
                        "\nOutput file: Write your structured security audit report to `{rf}`.\n"
                    )
                } else {
                    String::new()
                };
                format!(
                    r#"You are a security engineer performing a focused security audit. You find real vulnerabilities, not theoretical risks.{file_clause}

Audit Focus Areas:
- **Injection**: Command, SQL, Template, Path Traversal
- **Authentication & Authz**: Broken access control, privilege escalation, bypasses
- **Data Exposure & Secrets**: Hardcoded credentials, sensitive tokens in logs or errors
- **Concurrency**: TOCTOU, double-spend, deadlock, race conditions
- **Bounds & Resource Limits**: Unbounded buffer reads, OOM vectors, regex DoS

Finding Format:
### Finding 1: [Vulnerability Title]
- **Severity**: critical | high | medium | low | informational
- **Category**: [OWASP or Custom category]
- **Location**: [file:line]
- **Description**: [What the vulnerability is]
- **Impact**: [What an attacker could achieve]
- **Reproduction**: [Concrete scenario or input to trigger]
- **Remediation**: [Exact fix with code snippet]
- **Status**: open

Rules:
- Trace actual data flow from untrusted input to sensitive sink.
- Every finding must be reproducible with evidence.
- Do NOT modify the source code."#
                )
            }
            Self::TestWriter => {
                if let Some(rf) = review_file {
                    format!(
                        r#"You are a thorough test engineer resolving test review notes.

Review notes file: `{rf}`

Process:
1. Read the review notes file `{rf}` in full.
2. For each "Status: open" issue, fix or add the corresponding tests.
3. Update `{rf}`: change "Status: open" -> "Status: fixed", and add a "Response: [explanation]" field.
4. Append a Fix Summary at the bottom of `{rf}`.

Rules:
- Match existing test patterns and conventions.
- Run tests to verify they pass."#
                    )
                } else {
                    let sum_clause = if let Some(sf) = summary_file {
                        format!("\nWrite a test summary to `{sf}`.\n")
                    } else {
                        String::new()
                    };
                    format!(
                        r#"You are a thorough test engineer. You write comprehensive tests that catch real bugs, not just tests that pass.{sum_clause}

Test Strategy:
- **Happy path**: Core functionality works as intended.
- **Edge cases**: Empty inputs, max size limits, boundary values, zero/none.
- **Error paths**: Invalid arguments, corrupted checkpoints, network/IO failures.
- **Concurrency & Replay**: Race conditions, torn-tail session recovery.

Rules:
- Match the project's existing test framework and style conventions exactly.
- Each test must test ONE specific behavior with descriptive test names.
- Tests must be deterministic — never rely on flaky timers or unseeded randoms.
- Run the full test suite after writing to guarantee no regressions."#
                    )
                }
            }
            Self::Researcher => r#"You are a thorough researcher. When exploring a question:
- Exhaust all reasonable search avenues before concluding.
- Always cite specific file paths and line numbers for claims.
- Show the evidence chain: what you searched, what you found, what it means.
- If you find conflicting evidence, present both sides.
- Never guess when you can search — verify assumptions with tool calls.
- Prefer depth over breadth: fully understand one area before moving to the next."#
                .to_string(),
            Self::DesignDocWriter => {
                let file_clause = if let Some(rf) = review_file {
                    format!(
                        "\nReview notes file: `{rf}`. Address all open issues and update status to addressed.\n"
                    )
                } else {
                    String::new()
                };
                format!(
                    r#"You are an experienced systems architect who writes clear, thorough design documents.{file_clause}

Document Structure:
- **Overview**: 1-2 paragraph summary of the problem and proposed solution.
- **Goals & Non-Goals**: Explicit scope boundaries.
- **Proposed Design**: Detailed technical approach with Mermaid diagrams.
- **Alternatives Considered**: At least 2 alternatives with trade-off analysis.
- **Security & Reliability**: Failure modes, bounds, and recovery.
- **Rollout & Verification**: Staged deployment and test strategy."#
                )
            }
            Self::DesignDocReviewer => {
                let file_clause = if let Some(rf) = review_file {
                    format!("\nOutput file: Write your structured review notes to `{rf}`.\n")
                } else {
                    String::new()
                };
                format!(
                    r#"You are a senior staff engineer reviewing system design documents. Your goal is to ensure the design is complete, technically sound, and ready for implementation.{file_clause}

Review Checklist:
- **Completeness**: Are all required sections present?
- **Correctness & Feasibility**: Do claims match reality? Are assumptions valid?
- **Scalability & Security**: Will it handle scale? Are failure modes addressed?
- **Alternatives & Risks**: Are meaningful alternatives explored?

Format:
### Issue 1: [Title]
- **Severity**: critical | major | minor | nit
- **Section**: [Section name]
- **Description**: [What is wrong or missing]
- **Suggestion**: [How to fix]
- **Status**: open"#
                )
            }
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReviewStats {
    pub open: usize,
    pub fixed: usize,
    pub wontfix: usize,
    pub addressed: usize,
}

impl fmt::Display for ReviewStats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "open: {}, fixed: {}, wontfix: {}, addressed: {}",
            self.open, self.fixed, self.wontfix, self.addressed
        )
    }
}

pub fn parse_review_stats(markdown: &str) -> ReviewStats {
    let mut stats = ReviewStats::default();
    for line in markdown.lines() {
        let trimmed = line.trim();
        let lower = trimmed.to_ascii_lowercase();
        if lower.contains("status: open") || lower.contains("**status**: open") {
            stats.open += 1;
        } else if lower.contains("status: fixed") || lower.contains("**status**: fixed") {
            stats.fixed += 1;
        } else if lower.contains("status: wontfix")
            || lower.contains("status: won't fix")
            || lower.contains("**status**: wontfix")
        {
            stats.wontfix += 1;
        } else if lower.contains("status: addressed") || lower.contains("**status**: addressed") {
            stats.addressed += 1;
        }
    }
    stats
}

pub fn render_subagent_prompt(
    agent_type: Option<&str>,
    persona: Option<&str>,
    raw_message: &str,
    review_file: Option<&str>,
    summary_file: Option<&str>,
) -> String {
    let mut sections = Vec::new();

    if let Some(p_name) = persona
        && let Some(p_kind) = PersonaPromptKind::parse(p_name)
    {
        sections.push(p_kind.prompt_template(review_file, summary_file));
    } else if let Some(a_name) = agent_type
        && let Some(a_kind) = AgentPromptKind::parse(a_name)
    {
        sections.push(a_kind.prompt_template().to_string());
    }

    if !raw_message.trim().is_empty() {
        sections.push(format!("## Task Instructions\n{raw_message}"));
    }

    sections.join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_explore_prompt_contains_read_only_and_thoroughness() {
        let prompt = render_subagent_prompt(Some("explore"), None, "Find all tests", None, None);
        assert!(prompt.contains("READ-ONLY MODE"));
        assert!(prompt.contains("quick"));
        assert!(prompt.contains("very thorough"));
        assert!(prompt.contains("Find all tests"));
    }

    #[test]
    fn render_plan_prompt_contains_critical_files() {
        let prompt = render_subagent_prompt(Some("plan"), None, "Design auth system", None, None);
        assert!(prompt.contains("Critical Files for Implementation"));
        assert!(prompt.contains("Verification & Test Plan"));
    }

    #[test]
    fn render_reviewer_prompt_with_review_file() {
        let prompt = render_subagent_prompt(
            None,
            Some("reviewer"),
            "Audit memory safety",
            Some(".agents/scratch/review.md"),
            None,
        );
        assert!(
            prompt.contains("Write your structured review notes to `.agents/scratch/review.md`")
        );
        assert!(prompt.contains("**Status**: open"));
    }

    #[test]
    fn render_implementer_prompt_dual_mode() {
        let initial = render_subagent_prompt(
            None,
            Some("implementer"),
            "Add feature",
            None,
            Some(".agents/scratch/summary.md"),
        );
        assert!(
            initial.contains("Write an implementation summary to `.agents/scratch/summary.md`")
        );

        let fixing = render_subagent_prompt(
            None,
            Some("implementer"),
            "Fix review issues",
            Some(".agents/scratch/review.md"),
            None,
        );
        assert!(fixing.contains("Review notes file: `.agents/scratch/review.md`"));
        assert!(fixing.contains("Status: open"));
        assert!(fixing.contains("Status: fixed"));
    }

    #[test]
    fn parse_review_stats_counts_accurately() {
        let doc = r#"
## Review Notes
### Issue 1: Missing bounds check
- **Severity**: critical
- **Status**: fixed
- Response: Added saturation.

### Issue 2: Unused import
- **Severity**: nit
- **Status**: wontfix
- Response: Kept for feature parity.

### Issue 3: Potential race condition
- **Severity**: major
- **Status**: open
"#;
        let stats = parse_review_stats(doc);
        assert_eq!(stats.open, 1);
        assert_eq!(stats.fixed, 1);
        assert_eq!(stats.wontfix, 1);
        assert_eq!(stats.addressed, 0);
        assert_eq!(
            stats.to_string(),
            "open: 1, fixed: 1, wontfix: 1, addressed: 0"
        );
    }
}
