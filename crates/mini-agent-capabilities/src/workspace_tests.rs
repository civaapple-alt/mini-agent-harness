use super::*;
use crate::BackgroundShellManager;
use crate::test_support::{approval_controller, remove_test_root, test_root};
use mini_agent_protocol::{
    ApprovalOutcome, ApprovalPolicy, ThreadId, ToolApprovalRequest, ToolExecutionContext,
    ToolExecutionRequest, ToolExecutionStatus, TurnId,
};

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
    Arc::new(
        Workspace::with_read_roots_and_skill_roots(
            root,
            approval,
            extra_read_roots,
            Vec::new(),
            SkillReadRoots::default(),
            Vec::new(),
            sandbox,
        )
        .unwrap(),
    )
}

fn local_shell_contract_backends() -> Vec<SandboxKind> {
    let mut backends = vec![SandboxKind::Native];
    if docker_linux_container_runtime_available() {
        backends.push(SandboxKind::Docker);
    }
    backends
}

fn automatic_workspace(root: PathBuf) -> Arc<Workspace> {
    workspace(
        root,
        approval_controller(ApprovalPolicy::Automatic, ApprovalOutcome::Approved),
        Vec::new(),
        SandboxKind::Native,
    )
}

fn skill_workspace(root: PathBuf, skill_root: PathBuf) -> Arc<Workspace> {
    Arc::new(
        Workspace::with_read_roots_and_skill_roots(
            root,
            approval_controller(ApprovalPolicy::Interactive, ApprovalOutcome::Denied),
            Vec::new(),
            Vec::new(),
            SkillReadRoots::from_paths(vec![skill_root]),
            Vec::new(),
            SandboxKind::Native,
        )
        .unwrap(),
    )
}

fn turn_context(turn_id: &str) -> ToolExecutionContext {
    ToolExecutionContext {
        thread_id: ThreadId::new("skill-resource-thread"),
        turn_id: TurnId::new(turn_id),
        project_id: None,
        workspace_id: None,
        workspace_revision: None,
        session_id: None,
    }
}

#[test]
fn policy_replacement_uses_full_machine_file_allowance() {
    let approval = ApprovalController::with_callback(ApprovalPolicy::Interactive, |_| {
        panic!("FullMachine file access should not ask the frontend")
    });
    approval.set_policy(SecurityPolicy::for_preset(SecurityPreset::FullMachine));

    assert_eq!(approval.preset(), SecurityPreset::FullMachine);
    approval.approve("read C:/some/file.txt").unwrap();
}

