use serde_json::Value;
use serde_json::json;
use std::fs;
use std::io::ErrorKind;
use std::io::Read;
use std::io::Write;
use std::net::TcpListener;
use std::net::TcpStream;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::process::Stdio;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;
use std::time::Instant;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

#[test]
fn ask_reads_stdin_and_keeps_machine_output_clean() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let (requests_tx, requests_rx) = mpsc::channel();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        requests_tx.send(read_request_body(&mut stream)).unwrap();
        write_reasoning_sse_response(&mut stream, "checking", "script answer");
    });
    let root = test_root();
    fs::write(
        root.join(".env"),
        format!(
            "OPENAI_API_KEY=test-key\nOPENAI_MODEL=test-model\nOPENAI_BASE_URL=http://{address}/v1\n"
        ),
    )
    .unwrap();
    fs::write(root.join("AGENTS.md"), "Use the release contract.\n").unwrap();
    fs::create_dir_all(root.join(".agents/skills/release-review")).unwrap();
    fs::write(
        root.join(".agents/skills/release-review/SKILL.md"),
        "---\nname: release-review\ndescription: Review release readiness when preparing a release.\n---\nFULL SKILL BODY LOADS ON DEMAND.\n",
    )
    .unwrap();
    let mut child = mini_agent(&root)
        .args(["ask", "--json"])
        .env_remove("OPENAI_API_KEY")
        .env_remove("OPENAI_MODEL")
        .env_remove("OPENAI_BASE_URL")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"summarize this repository\n")
        .unwrap();

    let status = wait_for_child(&mut child);
    let mut stdout = String::new();
    let mut stderr = String::new();
    child
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut stdout)
        .unwrap();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();
    server.join().unwrap();
    fs::remove_dir_all(root).unwrap();

    assert!(status.success(), "stderr: {stderr}");
    let output: Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(output["output"], "script answer");
    assert_eq!(output["exit_code"], 0);
    assert_eq!(output["model"], "test-model");
    assert_eq!(output["usage"]["requests"], 1);
    assert!(stderr.contains("thinking> checking"));
    assert!(!stderr.contains("assistant>"));
    let request: Value = serde_json::from_slice(&requests_rx.recv().unwrap()).unwrap();
    assert!(
        request["input"]
            .to_string()
            .contains("summarize this repository")
    );
    assert_eq!(request["input"][0]["role"], "developer");
    assert!(
        request["input"][0]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("<world_state>")
    );
    assert!(
        request["instructions"]
            .as_str()
            .unwrap()
            .contains("Use the release contract.")
    );
    let instructions = request["instructions"].as_str().unwrap();
    assert!(instructions.contains("release-review"));
    assert!(instructions.contains(".agents/skills/release-review/SKILL.md"));
    assert!(!instructions.contains("FULL SKILL BODY LOADS ON DEMAND"));
}

#[test]
fn ask_no_tools_uses_model_only_scope_without_extension_tools() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let (request_tx, request_rx) = mpsc::channel();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        request_tx.send(read_request_body(&mut stream)).unwrap();
        write_reasoning_sse_response(&mut stream, "checking", "model only");
    });
    let root = test_root();
    fs::write(
        root.join(".env"),
        format!(
            "OPENAI_API_KEY=test-key\nOPENAI_MODEL=test-model\nOPENAI_BASE_URL=http://{address}/v1\n"
        ),
    )
    .unwrap();
    fs::create_dir_all(root.join(".agents/skills/should-not-load")).unwrap();
    fs::write(
        root.join(".agents/skills/should-not-load/SKILL.md"),
        "---\nname: should-not-load\ndescription: must not be loaded\n---\n",
    )
    .unwrap();

    let output = mini_agent(&root)
        .args(["ask", "--json", "--no-tools", "explain the scope"])
        .env_remove("OPENAI_API_KEY")
        .env_remove("OPENAI_MODEL")
        .env_remove("OPENAI_BASE_URL")
        .output()
        .unwrap();
    server.join().unwrap();
    let request: Value = serde_json::from_slice(&request_rx.recv().unwrap()).unwrap();
    let body = String::from_utf8(output.stdout).unwrap();
    let response: Value = serde_json::from_str(body.trim()).unwrap();
    fs::remove_dir_all(root).unwrap();

    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    assert_eq!(request["tools"].as_array().unwrap().len(), 0);
    assert!(
        !request["instructions"]
            .as_str()
            .unwrap()
            .contains("should-not-load")
    );
    assert_eq!(response["capabilities"]["profile"], "ask-no-tools");
    assert!(
        response["capabilities"]["disabled"]
            .to_string()
            .contains("tools")
    );
}

