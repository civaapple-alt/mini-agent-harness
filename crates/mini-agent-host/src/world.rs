use mini_agent_capabilities::SandboxKind;
use mini_agent_capabilities::SecurityPreset;
use mini_agent_protocol::ApprovalPolicy;
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
pub struct WorldState {
    workspace: PathBuf,
    associated_read_roots: Vec<PathBuf>,
    associated_write_roots: Vec<PathBuf>,
    session_read_roots: Vec<PathBuf>,
    root_fingerprint: String,
    os: &'static str,
    arch: &'static str,
    shell: &'static str,
    access: SecurityPreset,
    policy: ApprovalPolicy,
    sandbox: SandboxKind,
    available_commands: Vec<&'static str>,
    unavailable_commands: Vec<&'static str>,
    workspace_commands: Vec<&'static str>,
    project_kinds: Vec<&'static str>,
}

impl WorldState {
    pub fn detect_with_roots(
        workspace: &Path,
        extra_roots: Vec<PathBuf>,
        access: SecurityPreset,
        policy: ApprovalPolicy,
        sandbox: SandboxKind,
    ) -> Self {
        Self::detect_with_root_sets(
            workspace,
            extra_roots,
            Vec::new(),
            Vec::new(),
            access,
            policy,
            sandbox,
        )
    }

    pub fn detect_with_root_sets(
        workspace: &Path,
        associated_read_roots: Vec<PathBuf>,
        associated_write_roots: Vec<PathBuf>,
        session_read_roots: Vec<PathBuf>,
        access: SecurityPreset,
        policy: ApprovalPolicy,
        sandbox: SandboxKind,
    ) -> Self {
        let associated_write_roots = normalize_roots(associated_write_roots);
        let associated_read_roots = normalize_roots(associated_read_roots)
            .into_iter()
            .filter(|root| !associated_write_roots.contains(root))
            .collect::<Vec<_>>();
        let session_read_roots = normalize_roots(session_read_roots);
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
        let mut project_kinds = detect_project_kinds(workspace);
        project_kinds.extend(
            associated_read_roots
                .iter()
                .chain(associated_write_roots.iter())
                .flat_map(|r| detect_project_kinds(r)),
        );
        project_kinds.sort();
        project_kinds.dedup();
        let root_fingerprint =
            root_fingerprint(workspace, &associated_read_roots, &associated_write_roots);
        Self {
            workspace: workspace.to_path_buf(),
            associated_read_roots,
            associated_write_roots,
            session_read_roots,
            root_fingerprint,
            os: env::consts::OS,
            arch: env::consts::ARCH,
            shell: if cfg!(windows) { "pwsh" } else { "sh" },
            access,
            policy,
            sandbox,
            available_commands,
            unavailable_commands,
            workspace_commands,
            project_kinds,
        }
    }

    pub fn with_execution(
        &self,
        access: SecurityPreset,
        policy: ApprovalPolicy,
        sandbox: SandboxKind,
    ) -> Self {
        let mut state = self.clone();
        state.access = access;
        state.policy = policy;
        state.sandbox = sandbox;
        state
    }

    pub fn access(&self) -> SecurityPreset {
        self.access
    }

    pub fn policy(&self) -> ApprovalPolicy {
        self.policy
    }

    pub fn sandbox(&self) -> SandboxKind {
        self.sandbox
    }

    pub fn workspace(&self) -> &Path {
        &self.workspace
    }

    pub fn associated_read_roots(&self) -> &[PathBuf] {
        &self.associated_read_roots
    }

    pub fn associated_write_roots(&self) -> &[PathBuf] {
        &self.associated_write_roots
    }

    pub fn session_read_roots(&self) -> &[PathBuf] {
        &self.session_read_roots
    }

    pub fn root_fingerprint(&self) -> &str {
        &self.root_fingerprint
    }

    /// Compatibility view for callers that have not yet classified associated roots.
    pub fn extra_roots(&self) -> &[PathBuf] {
        &self.associated_read_roots
    }

