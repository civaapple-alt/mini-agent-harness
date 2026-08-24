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
fn interactive_terminal_keeps_history_until_new() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let (requests_tx, requests_rx) = mpsc::channel();
    let server = thread::spawn(move || {
        for reply in ["reply-one", "reply-two", "reply-three"] {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            requests_tx.send(read_request_body(&mut stream)).unwrap();
            write_sse_response(&mut stream, reply);
        }
    });
    let root = test_root();
    let mut child = Command::new(env!("CARGO_BIN_EXE_mini-codex"))
        .current_dir(&root)
        .env("OPENAI_API_KEY", "test-key")
        .env("OPENAI_MODEL", "test-model")
        .env("OPENAI_BASE_URL", format!("http://{address}/v1"))
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
    assert!(stdout.contains("mini-codex — /new /help /exit"));
    assert!(stdout.contains("assistant> reply-one"));
    assert!(stdout.contains("assistant> reply-two"));
    assert!(stdout.contains("new conversation"));
    assert!(stdout.contains("assistant> reply-three"));
    let first: Value = serde_json::from_slice(&requests_rx.recv().unwrap()).unwrap();
    let second: Value = serde_json::from_slice(&requests_rx.recv().unwrap()).unwrap();
    let third: Value = serde_json::from_slice(&requests_rx.recv().unwrap()).unwrap();
    assert!(first["input"].to_string().contains("hello"));
    assert!(second["input"].to_string().contains("hello"));
    assert!(second["input"].to_string().contains("again"));
    assert!(third["input"].to_string().contains("world"));
    assert!(!third["input"].to_string().contains("hello"));
    assert!(!third["input"].to_string().contains("again"));
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
