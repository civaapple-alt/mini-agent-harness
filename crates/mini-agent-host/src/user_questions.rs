use mini_agent_protocol::{
    ToolAdmission, ToolError, ToolExecutionOutcome, ToolExecutionRequest, ToolReplaySafety,
    ToolRuntime, ToolSpec, UserQuestion, UserQuestionAnswer, UserQuestionOption,
    UserQuestionRequest,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

const MAX_QUESTION_BYTES: usize = 1_000;
const MAX_OPTION_BYTES: usize = 240;

/// Host adapter for a client that can collect bounded answers from a user.
pub trait UserQuestionHandler: Send + Sync {
    fn ask(
        &self,
        request: UserQuestionRequest,
        cancellation: Option<Arc<AtomicBool>>,
    ) -> Result<Vec<UserQuestionAnswer>, String>;
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AskUserArguments {
    questions: Vec<QuestionInput>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct QuestionInput {
    question: String,
    #[serde(default)]
    options: Vec<OptionInput>,
    #[serde(default = "default_true")]
    allow_free_text: bool,
    #[serde(default = "default_true")]
    allow_skip: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OptionInput {
    label: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    recommended: bool,
    #[serde(default)]
    recommendation_reason: Option<String>,
}

fn default_true() -> bool {
    true
}

pub struct AskUserTool {
    handler: Arc<dyn UserQuestionHandler>,
}

impl AskUserTool {
    pub fn new(handler: Arc<dyn UserQuestionHandler>) -> Self {
        Self { handler }
    }
}

impl mini_agent_protocol::ToolHandler for AskUserTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "ask_user".to_string(),
            description: "向用户提出至多三个有边界的问题。给可执行的选项；若能根据用户目标判断最佳选择，将恰好一个选项标为 recommended 并提供简短 recommendationReason。不要为了填标签而猜测。用户也可以输入自己的答案或跳过当前问题。".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "questions": {
                        "type": "array",
                        "minItems": 1,
                        "maxItems": 3,
                        "items": {
                            "type": "object",
                            "properties": {
                                "question": { "type": "string", "minLength": 1, "maxLength": 1000 },
                                "options": {
                                    "type": "array",
                                    "maxItems": 6,
                                    "items": {
                                        "type": "object",
                                        "properties": {
                                            "label": { "type": "string", "minLength": 1, "maxLength": 240 },
                                            "description": { "type": "string", "maxLength": 240 },
                                            "recommended": { "type": "boolean" },
                                            "recommendationReason": { "type": "string", "maxLength": 240 }
                                        },
                                        "required": ["label"],
                                        "additionalProperties": false
                                    }
                                },
                                "allowFreeText": { "type": "boolean", "default": true },
                                "allowSkip": { "type": "boolean", "default": true }
                            },
                            "required": ["question"],
                            "additionalProperties": false
                        }
                    }
                },
                "required": ["questions"],
                "additionalProperties": false
            }),
        }
    }

    fn admission(&self, request: &ToolExecutionRequest) -> Result<ToolAdmission, ToolError> {
        parse_questions(&request.arguments).map_err(ToolError)?;
        Ok(ToolAdmission::Allowed {
            target_paths: Vec::new(),
        })
    }
}

impl ToolRuntime for AskUserTool {
    fn execute(&self, _arguments: &Value) -> Result<String, ToolError> {
        Err(ToolError(
            "ask_user requires an interactive App Server client".to_string(),
        ))
    }

    fn recovery_replay_safety(&self, _request: &ToolExecutionRequest) -> ToolReplaySafety {
        ToolReplaySafety::Safe
    }

    fn execute_after_admission(
        &self,
        request: &ToolExecutionRequest,
        _admission: &ToolAdmission,
    ) -> ToolExecutionOutcome {
        let Some(context) = request.context.clone() else {
            return ToolExecutionOutcome::failed(
                "ask_user requires an App Server Thread and Turn identity",
            );
        };
        let questions = match parse_questions(&request.arguments) {
            Ok(questions) => questions,
            Err(error) => return ToolExecutionOutcome::failed(error),
        };
        let response_questions = questions.clone();
        match self.handler.ask(
            UserQuestionRequest {
                context,
                call_id: request.call_id.clone(),
                questions,
            },
            request.cancellation.clone(),
        ) {
            Ok(answers) => {
                if answers.len() != response_questions.len() {
                    return ToolExecutionOutcome::failed(
                        "user question handler returned an incomplete answer set",
                    );
                }
                let answers = response_questions
                    .into_iter()
                    .zip(answers)
                    .map(|(question, answer)| {
                        let answer_label = match &answer {
                            UserQuestionAnswer::Option { option_id } => question
                                .options
                                .iter()
                                .find(|option| option.id == *option_id)
                                .map(|option| option.label.clone())
                                .unwrap_or_else(|| "已选择选项".to_string()),
                            UserQuestionAnswer::Text { text } => text.clone(),
                            UserQuestionAnswer::Skipped => "已跳过".to_string(),
                        };
                        json!({
                            "questionId": question.id,
                            "question": question.prompt,
                            "answer": answer,
                            "answerLabel": answer_label,
                        })
                    })
                    .collect::<Vec<_>>();
                ToolExecutionOutcome::completed(
                    serde_json::to_string(&json!({ "answers": answers }))
                        .unwrap_or_else(|_| "{\"answers\":[]}".to_string()),
                )
            }
            Err(error) => ToolExecutionOutcome::failed(error),
        }
    }
}

