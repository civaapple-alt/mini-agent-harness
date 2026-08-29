use std::sync::Mutex;

/// Serializes tests that temporarily replace process-level home directory variables.
pub(crate) static HOME_LOCK: Mutex<()> = Mutex::new(());