#[test]
fn ask_applies_workspace_profile_file_before_running_a_turn() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let (request_tx, request_rx) = mpsc::channel();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        request_tx.send(read_request_body(&mut stream)).unwrap();
        write_reasoning_sse_response(&mut stream, "checking", "profile answer");
    });
    let root = test_root();
    fs::write(
        root.join(".env"),
        format!(
            "OPENAI_API_KEY=test-key\nOPENAI_MODEL=test-model\nOPENAI_BASE_URL=http://{address}/v1\n"
        ),
    )
    .unwrap();
    fs::create_dir_all(root.join(".agents")).unwrap();
    fs::write(
        root.join(".agents/profile.json"),
        r#"{"name":"repo-review","tools":"none","extensionDepth":"none","agent":"plan","persona":"reviewer","workflows":"disabled","sandbox":"none","security":"full-machine"}"#,
    )
    .unwrap();
    let mut child = mini_agent(&root)
        .args(["ask", "--json", "review the repository"])
        .env_remove("OPENAI_API_KEY")
        .env_remove("OPENAI_MODEL")
        .env_remove("OPENAI_BASE_URL")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let status = wait_for_child(&mut child);
    let mut stdout = String::new();
    let mut stderr = String::new();
    child
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut stdout)
        .unwrap();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();
    server.join().unwrap();
    fs::remove_dir_all(root).unwrap();

    assert!(status.success(), "stderr: {stderr}");
    let output: Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(output["output"], "profile answer");
    assert_eq!(output["capabilities"]["profile"], "repo-review");
    assert_eq!(output["capabilities"]["sandbox"], "none");
    assert_eq!(output["capabilities"]["security"], "full-machine");
    assert!(
        output["capabilities"]["disabled"]
            .to_string()
            .contains("no-tools")
    );
    let request: Value = serde_json::from_slice(&request_rx.recv().unwrap()).unwrap();
    assert_eq!(request["tools"].as_array().unwrap().len(), 0);
    assert!(
        request["instructions"]
            .as_str()
            .unwrap()
            .contains("read-only software architect")
    );
}