#[test]
fn reads_and_patches_inside_workspace() {
    let root = test_root();
    fs::write(root.join("note.txt"), "hello world").unwrap();
    let workspace = automatic_workspace(root.clone());
    let read = ReadFile(Arc::clone(&workspace));
    let patch = ApplyPatch(workspace);

    let request =
        ToolExecutionRequest::new("read-inside", "read_file", json!({"path": "note.txt"}));
    let admission = read.admission(&request).unwrap();
    assert!(matches!(&admission, ToolAdmission::Allowed { .. }));
    assert_eq!(
        read.execute_after_admission(&request, &admission).status,
        ToolExecutionStatus::Completed
    );

    let first = read.execute(&json!({"path": "note.txt"})).unwrap();
    assert!(first.contains("total_lines=1 | offset=0 | limit=200"));
    assert!(first.contains("1: hello world"));
    let abs_path = root.join("note.txt").to_string_lossy().to_string();
    let absolute = read.execute(&json!({"path": abs_path})).unwrap();
    assert!(absolute.contains("total_lines=1 | offset=0 | limit=200"));
    assert!(absolute.contains("1: hello world"));
    patch
        .execute(&json!({
            "patch": "*** Begin Patch\n*** Update File: note.txt\n@@\n-hello world\n+hello agent\n*** End Patch"
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
    let workspace = automatic_workspace(root.clone());
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
    let workspace = automatic_workspace(root.clone());

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
    let workspace = automatic_workspace(root.clone());
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
    let workspace = automatic_workspace(root.clone());
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
        approval_controller(ApprovalPolicy::Interactive, ApprovalOutcome::Denied),
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
            action: "apply_patch".to_string(),
            target_paths: vec![
                root.join("note.txt")
                    .canonicalize()
                    .unwrap()
                    .display()
                    .to_string()
            ],
            action_summary: Some("apply_patch · 修改 1 个文件".to_string()),
        }
    );
    let error = patch.execute(&request.arguments).unwrap_err();
    assert!(error.0.contains("denied"), "{error:?}");
    assert_eq!(fs::read_to_string(root.join("note.txt")).unwrap(), "keep\n");

    remove_test_root(&root);
}

#[test]
fn trusted_policy_directly_admits_non_destructive_patches_but_not_deletes() {
    let root = test_root();
    fs::write(root.join("note.txt"), "keep\n").unwrap();
    let approval = ApprovalController::with_callback(ApprovalPolicy::Trusted, |_| {
        panic!("trusted non-destructive patch should not request approval")
    });
    let workspace = workspace(root.clone(), approval, Vec::new(), SandboxKind::Native);
    let patch = ApplyPatch(Arc::clone(&workspace));
    let update = ToolExecutionRequest::new(
        "trusted-update",
        "apply_patch",
        json!({
            "patch": "*** Begin Patch\n*** Update File: note.txt\n@@\n-keep\n+changed\n*** End Patch"
        }),
    );
    assert!(matches!(
        patch.admission(&update).unwrap(),
        ToolAdmission::Allowed { .. }
    ));

    let delete = ToolExecutionRequest::new(
        "trusted-delete",
        "apply_patch",
        json!({"patch": "*** Begin Patch\n*** Delete File: note.txt\n*** End Patch"}),
    );
    match patch.admission(&delete).unwrap() {
        ToolAdmission::ApprovalRequired {
            action_summary,
            target_paths,
            ..
        } => {
            assert_eq!(
                action_summary.as_deref(),
                Some("apply_patch · 删除 1 个文件")
            );
            assert_eq!(
                target_paths,
                vec![
                    root.join("note.txt")
                        .canonicalize()
                        .unwrap()
                        .display()
                        .to_string()
                ]
            );
        }
        other => panic!("expected delete approval, got {other:?}"),
    }

    let shell = Shell(
        Arc::clone(&workspace),
        ResultStore::default(),
        BackgroundShellManager::new(),
    );
    let high_risk = ToolExecutionRequest::new(
        "trusted-shell",
        "shell",
        json!({"command": "git reset --hard HEAD"}),
    );
    assert!(matches!(
        shell.admission(&high_risk).unwrap(),
        ToolAdmission::ApprovalRequired { .. }
    ));

    remove_test_root(&root);
}

#[test]
fn trusted_policy_auto_approves_ordinary_shell_commands() {
    let root = test_root();
    let approval = ApprovalController::with_callback(ApprovalPolicy::Trusted, |_| {
        panic!("ordinary trusted shell command should not request approval")
    });
    let workspace = workspace(root.clone(), approval, Vec::new(), SandboxKind::Native);
    let shell = Shell(
        workspace,
        ResultStore::default(),
        BackgroundShellManager::new(),
    );
    let command = if cfg!(windows) {
        "Write-Output trusted-shell"
    } else {
        "printf trusted-shell"
    };

    assert!(shell.execute(&json!({"command": command})).is_ok());
    remove_test_root(&root);
}

#[test]
fn read_image_uploads_and_rejects_type_mismatch() {
    let root = test_root();
    fs::write(root.join("shot.png"), crate::image::TINY_PNG).unwrap();
    fs::write(root.join("shot.jpg"), crate::image::TINY_PNG).unwrap();
    let workspace = automatic_workspace(root.clone());
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
    let workspace = automatic_workspace(root.clone());
    let tool = ReadImage {
        workspace: Arc::clone(&workspace),
        store: crate::image::ImageStore::with_uploader(Arc::new(StubFiles("file-api-outside"))),
    };
    let request = ToolExecutionRequest::new(
        "read-image-outside",
        "read_image",
        json!({"path": abs.to_string_lossy().to_string()}),
    );
    assert!(matches!(
        tool.admission(&request).unwrap(),
        ToolAdmission::ApprovalRequired { .. }
    ));
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
        approval_controller(ApprovalPolicy::Interactive, ApprovalOutcome::Denied),
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
fn read_file_outside_workspace_requires_typed_admission() {
    let root = test_root();
    let other = test_root();
    fs::write(other.join("secret.txt"), "secret").unwrap();
    let workspace = automatic_workspace(root.clone());
    let read = ReadFile(workspace);
    let request = ToolExecutionRequest::new(
        "read-outside",
        "read_file",
        json!({"path": other.join("secret.txt").to_string_lossy().to_string()}),
    );

    let admission = read.admission(&request).unwrap();
    assert!(matches!(&admission, ToolAdmission::ApprovalRequired { .. }));
    assert_eq!(
        read.execute_after_admission(&request, &admission).status,
        ToolExecutionStatus::Completed
    );

    remove_test_root(&other);
    remove_test_root(&root);
}

#[cfg(unix)]
#[test]
fn read_file_rejects_symlink_retarget_after_admission() {
    use std::os::unix::fs::symlink;

    let root = test_root();
    let outside = test_root();
    let inside = root.join("inside.txt");
    let secret = outside.join("secret.txt");
    let alias = root.join("alias.txt");
    fs::write(&inside, "inside").unwrap();
    fs::write(&secret, "outside secret").unwrap();
    symlink(&inside, &alias).unwrap();
    let read = ReadFile(automatic_workspace(root.clone()));
    let request = ToolExecutionRequest::new(
        "read-symlink-race",
        "read_file",
        json!({"path": "alias.txt"}),
    );
    let admission = read.admission(&request).unwrap();
    assert!(matches!(&admission, ToolAdmission::Allowed { .. }));

    fs::remove_file(&alias).unwrap();
    symlink(&secret, &alias).unwrap();
    let outcome = read.execute_after_admission(&request, &admission);
    assert_eq!(outcome.status, ToolExecutionStatus::Failed);
    assert!(!outcome.content.contains("outside secret"));

    remove_test_root(&outside);
    remove_test_root(&root);
}

#[cfg(unix)]
#[test]
fn read_image_rejects_symlink_retarget_after_admission() {
    use std::os::unix::fs::symlink;

    let root = test_root();
    let outside = test_root();
    let inside = root.join("inside.png");
    let secret = outside.join("secret.png");
    let alias = root.join("alias.png");
    fs::write(&inside, crate::image::TINY_PNG).unwrap();
    fs::write(&secret, crate::image::TINY_PNG).unwrap();
    symlink(&inside, &alias).unwrap();
    let tool = ReadImage {
        workspace: automatic_workspace(root.clone()),
        store: crate::image::ImageStore::memory_only(),
    };
    let request = ToolExecutionRequest::new(
        "read-image-symlink-race",
        "read_image",
        json!({"path": "alias.png"}),
    );
    let admission = tool.admission(&request).unwrap();
    assert!(matches!(&admission, ToolAdmission::Allowed { .. }));

    fs::remove_file(&alias).unwrap();
    symlink(&secret, &alias).unwrap();
    let outcome = tool.execute_after_admission(&request, &admission);
    assert_eq!(outcome.status, ToolExecutionStatus::Failed);

    remove_test_root(&outside);
    remove_test_root(&root);
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
        approval_controller(ApprovalPolicy::Interactive, ApprovalOutcome::Denied),
        Vec::new(),
        SandboxKind::Docker,
    );
    let shell = Shell(
        workspace,
        ResultStore::default(),
        BackgroundShellManager::new(),
    );

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

    let workspace = automatic_workspace(root.clone());

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
        approval_controller(ApprovalPolicy::Automatic, ApprovalOutcome::Approved),
        vec![extra_root],
        SandboxKind::Native,
    );
    let location = skill.to_string_lossy().replace('\\', "/");
    let read = ReadFile(Arc::clone(&workspace));

    assert!(
        read.execute(&json!({"path": location}))
            .unwrap()
            .contains("1: extension body")
    );
    assert_eq!(
        fs::read_to_string(extra.join("SKILL.md")).unwrap(),
        "extension body"
    );

    remove_test_root(&extra);
    remove_test_root(&root);
}