    pub fn model_context(&self) -> Result<String, String> {
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
        context.push_str("chat");
        context.push_str("\" policy=\"");
        context.push_str(self.policy_name());
        context.push_str("\" access=\"");
        context.push_str(self.access.name());
        context.push_str("\" command_sandbox=\"");
        context.push_str(self.sandbox.name());
        context.push_str("\" direct_file_scope=\"workspace\" />");
        context.push_str("<workspace_roots revision=\"");
        push_xml_escaped(&mut context, &self.root_fingerprint);
        context.push_str("\">");
        push_root(&mut context, &self.workspace, "primary", "read_write");
        for root in &self.associated_read_roots {
            push_root(&mut context, root, "associated", "read_only");
        }
        for root in &self.associated_write_roots {
            push_root(&mut context, root, "associated", "read_write");
        }
        context.push_str("</workspace_roots>");
        for (tag, list) in [
            ("project_kinds", &self.project_kinds),
            ("available_commands", &self.available_commands),
            ("unavailable_commands", &self.unavailable_commands),
            ("workspace_commands", &self.workspace_commands),
        ] {
            push_list_element(&mut context, tag, list);
        }
        context.push_str("<execution_guidance>");
        if !self.associated_read_roots.is_empty() || !self.associated_write_roots.is_empty() {
            context.push_str("Associated roots are part of this project. Read-only roots may be inspected but not modified; read-write roots may be modified within the normal approval and sandbox rules. ");
        }
        context.push_str(match self.policy {
            ApprovalPolicy::Interactive => "Sensitive actions pause for an explicit decision; the decision may be remembered only for its returned action grant scope.",
            ApprovalPolicy::Automatic => "Automatic policy runs low-risk actions without interruption; high-risk, denied, or incomplete actions still require explicit approval.",
            ApprovalPolicy::Trusted => "Trusted policy runs bounded workspace updates without interruption; high-risk, destructive, denied, or external actions still require explicit approval.",
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

    pub fn status_json(&self) -> Value {
        let entry = |p: &Path, primary: bool| {
            json!({
                "name": p.file_name().and_then(|n| n.to_str()).unwrap_or(""),
                "path": p.to_string_lossy(),
                "primary": primary,
                "role": if primary { "primary" } else { "associated" },
                "access": if primary { "read_write" } else { "read_only" },
            })
        };
        let mut roots = vec![entry(&self.workspace, true)];
        roots.extend(self.associated_read_roots.iter().map(|r| entry(r, false)));
        roots.extend(self.associated_write_roots.iter().map(|path| {
            json!({
                "name": path.file_name().and_then(|n| n.to_str()).unwrap_or(""),
                "path": path.to_string_lossy(),
                "primary": false,
                "role": "associated",
                "access": "read_write",
            })
        }));
        json!({
            "os": self.os,
            "arch": self.arch,
            "shell": self.shell,
            "mode": "chat",
            "access": self.access.name(),
            "policy": self.policy_name(),
            "command_sandbox": self.sandbox.name(),
            "direct_file_scope": "workspace",
            "root_fingerprint": self.root_fingerprint,
            "workspace_roots": roots,
            "session_read_roots": self.session_read_roots.iter().map(|path| json!({
                "name": path.file_name().and_then(|n| n.to_str()).unwrap_or(""),
                "path": path.to_string_lossy(),
                "access": "read_only",
                "role": "session_attachment",
            })).collect::<Vec<_>>(),
            "project_kinds": self.project_kinds,
            "available_commands": self.available_commands,
            "unavailable_commands": self.unavailable_commands,
            "workspace_commands": self.workspace_commands,
        })
    }

    pub fn status_lines(&self) -> Vec<String> {
        let mut lines = vec![
            format!("world_os: {} {}", self.os, self.arch),
            format!("world_shell: {}", self.shell),
            "mode: chat".to_string(),
            format!("access: {}", self.access.name()),
            format!("policy: {}", self.policy_name()),
        ];
        for (k, v) in [
            ("project_kinds", &self.project_kinds),
            ("commands_available", &self.available_commands),
            ("commands_unavailable", &self.unavailable_commands),
            ("workspace_commands", &self.workspace_commands),
        ] {
            lines.push(format!("{k}: {}", display_list(v)));
        }
        lines
    }

    fn policy_name(&self) -> &'static str {
        match self.policy {
            ApprovalPolicy::Interactive => "interactive",
            ApprovalPolicy::Automatic => "automatic",
            ApprovalPolicy::Trusted => "trusted",
        }
    }
}

pub fn session_capabilities_context() -> &'static str {
    "<session_capabilities><artifact id=\"session.plan\" logical_path=\"plan.md\" access=\"read_write\" owner=\"plan_runtime\" /><artifact id=\"session.goal\" logical_path=\"goal/\" access=\"controlled\" owner=\"goal_runtime\" /><artifact id=\"session.notebook\" logical_path=\"notebook\" access=\"managed\" owner=\"notebook_runtime\" /><artifact id=\"session.attachments\" logical_path=\"current_turn_attachments\" access=\"read_only\" owner=\"gateway\" /></session_capabilities>"
}

fn push_root(output: &mut String, path: &Path, role: &str, access: &str) {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    output.push_str("<root name=\"");
    push_xml_escaped(output, name);
    output.push_str("\" path=\"");
    push_xml_escaped(output, &path.to_string_lossy());
    output.push_str("\" role=\"");
    push_xml_escaped(output, role);
    output.push_str("\" access=\"");
    push_xml_escaped(output, access);
    output.push_str("\" primary=\"");
    output.push_str(if role == "primary" { "true" } else { "false" });
    output.push_str("\" />");
}

fn normalize_roots(mut roots: Vec<PathBuf>) -> Vec<PathBuf> {
    roots.retain(|path| !path.as_os_str().is_empty());
    roots.sort();
    roots.dedup();
    roots
}

fn root_fingerprint(
    workspace: &Path,
    associated_read_roots: &[PathBuf],
    associated_write_roots: &[PathBuf],
) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    let mut add = |value: &str| {
        for byte in value.as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
        hash ^= 0xff;
        hash = hash.wrapping_mul(0x100000001b3);
    };
    add("primary:read_write");
    add(&workspace.to_string_lossy());
    for root in associated_read_roots {
        add("associated:read_only");
        add(&root.to_string_lossy());
    }
    for root in associated_write_roots {
        add("associated:read_write");
        add(&root.to_string_lossy());
    }
    format!("{hash:016x}")
}

