use super::*;
use crate::test_support::{remove_test_root, test_root};
use mini_agent_protocol::ToolExecutionStatus;

struct StubFiles(&'static str);

impl crate::image::FileUploader for StubFiles {
    fn upload(&self, _: &str, _: &str, _: &[u8]) -> Result<String, ToolError> {
        Ok(self.0.to_string())
    }
}

fn workspace(
    root: PathBuf,
    approval: ApprovalController,
    extra_read_roots: Vec<PathBuf>,
    sandbox: SandboxKind,
) -> Arc<Workspace> {
    Arc::new(Workspace::with_read_roots(root, approval, extra_read_roots, sandbox).unwrap())
}

#[test]
fn policy_can_be_replaced_after_frontend_callback_creation() {
    let approval = ApprovalController::with_callback(ApprovalMode::Interactive, |_| {
        panic!("profile-selected allow should not ask the frontend")
    });
    approval.set_policy(SecurityPolicy::for_preset(SecurityPreset::Turbomode));

    assert_eq!(approval.preset(), SecurityPreset::Turbomode);
    approval.approve("shell:echo profile").unwrap();
}

#[test]
fn reads_and_edits_inside_workspace() {
    let root = test_root();
    fs::write(root.join("note.txt"), "hello world").unwrap();
    let workspace = workspace(
        root.clone(),
        ApprovalController::new(ApprovalMode::Automatic),
        Vec::new(),
        SandboxKind::Native,
    );
    let read = ReadFile(Arc::clone(&workspace));
    let edit = EditFile(workspace);

    let first = read.execute(&json!({"path": "note.txt"})).unwrap();
    assert!(first.contains("total_lines=1 | offset=0 | limit=200"));
    assert!(first.contains("1: hello world"));
    let abs_path = root.join("note.txt").to_string_lossy().to_string();
    let absolute = read.execute(&json!({"path": abs_path})).unwrap();
    assert!(absolute.contains("total_lines=1 | offset=0 | limit=200"));
    assert!(absolute.contains("1: hello world"));
    edit.execute(&json!({
        "path": abs_path,
        "old_text": "world",
        "new_text": "agent"
    }))
    .unwrap();
    assert_eq!(
        fs::read_to_string(root.join("note.txt")).unwrap(),
        "hello agent"
    );

    remove_test_root(&root);
}

#[test]
fn read_file_paginates_large_sources_without_shell_fallback() {
    let root = test_root();
    let content = (0..900)
        .map(|index| format!("line-{index:04}"))
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(root.join("large.txt"), content).unwrap();
    let workspace = workspace(
        root.clone(),
        ApprovalController::new(ApprovalMode::Automatic),
        Vec::new(),
        SandboxKind::Native,
    );
    let read = ReadFile(workspace);

    let first = read
        .execute(&json!({"path": "large.txt", "limit": 40}))
        .unwrap();
    assert!(first.contains("total_lines=900"));
    assert!(first.contains("1: line-0000"));
    assert!(first.contains("next_offset=40"));
    assert!(first.len() <= MAX_READ_PAGE_BYTES);

    let later = read
        .execute(&json!({"path": "large.txt", "offset": 400, "limit": 3}))
        .unwrap();
    assert!(later.contains("401: line-0400"));
    assert!(later.contains("next_offset=403"));
    assert!(!later.contains("1: line-0000"));

    remove_test_root(&root);
}

#[test]
fn read_file_can_seek_past_the_legacy_128_kibibyte_limit() {
    let root = test_root();
    let content = (0..40_000)
        .map(|index| format!("source-line-{index:05}"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(content.len() > 128 * 1024);
    fs::write(root.join("generated.rs"), content).unwrap();
    let workspace = workspace(
        root.clone(),
        ApprovalController::new(ApprovalMode::Automatic),
        Vec::new(),
        SandboxKind::Native,
    );

    let output = ReadFile(workspace)
        .execute(&json!({
            "path": "generated.rs",
            "offset": 39_999,
            "limit": 1
        }))
        .unwrap();
    assert!(output.contains("40000: source-line-39999"));

    remove_test_root(&root);
}

#[test]
fn apply_patch_updates_adds_and_deletes_as_one_validated_change() {
    let root = test_root();
    fs::write(root.join("old.txt"), "one\ntwo\nthree\n").unwrap();
    fs::write(root.join("remove.txt"), "remove me\n").unwrap();
    let workspace = workspace(
        root.clone(),
        ApprovalController::new(ApprovalMode::Automatic),
        Vec::new(),
        SandboxKind::Native,
    );
    let patch = ApplyPatch(workspace);

    patch
        .execute(&json!({
            "patch": "*** Begin Patch\n*** Update File: old.txt\n@@\n one\n-two\n+TWO\n three\n*** Add File: created.txt\n+created\n*** Delete File: remove.txt\n*** End Patch"
        }))
        .unwrap();

    assert_eq!(
        fs::read_to_string(root.join("old.txt")).unwrap(),
        "one\nTWO\nthree\n"
    );
    assert_eq!(
        fs::read_to_string(root.join("created.txt")).unwrap(),
        "created\n"
    );
    assert!(!root.join("remove.txt").exists());

    patch
        .execute(&json!({
            "patch": "*** Begin Patch\n*** Update File: old.txt\n*** Move to: moved.txt\n@@\n one\n-TWO\n+two\n three\n*** End Patch"
        }))
        .unwrap();
    assert!(!root.join("old.txt").exists());
    assert_eq!(
        fs::read_to_string(root.join("moved.txt")).unwrap(),
        "one\ntwo\nthree\n"
    );

    remove_test_root(&root);
}

#[test]
fn apply_patch_validates_every_file_before_writing_any_file() {
    let root = test_root();
    fs::write(root.join("first.txt"), "first\n").unwrap();
    fs::write(root.join("second.txt"), "second\n").unwrap();
    let workspace = workspace(
        root.clone(),
        ApprovalController::new(ApprovalMode::Automatic),
        Vec::new(),
        SandboxKind::Native,
    );
    let patch = ApplyPatch(workspace);

    let error = patch
        .execute(&json!({
            "patch": "*** Begin Patch\n*** Update File: first.txt\n@@\n-first\n+changed\n*** Update File: second.txt\n@@\n-not-present\n+changed\n*** End Patch"
        }))
        .unwrap_err();
    assert!(error.0.contains("did not match"), "{error:?}");
    assert_eq!(
        fs::read_to_string(root.join("first.txt")).unwrap(),
        "first\n"
    );
    assert_eq!(
        fs::read_to_string(root.join("second.txt")).unwrap(),
        "second\n"
    );

    remove_test_root(&root);
}

#[test]
fn apply_patch_denial_is_explicit_and_has_no_effect() {
    let root = test_root();
    fs::write(root.join("note.txt"), "keep\n").unwrap();
    let workspace = workspace(
        root.clone(),
        ApprovalController::with_callback(ApprovalMode::Interactive, |_| Ok(false)),
        Vec::new(),
        SandboxKind::Native,
    );
    let patch = ApplyPatch(workspace);
    let request = ToolExecutionRequest::new(
        "patch-denied",
        "apply_patch",
        json!({
            "patch": "*** Begin Patch\n*** Update File: note.txt\n@@\n-keep\n+changed\n*** End Patch"
        }),
    );

    assert_eq!(
        patch.admission(&request).unwrap(),
        ToolAdmission::ApprovalRequired {
            action: "apply patch to 1 file(s)".to_string(),
        }
    );
    let error = patch.execute(&request.arguments).unwrap_err();
    assert!(error.0.contains("denied"), "{error:?}");
    assert_eq!(fs::read_to_string(root.join("note.txt")).unwrap(), "keep\n");

    remove_test_root(&root);
}

#[test]
fn read_image_uploads_and_rejects_type_mismatch() {
    let root = test_root();
    fs::write(root.join("shot.png"), crate::image::TINY_PNG).unwrap();
    fs::write(root.join("shot.jpg"), crate::image::TINY_PNG).unwrap();
    let workspace = workspace(
        root.clone(),
        ApprovalController::new(ApprovalMode::Automatic),
        Vec::new(),
        SandboxKind::Native,
    );
    let ok = ReadImage {
        workspace: Arc::clone(&workspace),
        store: crate::image::ImageStore::with_uploader(Arc::new(StubFiles("file-api-test"))),
    };
    let out = ok.execute(&json!({"path": "shot.png"})).unwrap();
    assert!(out.contains("file_id=\"file-api-test\""));
    let mismatch = ReadImage {
        workspace,
        store: crate::image::ImageStore::memory_only(),
    };
    let error = mismatch.execute(&json!({"path": "shot.jpg"})).unwrap_err();
    assert!(error.0.contains("extension declares"));
    remove_test_root(&root);
}

#[test]
fn read_image_accepts_absolute_path_outside_workspace_after_approval() {
    let root = test_root();
    let pictures = test_root();
    fs::write(pictures.join("outside.png"), crate::image::TINY_PNG).unwrap();
    let abs = pictures.join("outside.png").canonicalize().unwrap();
    let workspace = workspace(
        root.clone(),
        ApprovalController::new(ApprovalMode::Automatic),
        Vec::new(),
        SandboxKind::Native,
    );
    let tool = ReadImage {
        workspace: Arc::clone(&workspace),
        store: crate::image::ImageStore::with_uploader(Arc::new(StubFiles("file-api-outside"))),
    };
    let out = tool
        .execute(&json!({"path": abs.to_string_lossy().to_string()}))
        .unwrap();
    assert!(out.contains("file_id=\"file-api-outside\""), "{out}");
    assert!(
        ReadFile(workspace)
            .execute(&json!({"path": abs.to_string_lossy().to_string()}))
            .is_err()
    );
    remove_test_root(&root);
    remove_test_root(&pictures);
}

#[test]
fn read_image_outside_workspace_can_be_denied() {
    let root = test_root();
    let pictures = test_root();
    fs::write(pictures.join("secret.png"), crate::image::TINY_PNG).unwrap();
    let abs = pictures.join("secret.png").canonicalize().unwrap();
    let workspace = workspace(
        root.clone(),
        ApprovalController::with_callback(ApprovalMode::Interactive, |_| Ok(false)),
        Vec::new(),
        SandboxKind::Native,
    );
    let tool = ReadImage {
        workspace,
        store: crate::image::ImageStore::memory_only(),
    };
    let error = tool
        .execute(&json!({"path": abs.to_string_lossy().to_string()}))
        .unwrap_err();
    assert!(error.0.contains("denied"), "{error:?}");
    remove_test_root(&root);
    remove_test_root(&pictures);
}

#[test]
fn shell_denial_is_explicit_before_sandbox_execution() {
    let root = test_root();
    let marker = root.join("should-not-run.txt");
    let marker_text = marker.to_string_lossy();
    let command = if cfg!(windows) {
        format!("Set-Content -LiteralPath '{marker_text}' -Value blocked")
    } else {
        format!("printf blocked > '{marker_text}'")
    };
    let workspace = workspace(
        root.clone(),
        ApprovalController::with_callback(ApprovalMode::Interactive, |_| Ok(false)),
        Vec::new(),
        SandboxKind::Docker,
    );
    let shell = Shell(workspace, ResultStore::default());

    let outcome = shell.execute_outcome(&json!({"command": &command}));

    assert_eq!(outcome.status, ToolExecutionStatus::Failed);
    assert_eq!(
        outcome.content,
        format!("user denied: shell command `{command}`")
    );
    assert!(!marker.exists());
    remove_test_root(&root);
}

#[test]
fn rejects_escape_and_git_paths() {
    let root = test_root();
    let other = test_root();
    fs::write(other.join("secret.txt"), "secret data").unwrap();
    let outside_abs = other.join("secret.txt").to_string_lossy().to_string();

    let workspace = workspace(
        root.clone(),
        ApprovalController::new(ApprovalMode::Automatic),
        Vec::new(),
        SandboxKind::Native,
    );

    assert!(workspace.candidate(&json!({"path": "../secret"})).is_err());
    assert!(
        workspace
            .candidate(&json!({"path": ".git/config"}))
            .is_err()
    );
    assert!(
        workspace
            .candidate(&json!({"path": ".GIT/config"}))
            .is_err()
    );

    let read = ReadFile(Arc::clone(&workspace));
    let err = read.execute(&json!({"path": outside_abs})).unwrap_err();
    assert!(err.0.contains("escapes the workspace"));

    remove_test_root(&root);
    remove_test_root(&other);
}

#[test]
fn read_file_accepts_configured_extension_roots() {
    let root = test_root();
    let extra = test_root();
    fs::write(extra.join("SKILL.md"), "extension body").unwrap();
    let extra_root = extra.canonicalize().unwrap();
    let skill = extra.join("SKILL.md").canonicalize().unwrap();
    let workspace = workspace(
        root.clone(),
        ApprovalController::new(ApprovalMode::Automatic),
        vec![extra_root],
        SandboxKind::Native,
    );
    let location = skill.to_string_lossy().replace('\\', "/");
    let read = ReadFile(Arc::clone(&workspace));
    let edit = EditFile(Arc::clone(&workspace));

    assert!(
        read.execute(&json!({"path": location}))
            .unwrap()
            .contains("1: extension body")
    );
    assert!(
        edit.execute(&json!({
            "path": location,
            "old_text": "extension",
            "new_text": "changed"
        }))
        .is_err()
    );
    assert_eq!(
        fs::read_to_string(extra.join("SKILL.md")).unwrap(),
        "extension body"
    );

    remove_test_root(&extra);
    remove_test_root(&root);
}

#[test]
fn plan_mode_aliases_plan_md_and_locks_workspace_writes() {
    let root = test_root();
    let session = test_root();
    fs::write(root.join("note.txt"), "workspace note").unwrap();
    let plan = session.join("plan.md");
    fs::write(&plan, "# Implementation Plan\n").unwrap();
    let approval = ApprovalController::new(ApprovalMode::Automatic);
    approval.set_living_plan(Some(plan.clone()));
    let workspace = workspace(root.clone(), approval, Vec::new(), SandboxKind::Native);
    let read = ReadFile(Arc::clone(&workspace));
    let edit = EditFile(Arc::clone(&workspace));
    let write = WriteFile(Arc::clone(&workspace));

    let locked = write
        .execute(&json!({"path": "src.rs", "content": "fn main() {}"}))
        .unwrap_err();
    assert!(
        locked.0.contains("workspace mutations locked in Plan Mode"),
        "{locked:?}"
    );
    let locked_edit = edit
        .execute(&json!({
            "path": "note.txt",
            "old_text": "workspace",
            "new_text": "changed"
        }))
        .unwrap_err();
    assert!(
        locked_edit
            .0
            .contains("workspace mutations locked in Plan Mode")
    );

    write
        .execute(&json!({
            "path": "plan.md",
            "content": "# Implementation Plan\n\n- Goals:\n  - implement auth\n"
        }))
        .unwrap();
    let living = fs::read_to_string(&plan).unwrap();
    assert!(living.contains("- implement auth"));
    assert!(!root.join("plan.md").exists());
    assert!(
        read.execute(&json!({"path": "plan.md"}))
            .unwrap()
            .contains("1: # Implementation Plan")
    );

    edit.execute(&json!({
        "path": "plan.md",
        "old_text": "- implement auth",
        "new_text": "- implement auth\n  - add restore"
    }))
    .unwrap();
    assert!(fs::read_to_string(&plan).unwrap().contains("- add restore"));

    let shell = Shell(Arc::clone(&workspace), ResultStore::default());
    let marker = root.join("should-not-run.txt");
    let marker_text = marker.to_string_lossy();
    let command = if cfg!(windows) {
        format!("Set-Content -LiteralPath '{marker_text}' -Value blocked")
    } else {
        format!("printf blocked > '{marker_text}'")
    };
    let locked_shell = shell.execute(&json!({"command": command})).unwrap_err();
    assert!(
        locked_shell
            .0
            .contains("workspace mutations locked in Plan Mode")
    );
    assert!(!marker.exists());

    remove_test_root(&session);
    remove_test_root(&root);
}

#[test]
fn plan_mode_allows_read_only_shell_inspection() {
    let root = test_root();
    fs::write(root.join("note.txt"), "workspace note").unwrap();
    let session = test_root();
    let plan = session.join("plan.md");
    fs::write(&plan, "# Plan\n").unwrap();
    let approval = ApprovalController::new(ApprovalMode::Automatic);
    approval.set_living_plan(Some(plan));
    let workspace = workspace(root.clone(), approval, Vec::new(), SandboxKind::Native);
    let shell = Shell(Arc::clone(&workspace), ResultStore::default());
    let command = if cfg!(windows) {
        "Get-ChildItem -Force | Select-Object Name | Format-Table -AutoSize"
    } else {
        "pwd"
    };
    let request =
        ToolExecutionRequest::new("call-shell-read-only", "shell", json!({"command": command}));

    assert_eq!(
        shell.admission(&request).unwrap(),
        ToolAdmission::ApprovalRequired {
            action: format!("shell command `{command}`"),
        }
    );
    let output = shell.execute(&request.arguments).unwrap();
    assert!(output.contains("note.txt") || output.contains(root.to_string_lossy().as_ref()));

    let chained = if cfg!(windows) {
        "Write-Host '---workspace---'; Get-ChildItem -Force | Select-Object Name"
    } else {
        "pwd; ls"
    };
    assert!(shell.execute(&json!({"command": chained})).is_ok());
    assert!(shell.execute(&json!({"command": "git ls-files"})).is_ok());

    remove_test_root(&session);
    remove_test_root(&root);
}

#[test]
fn read_only_shell_subset_rejects_side_effect_flags() {
    assert!(is_read_only_shell_command("git branch --show-current"));
    assert!(!is_read_only_shell_command("git branch -D stale"));
    assert!(!is_read_only_shell_command("fd --exec echo value"));
    assert!(!is_read_only_shell_command("rg --pre formatter --files"));
    assert!(!is_read_only_shell_command(
        "Get-ChildItem | Where-Object { $_.Name }"
    ));
}

#[test]
fn read_only_agent_rule_locks_workspace_mutations() {
    let root = test_root();
    let approval = ApprovalController::new(ApprovalMode::Automatic);
    approval.set_read_only_agent(true);
    let workspace = workspace(root.clone(), approval, Vec::new(), SandboxKind::Native);
    let write = WriteFile(Arc::clone(&workspace));

    let error = write
        .execute(&json!({"path": "note.txt", "content": "blocked"}))
        .unwrap_err();

    assert_eq!(
        error.0,
        "workspace mutations disabled by the active agent profile"
    );
    remove_test_root(&root);
}

#[test]
fn goal_mode_allows_session_goal_plan_reads_and_workspace_writes() {
    let root = test_root();
    let session = test_root();
    let goal_dir = session.join("goal");
    fs::create_dir_all(&goal_dir).unwrap();
    fs::write(
        goal_dir.join("plan.md"),
        "# Autonomous Goal Plan: Ship HTML intro\n\n## Milestone 1\n",
    )
    .unwrap();
    let approval = ApprovalController::new(ApprovalMode::Automatic);
    approval.set_goal_dir(Some(goal_dir.clone()));
    let workspace = workspace(root.clone(), approval, Vec::new(), SandboxKind::Native);
    let read = ReadFile(Arc::clone(&workspace));
    let write = WriteFile(Arc::clone(&workspace));

    let plan = read.execute(&json!({"path": "goal/plan.md"})).unwrap();
    assert!(plan.contains("Autonomous Goal Plan: Ship HTML intro"));
    let abs = goal_dir.join("plan.md").to_string_lossy().to_string();
    assert!(
        read.execute(&json!({"path": abs}))
            .unwrap()
            .contains("Milestone 1")
    );

    write
        .execute(&json!({
            "path": "goal/plan.md",
            "content": "# Autonomous Goal Plan\n- [x] Milestone 1\n"
        }))
        .unwrap();
    assert!(
        fs::read_to_string(goal_dir.join("plan.md"))
            .unwrap()
            .contains("Milestone 1")
    );
    assert!(!root.join("goal").exists());

    write
        .execute(&json!({"path": "intro.html", "content": "<html></html>"}))
        .unwrap();
    assert_eq!(
        fs::read_to_string(root.join("intro.html")).unwrap(),
        "<html></html>"
    );

    remove_test_root(&session);
    remove_test_root(&root);
}

#[test]
fn write_file_creates_but_does_not_replace() {
    let root = test_root();
    fs::write(root.join("existing.txt"), "keep me").unwrap();
    let workspace = workspace(
        root.clone(),
        ApprovalController::new(ApprovalMode::Automatic),
        Vec::new(),
        SandboxKind::Native,
    );
    let write = WriteFile(workspace);

    write
        .execute(&json!({"path": "new.txt", "content": "new file"}))
        .unwrap();
    assert_eq!(
        fs::read_to_string(root.join("new.txt")).unwrap(),
        "new file"
    );
    assert!(
        write
            .execute(&json!({"path": "existing.txt", "content": "replaced"}))
            .is_err()
    );
    assert_eq!(
        fs::read_to_string(root.join("existing.txt")).unwrap(),
        "keep me"
    );

    remove_test_root(&root);
}

#[test]
fn shell_process_has_a_timeout() {
    let root = test_root();
    let command = if cfg!(windows) {
        "Start-Sleep -Seconds 5"
    } else {
        "sleep 5"
    };

    let output = run_shell(
        command,
        &root,
        SandboxKind::Native,
        Duration::from_millis(50),
    )
    .unwrap();

    assert!(output.text.contains("timed out"));
    remove_test_root(&root);
}

#[test]
fn shell_preserves_utf8_from_workspace_files() {
    let root = test_root();
    fs::write(
        root.join("note.html"),
        "/* 数据统计卡片 */\n<p class=\"tagline\">小巧强悍，性能出众</p>\n",
    )
    .unwrap();
    let command = if cfg!(windows) {
        "$lines = Get-Content note.html; $lines[0..20]"
    } else {
        "cat note.html"
    };
    let output = run_shell(command, &root, SandboxKind::Native, COMMAND_TIMEOUT).unwrap();
    assert!(
        output.text.contains("小巧强悍，性能出众"),
        "stdout was {:?}",
        output.text
    );
    assert!(
        output.text.contains("数据统计卡片"),
        "stdout was {:?}",
        output.text
    );
    let python = if cfg!(windows) { "python" } else { "python3" };
    let py = run_shell(
        &format!(
            "{python} -c \"from pathlib import Path; print(Path('note.html').read_text(encoding='utf-8'))\""
        ),
        &root,
        SandboxKind::Native,
        COMMAND_TIMEOUT,
    );
    if let Ok(py) = py
        && py.text.starts_with("exit: 0\n")
    {
        assert!(
            py.text.contains("小巧强悍，性能出众"),
            "python stdout was {:?}",
            py.text
        );
    }

    remove_test_root(&root);
}

#[test]
fn large_shell_output_is_retained_as_bounded_artifact() {
    let root = test_root();
    let workspace = workspace(
        root.clone(),
        ApprovalController::new(ApprovalMode::Automatic),
        Vec::new(),
        SandboxKind::Native,
    );
    let results = ResultStore::default();
    let shell = Shell(workspace, results.clone());
    let command = if cfg!(windows) {
        "Write-Output ('x' * 20000)"
    } else {
        "printf '%020000d' 0"
    };

    let output = shell.execute(&json!({"command": command})).unwrap();
    assert!(output.contains("handle=\"result-1\""), "{output}");

    remove_test_root(&root);
}

#[test]
fn docker_sandbox_mounts_workspace_and_keeps_container_tmp_ephemeral() {
    let root = test_root();
    let command = "printf mounted > /workspace/docker-mounted.txt; printf ephemeral > /tmp/mini-agent-container-only; pwd; cat /workspace/docker-mounted.txt";
    let result = run_shell(command, &root, SandboxKind::Docker, Duration::from_secs(5));

    match result {
        Ok(output) => {
            assert!(output.text.contains("exit: 0"), "{}", output.text);
            assert!(output.text.contains("/workspace"), "{}", output.text);
            assert!(output.text.contains("mounted"), "{}", output.text);
            assert_eq!(
                fs::read_to_string(root.join("docker-mounted.txt")).unwrap(),
                "mounted"
            );
            assert!(!root.join("mini-agent-container-only").exists());
        }
        Err(error) if error.0.contains("docker sandbox is unavailable") => {}
        Err(error) => panic!("Docker sandbox probe failed: {error}"),
    }
    remove_test_root(&root);
}

#[test]
fn full_machine_preset_permits_paths_outside_workspace() {
    let root = test_root();
    let outside = test_root();
    fs::write(outside.join("outside.txt"), "outside data").unwrap();
    let outside_file = outside.join("outside.txt").to_string_lossy().to_string();

    let default_workspace = workspace(
        root.clone(),
        ApprovalController::with_preset(ApprovalMode::Automatic, SecurityPreset::Default),
        Vec::new(),
        SandboxKind::Native,
    );
    let default_read = ReadFile(default_workspace);
    assert!(
        default_read
            .execute(&json!({"path": &outside_file}))
            .is_err()
    );

    let full_workspace = workspace(
        root.clone(),
        ApprovalController::with_preset(ApprovalMode::Automatic, SecurityPreset::FullMachine),
        Vec::new(),
        SandboxKind::Native,
    );
    let full_read = ReadFile(full_workspace);
    assert!(
        full_read
            .execute(&json!({"path": &outside_file}))
            .unwrap()
            .contains("1: outside data")
    );

    remove_test_root(&root);
    remove_test_root(&outside);
}