#[test]
fn external_enabled_skill_roots_are_readable_without_approval_but_not_writable() {
    let root = test_root();
    let skill_root = test_root();
    fs::create_dir_all(skill_root.join("references")).unwrap();
    fs::create_dir_all(skill_root.join("scripts")).unwrap();
    fs::write(
        skill_root.join("references/patterns.md"),
        "reference pattern\n",
    )
    .unwrap();
    fs::write(skill_root.join("scripts/check.py"), "print('check')\n").unwrap();
    let approval = ApprovalController::with_callback(ApprovalPolicy::Automatic, |_| {
        panic!("external Skill writes must be rejected before requesting approval")
    });
    approval.set_policy(SecurityPolicy::for_preset(SecurityPreset::FullMachine));
    let workspace = Arc::new(
        Workspace::with_read_roots_and_skill_roots(
            root.clone(),
            approval,
            Vec::new(),
            Vec::new(),
            SkillReadRoots::from_paths(vec![skill_root.clone()]),
            Vec::new(),
            SandboxKind::Native,
        )
        .unwrap(),
    );
    let read = ReadFile(Arc::clone(&workspace));
    let path = skill_root.join("references/patterns.md");
    let request = ToolExecutionRequest::new(
        "skill-reference",
        "read_file",
        json!({"path": path.to_string_lossy().to_string()}),
    )
    .with_context(turn_context("turn-read"));

    let admission = read.admission(&request).unwrap();
    assert!(matches!(&admission, ToolAdmission::Allowed { .. }));
    assert!(
        read.execute_after_admission(&request, &admission)
            .content
            .contains("1: reference pattern")
    );
    let script_request = ToolExecutionRequest::new(
        "skill-script",
        "read_file",
        json!({"path": skill_root.join("scripts/check.py").to_string_lossy().to_string()}),
    )
    .with_context(turn_context("turn-read"));
    let script_admission = read.admission(&script_request).unwrap();
    assert!(matches!(&script_admission, ToolAdmission::Allowed { .. }));
    assert!(
        read.execute_after_admission(&script_request, &script_admission)
            .content
            .contains("1: print('check')")
    );

    let patch = ApplyPatch(Arc::clone(&workspace));
    let write = patch.execute(&json!({
        "patch": format!(
            "*** Begin Patch\n*** Update File: {}\n@@\n-reference pattern\n+changed\n*** End Patch",
            path.display()
        )
    }));
    assert!(write.is_err());
    assert_eq!(fs::read_to_string(&path).unwrap(), "reference pattern\n");

    remove_test_root(&skill_root);
    remove_test_root(&root);
}