#[test]
fn durable_session_resumes_settled_history_after_restart() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let (requests_tx, requests_rx) = mpsc::channel();
    let server = thread::spawn(move || {
        for reply in ["first durable answer", "resumed answer"] {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            requests_tx.send(read_request_body(&mut stream)).unwrap();
            write_sse_response(&mut stream, reply);
        }
    });
    let root = test_root();
    fs::write(
        root.join(".env"),
        format!(
            "OPENAI_API_KEY=test-key\nOPENAI_MODEL=test-model\nOPENAI_BASE_URL=http://{address}/v1\n"
        ),
    )
    .unwrap();
    let run = |arguments: &[&str], input: &[u8]| {
        let mut child = mini_agent(&root)
            .args(arguments)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(input).unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    };

    let first_stdout = run(&[], b"first question\n/exit\n");
    let session_id = first_stdout
        .lines()
        .find_map(|line| line.strip_prefix("session> new "))
        .and_then(|line| line.split_once(" |"))
        .map(|(id, _)| id)
        .unwrap();
    let first_session_records = fs::read_to_string(find_session_file(&root, session_id)).unwrap();
    assert!(first_session_records.contains("\"turn_id\":\"turn-1\""));
    let second_stdout = run(&["--session-id", session_id], b"second question\n/exit\n");

    server.join().unwrap();
    let first: Value = serde_json::from_slice(&requests_rx.recv().unwrap()).unwrap();
    let second: Value = serde_json::from_slice(&requests_rx.recv().unwrap()).unwrap();
    assert!(first["input"].to_string().contains("first question"));
    assert!(second_stdout.contains(&format!("session> resumed {session_id}")));
    assert!(second["input"].to_string().contains("first question"));
    assert!(second["input"].to_string().contains("first durable answer"));
    assert!(second["input"].to_string().contains("second question"));
    let resumed_session_records = fs::read_to_string(find_session_file(&root, session_id)).unwrap();
    assert!(resumed_session_records.contains("\"turn_id\":\"turn-2\""));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn goal_mode_runs_a_tool_turn_and_verifies_the_settled_history() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let (requests_tx, requests_rx) = mpsc::channel();
    listener.set_nonblocking(true).unwrap();
    let server = thread::spawn(move || {
        for request_index in 0..7 {
            let started = Instant::now();
            let (mut stream, _) = loop {
                match listener.accept() {
                    Ok(connection) => break connection,
                    Err(error)
                        if (error.kind() == ErrorKind::WouldBlock
                            || error.raw_os_error() == Some(10035))
                            && started.elapsed() < Duration::from_secs(10) =>
                    {
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => {
                        panic!("goal fixture did not receive request {request_index}: {error}")
                    }
                }
            };
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            requests_tx.send(read_request_body(&mut stream)).unwrap();
            match request_index {
                0 => write_tool_sse_response(&mut stream, "echo goal-evidence"),
                1 | 3 | 5 => write_sse_response(&mut stream, "milestone settled"),
                2 | 4 | 6 => write_sse_response(
                    &mut stream,
                    "verdict: approved\nscore: 100\nsummary: fixture accepted",
                ),
                _ => unreachable!(),
            }
        }
    });

    let root = test_root();
    fs::write(
        root.join(".env"),
        format!(
            "OPENAI_API_KEY=test-key\nOPENAI_MODEL=primary-model\nOPENAI_BASE_URL=http://{address}/v1\nVERIFIER_OPENAI_MODEL=verifier-model\n"
        ),
    )
    .unwrap();
    let mut child = mini_agent(&root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"/goal Verify the release\n/exit\n")
        .unwrap();
    let status = wait_for_child(&mut child);
    let mut stdout_bytes = Vec::new();
    let mut stderr_bytes = Vec::new();
    child
        .stdout
        .take()
        .unwrap()
        .read_to_end(&mut stdout_bytes)
        .unwrap();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_end(&mut stderr_bytes)
        .unwrap();
    server.join().unwrap();

    let stdout = String::from_utf8(stdout_bytes).unwrap();
    let stderr = String::from_utf8(stderr_bytes).unwrap();
    assert!(status.success(), "stdout: {stdout}\nstderr: {stderr}");
    assert!(!stderr.contains("there is no reactor running"), "{stderr}");
    assert!(stdout.contains("goal> verifier: Converged"), "{stdout}");

    let requests = (0..7)
        .map(|_| serde_json::from_slice::<Value>(&requests_rx.recv().unwrap()).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(requests[0]["model"], "primary-model");
    assert!(requests[1]["input"].to_string().contains("goal-evidence"));
    for request in [2, 4, 6] {
        assert_eq!(requests[request]["model"], "verifier-model");
        assert_eq!(requests[request]["tools"], json!([]));
        assert!(
            requests[request]["input"]
                .to_string()
                .contains("goal-evidence")
        );
    }

    let session_id = stdout
        .lines()
        .find_map(|line| line.strip_prefix("session> new "))
        .and_then(|line| line.split_once(" |"))
        .map(|(id, _)| id)
        .unwrap();
    let session_file = find_session_file(&root, session_id);
    let state: Value = serde_json::from_slice(
        &fs::read(session_file.parent().unwrap().join("goal/state.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(state["status"], "converged");
    assert_eq!(state["current_milestone"], 3);
    assert!(
        fs::read_to_string(
            session_file
                .parent()
                .unwrap()
                .join("goal/verifier_verdict.md")
        )
        .unwrap()
        .contains("source_checkpoint_seq:")
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn goal_mode_timeout_is_deterministic_and_keeps_repl_alive() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let _ = read_request_body(&mut stream);
        thread::sleep(Duration::from_millis(1_500));
    });

    let root = test_root();
    fs::write(
        root.join(".env"),
        format!(
            "OPENAI_API_KEY=test-key\nOPENAI_MODEL=primary-model\nOPENAI_BASE_URL=http://{address}/v1\nVERIFIER_OPENAI_MODEL=verifier-model\nMINI_AGENT_GOAL_TIMEOUT_SECS=1\n"
        ),
    )
    .unwrap();
    let mut child = mini_agent(&root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"/goal timeout fixture\n/exit\n")
        .unwrap();
    let status = wait_for_child(&mut child);
    let mut stdout = String::new();
    let mut stderr = String::new();
    child
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut stdout)
        .unwrap();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();
    server.join().unwrap();

    assert!(status.success(), "stdout: {stdout}\nstderr: {stderr}");
    assert!(
        stderr.contains("goal> milestone timed out after 1 seconds"),
        "{stderr}"
    );
    assert!(stdout.contains("mini-agent"), "{stdout}");
    let session_id = stdout
        .lines()
        .find_map(|line| line.strip_prefix("session> new "))
        .and_then(|line| line.split_once(" |"))
        .map(|(id, _)| id)
        .unwrap();
    let session_file = find_session_file(&root, session_id);
    let state: Value = serde_json::from_slice(
        &fs::read(session_file.parent().unwrap().join("goal/state.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(state["status"], "failed");
    assert_eq!(state["current_milestone"], 1);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn running_goal_is_paused_when_a_session_restarts() {
    let root = test_root();
    fs::write(
        root.join(".env"),
        "OPENAI_API_KEY=test-key\nOPENAI_MODEL=primary-model\nOPENAI_BASE_URL=http://127.0.0.1:9/v1\nVERIFIER_OPENAI_MODEL=verifier-model\n",
    )
    .unwrap();
    let mut first = mini_agent(&root)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    first
        .stdin
        .take()
        .unwrap()
        .write_all(b"/goal restart fixture\n")
        .unwrap();

    let (session_file, session_id) = wait_for_goal_state(&root);
    let _ = first.kill();
    let _ = first.wait();

    let mut resumed = mini_agent(&root)
        .args(["resume", &session_id])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    resumed.stdin.take().unwrap().write_all(b"/exit\n").unwrap();
    let status = wait_for_child(&mut resumed);
    let mut stdout = String::new();
    let mut stderr = String::new();
    resumed
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut stdout)
        .unwrap();
    resumed
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();

    assert!(status.success(), "stdout: {stdout}\nstderr: {stderr}");
    assert!(stderr.contains("goal> paused on restart"), "{stderr}");
    let state: Value = serde_json::from_slice(
        &fs::read(session_file.parent().unwrap().join("goal/state.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(state["status"], "user_paused");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn steer_interrupts_a_running_turn_at_a_checkpoint() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let (first_request_tx, first_request_rx) = mpsc::channel();
    let (second_request_tx, second_request_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let server = thread::spawn(move || {
        let (mut first_stream, _) = listener.accept().unwrap();
        first_stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        first_request_tx
            .send(read_request_body(&mut first_stream))
            .unwrap();
        release_rx.recv().unwrap();
        write_sse_response(&mut first_stream, "the first turn drifted");

        let (mut second_stream, _) = listener.accept().unwrap();
        second_stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        second_request_tx
            .send(read_request_body(&mut second_stream))
            .unwrap();
        write_sse_response(&mut second_stream, "corrected answer");
    });
    let root = test_root();
    fs::write(
        root.join(".env"),
        format!(
            "OPENAI_API_KEY=test-key\nOPENAI_MODEL=test-model\nOPENAI_BASE_URL=http://{address}/v1\n"
        ),
    )
    .unwrap();
    let mut child = mini_agent(&root)
        .arg("auto")
        .env_remove("OPENAI_API_KEY")
        .env_remove("OPENAI_MODEL")
        .env_remove("OPENAI_BASE_URL")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    stdin.write_all(b"initial request\n").unwrap();
    let first_request = first_request_rx
        .recv_timeout(Duration::from_secs(5))
        .unwrap();
    stdin
        .write_all(b"/steer focus on the actual bug\n/exit\n")
        .unwrap();
    thread::sleep(Duration::from_millis(100));
    release_tx.send(()).unwrap();
    drop(stdin);

    let status = wait_for_child(&mut child);
    let mut stdout = String::new();
    let mut stderr = String::new();
    child
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut stdout)
        .unwrap();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();
    let second_request = second_request_rx
        .recv_timeout(Duration::from_secs(5))
        .unwrap();
    server.join().unwrap();

    assert!(status.success(), "stdout: {stdout}\nstderr: {stderr}");
    assert!(String::from_utf8_lossy(&first_request).contains("initial request"));
    assert!(String::from_utf8_lossy(&second_request).contains("focus on the actual bug"));
    assert!(stdout.contains("steer requested"), "{stdout}");
    assert!(stdout.contains("checkpoint saved"), "{stdout}");
    assert!(stdout.contains("assistant> corrected answer"), "{stdout}");
    let session_id = stdout
        .lines()
        .find_map(|line| line.strip_prefix("session> new "))
        .and_then(|line| line.split_once(" |"))
        .map(|(id, _)| id)
        .unwrap();
    let session_file = find_session_file(&root, session_id);
    let session_content = fs::read_to_string(session_file).unwrap();
    assert!(session_content.contains("\"status\":\"steered\""));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn follow_up_is_queued_until_the_running_turn_finishes() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let (first_request_tx, first_request_rx) = mpsc::channel();
    let (second_request_tx, second_request_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let server = thread::spawn(move || {
        let (mut first_stream, _) = listener.accept().unwrap();
        first_stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        first_request_tx
            .send(read_request_body(&mut first_stream))
            .unwrap();
        release_rx.recv().unwrap();
        write_sse_response(&mut first_stream, "first answer");

        let (mut second_stream, _) = listener.accept().unwrap();
        second_stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        second_request_tx
            .send(read_request_body(&mut second_stream))
            .unwrap();
        write_sse_response(&mut second_stream, "follow-up answer");
    });
    let root = test_root();
    fs::write(
        root.join(".env"),
        format!(
            "OPENAI_API_KEY=test-key\nOPENAI_MODEL=test-model\nOPENAI_BASE_URL=http://{address}/v1\n"
        ),
    )
    .unwrap();
    let mut child = mini_agent(&root)
        .arg("auto")
        .env_remove("OPENAI_API_KEY")
        .env_remove("OPENAI_MODEL")
        .env_remove("OPENAI_BASE_URL")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    stdin.write_all(b"initial request\n").unwrap();
    let first_request = first_request_rx
        .recv_timeout(Duration::from_secs(5))
        .unwrap();
    stdin.write_all(b"follow-up request\n").unwrap();
    release_tx.send(()).unwrap();
    let second_request = second_request_rx
        .recv_timeout(Duration::from_secs(5))
        .unwrap();
    stdin.write_all(b"/exit\n").unwrap();
    drop(stdin);

    let status = wait_for_child(&mut child);
    let mut stdout = String::new();
    let mut stderr = String::new();
    child
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut stdout)
        .unwrap();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();
    server.join().unwrap();
    fs::remove_dir_all(root).unwrap();

    assert!(status.success(), "stdout: {stdout}\nstderr: {stderr}");
    assert!(String::from_utf8_lossy(&first_request).contains("initial request"));
    assert!(String::from_utf8_lossy(&second_request).contains("follow-up request"));
    assert!(stdout.contains("follow-up answer"), "{stdout}");
}

#[test]
fn ask_without_auto_denies_shell_when_stdin_is_not_a_tty() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let (requests_tx, requests_rx) = mpsc::channel();
    let shell_command = if cfg!(windows) {
        "Write-Output should-not-run"
    } else {
        "printf should-not-run"
    };
    let server = thread::spawn(move || {
        let (mut first_stream, _) = listener.accept().unwrap();
        first_stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        requests_tx
            .send(read_request_body(&mut first_stream))
            .unwrap();
        write_tool_sse_response(&mut first_stream, shell_command);

        let (mut second_stream, _) = listener.accept().unwrap();
        second_stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let second = read_request_body(&mut second_stream);
        requests_tx.send(second).unwrap();
        write_sse_response(&mut second_stream, "denied and stopped");
    });
    let root = test_root();
    fs::write(
        root.join(".env"),
        format!(
            "OPENAI_API_KEY=test-key\nOPENAI_MODEL=test-model\nOPENAI_BASE_URL=http://{address}/v1\n"
        ),
    )
    .unwrap();
    let output = mini_agent(&root)
        .args(["ask", "inspect the workspace"])
        .env_remove("OPENAI_API_KEY")
        .env_remove("OPENAI_MODEL")
        .env_remove("OPENAI_BASE_URL")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let first = requests_rx.recv().unwrap();
    let second = requests_rx.recv().unwrap();
    server.join().unwrap();
    fs::remove_dir_all(root).unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "stderr: {stderr}");
    drop(first);
    let second: Value = serde_json::from_slice(&second).unwrap();
    assert!(
        second.to_string().contains("denied non-interactive"),
        "{}",
        second
    );
    assert!(!stderr.contains("unsandboxed shell commands without approval"));
}

#[test]
fn ask_recovers_from_unknown_tool_on_public_path() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let (requests_tx, requests_rx) = mpsc::channel();
    let server = thread::spawn(move || {
        let (mut first_stream, _) = listener.accept().unwrap();
        first_stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        requests_tx
            .send(read_request_body(&mut first_stream))
            .unwrap();
        write_function_call_sse_response(
            &mut first_stream,
            "missing-call",
            "missing_fixture",
            json!({}),
        );

        let (mut second_stream, _) = listener.accept().unwrap();
        second_stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        requests_tx
            .send(read_request_body(&mut second_stream))
            .unwrap();
        write_sse_response(&mut second_stream, "recovered from the tool failure");
    });
    let root = test_root();
    fs::write(
        root.join(".env"),
        format!(
            "OPENAI_API_KEY=test-key\nOPENAI_MODEL=test-model\nOPENAI_BASE_URL=http://{address}/v1\n"
        ),
    )
    .unwrap();

    let output = mini_agent(&root)
        .args(["ask", "--json", "recover from the missing tool"])
        .env_remove("OPENAI_API_KEY")
        .env_remove("OPENAI_MODEL")
        .env_remove("OPENAI_BASE_URL")
        .output()
        .unwrap();
    let _first_request = requests_rx.recv().unwrap();
    let second_request = requests_rx.recv().unwrap();
    server.join().unwrap();
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();
    fs::remove_dir_all(root).unwrap();

    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    assert_eq!(response["output"], "recovered from the tool failure");
    assert!(String::from_utf8_lossy(&second_request).contains("unknown tool: missing_fixture"));
}

#[test]
fn ask_completes_bounded_cross_file_refactor_on_public_path() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let (requests_tx, requests_rx) = mpsc::channel();
    let server = thread::spawn(move || {
        let calls = [
            ("read-a", "read_file", json!({"path": "src/a.txt"})),
            ("read-b", "read_file", json!({"path": "src/b.txt"})),
            (
                "edit-a",
                "edit_file",
                json!({
                    "path": "src/a.txt",
                    "old_text": "shared_name",
                    "new_text": "renamed_name"
                }),
            ),
            (
                "edit-b",
                "edit_file",
                json!({
                    "path": "src/b.txt",
                    "old_text": "shared_name",
                    "new_text": "renamed_name"
                }),
            ),
        ];
        for (call_id, name, arguments) in calls {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            requests_tx.send(read_request_body(&mut stream)).unwrap();
            write_function_call_sse_response(&mut stream, call_id, name, arguments);
        }
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        requests_tx.send(read_request_body(&mut stream)).unwrap();
        write_sse_response(&mut stream, "refactored both files");
    });
    let root = test_root();
    fs::create_dir(root.join("src")).unwrap();
    fs::write(root.join("src/a.txt"), "use shared_name here\n").unwrap();
    fs::write(root.join("src/b.txt"), "also uses shared_name\n").unwrap();
    fs::write(
        root.join(".env"),
        format!(
            "OPENAI_API_KEY=test-key\nOPENAI_MODEL=test-model\nOPENAI_BASE_URL=http://{address}/v1\n"
        ),
    )
    .unwrap();

    let output = mini_agent(&root)
        .args([
            "ask",
            "--json",
            "--auto-approve",
            "rename shared_name in both files",
        ])
        .env_remove("OPENAI_API_KEY")
        .env_remove("OPENAI_MODEL")
        .env_remove("OPENAI_BASE_URL")
        .output()
        .unwrap();
    let requests = (0..5)
        .map(|_| requests_rx.recv().unwrap())
        .collect::<Vec<_>>();
    server.join().unwrap();
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();
    let first = fs::read_to_string(root.join("src/a.txt")).unwrap();
    let second = fs::read_to_string(root.join("src/b.txt")).unwrap();
    fs::remove_dir_all(root).unwrap();

    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    assert_eq!(response["output"], "refactored both files");
    assert_eq!(first, "use renamed_name here\n");
    assert_eq!(second, "also uses renamed_name\n");
    let combined_reads = String::from_utf8_lossy(&requests[2]);
    assert!(combined_reads.contains("use shared_name here"));
    assert!(combined_reads.contains("also uses shared_name"));
}

#[test]
fn bare_auto_session_can_disable_and_reenable_auto_mode() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let (requests_tx, requests_rx) = mpsc::channel();
    let (first_command, blocked_command, third_command) = if cfg!(windows) {
        (
            "Write-Output auto-started",
            "Set-Content -LiteralPath blocked.txt -Value blocked",
            "Write-Output auto-resumed",
        )
    } else {
        (
            "printf auto-started",
            "printf blocked > blocked.txt",
            "printf auto-resumed",
        )
    };
    let server = thread::spawn(move || {
        let responses = [
            (Some(first_command), ""),
            (None, "first complete"),
            (Some(blocked_command), ""),
            (None, "second complete"),
            (Some(third_command), ""),
            (None, "third complete"),
        ];
        for (command, reply) in responses {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            requests_tx.send(read_request_body(&mut stream)).unwrap();
            if let Some(command) = command {
                write_tool_sse_response(&mut stream, command);
            } else {
                write_sse_response(&mut stream, reply);
            }
        }
    });
    let root = test_root();
    fs::write(
        root.join(".env"),
        format!(
            "OPENAI_API_KEY=test-key\nOPENAI_MODEL=test-model\nOPENAI_BASE_URL=http://{address}/v1\n"
        ),
    )
    .unwrap();
    fs::write(
        root.join("AGENTS.md"),
        "Keep the stable project contract.\n",
    )
    .unwrap();
    let mut child = mini_agent(&root)
        .arg("auto")
        .env_remove("OPENAI_API_KEY")
        .env_remove("OPENAI_MODEL")
        .env_remove("OPENAI_BASE_URL")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"first\n/auto off\nsecond\n/auto\nthird\n/exit\n")
        .unwrap();

    let status = wait_for_child(&mut child);
    let mut stdout = String::new();
    let mut stderr = String::new();
    child
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut stdout)
        .unwrap();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();
    server.join().unwrap();

    assert!(status.success(), "stderr: {stderr}");
    assert!(
        stdout.contains("mini-agent — /auto /status /world /session /mcp /queue /new /help /exit")
    );
    assert!(stdout.contains("auto mode on"));
    assert!(stdout.contains("auto-started"));
    assert!(stdout.contains("auto mode off; writes and shell commands require approval"));
    assert!(stdout.contains("denied non-interactive action"));
    assert!(stdout.contains("auto-resumed"));
    assert!(!root.join("blocked.txt").exists());
    assert_eq!(
        stderr
            .matches("unsandboxed shell commands without approval")
            .count(),
        2
    );

    let requests = (0..6)
        .map(|_| serde_json::from_slice::<Value>(&requests_rx.recv().unwrap()).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(requests[0]["instructions"], requests[1]["instructions"]);
    assert_eq!(requests[0]["instructions"], requests[2]["instructions"]);
    assert_eq!(requests[0]["instructions"], requests[4]["instructions"]);
    assert!(
        requests[4]["instructions"]
            .as_str()
            .unwrap()
            .contains("Keep the stable project contract.")
    );
    assert!(
        requests[0]["input"]
            .to_string()
            .contains("mode=\\\"auto\\\"")
    );
    assert!(
        requests[2]["input"]
            .to_string()
            .contains("mode=\\\"default\\\"")
    );
    assert!(
        requests[0]["tools"][3]["description"]
            .as_str()
            .unwrap()
            .contains("without per-command approval")
    );
    assert!(
        requests[2]["tools"][3]["description"]
            .as_str()
            .unwrap()
            .contains("after user approval")
    );

    fs::remove_dir_all(root).unwrap();
}

fn read_request_body(stream: &mut TcpStream) -> Vec<u8> {
    let mut received = Vec::new();
    let mut buffer = [0u8; 4096];
    let header_end = loop {
        let count = stream.read(&mut buffer).unwrap();
        assert!(count > 0, "connection ended before HTTP headers");
        received.extend_from_slice(&buffer[..count]);
        if let Some(index) = received.windows(4).position(|window| window == b"\r\n\r\n") {
            break index + 4;
        }
    };
    let headers = String::from_utf8_lossy(&received[..header_end]);
    let content_length = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().unwrap())
        })
        .expect("request missing content-length");
    while received.len() - header_end < content_length {
        let count = stream.read(&mut buffer).unwrap();
        assert!(count > 0, "connection ended before HTTP body");
        received.extend_from_slice(&buffer[..count]);
    }
    received[header_end..header_end + content_length].to_vec()
}

