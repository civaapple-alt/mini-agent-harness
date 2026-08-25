use super::*;
use serde_json::json;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

#[test]
fn selects_only_local_marketplace_plugins() {
    let root = test_root();
    let marketplace = root.join(".agents/marketplaces/catalog");
    fs::create_dir_all(marketplace.join(".claude-plugin")).unwrap();
    fs::create_dir_all(marketplace.join("plugins/local")).unwrap();
    fs::write(
        marketplace.join(".claude-plugin/marketplace.json"),
        serde_json::to_vec(&json!({
            "name": "catalog",
            "plugins": [
                {"name": "local", "source": "./plugins/local"},
                {"name": "remote", "source": {"source": "url", "url": "https://example.com/plugin.git"}}
            ]
        }))
        .unwrap(),
    )
    .unwrap();
    fs::write(
        root.join(".agents/marketplaces.json"),
        serde_json::to_vec(&json!({"marketplaces": {"catalog": ["local", "remote"]}})).unwrap(),
    )
    .unwrap();

    let discovery = discover(&root.canonicalize().unwrap());

    assert_eq!(discovery.marketplace_count, 1);
    assert_eq!(discovery.plugins.len(), 1);
    assert_eq!(discovery.plugins[0].name, "local");
    assert!(discovery.plugins[0].is_plugin);
    assert!(
        discovery
            .diagnostics
            .iter()
            .any(|message| message.contains("is remote"))
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn rejects_marketplace_paths_that_escape_the_clone() {
    let root = test_root();
    let marketplace = root.join(".agents/marketplaces/catalog");
    fs::create_dir_all(marketplace.join(".grok-plugin")).unwrap();
    fs::write(
        marketplace.join(".grok-plugin/marketplace.json"),
        serde_json::to_vec(&json!({
            "name": "catalog",
            "plugins": [{"name": "escape", "source": {"type": "local", "path": "./../outside"}}]
        }))
        .unwrap(),
    )
    .unwrap();
    fs::write(
        root.join(".agents/marketplaces.json"),
        serde_json::to_vec(&json!({"marketplaces": {"catalog": ["escape"]}})).unwrap(),
    )
    .unwrap();

    let discovery = discover(&root.canonicalize().unwrap());

    assert!(discovery.plugins.is_empty());
    assert!(
        discovery
            .diagnostics
            .iter()
            .any(|message| message.contains("not a contained relative directory"))
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn accepts_bounded_official_size_marketplace_manifest() {
    let root = test_root();
    let marketplace = root.join(".agents/marketplaces/catalog");
    fs::create_dir_all(marketplace.join(".claude-plugin")).unwrap();
    fs::create_dir_all(marketplace.join("plugins/local")).unwrap();
    fs::write(
        marketplace.join(".claude-plugin/marketplace.json"),
        serde_json::to_vec(&json!({
            "name": "catalog",
            "description": "x".repeat(100_000),
            "plugins": [{"name": "local", "source": "./plugins/local"}]
        }))
        .unwrap(),
    )
    .unwrap();
    fs::write(
        root.join(".agents/marketplaces.json"),
        serde_json::to_vec(&json!({"marketplaces": {"catalog": ["local"]}})).unwrap(),
    )
    .unwrap();

    let discovery = discover(&root.canonicalize().unwrap());

    assert_eq!(discovery.plugins.len(), 1);
    assert!(discovery.diagnostics.is_empty());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn selects_a_root_marketplace_skill_by_directory_name() {
    let root = test_root();
    let marketplace = root.join(".agents/marketplaces/catalog");
    let skill = marketplace.join("skills/skill-creator");
    fs::create_dir_all(marketplace.join(".claude-plugin")).unwrap();
    fs::create_dir_all(&skill).unwrap();
    fs::write(
        marketplace.join(".claude-plugin/marketplace.json"),
        serde_json::to_vec(&json!({
            "name": "catalog",
            "plugins": [{
                "name": "example-skills",
                "source": "./",
                "skills": ["./skills/skill-creator", "./skills/other"]
            }]
        }))
        .unwrap(),
    )
    .unwrap();
    fs::write(
        skill.join("SKILL.md"),
        "---\nname: skill-creator\ndescription: Create skills.\n---\n",
    )
    .unwrap();
    fs::write(
        root.join(".agents/marketplaces.json"),
        serde_json::to_vec(&json!({"marketplaces": {"catalog": ["skill-creator"]}})).unwrap(),
    )
    .unwrap();

    let discovery = discover(&root.canonicalize().unwrap());

    assert_eq!(discovery.marketplace_count, 1);
    assert_eq!(discovery.plugins.len(), 1);
    assert_eq!(discovery.plugins[0].name, "skill-creator");
    assert!(!discovery.plugins[0].is_plugin);
    assert_eq!(
        discovery.plugins[0].root,
        marketplace.canonicalize().unwrap()
    );
    assert_eq!(
        discovery.plugins[0].explicit_skills,
        Some(vec![skill.canonicalize().unwrap()])
    );
    assert!(discovery.diagnostics.is_empty());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn direct_marketplace_skill_wins_over_same_named_plugin_bundle() {
    let root = test_root();
    let marketplace = root.join(".agents/marketplaces/taste-skill");
    let skill = marketplace.join("skills/taste-skill");
    fs::create_dir_all(marketplace.join(".claude-plugin")).unwrap();
    fs::create_dir_all(&skill).unwrap();
    fs::write(
        marketplace.join(".claude-plugin/marketplace.json"),
        serde_json::to_vec(&json!({
            "name": "taste-skill",
            "plugins": [{"name": "taste-skill", "source": "./"}]
        }))
        .unwrap(),
    )
    .unwrap();
    fs::write(
        skill.join("SKILL.md"),
        "---\nname: taste-skill\ndescription: Add taste.\n---\n",
    )
    .unwrap();
    fs::write(
        root.join(".agents/marketplaces.json"),
        serde_json::to_vec(&json!({"marketplaces": {"taste-skill": ["taste-skill"]}})).unwrap(),
    )
    .unwrap();

    let discovery = discover(&root.canonicalize().unwrap());

    assert_eq!(discovery.plugins.len(), 1);
    assert!(!discovery.plugins[0].is_plugin);
    assert_eq!(
        discovery.plugins[0].explicit_skills,
        Some(vec![skill.canonicalize().unwrap()])
    );
    assert!(discovery.diagnostics.is_empty());
    fs::remove_dir_all(root).unwrap();
}

fn test_root() -> PathBuf {
    static NEXT_ROOT: AtomicU64 = AtomicU64::new(0);
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let sequence = NEXT_ROOT.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!("mini-agent-marketplace-{nonce}-{sequence}"));
    fs::create_dir(&root).unwrap();
    root
}