#[test]
fn workspace_skill_roots_follow_project_write_admission() {
    let root = test_root();
    let skill_root = root.join(".agents/skills/blender-modeling");
    let scripts = skill_root.join("scripts");
    fs::create_dir_all(&scripts).unwrap();
    fs::write(scripts.join("measure.py"), "old\n").unwrap();
    fs::write(scripts.join("remove.py"), "remove me\n").unwrap();
    let workspace = Arc::new(
        Workspace::with_read_roots_and_skill_roots(
            root.clone(),
            approval_controller(ApprovalPolicy::Interactive, ApprovalOutcome::Approved),
            Vec::new(),
            Vec::new(),
            SkillReadRoots::from_paths(vec![skill_root]),
            Vec::new(),
            SandboxKind::Native,
        )
        .unwrap(),
    );
    let patch = ApplyPatch(workspace);
    let patch_text = "*** Begin Patch\n\
*** Update File: .agents/skills/blender-modeling/scripts/measure.py\n\
@@\n-old\n+updated\n\
*** Add File: .agents/skills/blender-modeling/scripts/new.py\n\
+created\n\
*** Delete File: .agents/skills/blender-modeling/scripts/remove.py\n\
*** End Patch";
    let request = ToolExecutionRequest::new(
        "workspace-skill-edit",
        "apply_patch",
        json!({"patch": patch_text}),
    );

    assert!(matches!(
        patch.admission(&request).unwrap(),
        ToolAdmission::ApprovalRequired { .. }
    ));
    patch.execute(&json!({"patch": patch_text})).unwrap();

    assert_eq!(
        fs::read_to_string(scripts.join("measure.py")).unwrap(),
        "updated\n"
    );
    assert_eq!(
        fs::read_to_string(scripts.join("new.py")).unwrap(),
        "created\n"
    );
    assert!(!scripts.join("remove.py").exists());

    remove_test_root(&root);
}

#[test]
fn refreshed_skill_read_roots_change_read_admission_on_the_live_tool() {
    let root = test_root();
    let skill_root = test_root();
    let reference = skill_root.join("reference.md");
    fs::write(&reference, "current skill reference\n").unwrap();
    let roots = SkillReadRoots::default();
    let workspace = Arc::new(
        Workspace::with_read_roots_and_skill_roots(
            root.clone(),
            approval_controller(ApprovalPolicy::Interactive, ApprovalOutcome::Denied),
            Vec::new(),
            Vec::new(),
            roots.clone(),
            Vec::new(),
            SandboxKind::Native,
        )
        .unwrap(),
    );
    let read = ReadFile(workspace);
    let request = json!({"path": reference.to_string_lossy().to_string()});

    assert!(
        read.execute(&request)
            .unwrap_err()
            .0
            .contains("escapes the workspace")
    );
    roots.replace(vec![skill_root.clone()]).unwrap();
    assert!(
        read.execute(&request)
            .unwrap()
            .contains("current skill reference")
    );
    roots.replace(Vec::new()).unwrap();
    assert!(
        read.execute(&request)
            .unwrap_err()
            .0
            .contains("escapes the workspace")
    );

    remove_test_root(&skill_root);
    remove_test_root(&root);
}

#[test]
fn session_attachment_roots_are_read_only_and_do_not_open_session_log() {
    let root = test_root();
    let session = test_root();
    let attachments = session.join("attachments");
    fs::create_dir_all(&attachments).unwrap();
    fs::write(attachments.join("note.txt"), "attachment\n").unwrap();
    fs::write(session.join("session.jsonl"), "private\n").unwrap();
    let workspace = Arc::new(
        Workspace::with_read_roots_and_skill_roots(
            root.clone(),
            approval_controller(ApprovalPolicy::Interactive, ApprovalOutcome::Denied),
            Vec::new(),
            vec![attachments.clone()],
            SkillReadRoots::default(),
            Vec::new(),
            SandboxKind::Native,
        )
        .unwrap(),
    );
    let read = ReadFile(Arc::clone(&workspace));
    let attachment = attachments.join("note.txt");
    let request = ToolExecutionRequest::new(
        "session-attachment",
        "read_file",
        json!({"path": attachment.to_string_lossy().to_string()}),
    )
    .with_context(turn_context("turn-session"));
    let admission = read.admission(&request).unwrap();
    assert!(matches!(&admission, ToolAdmission::Allowed { .. }));
    assert!(
        read.execute_after_admission(&request, &admission)
            .content
            .contains("attachment")
    );

    let private = session.join("session.jsonl");
    assert!(
        read.execute(&json!({"path": private.to_string_lossy().to_string()}))
            .is_err()
    );
    let patch = ApplyPatch(Arc::clone(&workspace));
    assert!(patch
        .execute(&json!({
            "patch": format!(
                "*** Begin Patch\n*** Update File: {}\n@@\n-attachment\n+changed\n*** End Patch",
                attachment.display()
            )
        }))
        .is_err());
    assert_eq!(fs::read_to_string(&attachment).unwrap(), "attachment\n");

    remove_test_root(&session);
    remove_test_root(&root);
}

