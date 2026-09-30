use crate::{ThreadId, ToolExecutionContext, TurnId};
use serde::{Deserialize, Serialize};

/// A bounded model-authored choice shown to a human by an interactive client.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UserQuestionOption {
    pub id: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub recommended: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recommendation_reason: Option<String>,
}

/// One question in a bounded, sequential user interaction.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UserQuestion {
    pub id: String,
    pub prompt: String,
    #[serde(default)]
    pub options: Vec<UserQuestionOption>,
    #[serde(default)]
    pub allow_free_text: bool,
    #[serde(default)]
    pub allow_skip: bool,
}

/// A structured answer returned to the model as the `ask_user` tool result.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum UserQuestionAnswer {
    Option { option_id: String },
    Text { text: String },
    Skipped,
}

/// Stable question batch identity carried through live requests and recovery.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UserQuestionInteraction {
    pub interaction_id: String,
    pub thread_id: ThreadId,
    pub turn_id: TurnId,
    pub call_id: String,
    pub questions: Vec<UserQuestion>,
    pub answers: Vec<Option<UserQuestionAnswer>>,
    pub current_index: usize,
}

impl UserQuestionInteraction {
    pub fn is_complete(&self) -> bool {
        self.answers.len() == self.questions.len() && self.answers.iter().all(Option::is_some)
    }

    pub fn current_question(&self) -> Option<&UserQuestion> {
        self.questions.get(self.current_index)
    }
}

/// Host-side request passed to the interactive question provider.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UserQuestionRequest {
    pub context: ToolExecutionContext,
    pub call_id: String,
    pub questions: Vec<UserQuestion>,
}
