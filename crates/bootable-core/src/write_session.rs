use std::time::Instant;

use crate::error::{Error, Result};
use crate::model::{Progress, ProgressPhase, WritePlan};
use crate::operation::OperationControl;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteCompletion {
    Succeeded,
    AuthenticationDenied,
    Cancelled,
    Failed(String),
}

impl WriteCompletion {
    pub fn from_result(result: Result<()>) -> Self {
        match result {
            Ok(()) => Self::Succeeded,
            Err(Error::PrivilegeDenied) => Self::AuthenticationDenied,
            Err(Error::OperationCancelled) => Self::Cancelled,
            Err(error) => Self::Failed(error.to_string()),
        }
    }

    pub fn succeeded(&self) -> bool {
        matches!(self, Self::Succeeded)
    }

    pub fn status(&self) -> String {
        match self {
            Self::Succeeded => {
                "Complete • image written and verified • target can be safely removed".into()
            }
            Self::AuthenticationDenied => {
                "Write cancelled before erasure • administrator authentication was cancelled or denied"
                    .into()
            }
            Self::Cancelled => {
                "Write stopped safely • media is incomplete and must be rewritten before use"
                    .into()
            }
            Self::Failed(error) => format!("Write failed • {error}"),
        }
    }
}

#[derive(Clone)]
pub struct WriteLaunch {
    pub plan: WritePlan,
    pub confirmation: String,
    pub control: OperationControl,
}

#[derive(Default)]
pub struct ReviewedWriteSession {
    plan: Option<WritePlan>,
    confirmation_open: bool,
    acknowledged: bool,
    active: bool,
    control: Option<OperationControl>,
    progress: Option<Progress>,
    started_at: Option<Instant>,
    completion: Option<WriteCompletion>,
}

impl ReviewedWriteSession {
    pub fn open(&mut self, plan: WritePlan) {
        *self = Self {
            plan: Some(plan),
            ..Self::default()
        };
    }

    pub fn close(&mut self) -> bool {
        if self.active {
            return false;
        }
        *self = Self::default();
        true
    }

    pub fn open_confirmation(&mut self) -> bool {
        if self.active || self.succeeded() || self.plan.is_none() {
            return false;
        }
        self.confirmation_open = true;
        self.acknowledged = false;
        true
    }

    pub fn close_confirmation(&mut self) {
        if !self.active {
            self.confirmation_open = false;
            self.acknowledged = false;
        }
    }

    pub fn set_acknowledged(&mut self, acknowledged: bool) {
        if self.confirmation_open && !self.active && !self.succeeded() {
            self.acknowledged = acknowledged;
        }
    }

    pub fn toggle_acknowledged(&mut self) {
        self.set_acknowledged(!self.acknowledged);
    }

    pub fn begin(&mut self) -> std::result::Result<WriteLaunch, &'static str> {
        if self.active || self.succeeded() {
            return Err("the reviewed write cannot be started again");
        }
        let Some(plan) = self.plan.clone() else {
            return Err("Review the write plan before writing");
        };
        if !self.confirmation_open || !self.acknowledged {
            return Err("Acknowledge the consequences before confirming the write");
        }

