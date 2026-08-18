use std::path::Path;

use crate::error::{Error, Result};
use crate::model::{Device, Progress, WritePlan};
use crate::operation::OperationControl;

pub(crate) struct NativePlatform;

impl NativePlatform {
    pub(crate) fn new() -> Self {
        Self
    }

    pub(crate) fn devices(&self) -> Result<Vec<Device>> {
        Err(Error::PlatformUnavailable(std::env::consts::OS.into()))
    }

    pub(crate) fn write(
        &self,
        _plan: &WritePlan,
        _confirmation: &str,
        _control: &OperationControl,
        _progress: &mut dyn FnMut(Progress),
    ) -> Result<()> {
        Err(Error::PlatformUnavailable(std::env::consts::OS.into()))
    }

    pub(crate) fn backup(
        &self,
        _device_id_or_path: &str,
        _destination: &Path,
        _progress: &mut dyn FnMut(Progress),
    ) -> Result<()> {
        Err(Error::PlatformUnavailable(std::env::consts::OS.into()))
    }

    pub(crate) fn inspect_override(
        &self,
        _path: &Path,
    ) -> Option<Result<crate::model::ImageReport>> {
        None
    }
}
