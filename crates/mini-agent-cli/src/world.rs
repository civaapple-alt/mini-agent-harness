use crate::sandbox::SandboxKind;
use crate::workspace::ApprovalMode;
use serde_json::Value;
use serde_json::json;
use std::env;
use std::fs;
use std::path::Path;
use std::path::PathBuf;

const MAX_WORLD_CONTEXT_BYTES: usize = 8 * 1024;
const MAX_PATH_BYTES: usize = 1024;
const COMMANDS: &[&str] = &[
    "git", "rg", "fd", "tree", "curl", "jq", "cargo", "rustc", "java", "javac", "mvn", "gradle",
    "go", "python", "python3", "uv", "node", "npm", "pnpm", "bun", "deno", "dotnet", "cmake",
    "make", "just", "docker",
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WorldState {
    workspace: PathBuf,
    os: &'static str,
    arch: &'static str,
    shell: &'static str,
    approval: ApprovalMode,
    copilot: bool,
    sandbox: SandboxKind,
    available_commands: Vec<&'static str>,
    unavailable_commands: Vec<&'static str>,
    workspace_commands: Vec<&'static str>,
    project_kinds: Vec<&'static str>,
}

impl WorldState {
    pub(crate) fn detect(
        workspace: &Path,
        approval: ApprovalMode,
        copilot: bool,
        sandbox: SandboxKind,
    ) -> Self {
        let search_paths = env::var_os("PATH")
            .map(|path| env::split_paths(&path).collect::<Vec<_>>())
            .unwrap_or_default();
        let extensions = executable_extensions();
        let (available_commands, unavailable_commands) = COMMANDS
            .iter()
            .copied()
            .partition(|name| command_available(name, &search_paths, &extensions));
        let workspace_commands = ["mvnw", "gradlew"]
            .into_iter()
            .filter(|name| workspace_command_available(workspace, name))
            .collect();
        Self {
            workspace: workspace.to_path_buf(),
            os: env::consts::OS,
            arch: env::consts::ARCH,
            shell: if cfg!(windows) { "pwsh" } else { "sh" },
            approval,
            copilot,
            sandbox,
            available_commands,
            unavailable_commands,
            workspace_commands,
            project_kinds: detect_project_kinds(workspace),
        }
    }

    pub(crate) fn with_execution(
        &self,
        approval: ApprovalMode,
        copilot: bool,
        sandbox: SandboxKind,
    ) -> Self {
        let mut state = self.clone();
        state.approval = approval;
        state.copilot = copilot;
        state.sandbox = sandbox;
        state
    }

    pub(crate) fn approval(&self) -> ApprovalMode {
        self.approval
    }

    pub(crate) fn copilot(&self) -> bool {
        self.copilot
    }

    pub(crate) fn sandbox(&self) -> SandboxKind {
        self.sandbox
    }

    pub(crate) fn workspace(&self) -> &Path {
        &self.workspace
    }

    pub(crate) fn model_context(&self) -> Result<String, String> {
        let mut context = String::from("<world_state>");
        context.push_str("<environment os=\"");
        push_xml_escaped(&mut context, self.os);
        context.push_str("\" arch=\"");
        push_xml_escaped(&mut context, self.arch);
        context.push_str("\" shell=\"");
        push_xml_escaped(&mut context, self.shell);
        context.push_str("\" cwd=\"");
        push_xml_escaped(
            &mut context,
            &bounded_text(&self.workspace.to_string_lossy(), MAX_PATH_BYTES),
        );
        context.push_str("\" />");
        context.push_str("<execution mode=\"");
        context.push_str(self.mode_name());
        context.push_str("\" approval=\"");
        context.push_str(self.approval_name());
        context.push_str("\" command_sandbox=\"");
        context.push_str(self.sandbox.as_str());
        context.push_str("\" direct_file_scope=\"workspace\" />");
        push_list_element(&mut context, "project_kinds", &self.project_kinds);
        push_list_element(&mut context, "available_commands", &self.available_commands);
        push_list_element(
            &mut context,
            "unavailable_commands",
            &self.unavailable_commands,
        );
        push_list_element(&mut context, "workspace_commands", &self.workspace_commands);
        context.push_str("<execution_guidance>");
        context.push_str(match self.approval {
            ApprovalMode::Interactive => {
                "Sensitive writes, shell commands, managed process starts, MCP connections, and MCP calls require per-action user approval."
            }
            ApprovalMode::Automatic => {
                "Work continuously toward the user's goal. Inspect the workspace before editing, use tools as needed, keep changes scoped, and run relevant checks. Do not stop at intermediate progress or ask for confirmation unless blocked by missing information or an unsafe action outside the workspace. Sensitive effects may run without per-action approval, and shell commands are not sandboxed."
            }
        });
        context.push_str("</execution_guidance></world_state>");
        if context.len() > MAX_WORLD_CONTEXT_BYTES {
            Err(format!(
                "world state exceeds {MAX_WORLD_CONTEXT_BYTES} byte limit"
            ))
        } else {
            Ok(context)
        }
    }

    pub(crate) fn status_json(&self) -> Value {
        json!({
            "os": self.os,
            "arch": self.arch,
            "shell": self.shell,
            "mode": self.mode_name(),
            "approval": self.approval_name(),
            "command_sandbox": self.sandbox.as_str(),
            "direct_file_scope": "workspace",
            "project_kinds": self.project_kinds,
            "available_commands": self.available_commands,
            "unavailable_commands": self.unavailable_commands,
            "workspace_commands": self.workspace_commands,
        })
    }

    pub(crate) fn status_lines(&self) -> Vec<String> {
        vec![
            format!("world_os: {} {}", self.os, self.arch),
            format!("world_shell: {}", self.shell),
            format!("mode: {}", self.mode_name()),
            format!("approval: {}", self.approval_name()),
            format!("project_kinds: {}", display_list(&self.project_kinds)),
            format!(
                "commands_available: {}",
                display_list(&self.available_commands)
            ),
            format!(
                "commands_unavailable: {}",
                display_list(&self.unavailable_commands)
            ),
            format!(
                "workspace_commands: {}",
                display_list(&self.workspace_commands)
            ),
        ]
    }

    pub(crate) fn summary(&self) -> String {
        format!(
            "{} {} | {} | {} | approval {} | {} project kind(s) | {}/{} commands available",
            self.os,
            self.arch,
            self.shell,
            self.mode_name(),
            self.approval_name(),
            self.project_kinds.len(),
            self.available_commands.len(),
            COMMANDS.len()
        )
    }

    fn mode_name(&self) -> &'static str {
        if self.copilot { "auto" } else { "default" }
    }

    fn approval_name(&self) -> &'static str {
        match self.approval {
            ApprovalMode::Interactive => "per_action",
            ApprovalMode::Automatic => "automatic",
        }
    }
}

