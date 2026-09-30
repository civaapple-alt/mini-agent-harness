use std::collections::{BTreeSet, HashSet};
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

pub const MAX_PROJECT_INSTRUCTIONS_BYTES: usize = 16 * 1024;
pub(crate) const MAX_PROJECT_INSTRUCTION_ROOTS: usize = 16;
pub(crate) const MAX_PROJECT_INSTRUCTIONS_TOTAL_BYTES: usize =
    MAX_PROJECT_INSTRUCTIONS_BYTES * MAX_PROJECT_INSTRUCTION_ROOTS;
const MAX_APPLICABLE_INSTRUCTIONS: usize = 16;
const TRUNCATION_MARKER: &str = "\n[truncated]\n";

#[derive(Clone, Debug)]
pub(crate) struct ProjectInstructionRoot {
    pub label: String,
    pub path: PathBuf,
}

#[derive(Clone, Debug)]
pub(crate) struct InjectedProjectInstruction {
    pub message: String,
    pub record: mini_agent_protocol::ContextInjectionRecord,
    pub warning: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct ProjectInstructionLoader {
    roots: Vec<ProjectInstructionRoot>,
}

impl ProjectInstructionLoader {
    pub(crate) fn from_configured_roots(workspace: &Path, extra_read_roots: &[PathBuf]) -> Self {
        let mut configured = Vec::with_capacity(extra_read_roots.len() + 1);
        configured.push(("主工作区".to_string(), workspace.to_path_buf()));
        configured.extend(
            extra_read_roots
                .iter()
                .enumerate()
                .map(|(index, path)| (format!("附加工作区 {}", index + 1), path.clone())),
        );
        let mut roots = Vec::new();
        let mut seen = HashSet::new();
        for (label, path) in configured {
            let Ok(path) = path.canonicalize() else {
                continue;
            };
            if !path.is_dir() || !seen.insert(path.clone()) {
                continue;
            }
            if roots.len() == MAX_PROJECT_INSTRUCTION_ROOTS {
                break;
            }
            roots.push(ProjectInstructionRoot { label, path });
        }
        Self { roots }
    }

    pub(crate) fn startup_injections(&self) -> Result<Vec<InjectedProjectInstruction>, String> {
        let mut injections = Vec::new();
        let mut total_bytes = 0usize;
        for root in &self.roots {
            if let Some(injection) = load_source(root, Path::new("AGENTS.md"))? {
                total_bytes = total_bytes.saturating_add(injection.record.bytes as usize);
                if total_bytes > MAX_PROJECT_INSTRUCTIONS_TOTAL_BYTES {
                    return Err(format!(
                        "workspace AGENTS.md sources exceed the {} byte aggregate limit",
                        MAX_PROJECT_INSTRUCTIONS_TOTAL_BYTES
                    ));
                }
                injections.push(injection);
            }
        }
        Ok(injections)
    }

    /// Finds applicable instructions only for paths already resolved and
    /// admitted by a structured file tool. Shell commands never reach here.
    pub(crate) fn applicable_injections(
        &self,
        target_paths: &[String],
        known: &[mini_agent_protocol::ContextInjectionRecord],
    ) -> Result<Vec<InjectedProjectInstruction>, String> {
        let mut sources = Vec::new();
        for root in &self.roots {
            let mut relative_directories = BTreeSet::new();
            for target in target_paths {
                let Some(target) = canonical_target_path(Path::new(target)) else {
                    continue;
                };
                if !target.starts_with(&root.path) {
                    continue;
                }
                let Some(parent) = target.parent() else {
                    continue;
                };
                let Ok(relative_parent) = parent.strip_prefix(&root.path) else {
                    continue;
                };
                let mut relative = PathBuf::new();
                relative_directories.insert(relative.clone());
                for component in relative_parent.components() {
                    match component {
                        Component::Normal(part) => relative.push(part),
                        Component::CurDir => {}
                        Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                            relative.clear();
                            break;
                        }
                    }
                    relative_directories.insert(relative.clone());
                }
            }
            for directory in relative_directories {
                let relative_file = directory.join("AGENTS.md");
                if let Some(source) = load_source(root, &relative_file)? {
                    sources.push(source);
                    if sources.len() > MAX_APPLICABLE_INSTRUCTIONS {
                        return Err(format!(
                            "applicable AGENTS.md sources exceed the {MAX_APPLICABLE_INSTRUCTIONS} source limit"
                        ));
                    }
                }
            }
        }
        let mut total_bytes = 0usize;
        for source in &sources {
            total_bytes = total_bytes.saturating_add(source.record.bytes as usize);
        }
        if total_bytes > MAX_PROJECT_INSTRUCTIONS_TOTAL_BYTES {
            return Err(format!(
                "applicable AGENTS.md sources exceed the {} byte aggregate limit",
                MAX_PROJECT_INSTRUCTIONS_TOTAL_BYTES
            ));
        }
        sources.retain(|source| {
            !known.iter().any(|record| {
                record.id == source.record.id && record.fingerprint == source.record.fingerprint
            })
        });
        Ok(sources)
    }
}

