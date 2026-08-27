use serde_json::Value;
use serde_json::json;
use std::fs;
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
fn ask_prints_the_final_answer_once() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let _ = read_request_body(&mut stream);
        write_reasoning_sse_response(&mut stream, "checking", "one final answer");
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
        .args(["ask", "answer once"])
        .env_remove("OPENAI_API_KEY")
        .env_remove("OPENAI_MODEL")
        .env_remove("OPENAI_BASE_URL")
        .output()
        .unwrap();
    server.join().unwrap();
    fs::remove_dir_all(root).unwrap();

    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "one final answer\n"
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("thinking> checking"));
    assert!(!stderr.contains("assistant>"));
}

#[test]
fn status_json_remains_structured_when_env_file_is_invalid() {
    let root = test_root();
    fs::write(root.join(".env"), "NOT VALID\n").unwrap();
    let output = mini_agent(&root)
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
fn interactive_prints_banner_before_initialization_error() {
    let root = test_root();
    fs::write(root.join(".env"), "NOT VALID\n").unwrap();
    let output = mini_agent(&root)
        .env_remove("OPENAI_API_KEY")
        .env_remove("OPENAI_MODEL")
        .env_remove("OPENAI_BASE_URL")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    fs::remove_dir_all(root).unwrap();

    assert_eq!(output.status.code(), Some(2));
    let stdout = String::from_utf8(output.stdout).unwrap();
    let mut lines = stdout.lines();
    let version = lines.next().unwrap_or_default();
    assert!(
        version.starts_with(&format!("mini-agent {} (", env!("CARGO_PKG_VERSION"))),
        "{version}"
    );
    assert!(version.ends_with(')'), "{version}");
    assert_eq!(
        lines.next().unwrap_or_default(),
        "mini-agent — /auto /status /world /session /mcp /queue /new /help /exit"
    );
    assert!(stdout.contains("world> "));
    assert!(stdout.contains("default | approval automatic"));
    assert!(stdout.ends_with("initializing extensions...\n"));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("invalid .env line"));
    assert!(stderr.contains("unsandboxed shell commands without approval"));
}

#[test]
fn interactive_mcp_command_reports_when_nothing_needs_retry() {
    let root = test_root();
    fs::write(
        root.join(".env"),
        "OPENAI_API_KEY=test-key\nOPENAI_MODEL=test-model\nOPENAI_BASE_URL=https://example.invalid\n",
    )
    .unwrap();
    let mut child = mini_agent(&root)
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
        .write_all(b"/world\n/world refresh\n/mcp\n/exit\n")
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
    fs::remove_dir_all(root).unwrap();

    assert!(status.success(), "stderr: {stderr}");
    assert!(stdout.contains("world> mode: default"));
    assert!(stdout.contains("world> approval: automatic"));
    assert!(stdout.contains("world> commands_available:"));
    assert!(stdout.contains("world> unchanged; no context item appended"));
    assert!(stdout.contains("no MCP servers are waiting to be enabled"));
}

#[test]
fn interactive_status_command_reports_runtime_security_and_sandbox() {
    let root = test_root();
    fs::write(
        root.join(".env"),
        "OPENAI_API_KEY=test-key\nOPENAI_MODEL=test-model\n",
    )
    .unwrap();
    let mut child = mini_agent(&root)
        .args(["--security-preset", "turbomode", "--sandbox", "native"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"/status\n/auto\n/status\n/exit\n")
        .unwrap();
    let status = child.wait().unwrap();
    let mut stdout = String::new();
    child
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut stdout)
        .unwrap();
    fs::remove_dir_all(root).unwrap();

    assert!(status.success());
    assert!(stdout.contains("status> security-preset:  turbomode"));
    assert!(stdout.contains("status> sandbox:          native"));
    assert!(stdout.contains("status> approval:         automatic (auto-approve)"));
    assert!(stdout.contains("status> web search:       enabled (built-in responses web_search)"));
    assert!(stdout.contains("status> copilot mode:     off"));
    assert!(stdout.contains("status> copilot mode:     on (unlimited steps)"));
    assert!(stdout.contains("status> session:"));
}

#[test]
fn subcommand_help_succeeds_without_configuration() {
    let root = test_root();
    let output = first_use_command(&root, &["ask", "--help"]);
    fs::remove_dir_all(root).unwrap();

    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("mini-agent ask"));
    assert!(stdout.contains("--auto-approve"));
    assert!(stdout.contains("32 KiB"));
}

