use crate::AppServerError;
use crate::ThreadFactory;
use mini_agent_core::Harness;
use mini_agent_core::RunControl;
use mini_agent_core::SteeringMode;
use mini_agent_core::Thread;
use mini_agent_core::ThreadCheckpoint;
use mini_agent_core::ThreadError;
use mini_agent_core::TurnResult;
use mini_agent_protocol::EventSink;
use mini_agent_protocol::Model;
use mini_agent_protocol::ThreadId;
use mini_agent_protocol::ThreadStatus;
use mini_agent_protocol::TurnInput;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;

/// The App Server's per-thread execution boundary.
///
/// Core still owns the turn loop. This handle owns the App Server-facing
/// lifecycle calls so callers do not reach through to `Thread` directly.
pub(super) struct ThreadHandle<M> {
    inner: Thread<M>,
}

impl<M: Model> ThreadHandle<M> {
    pub(super) fn new(inner: Thread<M>) -> Self {
        Self { inner }
    }

    pub(super) fn id(&self) -> &ThreadId {
        self.inner.id()
    }

    pub(super) fn status(&self) -> ThreadStatus {
        self.inner.status()
    }

    pub(super) fn next_turn_id(&self) -> mini_agent_protocol::TurnId {
        self.inner.next_turn_id()
    }

    pub(super) fn checkpoint(&self) -> Result<ThreadCheckpoint, ThreadError<M::Error>> {
        self.inner.checkpoint()
    }

    pub(super) fn restore_checkpoint(
        &mut self,
        checkpoint: ThreadCheckpoint,
    ) -> Result<(), ThreadError<M::Error>> {
        self.inner.restore_checkpoint(checkpoint)
    }

    pub(super) fn set_id(&mut self, id: ThreadId) {
        self.inner.set_id(id);
    }

    pub(super) fn set_next_turn_number(&mut self, next_turn_number: u64) {
        self.inner.set_next_turn_number(next_turn_number);
    }

    pub(super) fn close(&mut self) -> Result<(), ThreadError<M::Error>> {
        self.inner.close()
    }

    pub(super) fn harness(&self) -> &Harness<M> {
        self.inner.harness()
    }

    pub(super) fn harness_mut(&mut self) -> &mut Harness<M> {
        self.inner.harness_mut()
    }

    pub(super) async fn run_turn_with_events<S: EventSink + Send>(
        &mut self,
        input: TurnInput,
        sink: &mut S,
        control: &RunControl,
        steering_mode: SteeringMode,
    ) -> Result<TurnResult, ThreadError<M::Error>> {
        self.inner
            .run_turn_with_events(input, sink, control, steering_mode)
            .await
    }
}

/// Owns Thread lookup, factory creation, and identity changes for one worker.
pub(super) struct ThreadManager<M> {
    threads: HashMap<String, ThreadHandle<M>>,
    thread_ids: Arc<Mutex<Vec<ThreadId>>>,
    factory: Option<Arc<dyn ThreadFactory<M>>>,
}