/// Admission has already resolved the structured tool's target. Canonicalize
/// existing files and otherwise canonicalize the existing parent so workspace
/// aliases and symlinked directories still map to the configured root safely.
fn canonical_target_path(target: &Path) -> Option<PathBuf> {
    if let Ok(path) = target.canonicalize() {
        return Some(path);
    }
    let parent = target.parent()?.canonicalize().ok()?;
    Some(parent.join(target.file_name()?))
}

fn load_source(
    root: &ProjectInstructionRoot,
    relative_path: &Path,
) -> Result<Option<InjectedProjectInstruction>, String> {
    if relative_path.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        return Ok(None);
    }
    let requested_path = root.path.join(relative_path);
    let path = match requested_path.canonicalize() {
        Ok(path) if path.starts_with(&root.path) && path.is_file() => path,
        Ok(_) => return Ok(None),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("cannot read project instructions: {error}")),
    };
    let loaded = load_agents_file(&path)?;
    let AgentsMd::Loaded {
        body,
        truncated,
        source_bytes,
        fingerprint,
    } = loaded
    else {
        return Ok(None);
    };
    let relative = path
        .strip_prefix(&root.path)
        .map_err(|_| "project instruction path escaped its workspace".to_string())?
        .to_string_lossy()
        .replace('\\', "/");
    let identity = format!("{}\n{}", root.label, relative);
    let suffix = mini_agent_protocol::stable_digest(identity.as_bytes()).replace('-', "_");
    let id = format!("workspace_instruction_{suffix}");
    let source = if relative == "AGENTS.md" {
        "工作区 AGENTS.md".to_string()
    } else {
        "子目录 AGENTS.md".to_string()
    };
    let directory = relative
        .rsplit_once('/')
        .map_or(".".to_string(), |(parent, _)| parent.to_string());
    let scope = if directory == "." {
        "整个工作区及其子目录".to_string()
    } else {
        format!("{directory} 及其子目录")
    };
    let message_body = format!("[来源: {source} · {} · {relative}]\n{body}", root.label,);
    let record = mini_agent_protocol::ContextInjectionRecord {
        id,
        kind: mini_agent_protocol::ContextInjectionKind::ProjectInstructions,
        source,
        workspace: Some(root.label.clone()),
        path: Some(relative),
        scope,
        bytes: source_bytes as u64,
        fingerprint,
        supersedes: None,
        reused: false,
    };
    let message = record.context_message(&message_body);
    Ok(Some(InjectedProjectInstruction {
        message,
        record,
        warning: truncated.then(|| {
            format!(
                "AGENTS.md exceeds {MAX_PROJECT_INSTRUCTIONS_BYTES} bytes ({source_bytes}); using bounded head and tail"
            )
        }),
    }))
}

#[derive(Debug, PartialEq, Eq)]
pub enum AgentsMd {
    Absent,
    Loaded {
        body: String,
        truncated: bool,
        source_bytes: usize,
        fingerprint: String,
    },
}

impl AgentsMd {
    pub fn fingerprint(&self) -> Option<String> {
        match self {
            Self::Absent => None,
            Self::Loaded { fingerprint, .. } => Some(fingerprint.clone()),
        }
    }

