use super::*;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

#[test]
fn detects_project_kinds_from_root_markers() {
    let root = test_root();
    fs::write(root.join("Cargo.toml"), "[workspace]\n").unwrap();
    fs::write(root.join("pyproject.toml"), "[project]\n").unwrap();

    assert_eq!(detect_project_kinds(&root), vec!["rust", "python"]);

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn renders_bounded_explicit_execution_state() {
    let state = WorldState {
        workspace: PathBuf::from("repo<&>"),
        os: "windows",
        arch: "x86_64",
        shell: "pwsh",
        mode: ApprovalMode::Interactive,
        available_commands: vec!["git", "cargo"],
        unavailable_commands: vec!["rg"],
        workspace_commands: vec![],
        project_kinds: vec!["rust"],
    };

    let context = state.model_context().unwrap();
    assert!(context.len() <= MAX_WORLD_CONTEXT_BYTES);
    assert!(context.contains("mode=\"default\" approval=\"per_action\""));
    assert!(context.contains("<available_commands>git,cargo</available_commands>"));
    assert!(context.contains("cwd=\"repo&lt;&amp;&gt;\""));

    let automatic = state.with_mode(ApprovalMode::Automatic);
    assert!(
        automatic
            .model_context()
            .unwrap()
            .contains("mode=\"auto\" approval=\"automatic\"")
    );
}

#[test]
fn command_probe_uses_explicit_search_paths() {
    let root = test_root();
    let executable = if cfg!(windows) {
        root.join("example.exe")
    } else {
        root.join("example")
    };
    fs::write(&executable, "test").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&executable, permissions).unwrap();
    }
    let extensions = if cfg!(windows) {
        vec![".exe".to_string()]
    } else {
        vec![String::new()]
    };

    assert!(command_available(
        "example",
        std::slice::from_ref(&root),
        &extensions
    ));
    assert!(!command_available(
        "missing",
        std::slice::from_ref(&root),
        &extensions
    ));

    fs::remove_dir_all(root).unwrap();
}

fn test_root() -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = env::temp_dir().join(format!("mini-agent-world-{nonce}"));
    fs::create_dir(&root).unwrap();
    root
}
