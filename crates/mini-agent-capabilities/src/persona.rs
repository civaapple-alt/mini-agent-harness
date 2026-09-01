#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentPromptKind {
    Explore,
    Plan,
    General,
}

impl AgentPromptKind {
    pub fn prompt_template(self) -> &'static str {
        match self {
            Self::Explore => include_str!("../builtin/prompts/agents/explore.md").trim_end(),
            Self::Plan => include_str!("../builtin/prompts/agents/plan.md").trim_end(),
            Self::General => include_str!("../builtin/prompts/agents/general.md").trim_end(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PersonaPromptKind {
    Reviewer,
    Implementer,
    Researcher,
}

impl PersonaPromptKind {
    pub fn prompt_template(self) -> &'static str {
        match self {
            Self::Reviewer => include_str!("../builtin/prompts/personas/reviewer.md").trim_end(),
            Self::Implementer => {
                include_str!("../builtin/prompts/personas/implementer.md").trim_end()
            }
            Self::Researcher => {
                include_str!("../builtin/prompts/personas/researcher.md").trim_end()
            }
        }
    }
}