#[test]
fn first_use_version_prints_the_package_version() {
    let root = test_root();
    let output = first_use_command(&root, &["--version"]);
    fs::remove_dir_all(&root).unwrap();

    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let stdout = stdout(&output);
    let line = stdout.trim();
    assert!(
        line.starts_with(&format!("mini-agent {} (", env!("CARGO_PKG_VERSION"))),
        "{line}"
    );
    assert!(line.ends_with(')'), "{line}");
}

#[test]
fn first_use_doctor_reports_missing_provider_settings() {
    let root = test_root();
    let plain = first_use_command(&root, &["doctor"]);
    let json = first_use_command(&root, &["doctor", "--json"]);
    fs::remove_dir_all(&root).unwrap();

    assert_eq!(plain.status.code(), Some(1), "stderr: {}", stderr(&plain));
    let plain_out = stdout(&plain);
    assert!(plain_out.contains("error: credential"), "{plain_out}");
    assert!(
        plain_out.contains("OPENAI_API_KEY is missing"),
        "{plain_out}"
    );
    assert!(plain_out.contains("error: model"), "{plain_out}");
    assert!(plain_out.contains("OPENAI_MODEL is missing"), "{plain_out}");
    assert!(plain_out.contains("ok: workspace"), "{plain_out}");
    assert!(
        plain_out.contains("ok: shell") || plain_out.contains("error: shell"),
        "{plain_out}"
    );

    assert_eq!(json.status.code(), Some(1), "stderr: {}", stderr(&json));
    let body: Value = serde_json::from_str(stdout(&json).trim()).unwrap();
    assert_eq!(body["ok"], false);
    let checks = body["checks"].as_array().expect("doctor checks");
    assert_eq!(check_status(checks, "workspace"), "ok");
    assert_eq!(check_status(checks, "credential"), "error");
    assert_eq!(check_status(checks, "model"), "error");
    assert_eq!(check_status(checks, "base_url"), "ok");
    assert!(matches!(check_status(checks, "shell"), "ok" | "error"));
}

