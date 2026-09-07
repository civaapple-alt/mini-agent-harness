use crate::AppServerError;
use crate::ThreadFactory;
use mini_agent_core::Thread;
use mini_agent_core::ThreadCheckpoint;
use mini_agent_protocol::Model;
use mini_agent_protocol::ThreadId;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;

/// Owns Thread lookup, factory creation, and identity changes for one worker.
pub(super) struct ThreadManager<M> {
    threads: HashMap<String, Thread<M>>,
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
            .map(|thread| (thread.id().as_str().to_string(), thread))
            .collect();
        Self {
            threads,
            thread_ids,
            factory,
        }
    }

    pub(super) fn get(&self, thread_id: &str) -> Option<&Thread<M>> {
        self.threads.get(thread_id)
    }

    pub(super) fn get_mut(&mut self, thread_id: &str) -> Option<&mut Thread<M>> {
        self.threads.get_mut(thread_id)
    }

    pub(super) fn remove(&mut self, thread_id: &str) -> Option<Thread<M>> {
        self.threads.remove(thread_id)
    }

    pub(super) fn insert(&mut self, thread: Thread<M>) {
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
        let thread = self.create_thread(thread_id.clone())?;
        self.insert(thread);
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
        let mut fork = self.create_thread(new_thread_id.clone())?;
        let mut checkpoint = checkpoint;
        checkpoint.thread_id = new_thread_id.clone();
        fork.restore_checkpoint(checkpoint)
            .map_err(|error| AppServerError::Checkpoint(error.to_string()))?;
        self.insert(fork);
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
        let mut thread = self.create_thread(thread_id.clone())?;
        thread
            .restore_checkpoint(checkpoint)
            .map_err(|error| AppServerError::Checkpoint(error.to_string()))?;
        self.insert(thread);
        self.thread_ids.lock().unwrap().push(thread_id.clone());
        Ok(thread_id)
    }

    fn create_thread(&self, thread_id: ThreadId) -> Result<Thread<M>, AppServerError> {
        let factory = self
            .factory
            .as_ref()
            .ok_or(AppServerError::ThreadFactoryUnavailable)?;
        let mut thread = factory.create(thread_id.clone())?;
        thread.set_id(thread_id);
        Ok(thread)
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