        let control = OperationControl::new();
        self.active = true;
        self.control = Some(control.clone());
        self.confirmation_open = false;
        self.acknowledged = false;
        self.progress = Some(Progress {
            phase: ProgressPhase::Preparing,
            completed: 0,
            total: Some(plan.image.size),
            message: "Waiting for administrator authentication, then revalidating the target"
                .into(),
        });
        self.started_at = Some(Instant::now());
        self.completion = None;
        Ok(WriteLaunch {
            confirmation: plan.confirmation_phrase.clone(),
            plan,
            control,
        })
    }

    pub fn apply_progress(&mut self, progress: Progress) -> String {
        let status = format!("{} • {}", progress.phase, progress.message);
        if self.active {
            self.progress = Some(progress);
        }
        status
    }

    pub fn finish(&mut self, completion: WriteCompletion) -> String {
        self.active = false;
        self.control = None;
        if !completion.succeeded() {
            self.progress = None;
            self.started_at = None;
        }
        let status = completion.status();
        self.completion = Some(completion);
        status
    }

    pub fn cancel(&self) -> bool {
        let Some(control) = &self.control else {
            return false;
        };
        control.cancel();
        true
    }

    pub fn plan(&self) -> Option<&WritePlan> {
        self.plan.as_ref()
    }

    pub fn is_reviewing(&self) -> bool {
        self.plan.is_some()
    }

    pub fn confirmation_open(&self) -> bool {
        self.confirmation_open
    }

    pub fn acknowledged(&self) -> bool {
        self.acknowledged
    }

    pub fn can_confirm(&self) -> bool {
        self.confirmation_open && self.acknowledged && !self.active && !self.succeeded()
    }

    pub fn active(&self) -> bool {
        self.active
    }

    pub fn control(&self) -> Option<&OperationControl> {
        self.control.as_ref()
    }

    pub fn progress(&self) -> Option<&Progress> {
        self.progress.as_ref()
    }

    pub fn started_at(&self) -> Option<Instant> {
        self.started_at
    }

    pub fn completion(&self) -> Option<&WriteCompletion> {
        self.completion.as_ref()
    }

    pub fn succeeded(&self) -> bool {
        self.completion
            .as_ref()
            .is_some_and(WriteCompletion::succeeded)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Device, DeviceId, ImageKind, ImageReport, WriteOptions, WriteStrategy};
    use std::path::PathBuf;

    fn plan() -> WritePlan {
        WritePlan {
            image: ImageReport {
                path: PathBuf::from("image.iso"),
                size: 4,
                kind: ImageKind::HybridIso,
                volume_label: None,
                warnings: Vec::new(),
            },
            target: Device {
                id: DeviceId::new("usb"),
                path: PathBuf::from("/dev/test"),
                vendor: None,
                model: None,
                serial: None,
                transport: Some("usb".into()),
                capacity: 8,
                removable: true,
                read_only: false,
                system_disk: false,
                mounts: Vec::new(),
            },
            strategy: WriteStrategy::RawVerified,
            options: WriteOptions::default(),
            steps: Vec::new(),
            required_tools: Vec::new(),
            confirmation_phrase: "ERASE /dev/test usb".into(),
        }
    }

    #[test]
    fn write_cannot_start_before_review_and_acknowledgment() {
        let mut session = ReviewedWriteSession::default();
        assert!(session.begin().is_err());
        session.open(plan());
        assert!(session.begin().is_err());
        assert!(session.open_confirmation());
        assert!(session.begin().is_err());
        session.set_acknowledged(true);
        assert!(session.begin().is_ok());
        assert!(session.active());
        assert!(!session.close());
    }

    #[test]
    fn typed_completion_distinguishes_denial_cancellation_and_failure() {
        assert_eq!(
            WriteCompletion::from_result(Err(Error::PrivilegeDenied)),
            WriteCompletion::AuthenticationDenied
        );
        assert_eq!(
            WriteCompletion::from_result(Err(Error::OperationCancelled)),
            WriteCompletion::Cancelled
        );
        assert!(matches!(
            WriteCompletion::from_result(Err(Error::StalePlan("changed".into()))),
            WriteCompletion::Failed(message) if message.contains("target changed")
        ));
    }

    #[test]
    fn successful_write_is_terminal_until_a_new_plan_opens() {
        let mut session = ReviewedWriteSession::default();
        session.open(plan());
        session.open_confirmation();
        session.set_acknowledged(true);
        session.begin().expect("launch");
        session.finish(WriteCompletion::Succeeded);
        assert!(session.succeeded());
        assert!(!session.open_confirmation());
        assert!(session.begin().is_err());
        session.open(plan());
        assert!(!session.succeeded());
    }
}
