use mini_agent_capabilities::SessionExecutionJournal;
use mini_agent_host::UserQuestionHandler;
use mini_agent_protocol::{
    ThreadId, ToolExecutionContext, TurnId, UserQuestion, UserQuestionAnswer,
    UserQuestionInteraction, UserQuestionRequest, stable_digest,
};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::Duration;
use tokio::sync::broadcast;

use mini_agent_app_server_protocol::{UserQuestionPhase, UserQuestionRespondParams};

const EVENT_CAPACITY: usize = 128;
const RESOLVED_CAPACITY: usize = 128;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserQuestionEvent {
    pub phase: UserQuestionPhase,
    pub interaction: UserQuestionInteraction,
}

#[derive(Clone)]
pub struct UserQuestionBroker {
    state: Arc<(Mutex<BrokerState>, Condvar)>,
    events: broadcast::Sender<UserQuestionEvent>,
    journals: Arc<Mutex<HashMap<(String, String), SessionExecutionJournal>>>,
}

#[derive(Default)]
struct BrokerState {
    interactions: HashMap<String, UserQuestionInteraction>,
    cancellations: HashMap<String, Weak<AtomicBool>>,
    terminal: Vec<String>,
}

impl UserQuestionBroker {
    pub fn new() -> Self {
        let (events, _) = broadcast::channel(EVENT_CAPACITY);
        Self {
            state: Arc::new((Mutex::new(BrokerState::default()), Condvar::new())),
            events,
            journals: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<UserQuestionEvent> {
        self.events.subscribe()
    }

    pub async fn next_event(
        &self,
        receiver: &mut broadcast::Receiver<UserQuestionEvent>,
    ) -> Result<UserQuestionEvent, broadcast::error::RecvError> {
        receiver.recv().await
    }

    pub fn respond(
        &self,
        params: UserQuestionRespondParams,
    ) -> Result<UserQuestionInteraction, String> {
        let (lock, changed) = &*self.state;
        let mut state = lock.lock().unwrap();
        let interaction = state
            .interactions
            .get(&params.interaction_id)
            .cloned()
            .ok_or_else(|| "user question interaction is no longer active".to_string())?;
        if interaction.thread_id != params.thread_id
            || interaction.turn_id != params.turn_id
            || interaction.call_id != params.call_id
        {
            return Err("user question identity does not match the active interaction".to_string());
        }
        if state
            .cancellations
            .get(&params.interaction_id)
            .and_then(Weak::upgrade)
            .is_some_and(|cancellation| cancellation.load(Ordering::Acquire))
        {
            state.interactions.remove(&params.interaction_id);
            state.cancellations.remove(&params.interaction_id);
            state.terminal.push(params.interaction_id.clone());
            trim_terminal(&mut state);
            changed.notify_all();
            drop(state);
            let _ = self.events.send(UserQuestionEvent {
                phase: UserQuestionPhase::Cancelled,
                interaction,
            });
            return Err("user question interaction is no longer active".to_string());
        }
        let index = interaction.current_index;
        if let Some(answered_index) = interaction
            .questions
            .iter()
            .position(|question| question.id == params.question_id)
            .filter(|question_index| *question_index < index)
        {
            return if interaction.answers[answered_index].as_ref() == Some(&params.answer) {
                Ok(interaction.clone())
            } else {
                Err("question already has a different answer".to_string())
            };
        }
        let question = interaction
            .current_question()
            .filter(|question| question.id == params.question_id)
            .cloned()
            .ok_or_else(|| "question is stale or is not the current question".to_string())?;
        validate_answer(&question, &params.answer)?;
        let mut snapshot = interaction;
        snapshot.answers[index] = Some(params.answer);
        snapshot.current_index += 1;
        if let Some(journal) = self.journals.lock().unwrap().get_mut(&(
            snapshot.thread_id.as_str().to_string(),
            snapshot.turn_id.as_str().to_string(),
        )) {
            journal.persist_user_question(&snapshot)?;
        }
        state
            .interactions
            .insert(params.interaction_id.clone(), snapshot.clone());
        let phase = if snapshot.is_complete() {
            state.terminal.push(params.interaction_id.clone());
            UserQuestionPhase::Resolved
        } else {
            UserQuestionPhase::Updated
        };
        trim_terminal(&mut state);
        changed.notify_all();
        drop(state);
        let _ = self.events.send(UserQuestionEvent {
            phase,
            interaction: snapshot.clone(),
        });
        Ok(snapshot)
    }

    pub fn pending_for_thread(&self, thread_id: &ThreadId) -> Option<UserQuestionInteraction> {
        self.state
            .0
            .lock()
            .unwrap()
            .interactions
            .values()
            .filter(|interaction| &interaction.thread_id == thread_id && !interaction.is_complete())
            .max_by_key(|interaction| interaction.current_index)
            .cloned()
    }

    pub fn restore_pending(&self, interaction: UserQuestionInteraction) {
        let mut state = self.state.0.lock().unwrap();
        state
            .terminal
            .retain(|id| id != &interaction.interaction_id);
        state
            .interactions
            .insert(interaction.interaction_id.clone(), interaction);
    }

    pub fn bind_journal(
        &self,
        thread_id: &ThreadId,
        turn_id: &TurnId,
        journal: SessionExecutionJournal,
    ) {
        if let Some(interaction) = journal
            .execution_state()
            .and_then(|state| state.pending_user_question)
        {
            self.restore_pending(interaction);
        }
        let key = (thread_id.as_str().to_string(), turn_id.as_str().to_string());
        let mut journals = self.journals.lock().unwrap();
        journals.retain(|(thread, _), _| thread != thread_id.as_str());
        journals.insert(key, journal);
    }
}

impl UserQuestionHandler for UserQuestionBroker {
    fn ask(
        &self,
        request: UserQuestionRequest,
        cancellation: Option<Arc<AtomicBool>>,
    ) -> Result<Vec<UserQuestionAnswer>, String> {
        let interaction_id = interaction_id(&request.context, &request.call_id);
        let (interaction, inserted) = {
            let (lock, _) = &*self.state;
            let mut state = lock.lock().unwrap();
            let inserted = !state.interactions.contains_key(&interaction_id);
            let interaction = state
                .interactions
                .entry(interaction_id.clone())
                .or_insert_with(|| UserQuestionInteraction {
                    interaction_id: interaction_id.clone(),
                    thread_id: request.context.thread_id.clone(),
                    turn_id: request.context.turn_id.clone(),
                    call_id: request.call_id.clone(),
                    answers: vec![None; request.questions.len()],
                    questions: request.questions.clone(),
                    current_index: 0,
                })
                .clone();
            if !interaction.is_complete()
                && let Some(cancellation) = cancellation.as_ref()
            {
                state
                    .cancellations
                    .insert(interaction_id.clone(), Arc::downgrade(cancellation));
            }
            (interaction, inserted)
        };
        if interaction.thread_id != request.context.thread_id
            || interaction.turn_id != request.context.turn_id
            || interaction.call_id != request.call_id
        {
            return Err("restored question identity does not match this tool call".to_string());
        }
        if interaction.questions != request.questions {
            return Err("restored questions do not match this tool call".to_string());
        }
        if interaction.is_complete() {
            return Ok(interaction.answers.into_iter().flatten().collect());
        }
        let persist_result = if inserted {
            self.journals
                .lock()
                .unwrap()
                .get_mut(&(
                    interaction.thread_id.as_str().to_string(),
                    interaction.turn_id.as_str().to_string(),
                ))
                .map_or(Ok(()), |journal| {
                    journal.persist_user_question(&interaction).map(|_| ())
                })
        } else {
            Ok(())
        };
        if let Err(error) = persist_result {
            let mut state = self.state.0.lock().unwrap();
            state.interactions.remove(&interaction_id);
            state.cancellations.remove(&interaction_id);
            return Err(error);
        }
        let _ = self.events.send(UserQuestionEvent {
            phase: UserQuestionPhase::Requested,
            interaction: interaction.clone(),
        });
        let (lock, changed) = &*self.state;
        let mut state = lock.lock().unwrap();
        loop {
            if let Some(interaction) = state
                .interactions
                .get(&interaction_id)
                .filter(|interaction| interaction.is_complete())
                .cloned()
            {
                state.cancellations.remove(&interaction_id);
                return Ok(interaction
                    .answers
                    .iter()
                    .filter_map(Clone::clone)
                    .collect());
            }
            if cancellation
                .as_ref()
                .is_some_and(|flag| flag.load(Ordering::Acquire))
            {
                let snapshot = state.interactions.remove(&interaction_id);
                state.cancellations.remove(&interaction_id);
                if let Some(interaction) = snapshot {
                    state.terminal.push(interaction_id.clone());
                    trim_terminal(&mut state);
                    drop(state);
                    let _ = self.events.send(UserQuestionEvent {
                        phase: UserQuestionPhase::Cancelled,
                        interaction,
                    });
                    return Err("user question was cancelled".to_string());
                }
                return Err("user question was cancelled".to_string());
            }
            let (next, _) = changed
                .wait_timeout(state, Duration::from_millis(50))
                .unwrap();
            state = next;
        }
    }
}

fn interaction_id(context: &ToolExecutionContext, call_id: &str) -> String {
    let input = format!(
        "{}\n{}\n{call_id}",
        context.thread_id.as_str(),
        context.turn_id.as_str()
    );
    format!("uq-{}", stable_digest(input.as_bytes()))
}

fn validate_answer(question: &UserQuestion, answer: &UserQuestionAnswer) -> Result<(), String> {
    match answer {
        UserQuestionAnswer::Option { option_id }
            if question
                .options
                .iter()
                .any(|option| option.id == *option_id) =>
        {
            Ok(())
        }
        UserQuestionAnswer::Option { .. } => Err("answer references an unknown option".to_string()),
        UserQuestionAnswer::Text { text }
            if question.allow_free_text && !text.trim().is_empty() && text.len() <= 4_000 =>
        {
            Ok(())
        }
        UserQuestionAnswer::Text { .. } => {
            Err("free-text answer must contain 1 to 4000 UTF-8 bytes".to_string())
        }
        UserQuestionAnswer::Skipped if question.allow_skip => Ok(()),
        UserQuestionAnswer::Skipped => Err("this question cannot be skipped".to_string()),
    }
}

fn trim_terminal(state: &mut BrokerState) {
    while state.terminal.len() > RESOLVED_CAPACITY {
        if let Some(id) = state.terminal.first().cloned() {
            state.terminal.remove(0);
            state.interactions.remove(&id);
        }
    }
}

impl Default for UserQuestionBroker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mini_agent_protocol::{ToolExecutionContext, UserQuestionOption};

    #[test]
    fn accepts_sequential_answers_and_rejects_conflicts_and_wrong_scope() {
        let broker = UserQuestionBroker::new();
        let context = ToolExecutionContext {
            thread_id: ThreadId::new("thread-a"),
            turn_id: TurnId::new("turn-a"),
            project_id: None,
            workspace_id: None,
            workspace_revision: None,
            session_id: None,
        };
        let request = UserQuestionRequest {
            context: context.clone(),
            call_id: "call-a".to_string(),
            questions: vec![
                UserQuestion {
                    id: "q1".to_string(),
                    prompt: "Pick".to_string(),
                    options: vec![UserQuestionOption {
                        id: "o1".to_string(),
                        label: "A".to_string(),
                        description: None,
                        recommended: true,
                        recommendation_reason: Some("fits".to_string()),
                    }],
                    allow_free_text: true,
                    allow_skip: true,
                },
                UserQuestion {
                    id: "q2".to_string(),
                    prompt: "Add details".to_string(),
                    options: Vec::new(),
                    allow_free_text: true,
                    allow_skip: true,
                },
            ],
        };
        let ask = {
            let broker = broker.clone();
            std::thread::spawn(move || broker.ask(request, None).unwrap())
        };
        let interaction = loop {
            if let Some(interaction) = broker.pending_for_thread(&context.thread_id) {
                break interaction;
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        let params = UserQuestionRespondParams {
            interaction_id: interaction.interaction_id.clone(),
            thread_id: context.thread_id.clone(),
            turn_id: context.turn_id.clone(),
            call_id: "call-a".to_string(),
            question_id: "q1".to_string(),
            answer: UserQuestionAnswer::Option {
                option_id: "o1".to_string(),
            },
        };
        broker.respond(params.clone()).unwrap();
        assert!(broker.respond(params.clone()).is_ok());
        assert!(
            broker
                .respond(UserQuestionRespondParams {
                    answer: UserQuestionAnswer::Skipped,
                    ..params.clone()
                })
                .is_err()
        );
        broker
            .respond(UserQuestionRespondParams {
                question_id: "q2".to_string(),
                answer: UserQuestionAnswer::Text {
                    text: "details".to_string(),
                },
                ..params.clone()
            })
            .unwrap();
        assert!(broker.respond(params.clone()).is_ok());
        assert!(
            broker
                .respond(UserQuestionRespondParams {
                    answer: UserQuestionAnswer::Skipped,
                    ..params
                })
                .is_err()
        );
        assert_eq!(
            ask.join().unwrap(),
            [
                UserQuestionAnswer::Option {
                    option_id: "o1".to_string()
                },
                UserQuestionAnswer::Text {
                    text: "details".to_string()
                }
            ]
        );
    }

    #[test]
    fn rejects_late_answers_after_question_cancellation() {
        let broker = UserQuestionBroker::new();
        let context = ToolExecutionContext {
            thread_id: ThreadId::new("thread-cancel"),
            turn_id: TurnId::new("turn-cancel"),
            project_id: None,
            workspace_id: None,
            workspace_revision: None,
            session_id: None,
        };
        let cancellation = Arc::new(AtomicBool::new(false));
        let ask = {
            let broker = broker.clone();
            let cancellation = cancellation.clone();
            let context = context.clone();
            std::thread::spawn(move || {
                broker.ask(
                    UserQuestionRequest {
                        context,
                        call_id: "call-cancel".to_string(),
                        questions: vec![UserQuestion {
                            id: "q1".to_string(),
                            prompt: "Continue?".to_string(),
                            options: Vec::new(),
                            allow_free_text: true,
                            allow_skip: true,
                        }],
                    },
                    Some(cancellation),
                )
            })
        };
        let interaction = loop {
            if let Some(interaction) = broker.pending_for_thread(&context.thread_id) {
                break interaction;
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        cancellation.store(true, Ordering::Release);
        let response = broker.respond(UserQuestionRespondParams {
            interaction_id: interaction.interaction_id,
            thread_id: context.thread_id,
            turn_id: context.turn_id,
            call_id: "call-cancel".to_string(),
            question_id: "q1".to_string(),
            answer: UserQuestionAnswer::Skipped,
        });
        assert!(response.is_err());
        assert!(ask.join().unwrap().is_err());
    }
}
