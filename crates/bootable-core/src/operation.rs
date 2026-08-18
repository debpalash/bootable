use std::fmt;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};

use crate::error::{Error, Result};

const WORKSPACE_MARGIN: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum OperationState {
    #[default]
    Running,
    Paused,
    Cancelled,
}

#[derive(Clone, Default)]
pub struct OperationControl {
    inner: Arc<ControlInner>,
}

#[derive(Default)]
struct ControlInner {
    state: Mutex<OperationState>,
    changed: Condvar,
}

impl fmt::Debug for OperationControl {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OperationControl")
            .field("state", &self.state())
            .finish()
    }
}

impl OperationControl {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn state(&self) -> OperationState {
        *self.lock_state()
    }

    pub fn pause(&self) {
        let mut state = self.lock_state();
        if *state == OperationState::Running {
            *state = OperationState::Paused;
        }
    }

    pub fn resume(&self) {
        let mut state = self.lock_state();
        if *state == OperationState::Paused {
            *state = OperationState::Running;
            self.inner.changed.notify_all();
        }
    }

    pub fn cancel(&self) {
        *self.lock_state() = OperationState::Cancelled;
        self.inner.changed.notify_all();
    }

    pub(crate) fn checkpoint(&self) -> Result<()> {
        let mut state = self.lock_state();
        while *state == OperationState::Paused {
            state = self
                .inner
                .changed
                .wait(state)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
        if *state == OperationState::Cancelled {
            Err(Error::OperationCancelled)
        } else {
            Ok(())
        }
    }

    fn lock_state(&self) -> MutexGuard<'_, OperationState> {
        self.inner
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

pub(crate) fn ensure_workspace(path: &std::path::Path, payload_bytes: Option<u64>) -> Result<()> {
    let Some(payload_bytes) = payload_bytes else {
        return Ok(());
    };
    let required = payload_bytes.saturating_add(WORKSPACE_MARGIN);
    let available =
        fs2::available_space(path).map_err(|error| crate::error::io_error(path, error))?;
    if available < required {
        Err(Error::InsufficientSpace {
            required,
            available,
        })
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::time::Duration;

    use super::*;

    #[test]
    fn cancellation_is_sticky_and_wakes_a_paused_worker() {
        let control = OperationControl::new();
        control.pause();
        let worker_control = control.clone();
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = sender.send(worker_control.checkpoint());
        });

        assert!(receiver.recv_timeout(Duration::from_millis(20)).is_err());
        control.cancel();
        let result = receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("paused worker should wake");
        assert!(matches!(result, Err(Error::OperationCancelled)));
        assert_eq!(control.state(), OperationState::Cancelled);
        control.resume();
        assert_eq!(control.state(), OperationState::Cancelled);
    }

    #[test]
    fn resume_releases_a_paused_worker() {
        let control = OperationControl::new();
        control.pause();
        let worker_control = control.clone();
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = sender.send(worker_control.checkpoint());
        });

        assert!(receiver.recv_timeout(Duration::from_millis(20)).is_err());
        control.resume();
        assert!(
            receiver
                .recv_timeout(Duration::from_secs(1))
                .expect("paused worker should resume")
                .is_ok()
        );
    }
}