fn detect_project_kinds(workspace: &Path) -> Vec<&'static str> {
    let markers = [
        ("rust", ["Cargo.toml"].as_slice()),
        ("java_maven", ["pom.xml"].as_slice()),
        (
            "java_gradle",
            ["build.gradle", "build.gradle.kts"].as_slice(),
        ),
        ("go", ["go.mod"].as_slice()),
        (
            "python",
            ["pyproject.toml", "requirements.txt", "setup.py"].as_slice(),
        ),
        ("node", ["package.json"].as_slice()),
        ("dotnet", ["global.json"].as_slice()),
    ];
    markers
        .into_iter()
        .filter_map(|(kind, names)| {
            names
                .iter()
                .any(|name| workspace.join(name).is_file())
                .then_some(kind)
        })
        .collect()
}

fn executable_extensions() -> Vec<String> {
    if cfg!(windows) {
        env::var_os("PATHEXT")
            .map(|value| {
                value
                    .to_string_lossy()
                    .split(';')
                    .filter(|extension| !extension.is_empty())
                    .map(str::to_ascii_lowercase)
                    .collect::<Vec<String>>()
            })
            .unwrap_or_else(|| {
                [".com", ".exe", ".bat", ".cmd"]
                    .into_iter()
                    .map(str::to_string)
                    .collect()
            })
    } else {
        vec![String::new()]
    }
}

fn command_available(name: &str, search_paths: &[PathBuf], extensions: &[String]) -> bool {
    search_paths.iter().any(|directory| {
        extensions
            .iter()
            .any(|extension| executable_file(&directory.join(format!("{name}{extension}"))))
    })
}

fn workspace_command_available(workspace: &Path, name: &str) -> bool {
    let candidates = if cfg!(windows) {
        vec![
            workspace.join(format!("{name}.cmd")),
            workspace.join(format!("{name}.bat")),
            workspace.join(name),
        ]
    } else {
        vec![workspace.join(name)]
    };
    candidates.iter().any(|path| executable_file(path))
}

fn executable_file(path: &Path) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn push_list_element(output: &mut String, name: &str, values: &[&str]) {
    output.push_str(&format!("<{name}>"));
    push_xml_escaped(output, &values.join(","));
    output.push_str(&format!("</{name}>"));
}

fn push_xml_escaped(output: &mut String, value: &str) {
    for character in value.chars() {
        match character {
            '&' => output.push_str("&amp;"),
            '<' => output.push_str("&lt;"),
            '>' => output.push_str("&gt;"),
            '\"' => output.push_str("&quot;"),
            '\'' => output.push_str("&apos;"),
            _ => output.push(character),
        }
    }
}

fn bounded_text(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }
    let marker = "…";
    let mut end = max_bytes.saturating_sub(marker.len());
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{marker}", &value[..end])
}

fn display_list(values: &[&str]) -> String {
    if values.is_empty() {
        "none".to_string()
    } else {
        values.join(", ")
    }
}

#[cfg(test)]
#[path = "world_tests.rs"]
mod tests;