#[test]
fn first_use_status_reports_non_secret_snapshot() {
    let root = test_root();
    let plain = first_use_command(&root, &["status"]);
    let json = first_use_command(&root, &["status", "--json"]);
    fs::remove_dir_all(&root).unwrap();

    assert!(plain.status.success(), "stderr: {}", stderr(&plain));
    let plain_out = stdout(&plain);
    assert!(
        plain_out.contains(&format!("version: {}", env!("CARGO_PKG_VERSION"))),
        "{plain_out}"
    );
    assert!(
        plain_out.contains("provider: openai_responses"),
        "{plain_out}"
    );
    assert!(plain_out.contains("credential: missing"), "{plain_out}");
    assert!(plain_out.contains("web_search: enabled"), "{plain_out}");
    assert!(plain_out.contains("command_sandbox: native"), "{plain_out}");
    assert!(
        plain_out.contains("session_persistence: opt_in"),
        "{plain_out}"
    );
    assert!(
        !plain_out.to_ascii_lowercase().contains("sk-"),
        "{plain_out}"
    );

    assert!(json.status.success(), "stderr: {}", stderr(&json));
    let json_out = stdout(&json);
    let body: Value = serde_json::from_str(json_out.trim()).unwrap();
    assert_eq!(body["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(body["provider"], "openai_responses");
    assert_eq!(body["credential"], "missing");
    assert_eq!(body["web_search"], true);
    assert_eq!(body["command_sandbox"], true);
    assert_eq!(body["session_persistence"], false);
    assert_eq!(body["world"]["command_sandbox"], "native");
    assert!(body["model"].is_null());
    assert_eq!(body["telemetry"], false);
    assert!(!json_out.to_ascii_lowercase().contains("sk-"), "{json_out}");
}

#[test]
fn first_use_status_reads_user_env_without_workspace_file() {
    let root = test_root();
    fs::create_dir(root.join(".mini-agent")).unwrap();
    fs::write(
        root.join(".mini-agent/.env"),
        "OPENAI_API_KEY=user-secret-key\nOPENAI_MODEL=deepseek-v4-flash\nOPENAI_BASE_URL=https://api.deepseek.com\n",
    )
    .unwrap();
    let output = first_use_command(&root, &["status", "--json"]);
    fs::remove_dir_all(&root).unwrap();

    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let json_out = stdout(&output);
    let body: Value = serde_json::from_str(json_out.trim()).unwrap();
    assert_eq!(body["credential"], "configured");
    assert_eq!(body["credential_source"], "~/.mini-agent/.env");
    assert_eq!(body["model"], "deepseek-v4-flash");
    assert_eq!(body["model_source"], "~/.mini-agent/.env");
    assert_eq!(body["base_url"], "https://api.deepseek.com");
    assert_eq!(body["base_url_source"], "~/.mini-agent/.env");
    assert!(!json_out.contains("user-secret-key"), "{json_out}");
}

#[test]
fn first_use_status_reads_env_demo_without_a_secret() {
    let root = test_root();
    let template = env_demo_template();
    assert!(
        template.is_file(),
        "Quick start names {}, which must exist in the clone",
        template.display()
    );
    fs::copy(&template, root.join(".env")).unwrap();
    let output = first_use_command(&root, &["status", "--json"]);
    let doctor = first_use_command(&root, &["doctor", "--json"]);
    fs::remove_dir_all(&root).unwrap();

    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let body: Value = serde_json::from_str(stdout(&output).trim()).unwrap();
    assert_eq!(body["model"], "deepseek-v4-flash");
    assert_eq!(body["credential"], "missing");
    assert_eq!(body["base_url"], "https://api.deepseek.com");
    assert_eq!(body["base_url_source"], ".env");

    assert_eq!(doctor.status.code(), Some(1), "stderr: {}", stderr(&doctor));
    let doctor_body: Value = serde_json::from_str(stdout(&doctor).trim()).unwrap();
    assert_eq!(doctor_body["ok"], false);
    let checks = doctor_body["checks"].as_array().expect("doctor checks");
    assert_eq!(check_status(checks, "credential"), "error");
    assert_eq!(check_status(checks, "model"), "ok");
}

#[test]
fn ask_keeps_bounded_head_and_tail_of_oversized_agents_md() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let (requests_tx, requests_rx) = mpsc::channel();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        requests_tx.send(read_request_body(&mut stream)).unwrap();
        write_sse_response(&mut stream, "bounded instructions");
    });
    let root = test_root();
    let mut agents = String::from("UNIQUE-HEAD-RULE\n");
    agents.push_str(&"n".repeat(80 * 1024));
    agents.push_str("\nUNIQUE-TAIL-RULE\n");
    fs::write(root.join("AGENTS.md"), &agents).unwrap();
    fs::write(
        root.join(".env"),
        format!(
            "OPENAI_API_KEY=test-key\nOPENAI_MODEL=test-model\nOPENAI_BASE_URL=http://{address}/v1\n"
        ),
    )
    .unwrap();
    let output = mini_agent(&root)
        .args(["ask", "hello"])
        .env_remove("OPENAI_API_KEY")
        .env_remove("OPENAI_MODEL")
        .env_remove("OPENAI_BASE_URL")
        .env_remove("MENTOR_OPENAI_MODEL")
        .env_remove("MENTOR_OPENAI_API_KEY")
        .env_remove("MENTOR_OPENAI_BASE_URL")
        .output()
        .unwrap();
    server.join().unwrap();
    fs::remove_dir_all(root).unwrap();

    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let stderr = stderr(&output);
    assert!(
        stderr.contains("AGENTS.md exceeds") && stderr.contains("bounded head and tail"),
        "{stderr}"
    );
    let request: Value = serde_json::from_slice(&requests_rx.recv().unwrap()).unwrap();
    let instructions = request["instructions"].as_str().unwrap();
    assert!(instructions.contains("UNIQUE-HEAD-RULE"), "{instructions}");
    assert!(instructions.contains("[truncated]"), "{instructions}");
    assert!(instructions.contains("UNIQUE-TAIL-RULE"), "{instructions}");
    assert!(
        instructions.len() < agents.len(),
        "instructions should be bounded"
    );
}