fn write_sse_response(stream: &mut TcpStream, reply: &str) {
    let body = format!(
        "data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
        json!({"type": "response.output_text.delta", "delta": reply}),
        json!({
            "type": "response.completed",
            "response": {
                "usage": {
                    "input_tokens": 10,
                    "input_tokens_details": {"cached_tokens": 0},
                    "output_tokens": 2
                }
            }
        })
    );
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    stream.flush().unwrap();
}

fn write_reasoning_sse_response(stream: &mut TcpStream, reasoning: &str, reply: &str) {
    let body = format!(
        "data: {}\n\ndata: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
        json!({"type": "response.reasoning_text.delta", "delta": reasoning}),
        json!({"type": "response.output_text.delta", "delta": reply}),
        json!({
            "type": "response.completed",
            "response": {
                "usage": {
                    "input_tokens": 10,
                    "input_tokens_details": {"cached_tokens": 0},
                    "output_tokens": 4
                }
            }
        })
    );
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    stream.flush().unwrap();
}

fn write_tool_sse_response(stream: &mut TcpStream, command: &str) {
    write_function_call_sse_response(stream, "shell-call-1", "shell", json!({"command": command}));
}

fn write_function_call_sse_response(
    stream: &mut TcpStream,
    call_id: &str,
    name: &str,
    arguments: Value,
) {
    let body = format!(
        "data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
        json!({
            "type": "response.output_item.done",
            "item": {
                "type": "function_call",
                "call_id": call_id,
                "name": name,
                "arguments": serde_json::to_string(&arguments).unwrap()
            }
        }),
        json!({
            "type": "response.completed",
            "response": {
                "usage": {
                    "input_tokens": 10,
                    "input_tokens_details": {"cached_tokens": 0},
                    "output_tokens": 2
                }
            }
        })
    );
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    stream.flush().unwrap();
}

