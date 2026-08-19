use std::path::PathBuf;

use crate::{
    Bootable, DownloadJob, DownloadStatus, Error, ImageReport, OperationControl, OperationState,
    Progress, Result,
};

/// A worker launch prepared by [`ManagedDownloadSession`].
///
/// Frontends choose how to run the worker; the session owns the lifecycle and
/// queue invariants that must remain identical across GUI and TUI adapters.
#[derive(Clone)]
pub struct DownloadLaunch {
    pub id: String,
    pub destination: PathBuf,
    pub retry: bool,
    pub control: OperationControl,
}

#[derive(Debug, Clone)]
pub enum DownloadCompletion {
    Ready {
        report: ImageReport,
        destination: PathBuf,
    },
    Cancelled,
    Failed(String),
}

impl DownloadCompletion {
    pub fn from_result(result: Result<(ImageReport, PathBuf)>) -> Self {
        match result {
            Ok((report, destination)) => Self::Ready {
                report,
                destination,
            },
            Err(Error::OperationCancelled) => Self::Cancelled,
            Err(error) => Self::Failed(error.to_string()),
        }
    }
}

pub enum DownloadRequest {
    Launch(DownloadLaunch),
    Queued,
}

/// Owns the managed-download state machine above the persistent ledger.
///
/// The ledger remains responsible for durable recovery. This session decides
/// when a worker may launch, how retry interacts with an active job, and how
/// pause, cancellation, progress, and terminal outcomes are reduced.
#[derive(Default)]
pub struct ManagedDownloadSession {
    jobs: Vec<DownloadJob>,
    active_progress: Option<Progress>,
    active_control: Option<OperationControl>,
    active_job: Option<String>,
    completion: Option<DownloadCompletion>,
}

impl ManagedDownloadSession {
    pub fn refresh(&mut self, engine: &Bootable) -> Result<&[DownloadJob]> {
        self.jobs = engine.download_jobs()?;
        Ok(&self.jobs)
    }

    pub fn jobs(&self) -> &[DownloadJob] {
        &self.jobs
    }

    pub fn active_progress(&self) -> Option<&Progress> {
        self.active_progress.as_ref()
    }

    pub fn active_control(&self) -> Option<&OperationControl> {
        self.active_control.as_ref()
    }

    pub fn active_job(&self) -> Option<&str> {
        self.active_job.as_deref()
    }

    pub fn completion(&self) -> Option<&DownloadCompletion> {
        self.completion.as_ref()
    }

    pub fn is_active(&self) -> bool {
        self.active_control.is_some()
    }

    pub fn request(&mut self, id: String, destination: PathBuf, retry: bool) -> DownloadRequest {
        if self.is_active() {
            return DownloadRequest::Queued;
        }
        let control = OperationControl::new();
        self.active_control = Some(control.clone());
        self.active_job = Some(id.clone());
        self.active_progress = None;
        self.completion = None;
        DownloadRequest::Launch(DownloadLaunch {
            id,
            destination,
            retry,
            control,
        })
    }

    pub fn retry(&mut self, engine: &Bootable, id: &str) -> Result<DownloadRequest> {
        let job = self
            .jobs
            .iter()
            .find(|job| job.id == id)
            .ok_or_else(|| Error::DownloadManager("download job no longer exists".into()))?;
        if !job.status.can_retry() {
            return Err(Error::DownloadManager(format!(
                "{} cannot be retried while it is {}",
                job.label, job.status
            )));
        }
        let destination = job.destination.clone();
        if self.is_active() {
            engine.queue_download_retry(id)?;
            self.refresh(engine)?;
            Ok(DownloadRequest::Queued)
        } else {
            Ok(self.request(id.to_owned(), destination, true))
        }
    }

    pub fn next_queued(&mut self, engine: &Bootable) -> Result<Option<DownloadLaunch>> {
        if self.is_active() {
            return Ok(None);
        }
        Ok(engine.next_queued_download()?.and_then(|job| {
            match self.request(job.id, job.destination, false) {
                DownloadRequest::Launch(launch) => Some(launch),
                DownloadRequest::Queued => None,
            }
        }))
    }

    pub fn apply_progress(&mut self, progress: Progress) {
        self.active_progress = Some(progress);
    }

    pub fn finish(&mut self, completion: DownloadCompletion) {
        self.active_progress = None;
        self.active_control = None;
        self.active_job = None;
        self.completion = Some(completion);
    }

    pub fn toggle_pause(&mut self, engine: &Bootable) -> Result<Option<OperationState>> {
        let (Some(control), Some(id)) = (&self.active_control, self.active_job.as_deref()) else {
            return Ok(None);
        };
        let state = match control.state() {
            OperationState::Running => {
                control.pause();
                engine.set_download_paused(id, true)?;
                OperationState::Paused
            }
            OperationState::Paused => {
                control.resume();
                engine.set_download_paused(id, false)?;
                OperationState::Running
            }
            OperationState::Cancelled => OperationState::Cancelled,
        };
        Ok(Some(state))
    }

    pub fn cancel(&self) -> bool {
        let Some(control) = &self.active_control else {
            return false;
        };
        control.cancel();
        true
    }

    pub fn use_completed(&self, engine: &Bootable, id: &str) -> Result<ImageReport> {
        let job = self
            .jobs
            .iter()
            .find(|job| job.id == id)
            .ok_or_else(|| Error::DownloadManager("download job no longer exists".into()))?;
        if job.status != DownloadStatus::Completed {
            return Err(Error::DownloadManager(
                "only completed downloads can be selected".into(),
            ));
        }
        engine.inspect_image(&job.destination)
    }

    pub fn remove(&mut self, engine: &Bootable, id: &str) -> Result<()> {
        engine.remove_download_job(id)?;
        self.refresh(engine)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(status: DownloadStatus) -> DownloadJob {
        DownloadJob {
            id: "one".into(),
            kind: crate::DownloadKind::Iso,
            label: "image.iso".into(),
            destination: PathBuf::from("/tmp/image.iso"),
            status,
            completed: 0,
            total: None,
            message: String::new(),
            error: None,
            created_at: 1,
            updated_at: 1,
        }
    }

    #[test]
    fn permits_only_one_active_launch() {
        let mut session = ManagedDownloadSession::default();
        assert!(matches!(
            session.request("one".into(), "/tmp/one".into(), false),
            DownloadRequest::Launch(_)
        ));
        assert!(matches!(
            session.request("two".into(), "/tmp/two".into(), false),
            DownloadRequest::Queued
        ));
    }

    #[test]
    fn terminal_result_releases_active_slot() {
        let mut session = ManagedDownloadSession::default();
        let _ = session.request("one".into(), "/tmp/one".into(), false);
        session.finish(DownloadCompletion::Cancelled);
        assert!(!session.is_active());
        assert!(matches!(
            session.completion(),
            Some(DownloadCompletion::Cancelled)
        ));
    }

    #[test]
    fn retained_jobs_are_read_only_to_adapters() {
        let mut session = ManagedDownloadSession::default();
        session.jobs.push(job(DownloadStatus::Queued));
        assert_eq!(session.jobs()[0].id, "one");
    }
}