#[test]
fn first_use_demo_completes_the_model_tool_model_path() {
    let root = test_root();
    let output = first_use_command(&root, &["demo", "make this loud"]);
    fs::remove_dir_all(&root).unwrap();

    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let stdout = stdout(&output);
    assert!(stdout.contains("I will run one tool."), "{stdout}");
    assert!(stdout.contains("uppercase"), "{stdout}");
    assert!(stdout.contains("MAKE THIS LOUD"), "{stdout}");
    assert!(
        stdout.contains("The tool returned: MAKE THIS LOUD"),
        "{stdout}"
    );
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
    let mut child = mini_agent(&root)
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
    assert!(
        stdout.contains("mini-agent — /auto /status /world /session /mcp /queue /new /help /exit")
    );
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
    assert!(third["input"].to_string().contains("<world_state>"));
    assert!(!third["input"].to_string().contains("hello"));
    assert!(!third["input"].to_string().contains("again"));
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

    let first_stdout = run(&["--persist"], b"first question\n/exit\n");
    let session_id = first_stdout
        .lines()
        .find_map(|line| line.strip_prefix("session> new "))
        .and_then(|line| line.split_once(" |"))
        .map(|(id, _)| id)
        .unwrap();
    let second_stdout = run(&["resume", session_id], b"second question\n/exit\n");

    server.join().unwrap();
    let first: Value = serde_json::from_slice(&requests_rx.recv().unwrap()).unwrap();
    let second: Value = serde_json::from_slice(&requests_rx.recv().unwrap()).unwrap();
    assert!(first["input"].to_string().contains("first question"));
    assert!(second_stdout.contains(&format!("session> resumed {session_id}")));
    assert!(second["input"].to_string().contains("first question"));
    assert!(second["input"].to_string().contains("first durable answer"));
    assert!(second["input"].to_string().contains("second question"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn mentor_reviews_a_settled_checkpoint_without_polluting_primary_history() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let (requests_tx, requests_rx) = mpsc::channel();
    let server = thread::spawn(move || {
        for reply in [
            "primary answer",
            "mentor review",
            "verification result",
            "resumed primary answer",
        ] {
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
            "OPENAI_API_KEY=test-key\nOPENAI_MODEL=primary-model\nOPENAI_BASE_URL=http://{address}/v1\nMENTOR_OPENAI_MODEL=mentor-model\n"
        ),
    )
    .unwrap();
    let command = |arguments: &[&str], input: Option<&[u8]>| {
        let mut child = mini_agent(&root)
            .args(arguments)
            .stdin(if input.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        if let Some(input) = input {
            child.stdin.take().unwrap().write_all(input).unwrap();
        }
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        output
    };

    let first = command(&["--persist"], Some(b"first question\n/exit\n"));
    let first_stdout = String::from_utf8(first.stdout).unwrap();
    let session_id = first_stdout
        .lines()
        .find_map(|line| line.strip_prefix("session> new "))
        .and_then(|line| line.split_once(" |"))
        .map(|(id, _)| id.to_string())
        .unwrap();
    let mentor = command(&["mentor", "insight", &session_id, "--json"], None);
    let mentor_json: Value = serde_json::from_slice(&mentor.stdout).unwrap();
    let verification = command(
        &[
            "mentor",
            "verify",
            &session_id,
            "--json",
            "--",
            "the primary answer exists",
        ],
        None,
    );
    let verification_json: Value = serde_json::from_slice(&verification.stdout).unwrap();
    let _resumed = command(&["resume", &session_id], Some(b"second question\n/exit\n"));

    server.join().unwrap();
    let primary_request: Value = serde_json::from_slice(&requests_rx.recv().unwrap()).unwrap();
    let mentor_request: Value = serde_json::from_slice(&requests_rx.recv().unwrap()).unwrap();
    let verification_request: Value = serde_json::from_slice(&requests_rx.recv().unwrap()).unwrap();
    let resumed_request: Value = serde_json::from_slice(&requests_rx.recv().unwrap()).unwrap();
    let session_records = fs::read_to_string(find_session_file(&root, &session_id)).unwrap();

    assert_eq!(primary_request["model"], "primary-model");
    assert_eq!(mentor_request["model"], "mentor-model");
    assert_eq!(mentor_request["tools"], json!([]));
    assert!(
        mentor_request["instructions"]
            .as_str()
            .unwrap()
            .contains("independent mentor")
    );
    assert!(
        mentor_request["input"]
            .to_string()
            .contains("first question")
    );
    assert!(
        mentor_request["input"]
            .to_string()
            .contains("primary answer")
    );
    assert!(
        mentor_request["input"]
            .to_string()
            .contains("insight review")
    );
    assert_eq!(mentor_json["output"], "mentor review");
    assert_eq!(mentor_json["action"], "insight");
    assert_eq!(mentor_json["tool_calls"], json!([]));
    assert_eq!(verification_request["model"], "mentor-model");
    assert_eq!(verification_request["tools"], json!([]));
    assert!(
        verification_request["instructions"]
            .as_str()
            .unwrap()
            .contains("independent verifier")
    );
    assert!(
        verification_request["input"]
            .to_string()
            .contains("the primary answer exists")
    );
    assert_eq!(verification_json["output"], "verification result");
    assert_eq!(verification_json["action"], "verify");
    assert!(session_records.contains("\"kind\":\"derived_item\""));
    assert!(session_records.contains("\"output\":\"mentor review\""));
    assert!(session_records.contains("\"item_kind\":\"mentor_verification\""));
    assert!(session_records.contains("\"output\":\"verification result\""));
    assert!(
        resumed_request["input"]
            .to_string()
            .contains("primary answer")
    );
    assert!(
        !resumed_request["input"]
            .to_string()
            .contains("mentor review")
    );
    assert!(
        !resumed_request["input"]
            .to_string()
            .contains("verification result")
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn default_repl_executes_shell_without_approval() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let (requests_tx, requests_rx) = mpsc::channel();
    let shell_command = if cfg!(windows) {
        "Write-Output default-ready"
    } else {
        "printf default-ready"
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
        write_sse_response(&mut second_stream, "default complete");
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
        .write_all(b"inspect\n/exit\n")
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
    assert!(stdout.contains("default-ready"));
    assert!(stdout.contains("assistant> default complete"));
    assert!(stderr.contains("unsandboxed shell commands without approval"));
    assert!(!stderr.contains("approve shell command"));
    drop(requests_rx);
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
    let mut child = mini_agent(&root)
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

#[test]
fn subagent_spawn_runs_child_process_and_returns_output() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        // Request 1: Parent agent calls spawn_agent
        let (mut stream1, _) = listener.accept().unwrap();
        stream1
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let _ = read_request_body(&mut stream1);
        let body1 = format!(
            "data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
            json!({
                "type": "response.output_item.done",
                "item": {
                    "type": "function_call",
                    "call_id": "spawn-call-1",
                    "name": "spawn_agent",
                    "arguments": serde_json::to_string(&json!({
                        "task_name": "child_reviewer",
                        "message": "review changes"
                    })).unwrap()
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
            stream1,
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body1}",
            body1.len()
        )
        .unwrap();
        stream1.flush().unwrap();

        // Request 2: Child agent runs turn and returns answer
        let (mut stream2, _) = listener.accept().unwrap();
        stream2
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let _ = read_request_body(&mut stream2);
        write_sse_response(&mut stream2, "Child review complete: no issues found.");

        // Request 3: Parent agent receives tool output and finishes
        let (mut stream3, _) = listener.accept().unwrap();
        stream3
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let _ = read_request_body(&mut stream3);
        write_sse_response(&mut stream3, "All reviews completed successfully.");
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
        .args(["ask", "start review", "--json", "--auto-approve"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "ask command failed: {}",
        stderr(&output)
    );
    let stdout_str = stdout(&output);
    let parsed: Value = serde_json::from_str(&stdout_str).unwrap();
    assert_eq!(parsed["exit_code"], 0);
    assert!(
        parsed["output"]
            .as_str()
            .unwrap()
            .contains("All reviews completed successfully.")
    );

    server.join().unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn subagent_multi_turn_interactive_session_resumes_and_retains_context() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        // Request 1: Parent agent calls spawn_agent with persist=true
        let (mut stream1, _) = listener.accept().unwrap();
        stream1
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let _ = read_request_body(&mut stream1);
        let body1 = format!(
            "data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
            json!({
                "type": "response.output_item.done",
                "item": {
                    "type": "function_call",
                    "call_id": "spawn-call-1",
                    "name": "spawn_agent",
                    "arguments": serde_json::to_string(&json!({
                        "task_name": "auth_tester",
                        "message": "Turn 1: inspect auth module",
                        "persist": true
                    })).unwrap()
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
            stream1,
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body1}",
            body1.len()
        )
        .unwrap();
        stream1.flush().unwrap();

        // Request 2: Child agent runs Turn 1 and returns answer
        let (mut stream2, _) = listener.accept().unwrap();
        stream2
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let _ = read_request_body(&mut stream2);
        write_sse_response(&mut stream2, "Turn 1 Analysis: JWT validation is present.");

        // Request 3: Parent agent receives tool output with session_id and calls send_subagent_message
        let (mut stream3, _) = listener.accept().unwrap();
        stream3
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let parent_req2_body = read_request_body(&mut stream3);
        let parent_req2_str = String::from_utf8_lossy(&parent_req2_body);
        assert!(
            parent_req2_str.contains("JWT validation is present"),
            "{parent_req2_str}"
        );

        // Extract session_id from parent's tool response in prompt
        let session_marker = "[session_id: ";
        let start = parent_req2_str
            .find(session_marker)
            .expect("session_id marker in parent prompt")
            + session_marker.len();
        let end = parent_req2_str[start..]
            .find(']')
            .expect("closing bracket for session_id")
            + start;
        let session_id = &parent_req2_str[start..end];

        let body3 = format!(
            "data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
            json!({
                "type": "response.output_item.done",
                "item": {
                    "type": "function_call",
                    "call_id": "send-msg-call-1",
                    "name": "send_subagent_message",
                    "arguments": serde_json::to_string(&json!({
                        "session_id": session_id,
                        "message": "Turn 2: does it validate expiration?"
                    })).unwrap()
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
            stream3,
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body3}",
            body3.len()
        )
        .unwrap();
        stream3.flush().unwrap();

        // Request 4: Child subagent resumes session and processes Turn 2
        let (mut stream4, _) = listener.accept().unwrap();
        stream4
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let child_req2_body = read_request_body(&mut stream4);
        let child_req2_str = String::from_utf8_lossy(&child_req2_body);
        // Verify child retained Turn 1 context from resumed checkpoint
        assert!(
            child_req2_str.contains("Turn 1 Analysis: JWT validation is present"),
            "{child_req2_str}"
        );
        write_sse_response(&mut stream4, "Turn 2 Analysis: Expiration is verified.");

        // Request 5: Parent receives follow-up response and concludes
        let (mut stream5, _) = listener.accept().unwrap();
        stream5
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let _ = read_request_body(&mut stream5);
        write_sse_response(&mut stream5, "Multi-turn audit complete: auth is robust.");
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
        .args(["ask", "start multi-turn audit", "--json", "--auto-approve"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "ask command failed: {}",
        stderr(&output)
    );
    let stdout_str = stdout(&output);
    let parsed: Value = serde_json::from_str(&stdout_str).unwrap();
    assert_eq!(parsed["exit_code"], 0);
    assert!(
        parsed["output"]
            .as_str()
            .unwrap()
            .contains("Multi-turn audit complete: auth is robust.")
    );

    // Verify subagents directory was created and contains meta.json
    let sessions_dir = root.join(".agents/sessions");
    assert!(sessions_dir.is_dir());
    let mut found_meta = false;
    for entry in fs::read_dir(&sessions_dir).unwrap().filter_map(Result::ok) {
        if entry.path().join("meta.json").is_file() {
            found_meta = true;
            let meta: Value =
                serde_json::from_str(&fs::read_to_string(entry.path().join("meta.json")).unwrap())
                    .unwrap();
            assert_eq!(meta["status"], "completed");
        }
    }
    assert!(
        found_meta,
        "expected subagent meta.json in .agents/sessions"
    );

    server.join().unwrap();
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

fn mini_agent(root: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_mini-agent"));
    command
        .current_dir(root)
        .env("HOME", root)
        .env("USERPROFILE", root)
        .env_remove("OPENAI_API_KEY")
        .env_remove("OPENAI_MODEL")
        .env_remove("OPENAI_BASE_URL")
        .env_remove("MENTOR_OPENAI_MODEL")
        .env_remove("MENTOR_OPENAI_API_KEY")
        .env_remove("MENTOR_OPENAI_BASE_URL");
    command
}

fn first_use_command(root: &Path, args: &[&str]) -> std::process::Output {
    mini_agent(root).args(args).output().unwrap()
}

fn stdout(output: &std::process::Output) -> String {
    String::from_utf8(output.stdout.clone()).unwrap()
}

fn stderr(output: &std::process::Output) -> String {
    String::from_utf8(output.stderr.clone()).unwrap()
}

fn check_status<'a>(checks: &'a [Value], name: &str) -> &'a str {
    checks
        .iter()
        .find(|check| check["name"] == name)
        .and_then(|check| check["status"].as_str())
        .unwrap_or("missing")
}

fn env_demo_template() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.env.demo")
}
