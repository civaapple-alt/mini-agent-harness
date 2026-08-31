use super::*;
use crate::test_support::{remove_test_root, test_root};
use mini_agent_protocol::ToolExecutionStatus;
use std::io::Cursor;

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
    let workspace = Arc::new(
        Workspace::with_read_roots(
            root.clone(),
            ApprovalController::new(ApprovalMode::Automatic),
            Vec::new(),
            SandboxKind::Native,
        )
        .unwrap(),
    );
    let read = ReadFile(Arc::clone(&workspace));
    let edit = EditFile(workspace);

    assert_eq!(
        read.execute(&json!({"path": "note.txt"})).unwrap(),
        "hello world"
    );
    let abs_path = root.join("note.txt").to_string_lossy().to_string();
    assert_eq!(
        read.execute(&json!({"path": abs_path})).unwrap(),
        "hello world"
    );
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
fn read_image_uploads_and_rejects_type_mismatch() {
    struct StubFiles;
    impl crate::image::FileUploader for StubFiles {
        fn upload(&self, _: &str, _: &str, _: &[u8]) -> Result<String, ToolError> {
            Ok("file-api-test".to_string())
        }
    }

    let root = test_root();
    fs::write(root.join("shot.png"), crate::image::TINY_PNG).unwrap();
    fs::write(root.join("shot.jpg"), crate::image::TINY_PNG).unwrap();
    let workspace = Arc::new(
        Workspace::with_read_roots(
            root.clone(),
            ApprovalController::new(ApprovalMode::Automatic),
            Vec::new(),
            SandboxKind::Native,
        )
        .unwrap(),
    );
    let ok = ReadImage {
        workspace: Arc::clone(&workspace),
        store: crate::image::ImageStore::with_uploader(Arc::new(StubFiles)),
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
    struct StubFiles;
    impl crate::image::FileUploader for StubFiles {
        fn upload(&self, _: &str, _: &str, _: &[u8]) -> Result<String, ToolError> {
            Ok("file-api-outside".to_string())
        }
    }

    let root = test_root();
    let pictures = test_root();
    fs::write(pictures.join("outside.png"), crate::image::TINY_PNG).unwrap();
    let abs = pictures.join("outside.png").canonicalize().unwrap();
    let workspace = Arc::new(
        Workspace::with_read_roots(
            root.clone(),
            ApprovalController::new(ApprovalMode::Automatic),
            Vec::new(),
            SandboxKind::Native,
        )
        .unwrap(),
    );
    let tool = ReadImage {
        workspace: Arc::clone(&workspace),
        store: crate::image::ImageStore::with_uploader(Arc::new(StubFiles)),
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
    let workspace = Arc::new(
        Workspace::with_read_roots(
            root.clone(),
            ApprovalController::with_callback(ApprovalMode::Interactive, |_| Ok(false)),
            Vec::new(),
            SandboxKind::Native,
        )
        .unwrap(),
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
    let workspace = Arc::new(
        Workspace::with_read_roots(
            root.clone(),
            ApprovalController::with_callback(ApprovalMode::Interactive, |_| Ok(false)),
            Vec::new(),
            SandboxKind::Docker,
        )
        .unwrap(),
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

    let workspace = Arc::new(
        Workspace::with_read_roots(
            root.clone(),
            ApprovalController::new(ApprovalMode::Automatic),
            Vec::new(),
            SandboxKind::Native,
        )
        .unwrap(),
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
    let workspace = Arc::new(
        Workspace::with_read_roots(
            root.clone(),
            ApprovalController::new(ApprovalMode::Automatic),
            vec![extra_root],
            SandboxKind::Native,
        )
        .unwrap(),
    );
    let location = skill.to_string_lossy().replace('\\', "/");
    let read = ReadFile(Arc::clone(&workspace));
    let edit = EditFile(Arc::clone(&workspace));

    assert_eq!(
        read.execute(&json!({"path": location})).unwrap(),
        "extension body"
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
    let workspace = Arc::new(
        Workspace::with_read_roots(root.clone(), approval, Vec::new(), SandboxKind::Native)
            .unwrap(),
    );
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
    assert_eq!(read.execute(&json!({"path": "plan.md"})).unwrap(), living);

    edit.execute(&json!({
        "path": "plan.md",
        "old_text": "- implement auth",
        "new_text": "- implement auth\n  - add restore"
    }))
    .unwrap();
    assert!(fs::read_to_string(&plan).unwrap().contains("- add restore"));

    let shell = Shell(Arc::clone(&workspace), ResultStore::default());
    let locked_shell = shell
        .execute(&json!({"command": "printf should-not-run"}))
        .unwrap_err();
    assert!(
        locked_shell
            .0
            .contains("workspace mutations locked in Plan Mode")
    );

    remove_test_root(&session);
    remove_test_root(&root);
}

#[test]
fn read_only_agent_rule_locks_workspace_mutations() {
    let root = test_root();
    let approval = ApprovalController::new(ApprovalMode::Automatic);
    approval.set_read_only_agent(true);
    let workspace = Arc::new(
        Workspace::with_read_roots(root.clone(), approval, Vec::new(), SandboxKind::Native)
            .unwrap(),
    );
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
    let workspace = Arc::new(
        Workspace::with_read_roots(root.clone(), approval, Vec::new(), SandboxKind::Native)
            .unwrap(),
    );
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
    let workspace = Arc::new(
        Workspace::with_read_roots(
            root.clone(),
            ApprovalController::new(ApprovalMode::Automatic),
            Vec::new(),
            SandboxKind::Native,
        )
        .unwrap(),
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
fn bounded_capture_keeps_head_and_tail() {
    let captured = capture_bounded(Cursor::new(b"0123456789abcdef"), 8).unwrap();

    assert_eq!(captured.bytes, b"0123cdef");
    assert_eq!(captured.total_bytes, 16);
    assert!(captured.truncated);
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
fn large_shell_output_is_available_through_a_result_handle() {
    let root = test_root();
    let workspace = Arc::new(
        Workspace::with_read_roots(
            root.clone(),
            ApprovalController::new(ApprovalMode::Automatic),
            Vec::new(),
            SandboxKind::Native,
        )
        .unwrap(),
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
    let read = ReadToolResult(results)
        .execute(&json!({"handle": "result-1", "start_byte": 1, "byte_count": 128}))
        .unwrap();
    assert!(read.contains("stored_bytes="));
    assert!(read.len() >= 128);

    remove_test_root(&root);
}

#[test]
fn docker_sandbox_checks_availability_or_reports_clear_error() {
    let root = test_root();
    let result = run_shell(
        "echo hello",
        &root,
        SandboxKind::Docker,
        Duration::from_secs(5),
    );
    if let Err(err) = result {
        assert!(err.0.contains("docker sandbox is unavailable"));
    }
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

    let default_workspace = Arc::new(
        Workspace::with_read_roots(
            root.clone(),
            ApprovalController::with_preset(ApprovalMode::Automatic, SecurityPreset::Default),
            Vec::new(),
            SandboxKind::Native,
        )
        .unwrap(),
    );
    let default_read = ReadFile(default_workspace);
    assert!(
        default_read
            .execute(&json!({"path": &outside_file}))
            .is_err()
    );

    let full_workspace = Arc::new(
        Workspace::with_read_roots(
            root.clone(),
            ApprovalController::with_preset(ApprovalMode::Automatic, SecurityPreset::FullMachine),
            Vec::new(),
            SandboxKind::Native,
        )
        .unwrap(),
    );
    let full_read = ReadFile(full_workspace);
    assert_eq!(
        full_read.execute(&json!({"path": &outside_file})).unwrap(),
        "outside data"
    );

    remove_test_root(&root);
    remove_test_root(&outside);
}