#[test]
fn external_patch_requires_approval_and_can_use_an_explicit_grant() {
    let root = test_root();
    let outside = test_root();
    let path = outside.join("external.txt");
    fs::write(&path, "before\n").unwrap();
    let workspace = workspace(
        root.clone(),
        approval_controller(ApprovalPolicy::Interactive, ApprovalOutcome::Approved),
        Vec::new(),
        SandboxKind::Native,
    );
    let patch = ApplyPatch(Arc::clone(&workspace));
    let request = ToolExecutionRequest::new(
        "external-patch",
        "apply_patch",
        json!({
            "patch": format!(
                "*** Begin Patch\n*** Update File: {}\n@@\n-before\n+after\n*** End Patch",
                path.display()
            )
        }),
    )
    .with_context(turn_context("turn-external"));
    assert!(matches!(
        patch.admission(&request).unwrap(),
        ToolAdmission::ApprovalRequired { .. }
    ));
    patch.execute(&request.arguments).unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), "after\n");

    remove_test_root(&outside);
    remove_test_root(&root);
}

#[cfg(unix)]
#[test]
fn apply_patch_binds_approval_to_canonical_symlink_target() {
    use std::os::unix::fs::symlink;

    let root = test_root();
    let outside = test_root();
    let inside = root.join("inside.txt");
    let secret = outside.join("secret.txt");
    let alias = root.join("alias.txt");
    fs::write(&inside, "before\n").unwrap();
    fs::write(&secret, "before\n").unwrap();
    symlink(&inside, &alias).unwrap();
    let workspace = workspace(
        root.clone(),
        approval_controller(ApprovalPolicy::Interactive, ApprovalOutcome::Approved),
        Vec::new(),
        SandboxKind::Native,
    );
    let patch = ApplyPatch(workspace);
    let request = ToolExecutionRequest::new(
        "patch-symlink-race",
        "apply_patch",
        json!({
            "patch": "*** Begin Patch\n*** Update File: alias.txt\n@@\n-before\n+after\n*** End Patch"
        }),
    );
    let admission = patch.admission(&request).unwrap();
    let expected_target = inside.canonicalize().unwrap().display().to_string();
    assert!(matches!(
        &admission,
        ToolAdmission::ApprovalRequired { target_paths, .. }
            if target_paths.len() == 1 && target_paths[0] == expected_target
    ));

    let grant_key = |target: String| {
        let request = ToolApprovalRequest {
            action: "apply_patch".to_string(),
            tool_name: Some("apply_patch".to_string()),
            workspace_id: Some(root.display().to_string()),
            workspace_revision: Some(1),
            target_paths: vec![target],
            ..ToolApprovalRequest::default()
        };
        crate::security::action_grant_key(&request, "project").unwrap()
    };
    let old_key = grant_key(expected_target);

    fs::remove_file(&alias).unwrap();
    symlink(&secret, &alias).unwrap();
    let new_admission = patch.admission(&request).unwrap();
    let ToolAdmission::ApprovalRequired { target_paths, .. } = new_admission else {
        panic!("retargeted patch should require approval")
    };
    let new_key = grant_key(target_paths[0].clone());
    assert_ne!(old_key, new_key);

    let outcome = patch.execute_after_admission(&request, &admission);
    assert_eq!(outcome.status, ToolExecutionStatus::Failed);
    assert_eq!(fs::read_to_string(&secret).unwrap(), "before\n");

    remove_test_root(&outside);
    remove_test_root(&root);
}

#[test]
fn skill_read_budget_is_scoped_to_a_turn() {
    let root = test_root();
    let skill_root = test_root();
    let content = (0..2_000)
        .map(|index| format!("reference-line-{index:04}-{}", "x".repeat(32)))
        .collect::<Vec<_>>()
        .join("\n");
    let path = skill_root.join("references.md");
    fs::write(&path, content).unwrap();
    let workspace = skill_workspace(root.clone(), skill_root.clone());
    let read = ReadFile(Arc::clone(&workspace));

    let mut failed = false;
    for index in 0..8 {
        let request = ToolExecutionRequest::new(
            format!("skill-budget-{index}"),
            "read_file",
            json!({"path": path.to_string_lossy().to_string()}),
        )
        .with_context(turn_context("turn-budget"));
        let admission = read.admission(&request).unwrap();
        let outcome = read.execute_after_admission(&request, &admission);
        if outcome.status == ToolExecutionStatus::Failed {
            assert!(outcome.content.contains("skill file reads exceed"));
            failed = true;
            break;
        }
    }
    assert!(
        failed,
        "the skill read budget should eventually reject a page"
    );

    let next_turn = ToolExecutionRequest::new(
        "skill-budget-next-turn",
        "read_file",
        json!({"path": path.to_string_lossy().to_string()}),
    )
    .with_context(turn_context("turn-next"));
    let next_turn_admission = read.admission(&next_turn).unwrap();
    assert_eq!(
        read.execute_after_admission(&next_turn, &next_turn_admission)
            .status,
        ToolExecutionStatus::Completed
    );

    remove_test_root(&skill_root);
    remove_test_root(&root);
}

