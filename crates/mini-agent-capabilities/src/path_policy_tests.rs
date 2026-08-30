use super::*;
use std::path::Path;

#[test]
fn recognizes_plan_and_goal_artifact_aliases() {
    assert!(is_plan_md_alias(Path::new("plan.md")));
    assert!(is_plan_md_alias(Path::new("./plan.md")));
    assert!(!is_plan_md_alias(Path::new("docs/plan.md")));
    assert_eq!(
        goal_relative_rest(Path::new("goal/plan.md")).as_deref(),
        Some(Path::new("plan.md"))
    );
    assert_eq!(
        goal_relative_rest(Path::new("./goal/state.json")).as_deref(),
        Some(Path::new("state.json"))
    );
    assert_eq!(goal_relative_rest(Path::new("plan.md")), None);
}