    pub fn augment(self, base: &str) -> String {
        match self {
            Self::Absent => base.to_string(),
            Self::Loaded { body, .. } => {
                format!("{base}\n\nProject instructions from AGENTS.md:\n---\n{body}\n---")
            }
        }
    }

    pub fn truncation_warning(&self) -> Option<String> {
        match self {
            Self::Loaded {
                truncated: true,
                source_bytes,
                ..
            } => Some(format!(
                "AGENTS.md exceeds {MAX_PROJECT_INSTRUCTIONS_BYTES} bytes ({source_bytes}); using bounded head and tail"
            )),
            Self::Absent | Self::Loaded { .. } => None,
        }
    }
}

pub fn load_agents_md(workspace: &Path) -> Result<AgentsMd, String> {
    let path = workspace.join("AGENTS.md");
    load_agents_file(&path)
}

fn load_agents_file(path: &Path) -> Result<AgentsMd, String> {
    let instructions = match fs::read(path) {
        Ok(instructions) => instructions,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(AgentsMd::Absent),
        Err(error) => return Err(format!("cannot read {}: {error}", path.display())),
    };
    let source_bytes = instructions.len();
    let fingerprint = mini_agent_protocol::stable_digest(&instructions);
    let instructions = String::from_utf8(instructions)
        .map_err(|_| format!("{} must be valid UTF-8", path.display()))?;
    let instructions = instructions.trim();
    if instructions.is_empty() {
        return Ok(AgentsMd::Absent);
    }
    let truncated = instructions.len() > MAX_PROJECT_INSTRUCTIONS_BYTES;
    let body = if truncated {
        truncate_utf8(instructions.to_string(), MAX_PROJECT_INSTRUCTIONS_BYTES)
    } else {
        instructions.to_string()
    };
    Ok(AgentsMd::Loaded {
        body,
        truncated,
        source_bytes,
        fingerprint,
    })
}

fn truncate_utf8(mut content: String, max_bytes: usize) -> String {
    if content.len() <= max_bytes {
        return content;
    }
    if max_bytes <= TRUNCATION_MARKER.len() {
        let end = floor_char_boundary(&content, max_bytes);
        content.truncate(end);
        return content;
    }
    let retained_bytes = max_bytes - TRUNCATION_MARKER.len();
    let head_bytes = retained_bytes.div_ceil(2);
    let tail_bytes = retained_bytes - head_bytes;
    let head_end = floor_char_boundary(&content, head_bytes);
    let tail_start = ceil_char_boundary(&content, content.len() - tail_bytes);
    let mut output = String::with_capacity(max_bytes);
    output.push_str(&content[..head_end]);
    output.push_str(TRUNCATION_MARKER);
    output.push_str(&content[tail_start..]);
    output
}

