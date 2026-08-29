use super::*;
use crate::test_support::HOME_LOCK;
use std::env;

struct HomeGuard {
    _lock: std::sync::MutexGuard<'static, ()>,
    previous_home: Option<std::ffi::OsString>,
    previous_profile: Option<std::ffi::OsString>,
}

impl Drop for HomeGuard {
    fn drop(&mut self) {
        restore_env("HOME", self.previous_home.take());
        restore_env("USERPROFILE", self.previous_profile.take());
    }
}

fn restore_env(key: &str, previous: Option<std::ffi::OsString>) {
    match previous {
        Some(value) => unsafe { env::set_var(key, value) },
        None => unsafe { env::remove_var(key) },
    }
}

#[test]
fn persists_and_resumes_the_latest_settled_checkpoint() {
    let (root, _home) = test_root();
    let mut opened = SessionStore::open(&root, SessionRequest::New).unwrap();
    let session_id = opened.store.session_id().to_string();
    let context = Message::Context {
        text: "<world_state />".to_string(),
    };
    let initial = vec![context.clone()];
    opened.store.record_context(&context, &initial).unwrap();
    let turn_messages = vec![
        Message::User {
            text: "hello".to_string(),
        },
        Message::Assistant {
            reasoning: String::new(),
            text: "hi".to_string(),
            tool_calls: Vec::new(),
        },
    ];
    let mut checkpoint = initial;
    checkpoint.extend(turn_messages.clone());
    opened
        .store
        .record_turn(TurnCommit {
            started_at_ms: timestamp_ms(),
            prompt: "hello",
            status: TurnStatus::Completed,
            steps: 1,
            error: None,
            messages: &turn_messages,
            checkpoint: &checkpoint,
        })
        .unwrap();
    drop(opened);

    let resumed = SessionStore::open(&root, SessionRequest::Resume(session_id)).unwrap();

    assert!(resumed.resumed);
    assert_eq!(resumed.state.messages(), checkpoint);
    drop(resumed);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn removes_a_torn_final_record_and_uses_the_previous_checkpoint() {
    let (root, _home) = test_root();
    let mut opened = SessionStore::open(&root, SessionRequest::New).unwrap();
    let session_id = opened.store.session_id().to_string();
    let path = opened.store.path().to_path_buf();
    let context = Message::Context {
        text: "current".to_string(),
    };
    opened
        .store
        .record_context(&context, std::slice::from_ref(&context))
        .unwrap();
    drop(opened);
    let settled_bytes = fs::metadata(&path).unwrap().len();
    OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(b"{\"kind\":\"checkpoint\"")
        .unwrap();

    let resumed = SessionStore::open(&root, SessionRequest::Resume(session_id)).unwrap();

    assert_eq!(resumed.state.messages(), [context]);
    assert_eq!(fs::metadata(path).unwrap().len(), settled_bytes);
    drop(resumed);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn rejects_unsafe_session_ids() {
    let (root, _home) = test_root();
    let error = SessionStore::open(&root, SessionRequest::Resume("../outside".to_string()))
        .err()
        .unwrap();

    assert!(error.contains("session id must contain"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn derived_items_reference_a_checkpoint_without_entering_resume_history() {
    let (root, _home) = test_root();
    let mut opened = SessionStore::open(&root, SessionRequest::New).unwrap();
    let session_id = opened.store.session_id().to_string();
    let path = opened.store.path().to_path_buf();
    let context = Message::Context {
        text: "settled source".to_string(),
    };
    opened
        .store
        .record_context(&context, std::slice::from_ref(&context))
        .unwrap();
    let checkpoint_seq = opened.store.checkpoint_seq();
    opened
        .store
        .record_derived(DerivedItem {
            item_kind: "mentor_insight",
            provider: "openai_responses",
            model: "mentor-model",
            source_checkpoint_seq: checkpoint_seq,
            source_fingerprint: "fnv1a64:1234",
            criteria: None,
            output: "derived review",
        })
        .unwrap();
    drop(opened);

    let records = fs::read_to_string(&path).unwrap();
    let derived = records
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .find(|record| record["kind"] == "derived_item")
        .unwrap();
    let resumed = SessionStore::open(&root, SessionRequest::Resume(session_id)).unwrap();

    assert_eq!(derived["source"]["checkpoint_seq"], checkpoint_seq);
    assert_eq!(derived["output"], "derived review");
    assert_eq!(resumed.state.messages(), [context]);
    assert_eq!(resumed.store.checkpoint_seq(), checkpoint_seq);
    drop(resumed);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn stores_sessions_under_the_user_config_directory() {
    let (root, _home) = test_root();
    let opened = SessionStore::open(&root, SessionRequest::New).unwrap();
    let session_id = opened.store.session_id().to_string();
    let path = opened.store.path().to_path_buf();
    drop(opened);

    let expected = session_directory(&root)
        .unwrap()
        .join(&session_id)
        .join(SESSION_FILE_NAME);
    assert_eq!(path, expected);
    assert!(path.starts_with(root.join(".mini-agent").join("sessions")));
    assert!(
        !path
            .components()
            .any(|component| component.as_os_str() == ".agents")
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn forks_an_existing_session_into_a_new_session() {
    let (root, _home) = test_root();
    let mut parent = SessionStore::open(&root, SessionRequest::New).unwrap();
    let parent_id = parent.store.session_id().to_string();
    let context = Message::Context {
        text: "<initial_state />".to_string(),
    };
    let initial = vec![context.clone()];
    parent.store.record_context(&context, &initial).unwrap();
    let parent_attachments = parent.store.session_dir().join("attachments");
    fs::create_dir_all(&parent_attachments).unwrap();
    fs::write(parent_attachments.join("att-1.png"), b"png-bytes").unwrap();
    drop(parent);

    // Fork the parent session
    let forked = SessionStore::open(&root, SessionRequest::Fork(parent_id.clone())).unwrap();
    assert_ne!(forked.store.session_id(), parent_id);
    assert!(forked.resumed);
    assert_eq!(forked.state.messages(), [context]);
    assert_eq!(
        fs::read(
            forked
                .store
                .session_dir()
                .join("attachments")
                .join("att-1.png")
        )
        .unwrap(),
        b"png-bytes"
    );
    assert_eq!(
        fs::read(parent_attachments.join("att-1.png")).unwrap(),
        b"png-bytes"
    );
    drop(forked);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn creates_summary_signals_and_prompt_context_files() {
    let (root, _home) = test_root();
    fs::write(root.join("AGENTS.md"), "# Project Guidelines").unwrap();
    let mut opened = SessionStore::open(&root, SessionRequest::New).unwrap();
    let session_dir = opened.store.session_dir().to_path_buf();

    // Verify prompt_context.json was created with AGENTS.md content
    let prompt_ctx_file = session_dir.join(PROMPT_CONTEXT_FILE_NAME);
    assert!(prompt_ctx_file.is_file());
    let prompt_ctx: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&prompt_ctx_file).unwrap()).unwrap();
    assert_eq!(prompt_ctx["agents_md_present"], true);

    // Verify summary.json was created
    let summary_file = session_dir.join(SUMMARY_FILE_NAME);
    assert!(summary_file.is_file());

    // Record a turn and verify summary & signals update
    opened
        .store
        .record_turn(TurnCommit {
            started_at_ms: 1000,
            prompt: "build feature",
            status: TurnStatus::Completed,
            steps: 3,
            error: None,
            messages: &[],
            checkpoint: &[],
        })
        .unwrap();

    let summary: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&summary_file).unwrap()).unwrap();
    assert_eq!(summary["turn_count"], 1);
    assert_eq!(summary["last_prompt"], "build feature");
    assert_eq!(summary["last_status"], "completed");

    let signals_file = session_dir.join(SIGNALS_FILE_NAME);
    assert!(signals_file.is_file());
    let signals: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&signals_file).unwrap()).unwrap();
    assert_eq!(signals["turn_count"], 1);
    assert_eq!(signals["step_count"], 3);

    drop(opened);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn fast_reads_summary_json_in_session_list() {
    let (root, _home) = test_root();
    let mut opened = SessionStore::open(&root, SessionRequest::New).unwrap();
    let session_id = opened.store.session_id().to_string();

    opened
        .store
        .record_turn(TurnCommit {
            started_at_ms: 1000,
            prompt: "test fast listing",
            status: TurnStatus::Completed,
            steps: 2,
            error: None,
            messages: &[],
            checkpoint: &[],
        })
        .unwrap();
    drop(opened);

    let list = list(&root).unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].id, session_id);
    assert!(list[0].bytes > 0);

    fs::remove_dir_all(root).unwrap();
}

fn test_root() -> (PathBuf, HomeGuard) {
    let lock = HOME_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let root = std::env::temp_dir().join(new_id("mini-agent-session-test"));
    fs::create_dir(&root).unwrap();
    let guard = HomeGuard {
        _lock: lock,
        previous_home: env::var_os("HOME"),
        previous_profile: env::var_os("USERPROFILE"),
    };
    unsafe {
        env::set_var("HOME", &root);
        env::set_var("USERPROFILE", &root);
    }
    (root, guard)
}
