use serde_json::Value;
use serde_json::json;
use std::fs;
use std::io::Read;
use std::io::Write;
use std::net::TcpListener;
use std::net::TcpStream;
use std::process::Command;
use std::process::Stdio;
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
    let mut child = Command::new(env!("CARGO_BIN_EXE_mini-codex"))
        .current_dir(&root)
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
    assert!(stderr.contains("assistant> script answer"));
    let request: Value = serde_json::from_slice(&requests_rx.recv().unwrap()).unwrap();
    assert!(
        request["input"]
            .to_string()
            .contains("summarize this repository")
    );
    assert!(
        request["instructions"]
            .as_str()
            .unwrap()
            .contains("Use the release contract.")
    );
}

#[test]
fn status_json_remains_structured_when_env_file_is_invalid() {
    let root = test_root();
    fs::write(root.join(".env"), "NOT VALID\n").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_mini-codex"))
        .current_dir(&root)
        .args(["status", "--json"])
        .env_remove("OPENAI_API_KEY")
        .env_remove("OPENAI_MODEL")
        .env_remove("OPENAI_BASE_URL")
        .output()
        .unwrap();
    fs::remove_dir_all(root).unwrap();

    assert_eq!(output.status.code(), Some(2));
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .contains("invalid .env line")
    );
}

#[test]
fn subcommand_help_succeeds_without_configuration() {
    let root = test_root();
    let output = Command::new(env!("CARGO_BIN_EXE_mini-codex"))
        .current_dir(&root)
        .args(["ask", "--help"])
        .env_remove("OPENAI_API_KEY")
        .env_remove("OPENAI_MODEL")
        .env_remove("OPENAI_BASE_URL")
        .output()
        .unwrap();
    fs::remove_dir_all(root).unwrap();

    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("mini-codex ask"));
    assert!(stdout.contains("--auto"));
    assert!(stdout.contains("32 KiB"));
}

#[test]
fn interactive_terminal_keeps_history_until_new() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let (requests_tx, requests_rx) = mpsc::channel();
    let server = thread::spawn(move || {
        for (index, reply) in ["reply-one", "reply-two", "reply-three"]
            .into_iter()
            .enumerate()
        {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            requests_tx.send(read_request_body(&mut stream)).unwrap();
            if index == 0 {
                thread::sleep(Duration::from_millis(100));
                write_reasoning_sse_response(&mut stream, "inspect carefully", reply);
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
    let mut child = Command::new(env!("CARGO_BIN_EXE_mini-codex"))
        .current_dir(&root)
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
        .write_all(b"hello\nagain\n/new\nworld\n/exit\n")
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
    assert!(stdout.contains("mini-codex — /auto /queue /new /help /exit"));
    assert!(stdout.contains("assistant> reply-one"));
    assert!(stdout.contains("thinking> inspect carefully"));
    assert!(stdout.contains("queued ("));
    assert!(stdout.contains("assistant> reply-two"));
    assert!(stdout.contains("new conversation"));
    assert!(stdout.contains("assistant> reply-three"));
    let first: Value = serde_json::from_slice(&requests_rx.recv().unwrap()).unwrap();
    let second: Value = serde_json::from_slice(&requests_rx.recv().unwrap()).unwrap();
    let third: Value = serde_json::from_slice(&requests_rx.recv().unwrap()).unwrap();
    assert!(first["input"].to_string().contains("hello"));
    assert!(second["input"].to_string().contains("hello"));
    assert!(second["input"].to_string().contains("inspect carefully"));
    assert!(second["input"].to_string().contains("reasoning"));
    assert!(second["input"].to_string().contains("again"));
    assert!(third["input"].to_string().contains("world"));
    assert!(!third["input"].to_string().contains("hello"));
    assert!(!third["input"].to_string().contains("again"));
}

#[test]
fn auto_mode_executes_shell_without_approval() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let (requests_tx, requests_rx) = mpsc::channel();
    let shell_command = if cfg!(windows) {
        "Write-Output auto-ready"
    } else {
        "printf auto-ready"
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
        requests_tx
            .send(read_request_body(&mut second_stream))
            .unwrap();
        write_sse_response(&mut second_stream, "auto complete");
    });
    let root = test_root();
    fs::write(
        root.join(".env"),
        format!(
            "OPENAI_API_KEY=test-key\nOPENAI_MODEL=test-model\nOPENAI_BASE_URL=http://{address}/v1\n"
        ),
    )
    .unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_mini-codex"))
        .current_dir(&root)
        .args(["auto", "inspect the workspace"])
        .env_remove("OPENAI_API_KEY")
        .env_remove("OPENAI_MODEL")
        .env_remove("OPENAI_BASE_URL")
        .stdin(Stdio::null())
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
    assert!(stdout.contains("tool> shell"));
    assert!(stdout.contains("auto-ready"));
    assert!(stdout.contains("assistant> auto complete"));
    assert!(stderr.contains("unsandboxed shell commands without approval"));
    assert!(!stderr.contains("approve shell command"));
    let first: Value = serde_json::from_slice(&requests_rx.recv().unwrap()).unwrap();
    let second: Value = serde_json::from_slice(&requests_rx.recv().unwrap()).unwrap();
    assert_eq!(first["tools"][3]["name"], "shell");
    assert!(second["input"].to_string().contains("auto-ready"));
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
    let mut child = Command::new(env!("CARGO_BIN_EXE_mini-codex"))
        .current_dir(&root)
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
    assert!(stdout.contains("mini-codex — /auto /queue /new /help /exit"));
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
    assert_ne!(requests[0]["instructions"], requests[2]["instructions"]);
    assert_eq!(requests[0]["instructions"], requests[4]["instructions"]);
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
    let body = format!(
        "data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
        json!({
            "type": "response.output_item.done",
            "item": {
                "type": "function_call",
                "call_id": "shell-call-1",
                "name": "shell",
                "arguments": serde_json::to_string(&json!({"command": command})).unwrap()
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
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("mini-codex-interactive-{nonce}"));
    fs::create_dir(&root).unwrap();
    root
}