impl<M: Model + 'static> ThreadManager<M> {
    pub(super) fn new(
        threads: Vec<Thread<M>>,
        thread_ids: Arc<Mutex<Vec<ThreadId>>>,
        factory: Option<Arc<dyn ThreadFactory<M>>>,
    ) -> Self {
        let threads = threads
            .into_iter()
            .map(ThreadHandle::new)
            .map(|thread| (thread.id().as_str().to_string(), thread))
            .collect();
        Self {
            threads,
            thread_ids,
            factory,
        }
    }

    pub(super) fn get(&self, thread_id: &str) -> Option<&ThreadHandle<M>> {
        self.threads.get(thread_id)
    }

    pub(super) fn get_mut(&mut self, thread_id: &str) -> Option<&mut ThreadHandle<M>> {
        self.threads.get_mut(thread_id)
    }

    pub(super) fn remove(&mut self, thread_id: &str) -> Option<ThreadHandle<M>> {
        self.threads.remove(thread_id)
    }

    pub(super) fn insert(&mut self, thread: ThreadHandle<M>) {
        self.threads
            .insert(thread.id().as_str().to_string(), thread);
    }

    pub(super) fn contains(&self, thread_id: &str) -> bool {
        self.threads.contains_key(thread_id)
    }

    pub(super) fn create(&mut self, thread_id: ThreadId) -> Result<ThreadId, AppServerError> {
        if self.contains(thread_id.as_str()) {
            return Err(AppServerError::ThreadAlreadyExists(thread_id));
        }
        let factory = self
            .factory
            .as_ref()
            .ok_or(AppServerError::ThreadFactoryUnavailable)?;
        let mut thread = factory.create(thread_id.clone())?;
        thread.set_id(thread_id.clone());
        self.insert(ThreadHandle::new(thread));
        self.thread_ids.lock().unwrap().push(thread_id.clone());
        Ok(thread_id)
    }

    pub(super) fn fork(
        &mut self,
        source_thread_id: ThreadId,
        new_thread_id: ThreadId,
    ) -> Result<ThreadId, AppServerError> {
        if self.contains(new_thread_id.as_str()) {
            return Err(AppServerError::ThreadAlreadyExists(new_thread_id));
        }
        let checkpoint = self
            .get(source_thread_id.as_str())
            .ok_or_else(|| AppServerError::ThreadNotFound(source_thread_id.clone()))?
            .checkpoint()
            .map_err(|error| AppServerError::Checkpoint(error.to_string()))?;
        let factory = self
            .factory
            .as_ref()
            .ok_or(AppServerError::ThreadFactoryUnavailable)?;
        let mut fork = factory.create(new_thread_id.clone())?;
        let mut checkpoint = checkpoint;
        checkpoint.thread_id = new_thread_id.clone();
        fork.restore_checkpoint(checkpoint)
            .map_err(|error| AppServerError::Checkpoint(error.to_string()))?;
        self.insert(ThreadHandle::new(fork));
        self.thread_ids.lock().unwrap().push(new_thread_id.clone());
        Ok(new_thread_id)
    }

    pub(super) fn resume(
        &mut self,
        thread_id: ThreadId,
        mut checkpoint: ThreadCheckpoint,
    ) -> Result<ThreadId, AppServerError> {
        checkpoint.thread_id = thread_id.clone();
        if let Some(thread) = self.get_mut(thread_id.as_str()) {
            thread
                .restore_checkpoint(checkpoint)
                .map_err(|error| AppServerError::Checkpoint(error.to_string()))?;
            return Ok(thread_id);
        }
        let factory = self
            .factory
            .as_ref()
            .ok_or(AppServerError::ThreadFactoryUnavailable)?;
        let mut thread = factory.create(thread_id.clone())?;
        thread
            .restore_checkpoint(checkpoint)
            .map_err(|error| AppServerError::Checkpoint(error.to_string()))?;
        self.insert(ThreadHandle::new(thread));
        self.thread_ids.lock().unwrap().push(thread_id.clone());
        Ok(thread_id)
    }

    pub(super) fn rename(
        &mut self,
        old_thread_id: &ThreadId,
        new_thread_id: ThreadId,
        next_turn_number: u64,
    ) -> Result<(), AppServerError> {
        if self.contains(new_thread_id.as_str()) {
            return Err(AppServerError::ThreadAlreadyExists(new_thread_id));
        }
        let mut thread = self
            .remove(old_thread_id.as_str())
            .ok_or_else(|| AppServerError::ThreadNotFound(old_thread_id.clone()))?;
        thread.set_id(new_thread_id.clone());
        thread.set_next_turn_number(next_turn_number);
        self.insert(thread);
        if let Some(known) = self
            .thread_ids
            .lock()
            .unwrap()
            .iter_mut()
            .find(|known| **known == *old_thread_id)
        {
            *known = new_thread_id;
        }
        Ok(())
    }
}
