use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

/// Serializes tests that temporarily replace process-level home directory variables.
pub(crate) static HOME_LOCK: Mutex<()> = Mutex::new(());

pub(crate) fn test_root() -> PathBuf {
    static NEXT_ROOT: AtomicU64 = AtomicU64::new(0);
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let sequence = NEXT_ROOT.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!("mini-agent-test-{nonce}-{sequence}"));
    fs::create_dir(&root).unwrap();
    root
}

pub(crate) fn remove_test_root(root: &Path) {
    for _ in 0..50 {
        match fs::remove_dir_all(root) {
            Ok(()) => return,
            Err(_) => std::thread::sleep(std::time::Duration::from_millis(20)),
        }
    }
    fs::remove_dir_all(root).unwrap();
}

pub(crate) fn python_command() -> String {
    ["python3", "python"]
        .into_iter()
        .find(|command| {
            std::process::Command::new(command)
                .arg("--version")
                .output()
                .is_ok_and(|output| output.status.success())
        })
        .expect("Python 3 is required by the repository fixtures")
        .to_string()
}