fn floor_char_boundary(text: &str, mut index: usize) -> usize {
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn ceil_char_boundary(text: &str, mut index: usize) -> usize {
    while index < text.len() && !text.is_char_boundary(index) {
        index += 1;
    }
    index
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::test_root;
    use mini_agent_capabilities::{ApprovalController, ApprovalPolicy};
    use mini_agent_core::{Harness, HarnessConfig, ToolRouter};
    use mini_agent_protocol::{
        Event, Message, Model, ModelEventSink, ModelRequest, ModelResponse, Observer, Tool,
        ToolAdmission, ToolCall, ToolError, ToolExecutionRequest, ToolHandler, ToolRuntime,
        ToolSpec,
    };
    use serde_json::{Value, json};
    use std::convert::Infallible;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    struct ContextScenarioObservation {
        system_prompt: String,
        messages: Vec<Message>,
        tools: Vec<ToolSpec>,
    }

    struct ContextScenarioModel {
        calls: usize,
        target: String,
        observations: Arc<Mutex<Vec<ContextScenarioObservation>>>,
    }

    impl Model for ContextScenarioModel {
        type Error = Infallible;

        async fn respond<'a>(
            &'a mut self,
            request: ModelRequest<'a>,
            _events: &'a mut (dyn ModelEventSink + Send),
        ) -> Result<ModelResponse, Self::Error> {
            self.observations
                .lock()
                .unwrap()
                .push(ContextScenarioObservation {
                    system_prompt: request.system_prompt.to_string(),
                    messages: request.messages.to_vec(),
                    tools: request.tools.to_vec(),
                });
            let call_index = self.calls;
            self.calls += 1;
            if call_index < 2 {
                Ok(ModelResponse {
                    reasoning: String::new(),
                    text: String::new(),
                    tool_calls: vec![ToolCall {
                        id: format!("read-{call_index}"),
                        name: "read_file".to_string(),
                        arguments: json!({"path": self.target}),
                    }],
                    usage: None,
                })
            } else {
                Ok(ModelResponse {
                    reasoning: String::new(),
                    text: "read after reviewing the nested instructions".to_string(),
                    tool_calls: Vec::new(),
                    usage: None,
                })
            }
        }
    }

    struct ContextScenarioReadFile {
        target: String,
        executions: Arc<AtomicUsize>,
    }

    impl ToolHandler for ContextScenarioReadFile {
        fn spec(&self) -> ToolSpec {
            ToolSpec {
                name: "read_file".to_string(),
                description: "Read one test workspace file.".to_string(),
                parameters: json!({"type":"object","properties":{"path":{"type":"string"}}}),
            }
        }

        fn admission(&self, _request: &ToolExecutionRequest) -> Result<ToolAdmission, ToolError> {
            Ok(ToolAdmission::Allowed {
                target_paths: vec![self.target.clone()],
            })
        }
    }

    impl ToolRuntime for ContextScenarioReadFile {
        fn execute(&self, _arguments: &Value) -> Result<String, ToolError> {
            self.executions.fetch_add(1, Ordering::SeqCst);
            Ok("bounded file result".to_string())
        }
    }

    #[derive(Default)]
    struct ContextScenarioObserver(Vec<Event>);

    impl Observer for ContextScenarioObserver {
        fn observe(&mut self, event: &Event) {
            self.0.push(event.clone());
        }
    }

    #[tokio::test]
    async fn mock_model_reassesses_appended_multi_workspace_and_nested_instructions() {
        let workspace = test_root();
        let extra = test_root();
        fs::create_dir_all(workspace.join("src/nested")).unwrap();
        fs::write(workspace.join("AGENTS.md"), "main workspace instruction").unwrap();
        fs::write(extra.join("AGENTS.md"), "additional workspace instruction").unwrap();
        fs::write(workspace.join("src/AGENTS.md"), "src directory instruction").unwrap();
        fs::write(
            workspace.join("src/nested/AGENTS.md"),
            "nested directory instruction",
        )
        .unwrap();
        fs::write(workspace.join("src/nested/target.rs"), "fixture").unwrap();

        let loader = ProjectInstructionLoader::from_configured_roots(
            &workspace,
            std::slice::from_ref(&extra),
        );
        let observations = Arc::new(Mutex::new(Vec::new()));
        let executions = Arc::new(AtomicUsize::new(0));
        let target = workspace
            .join("src/nested/target.rs")
            .canonicalize()
            .unwrap()
            .display()
            .to_string();
        let tool: Box<dyn Tool> = Box::new(ContextScenarioReadFile {
            target: target.clone(),
            executions: Arc::clone(&executions),
        });
        let executor = Arc::new(
            crate::tool_orchestrator::ToolOrchestrator::new(ApprovalController::new(
                ApprovalPolicy::Automatic,
            ))
            .with_project_instructions(loader.clone()),
        );
        let mut harness = Harness::new(
            ContextScenarioModel {
                calls: 0,
                target,
                observations: Arc::clone(&observations),
            },
            ToolRouter::with_executor(vec![tool], executor),
            HarnessConfig::default(),
        );
        for injection in loader.startup_injections().unwrap() {
            assert!(
                harness
                    .append_context_injection(injection.message, injection.record)
                    .unwrap()
                    .is_some()
            );
        }

        let mut observer = ContextScenarioObserver::default();
        let outcome = harness
            .run("inspect the nested source", &mut observer)
            .await
            .unwrap();

        assert_eq!(
            outcome.final_text,
            "read after reviewing the nested instructions"
        );
        assert_eq!(executions.load(Ordering::SeqCst), 1);
        let observations = observations.lock().unwrap();
        assert_eq!(observations.len(), 3);
        let first_context = observations[0]
            .messages
            .iter()
            .filter_map(|message| match message {
                Message::Context { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(first_context.contains("main workspace instruction"));
        assert!(first_context.contains("additional workspace instruction"));
        assert!(!first_context.contains("nested directory instruction"));
        for observation in observations.iter() {
            assert!(!observation.system_prompt.contains("workspace instruction"));
            assert_eq!(observation.tools, observations[0].tools);
        }
        assert_eq!(
            observations[0].messages,
            observations[1].messages[..observations[0].messages.len()]
        );
        assert_eq!(
            observations[1].messages,
            observations[2].messages[..observations[1].messages.len()]
        );
        let reassessed_context = observations[1]
            .messages
            .iter()
            .filter_map(|message| match message {
                Message::Context { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            reassessed_context
                .find("src directory instruction")
                .unwrap()
                < reassessed_context
                    .find("nested directory instruction")
                    .unwrap()
        );
        assert!(reassessed_context.contains("src directory instruction"));
        assert!(reassessed_context.contains("nested directory instruction"));
        drop(observations);
        assert!(observer.0.iter().any(|event| matches!(
            event,
            Event::ContextInjected { records }
                if records.iter().any(|record| record.path.as_deref() == Some("src/AGENTS.md"))
                    && records.iter().any(|record| record.path.as_deref() == Some("src/nested/AGENTS.md"))
        )));
        assert!(outcome.messages.iter().any(|message| matches!(
            message,
            Message::Tool {
                outcome: Some(mini_agent_protocol::ToolExecutionStatus::Deferred),
                ..
            }
        )));
        assert!(outcome.messages.iter().any(|message| matches!(
            message,
            Message::Tool {
                outcome: Some(mini_agent_protocol::ToolExecutionStatus::Completed),
                ..
            }
        )));

        fs::remove_dir_all(workspace).unwrap();
        fs::remove_dir_all(extra).unwrap();
    }

    #[test]
    fn appends_bounded_project_instructions() {
        let root = test_root();
        fs::write(root.join("AGENTS.md"), "Run cargo test.\n").unwrap();

        let prompt = load_agents_md(&root).unwrap().augment("base");

        assert_eq!(
            prompt,
            "base\n\nProject instructions from AGENTS.md:\n---\nRun cargo test.\n---"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn truncates_oversized_project_instructions() {
        let root = test_root();
        let mut source = String::from("HEAD-");
        source.push_str(&"x".repeat(MAX_PROJECT_INSTRUCTIONS_BYTES));
        source.push_str("-TAIL");
        fs::write(root.join("AGENTS.md"), &source).unwrap();

        let loaded = load_agents_md(&root).unwrap();
        let AgentsMd::Loaded {
            body,
            truncated,
            source_bytes,
            ..
        } = loaded
        else {
            panic!("expected loaded instructions");
        };
        assert!(truncated);
        assert_eq!(source_bytes, source.len());
        assert!(body.len() <= MAX_PROJECT_INSTRUCTIONS_BYTES);
        assert!(body.starts_with("HEAD-"));
        assert!(body.contains(TRUNCATION_MARKER));
        assert!(body.ends_with("-TAIL"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_invalid_utf8_project_instructions() {
        let root = test_root();
        fs::write(root.join("AGENTS.md"), [0xff, 0xfe]).unwrap();

        let error = load_agents_md(&root).unwrap_err();

        assert!(error.contains("must be valid UTF-8"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn ignores_instruction_symlinks_that_escape_workspace() {
        let workspace = test_root();
        let outside = test_root();
        fs::write(outside.join("AGENTS.md"), "outside secret").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(outside.join("AGENTS.md"), workspace.join("AGENTS.md")).unwrap();
        let loader = ProjectInstructionLoader::from_configured_roots(&workspace, &[]);

        assert!(loader.startup_injections().unwrap().is_empty());
        fs::remove_dir_all(workspace).unwrap();
        fs::remove_dir_all(outside).unwrap();
    }
}