fn parse_questions(arguments: &Value) -> Result<Vec<UserQuestion>, String> {
    let input: AskUserArguments = serde_json::from_value(arguments.clone())
        .map_err(|error| format!("invalid ask_user arguments: {error}"))?;
    if input.questions.is_empty() || input.questions.len() > 3 {
        return Err("ask_user requires between one and three questions".to_string());
    }
    input
        .questions
        .into_iter()
        .enumerate()
        .map(|(question_index, question)| {
            let prompt = question.question.trim().to_string();
            if prompt.is_empty() || prompt.len() > MAX_QUESTION_BYTES {
                return Err("question text must contain 1 to 1000 UTF-8 bytes".to_string());
            }
            if question.options.len() > 6 {
                return Err("a question may contain at most six options".to_string());
            }
            if question.options.is_empty() && !question.allow_free_text {
                return Err("a question needs an option or free-text input".to_string());
            }
            if question
                .options
                .iter()
                .filter(|option| option.recommended)
                .count()
                > 1
            {
                return Err("a question may mark at most one option as recommended".to_string());
            }
            let options = question
                .options
                .into_iter()
                .enumerate()
                .map(|(option_index, option)| {
                    let label = option.label.trim().to_string();
                    if label.is_empty() || label.len() > MAX_OPTION_BYTES {
                        return Err("option labels must contain 1 to 240 UTF-8 bytes".to_string());
                    }
                    let description = bounded_optional(option.description, MAX_OPTION_BYTES)?;
                    let recommendation_reason =
                        bounded_optional(option.recommendation_reason, MAX_OPTION_BYTES)?;
                    if !option.recommended && recommendation_reason.is_some() {
                        return Err("recommendationReason requires recommended=true".to_string());
                    }
                    Ok(UserQuestionOption {
                        id: format!("q{}-o{}", question_index + 1, option_index + 1),
                        label,
                        description,
                        recommended: option.recommended,
                        recommendation_reason,
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(UserQuestion {
                id: format!("q{}", question_index + 1),
                prompt,
                options,
                allow_free_text: question.allow_free_text,
                allow_skip: question.allow_skip,
            })
        })
        .collect()
}

fn bounded_optional(value: Option<String>, max_bytes: usize) -> Result<Option<String>, String> {
    value
        .map(|value| {
            let value = value.trim().to_string();
            if value.len() > max_bytes {
                Err(format!("question detail exceeds {max_bytes} UTF-8 bytes"))
            } else if value.is_empty() {
                Ok(None)
            } else {
                Ok(Some(value))
            }
        })
        .transpose()
        .map(Option::flatten)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_recommendation_and_assigns_stable_question_and_option_ids() {
        let questions = parse_questions(&json!({"questions":[{
            "question":"Use which path?",
            "options":[
                {"label":"Recommended", "recommended":true, "recommendationReason":"Matches the request"},
                {"label":"Alternative"}
            ]
        }]})).unwrap();
        assert_eq!(questions[0].id, "q1");
        assert_eq!(questions[0].options[0].id, "q1-o1");
        assert!(questions[0].options[0].recommended);
        assert_eq!(
            questions[0].options[0].recommendation_reason.as_deref(),
            Some("Matches the request")
        );
        assert!(questions[0].allow_free_text);
        assert!(questions[0].allow_skip);
    }

    #[test]
    fn rejects_multiple_recommendations_and_oversized_batches() {
        assert!(
            parse_questions(&json!({"questions":[{
                "question":"Pick one",
                "options":[{"label":"A","recommended":true},{"label":"B","recommended":true}]
            }]}))
            .unwrap_err()
            .contains("at most one")
        );
        assert!(
            parse_questions(&json!({"questions":[
                {"question":"1"},{"question":"2"},{"question":"3"},{"question":"4"}
            ]}))
            .is_err()
        );
    }
}