#[test]
fn plan_mode_aliases_plan_md_and_locks_workspace_writes() {
    let root = test_root();
    let session = test_root();
    fs::write(root.join("note.txt"), "workspace note").unwrap();
    let plan_dir = session.join("plan");
    fs::create_dir_all(&plan_dir).unwrap();
    let plan = plan_dir.join("plan.md");
    fs::write(&plan, "# Implementation Plan\n").unwrap();
    let approval = approval_controller(ApprovalPolicy::Automatic, ApprovalOutcome::Approved);
    approval.set_plan_context(Some(plan.clone()), true);
    let workspace = workspace(root.clone(), approval, Vec::new(), SandboxKind::Native);
    let read = ReadFile(Arc::clone(&workspace));
    let patch = ApplyPatch(Arc::clone(&workspace));

    let locked = patch
        .execute(&json!({"patch": "*** Begin Patch\n*** Add File: src.rs\n+fn main() {}\n*** End Patch"}))
        .unwrap_err();
    assert!(
        locked.0.contains("workspace mutations locked in Plan Mode"),
        "{locked:?}"
    );
    let locked_edit = patch
        .execute(&json!({
            "patch": "*** Begin Patch\n*** Update File: note.txt\n@@\n-workspace note\n+changed note\n*** End Patch"
        }))
        .unwrap_err();
    assert!(
        locked_edit
            .0
            .contains("workspace mutations locked in Plan Mode")
    );

    patch
        .execute(&json!({
            "patch": "*** Begin Patch\n*** Update File: plan.md\n@@\n # Implementation Plan\n+- Goals:\n+  - implement auth\n*** End Patch"
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

    patch
        .execute(&json!({
            "patch": "*** Begin Patch\n*** Update File: plan.md\n@@\n - Goals:\n   - implement auth\n+  - add restore\n*** End Patch"
        }))
        .unwrap();
    assert!(fs::read_to_string(&plan).unwrap().contains("- add restore"));

    let shell = Shell(
        Arc::clone(&workspace),
        ResultStore::default(),
        BackgroundShellManager::new(),
    );
    let marker = root.join("should-not-run.txt");
    let marker_text = marker.to_string_lossy();
    let command = if cfg!(windows) {
        format!("Set-Content -LiteralPath '{marker_text}' -Value blocked")
    } else {
        format!("printf blocked > '{marker_text}'")
    };
    assert!(shell.execute(&json!({"command": command})).is_ok());
    assert_eq!(fs::read_to_string(marker).unwrap().trim(), "blocked");

    remove_test_root(&session);
    remove_test_root(&root);
}

#[test]
fn plan_mode_routes_shell_commands_to_approval_admission() {
    let root = test_root();
    let session = test_root();
    let plan = session.join("plan").join("plan.md");
    fs::create_dir_all(plan.parent().unwrap().join("scratch")).unwrap();
    fs::write(&plan, "# Plan\n").unwrap();
    let approval = approval_controller(ApprovalPolicy::Automatic, ApprovalOutcome::Approved);
    approval.set_plan_context(Some(plan), true);
    let workspace = workspace(root.clone(), approval, Vec::new(), SandboxKind::Native);
    let shell = Shell(
        Arc::clone(&workspace),
        ResultStore::default(),
        BackgroundShellManager::new(),
    );

    let request = ToolExecutionRequest::new(
        "plan-scratch",
        "shell",
        json!({"command": "python plan/scratch/probe.py"}),
    );
    assert!(matches!(
        shell.admission(&request),
        Ok(ToolAdmission::ApprovalRequired { .. })
    ));

    let blocked = ToolExecutionRequest::new(
        "plan-project",
        "shell",
        json!({"command": "python ../note.py"}),
    );
    assert!(matches!(
        shell.admission(&blocked),
        Ok(ToolAdmission::ApprovalRequired { .. })
    ));

    remove_test_root(&session);
    remove_test_root(&root);
}

#[test]
fn plan_mode_allows_read_only_shell_inspection() {
    let root = test_root();
    fs::write(root.join("note.txt"), "workspace note").unwrap();
    let session = test_root();
    let plan_dir = session.join("plan");
    fs::create_dir_all(&plan_dir).unwrap();
    let plan = plan_dir.join("plan.md");
    fs::write(&plan, "# Plan\n").unwrap();
    let approval = approval_controller(ApprovalPolicy::Automatic, ApprovalOutcome::Approved);
    approval.set_plan_context(Some(plan), true);
    let workspace = workspace(root.clone(), approval, Vec::new(), SandboxKind::Native);
    let shell = Shell(
        Arc::clone(&workspace),
        ResultStore::default(),
        BackgroundShellManager::new(),
    );
    let command = if cfg!(windows) {
        "Get-ChildItem -Force | Select-Object Name | Format-Table -AutoSize"
    } else {
        "pwd"
    };
    let request =
        ToolExecutionRequest::new("call-shell-read-only", "shell", json!({"command": command}));

    assert!(matches!(
        shell.admission(&request).unwrap(),
        ToolAdmission::Allowed { .. }
    ));
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
fn plan_mode_routes_shell_mutations_through_approval_policy() {
    let root = test_root();
    let session = test_root();
    let plan = session.join("plan").join("plan.md");
    fs::create_dir_all(plan.parent().unwrap()).unwrap();
    fs::write(&plan, "# Plan\n").unwrap();
    let approval = approval_controller(ApprovalPolicy::Automatic, ApprovalOutcome::Approved);
    approval.set_plan_context(Some(plan), true);
    let workspace = workspace(root.clone(), approval, Vec::new(), SandboxKind::Native);
    let shell = Shell(
        Arc::clone(&workspace),
        ResultStore::default(),
        BackgroundShellManager::new(),
    );
    let marker = root.join("approved-by-policy.txt");
    let marker_text = marker.to_string_lossy();
    let command = if cfg!(windows) {
        format!("Set-Content -LiteralPath '{marker_text}' -Value approved")
    } else {
        format!("printf approved > '{marker_text}'")
    };
    let request =
        ToolExecutionRequest::new("plan-shell-mutation", "shell", json!({"command": command}));

    assert!(matches!(
        shell.admission(&request),
        Ok(ToolAdmission::ApprovalRequired { .. })
    ));
    assert!(shell.execute(&request.arguments).is_ok());
    assert_eq!(fs::read_to_string(marker).unwrap().trim(), "approved");

    remove_test_root(&session);
    remove_test_root(&root);
}

#[test]
fn automatic_shell_admission_requires_workspace_bounded_paths() {
    let root = test_root();
    let approval = ApprovalController::with_callback(ApprovalPolicy::Automatic, |_| {
        panic!("bounded read-only shell inspection should not request approval")
    });
    let workspace = workspace(root.clone(), approval, Vec::new(), SandboxKind::Native);
    let shell = Shell(
        Arc::clone(&workspace),
        ResultStore::default(),
        BackgroundShellManager::new(),
    );

    let inside = if cfg!(windows) {
        format!(
            "Get-ChildItem {} -Recurse | Select-Object Name",
            root.display()
        )
    } else {
        format!("rg --files {}", root.display())
    };
    let inside_request =
        ToolExecutionRequest::new("bounded-shell-read", "shell", json!({"command": inside}));
    assert!(matches!(
        shell.admission(&inside_request).unwrap(),
        ToolAdmission::Allowed { .. }
    ));

    let outside = if cfg!(windows) {
        "Get-Content ..\\secret.txt"
    } else {
        "rg --files ../secret"
    };
    let outside_request =
        ToolExecutionRequest::new("outside-shell-read", "shell", json!({"command": outside}));
    assert!(matches!(
        shell.admission(&outside_request).unwrap(),
        ToolAdmission::ApprovalRequired { .. }
    ));

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
    let approval = approval_controller(ApprovalPolicy::Automatic, ApprovalOutcome::Approved);
    approval.set_read_only_agent(true);
    let workspace = workspace(root.clone(), approval, Vec::new(), SandboxKind::Native);
    let patch = ApplyPatch(Arc::clone(&workspace));

    let error = patch
        .execute(
            &json!({"patch": "*** Begin Patch\n*** Add File: note.txt\n+blocked\n*** End Patch"}),
        )
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
    let approval = approval_controller(ApprovalPolicy::Automatic, ApprovalOutcome::Approved);
    approval.set_goal_dir(Some(goal_dir.clone()));
    let workspace = workspace(root.clone(), approval, Vec::new(), SandboxKind::Native);
    let read = ReadFile(Arc::clone(&workspace));
    let patch = ApplyPatch(Arc::clone(&workspace));

    let plan = read.execute(&json!({"path": "goal/plan.md"})).unwrap();
    assert!(plan.contains("Autonomous Goal Plan: Ship HTML intro"));
    let abs = goal_dir.join("plan.md").to_string_lossy().to_string();
    assert!(
        read.execute(&json!({"path": abs}))
            .unwrap()
            .contains("Milestone 1")
    );

    patch
        .execute(&json!({
            "patch": "*** Begin Patch\n*** Update File: goal/plan.md\n@@\n # Autonomous Goal Plan: Ship HTML intro\n+- [x] Milestone 1\n*** End Patch"
        }))
        .unwrap();
    assert!(
        fs::read_to_string(goal_dir.join("plan.md"))
            .unwrap()
            .contains("Milestone 1")
    );
    assert!(!root.join("goal").exists());

    patch
        .execute(&json!({"patch": "*** Begin Patch\n*** Add File: intro.html\n+<html></html>\n*** End Patch"}))
        .unwrap();
    assert_eq!(
        fs::read_to_string(root.join("intro.html")).unwrap(),
        "<html></html>\n"
    );

    remove_test_root(&session);
    remove_test_root(&root);
}

#[test]
fn shell_process_has_a_timeout() {
    let root = test_root();
    for backend in local_shell_contract_backends() {
        let command = if cfg!(windows) && backend == SandboxKind::Native {
            "Start-Sleep -Seconds 5"
        } else {
            "sleep 5"
        };
        let output = run_shell(command, &root, backend, Duration::from_millis(50)).unwrap();
        assert!(
            output.text.contains("timed out"),
            "{backend}: {}",
            output.text
        );
    }
    remove_test_root(&root);
}

#[test]
fn shell_execution_uses_the_workspace_as_its_working_directory() {
    let root = test_root();
    let marker = ".mini-agent-workspace-cwd-probe";
    fs::write(root.join(marker), "workspace").unwrap();
    for backend in local_shell_contract_backends() {
        let command = if cfg!(windows) && backend == SandboxKind::Native {
            format!(
                "if (Test-Path -LiteralPath '{marker}') {{ 'workspace-cwd' }} else {{ 'wrong-cwd' }}"
            )
        } else {
            format!("test -f '{marker}' && printf 'workspace-cwd'")
        };
        let output = run_shell(&command, &root, backend, Duration::from_secs(5)).unwrap();
        assert!(
            output.text.contains("exit: 0") && output.text.contains("workspace-cwd"),
            "{backend}: command did not resolve the workspace marker: {}",
            output.text
        );
    }
    remove_test_root(&root);
}

#[test]
fn shell_output_capture_stays_bounded_for_each_available_backend() {
    let root = test_root();
    for backend in local_shell_contract_backends() {
        let command = if cfg!(windows) && backend == SandboxKind::Native {
            "Write-Output ('x' * 10000000)"
        } else {
            "printf '%010000000d' 0"
        };
        let output = run_shell(command, &root, backend, Duration::from_secs(10)).unwrap();
        assert!(output.source_truncated, "{backend}");
        assert!(output.source_bytes >= 10_000_000, "{backend}");
        assert!(
            output.text.len() <= MAX_COMMAND_CAPTURE_BYTES + 256,
            "{backend}"
        );
    }
    remove_test_root(&root);
}

#[test]
fn shell_process_can_be_cancelled_before_its_deadline() {
    let root = test_root();
    for backend in local_shell_contract_backends() {
        let command = if cfg!(windows) && backend == SandboxKind::Native {
            "Start-Sleep -Seconds 5"
        } else {
            "sleep 5"
        };
        let cancellation = Arc::new(AtomicBool::new(false));
        let cancellation_for_thread = cancellation.clone();
        let trigger = thread::spawn(move || {
            thread::sleep(Duration::from_millis(50));
            cancellation_for_thread.store(true, Ordering::Release);
        });

        let output = run_shell_with_cancel(
            command,
            &root,
            backend,
            Duration::from_secs(5),
            Some(cancellation),
        )
        .unwrap();

        trigger.join().unwrap();
        assert!(output.cancelled, "{backend}: {}", output.text);
        assert!(output.text.contains("cancelled by user"));
        if let Some(name) = output.docker_container_name {
            let status = Command::new("docker")
                .args(["inspect", name.as_str()])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .unwrap();
            assert!(
                !status.success(),
                "cancelled Docker container still exists: {name}"
            );
        }
    }
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
    let approval = ApprovalController::new(ApprovalPolicy::Automatic);
    let results = ResultStore::default();
    let tools = workspace_tools_with_read_roots_and_results(
        root.clone(),
        approval,
        Vec::new(),
        Vec::new(),
        SandboxKind::Native,
        crate::image::ImageStore::memory_only(),
        results,
    )
    .unwrap();
    let shell = tools
        .iter()
        .find(|tool| tool.spec().name == "shell")
        .unwrap();
    let read_output = tools
        .iter()
        .find(|tool| tool.spec().name == "read_tool_output")
        .unwrap();
    let command = if cfg!(windows) {
        "Write-Output ('x' * 20000)"
    } else {
        "printf '%020000d' 0"
    };

    let request = ToolExecutionRequest::new("large-output", "shell", json!({"command": command}));
    let admission = shell.admission(&request).unwrap();
    let outcome = shell.execute_after_admission(&request, &admission);
    assert!(outcome.output_truncated);
    assert!(outcome.content.contains("read_tool_output"));
    let handle = outcome
        .content
        .split("Full output handle: ")
        .nth(1)
        .unwrap()
        .split('.')
        .next()
        .unwrap();
    let page = read_output
        .execute(&json!({"handle": handle, "cursor": 0, "max_bytes": 1024}))
        .unwrap();
    assert!(page.contains("cursor: 0"));
    assert!(page.contains(&"0".repeat(64)) || page.contains(&"x".repeat(64)));

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
        ApprovalController::with_preset(ApprovalPolicy::Automatic, SecurityPreset::Default),
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
        ApprovalController::with_preset(ApprovalPolicy::Automatic, SecurityPreset::FullMachine),
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
