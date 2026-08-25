use super::*;

#[test]
fn persists_and_resumes_the_latest_settled_checkpoint() {
    let root = test_root();
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
    let root = test_root();
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
    let root = test_root();
    let error = SessionStore::open(&root, SessionRequest::Resume("../outside".to_string()))
        .err()
        .unwrap();

    assert!(error.contains("session id must contain"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn derived_items_reference_a_checkpoint_without_entering_resume_history() {
    let root = test_root();
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

fn test_root() -> PathBuf {
    let root = std::env::temp_dir().join(new_id("mini-agent-session-test"));
    fs::create_dir(&root).unwrap();
    root
}
