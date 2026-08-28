use std::fs;
use std::io;
use std::path::Path;

pub const MAX_PROJECT_INSTRUCTIONS_BYTES: usize = 16 * 1024;
const TRUNCATION_MARKER: &str = "\n[truncated]\n";

#[derive(Debug, PartialEq, Eq)]
pub enum AgentsMd {
    Absent,
    Loaded {
        body: String,
        truncated: bool,
        source_bytes: usize,
    },
}

impl AgentsMd {
    pub fn augment(self, base: &str) -> String {
        match self {
            Self::Absent => base.to_string(),
            Self::Loaded { body, .. } => {
                format!("{base}\n\nProject instructions from AGENTS.md:\n---\n{body}\n---")
            }
        }
    }

    pub fn truncation_warning(&self) -> Option<String> {
        match self {
            Self::Loaded {
                truncated: true,
                source_bytes,
                ..
            } => Some(format!(
                "AGENTS.md exceeds {MAX_PROJECT_INSTRUCTIONS_BYTES} bytes ({source_bytes}); using bounded head and tail"
            )),
            Self::Absent | Self::Loaded { .. } => None,
        }
    }
}

pub fn load_agents_md(workspace: &Path) -> Result<AgentsMd, String> {
    let path = workspace.join("AGENTS.md");
    let instructions = match fs::read(&path) {
        Ok(instructions) => instructions,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(AgentsMd::Absent),
        Err(error) => return Err(format!("cannot read {}: {error}", path.display())),
    };
    let source_bytes = instructions.len();
    let instructions = String::from_utf8(instructions)
        .map_err(|_| format!("{} must be valid UTF-8", path.display()))?;
    let instructions = instructions.trim();
    if instructions.is_empty() {
        return Ok(AgentsMd::Absent);
    }
    let truncated = instructions.len() > MAX_PROJECT_INSTRUCTIONS_BYTES;
    let body = if truncated {
        truncate_utf8(instructions.to_string(), MAX_PROJECT_INSTRUCTIONS_BYTES)
    } else {
        instructions.to_string()
    };
    Ok(AgentsMd::Loaded {
        body,
        truncated,
        source_bytes,
    })
}

fn truncate_utf8(mut content: String, max_bytes: usize) -> String {
    if content.len() <= max_bytes {
        return content;
    }
    if max_bytes <= TRUNCATION_MARKER.len() {
        let end = floor_char_boundary(&content, max_bytes);
        content.truncate(end);
        return content;
    }
    let retained_bytes = max_bytes - TRUNCATION_MARKER.len();
    let head_bytes = retained_bytes.div_ceil(2);
    let tail_bytes = retained_bytes - head_bytes;
    let head_end = floor_char_boundary(&content, head_bytes);
    let tail_start = ceil_char_boundary(&content, content.len() - tail_bytes);
    let mut output = String::with_capacity(max_bytes);
    output.push_str(&content[..head_end]);
    output.push_str(TRUNCATION_MARKER);
    output.push_str(&content[tail_start..]);
    output
}

fn floor_char_boundary(text: &str, mut index: usize) -> usize {
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn ceil_char_boundary(text: &str, mut index: usize) -> usize {
    while index < text.len() && !text.is_char_boundary(index) {
        index += 1;
    }
    index
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;
    use std::sync::atomic::Ordering;
    use std::time::SystemTime;
    use std::time::UNIX_EPOCH;

    #[test]
    fn appends_bounded_project_instructions() {
        let root = test_root();
        fs::write(root.join("AGENTS.md"), "Run cargo test.\n").unwrap();

        let prompt = load_agents_md(&root).unwrap().augment("base");

        assert_eq!(
            prompt,
            "base\n\nProject instructions from AGENTS.md:\n---\nRun cargo test.\n---"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn truncates_oversized_project_instructions() {
        let root = test_root();
        let mut source = String::from("HEAD-");
        source.push_str(&"x".repeat(MAX_PROJECT_INSTRUCTIONS_BYTES));
        source.push_str("-TAIL");
        fs::write(root.join("AGENTS.md"), &source).unwrap();

        let loaded = load_agents_md(&root).unwrap();
        let AgentsMd::Loaded {
            body,
            truncated,
            source_bytes,
        } = loaded
        else {
            panic!("expected loaded instructions");
        };
        assert!(truncated);
        assert_eq!(source_bytes, source.len());
        assert!(body.len() <= MAX_PROJECT_INSTRUCTIONS_BYTES);
        assert!(body.starts_with("HEAD-"));
        assert!(body.contains(TRUNCATION_MARKER));
        assert!(body.ends_with("-TAIL"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_invalid_utf8_project_instructions() {
        let root = test_root();
        fs::write(root.join("AGENTS.md"), [0xff, 0xfe]).unwrap();

        let error = load_agents_md(&root).unwrap_err();

        assert!(error.contains("must be valid UTF-8"));
        fs::remove_dir_all(root).unwrap();
    }

    fn test_root() -> std::path::PathBuf {
        static NEXT_ROOT: AtomicU64 = AtomicU64::new(0);
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let sequence = NEXT_ROOT.fetch_add(1, Ordering::Relaxed);
        let root =
            std::env::temp_dir().join(format!("mini-agent-project-context-{nonce}-{sequence}"));
        fs::create_dir(&root).unwrap();
        root
    }
}