fn detect_project_kinds(workspace: &Path) -> Vec<&'static str> {
    const MARKERS: &[(&str, &[&str])] = &[
        ("rust", &["Cargo.toml"]),
        ("java_maven", &["pom.xml"]),
        ("java_gradle", &["build.gradle", "build.gradle.kts"]),
        ("go", &["go.mod"]),
        (
            "python",
            &["pyproject.toml", "requirements.txt", "setup.py"],
        ),
        ("node", &["package.json"]),
        ("dotnet", &["global.json"]),
    ];
    MARKERS
        .iter()
        .filter_map(|(k, names)| {
            names
                .iter()
                .any(|n| workspace.join(n).is_file())
                .then_some(*k)
        })
        .collect()
}

fn executable_extensions() -> Vec<String> {
    if cfg!(windows) {
        env::var_os("PATHEXT")
            .map(|v| {
                env::split_paths(&v)
                    .filter_map(|p| p.to_str().map(str::to_ascii_lowercase))
                    .collect()
            })
            .unwrap_or_else(|| vec![".com".into(), ".exe".into(), ".bat".into(), ".cmd".into()])
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
    let exts: &[&str] = if cfg!(windows) {
        &[".cmd", ".bat", ""]
    } else {
        &[""]
    };
    exts.iter()
        .any(|ext| executable_file(&workspace.join(format!("{name}{ext}"))))
}

fn executable_file(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|m| {
        m.is_file() && {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                m.permissions().mode() & 0o111 != 0
            }
            #[cfg(not(unix))]
            true
        }
    })
}

fn push_list_element(output: &mut String, name: &str, values: &[&str]) {
    output.push_str(&format!("<{name}>"));
    push_xml_escaped(output, &values.join(","));
    output.push_str(&format!("</{name}>"));
}

fn push_xml_escaped(output: &mut String, value: &str) {
    for c in value.chars() {
        match c {
            '&' => output.push_str("&amp;"),
            '<' => output.push_str("&lt;"),
            '>' => output.push_str("&gt;"),
            '"' => output.push_str("&quot;"),
            '\'' => output.push_str("&apos;"),
            _ => output.push(c),
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
mod tests {
    use super::*;
    use crate::test_support::test_root;

    #[test]
    fn detect_with_roots_includes_workspace_roots_and_guidance() {
        let (root1, root2) = (test_root(), test_root());
        fs::write(root1.join("Cargo.toml"), "").unwrap();
        fs::write(root2.join("pyproject.toml"), "").unwrap();
        let world = WorldState::detect_with_roots(
            &root1,
            vec![root2.clone()],
            SecurityPreset::Default,
            ApprovalPolicy::Interactive,
            SandboxKind::Native,
        );
        assert_eq!(world.extra_roots(), &[root2]);
        assert!(world.project_kinds.contains(&"rust") && world.project_kinds.contains(&"python"));
        let ctx = world.model_context().unwrap();
        assert!(
            ctx.contains("<workspace_roots revision=")
                && ctx.contains("Associated roots are part of this project")
        );
        let roots = world.status_json()["workspace_roots"]
            .as_array()
            .unwrap()
            .len();
        assert_eq!(roots, 2);
    }

    #[test]
    fn session_roots_are_status_only_and_capabilities_are_path_free() {
        let workspace = test_root();
        let attachment_root = test_root();
        let world = WorldState::detect_with_root_sets(
            &workspace,
            Vec::new(),
            Vec::new(),
            vec![attachment_root.clone()],
            SecurityPreset::Default,
            ApprovalPolicy::Interactive,
            SandboxKind::Native,
        );
        let context = world.model_context().unwrap();
        assert!(!context.contains(attachment_root.to_string_lossy().as_ref()));
        assert!(context.contains("<workspace_roots revision="));
        assert!(session_capabilities_context().contains("session.attachments"));
        assert!(!session_capabilities_context().contains("mini-agent"));
        assert_eq!(
            world.status_json()["session_read_roots"][0]["role"],
            "session_attachment"
        );
    }
}
