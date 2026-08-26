use super::*;
use std::env;
use std::sync::Mutex;

static HOME_LOCK: Mutex<()> = Mutex::new(());

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
    assert_eq!(resumed.messages, checkpoint);
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

    assert_eq!(resumed.messages, vec![context]);
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
    assert_eq!(resumed.messages, vec![context]);
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
    drop(parent);

    // Fork the parent session
    let forked = SessionStore::open(&root, SessionRequest::Fork(parent_id.clone())).unwrap();
    assert_ne!(forked.store.session_id(), parent_id);
    assert!(forked.resumed);
    assert_eq!(forked.messages, vec![context]);
    drop(forked);
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