fn wait_for_child(child: &mut std::process::Child) -> std::process::ExitStatus {
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        if started.elapsed() > Duration::from_secs(10) {
            child.kill().unwrap();
            panic!("interactive process did not exit");
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn test_root() -> std::path::PathBuf {
    static NEXT_ROOT: AtomicU64 = AtomicU64::new(0);
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let sequence = NEXT_ROOT.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!("mini-agent-interactive-{nonce}-{sequence}"));
    fs::create_dir(&root).unwrap();
    root
}

fn find_session_file(root: &Path, session_id: &str) -> PathBuf {
    let sessions = root.join(".mini-agent").join("sessions");
    for project in fs::read_dir(&sessions).unwrap() {
        let candidate = project
            .unwrap()
            .path()
            .join(session_id)
            .join("session.jsonl");
        if candidate.is_file() {
            return candidate;
        }
    }
    panic!(
        "session {session_id} was not stored under {}",
        sessions.display()
    );
}

fn wait_for_goal_state(root: &Path) -> (PathBuf, String) {
    let started = Instant::now();
    loop {
        let sessions = root.join(".mini-agent").join("sessions");
        if let Ok(projects) = fs::read_dir(&sessions) {
            for project in projects.flatten() {
                if let Ok(session_dirs) = fs::read_dir(project.path()) {
                    for session in session_dirs.flatten() {
                        let state = session.path().join("goal/state.json");
                        if state.is_file() {
                            let id = session.file_name().to_string_lossy().into_owned();
                            return (session.path().join("session.jsonl"), id);
                        }
                    }
                }
            }
        }
        if started.elapsed() > Duration::from_secs(5) {
            panic!("goal state was not created under {}", sessions.display());
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn mini_agent(root: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_mini-agent"));
    command
        .current_dir(root)
        .env("HOME", root)
        .env("USERPROFILE", root)
        .env_remove("OPENAI_API_KEY")
        .env_remove("OPENAI_MODEL")
        .env_remove("OPENAI_BASE_URL")
        .env_remove("VERIFIER_OPENAI_MODEL")
        .env_remove("VERIFIER_OPENAI_API_KEY")
        .env_remove("VERIFIER_OPENAI_BASE_URL");
    command
}
