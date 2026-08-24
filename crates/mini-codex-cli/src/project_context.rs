use std::fs;
use std::io;
use std::path::Path;

const MAX_PROJECT_INSTRUCTIONS_BYTES: usize = 16 * 1024;

pub fn augment_system_prompt(base: &str, workspace: &Path) -> Result<String, String> {
    let path = workspace.join("AGENTS.md");
    let instructions = match fs::read(&path) {
        Ok(instructions) => instructions,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(base.to_string()),
        Err(error) => return Err(format!("cannot read {}: {error}", path.display())),
    };
    if instructions.len() > MAX_PROJECT_INSTRUCTIONS_BYTES {
        return Err(format!(
            "{} exceeds the {MAX_PROJECT_INSTRUCTIONS_BYTES} byte project instruction limit",
            path.display()
        ));
    }
    let instructions = String::from_utf8(instructions)
        .map_err(|_| format!("{} must be valid UTF-8", path.display()))?;
    if instructions.trim().is_empty() {
        return Ok(base.to_string());
    }
    Ok(format!(
        "{base}\n\nProject instructions from AGENTS.md:\n---\n{}\n---",
        instructions.trim()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::SystemTime;
    use std::time::UNIX_EPOCH;

    #[test]
    fn appends_bounded_project_instructions() {
        let root = test_root();
        fs::write(root.join("AGENTS.md"), "Run cargo test.\n").unwrap();

        let prompt = augment_system_prompt("base", &root).unwrap();

        assert_eq!(
            prompt,
            "base\n\nProject instructions from AGENTS.md:\n---\nRun cargo test.\n---"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_oversized_project_instructions() {
        let root = test_root();
        fs::write(
            root.join("AGENTS.md"),
            vec![b'x'; MAX_PROJECT_INSTRUCTIONS_BYTES + 1],
        )
        .unwrap();

        let error = augment_system_prompt("base", &root).unwrap_err();

        assert!(error.contains("project instruction limit"));
        fs::remove_dir_all(root).unwrap();
    }

    fn test_root() -> std::path::PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("mini-codex-project-context-{nonce}"));
        fs::create_dir(&root).unwrap();
        root
    }
}
