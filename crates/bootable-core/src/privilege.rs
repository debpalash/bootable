use std::env;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Once;
use std::thread;
use std::time::Duration;

use crate::error::{Error, Result, io_error};
use crate::model::{Progress, WritePlan};
use crate::operation::OperationControl;
use crate::privileged_protocol::run_privileged_client;

const PKEXEC: &str = "/usr/bin/pkexec";
const HELPER_NAME: &str = "bootable-helper";
const SYSTEM_HELPERS: [&str; 2] = [
    "/usr/libexec/bootable-helper",
    "/usr/lib/bootable/bootable-helper",
];
const AUTHENTICATION_AGENTS: [&str; 3] = [
    "/usr/libexec/polkit-mate-authentication-agent-1",
    "/usr/lib/polkit-gnome/polkit-gnome-authentication-agent-1",
    "/usr/libexec/lxqt-policykit-agent",
];
static AUTH_AGENT_START: Once = Once::new();

pub(crate) fn write_via_pkexec(
    plan: &WritePlan,
    confirmation: &str,
    control: &OperationControl,
    progress: &mut dyn FnMut(Progress),
) -> Result<()> {
    if !Path::new(PKEXEC).is_file() {
        return Err(Error::PrivilegedWriterUnavailable(
            "pkexec is not installed".into(),
        ));
    }
    let helper = helper_path()?;
    ensure_authentication_agent();
    let mut child = Command::new(PKEXEC)
        .arg("--disable-internal-agent")
        .arg(&helper)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| io_error(PKEXEC, error))?;

    let stdin = child.stdin.take().ok_or_else(|| {
        Error::PrivilegedWriterUnavailable("pkexec did not provide a request pipe".into())
    })?;
    let stderr = child.stderr.take().ok_or_else(|| {
        Error::PrivilegedWriterUnavailable("pkexec did not provide an error pipe".into())
    })?;
    let stderr_reader = thread::spawn(move || {
        let mut message = String::new();
        let _ = BufReader::new(stderr).read_to_string(&mut message);
        message
    });
    let stdout = child.stdout.take().ok_or_else(|| {
        Error::PrivilegedWriterUnavailable("pkexec did not provide a progress pipe".into())
    })?;
    let protocol = run_privileged_client(
        stdout,
        stdin,
        plan,
        confirmation,
        control,
        progress,
        "pkexec privileged channel",
    )?;
    let status = child.wait();
    let status = status.map_err(|error| io_error("bootable-helper", error))?;
    let stderr = stderr_reader.join().unwrap_or_default();

    let detail = stderr.trim();
    let fallback = if matches!(status.code(), Some(126 | 127)) {
        Error::PrivilegeDenied
    } else if detail.is_empty() {
        Error::PrivilegedWriteFailed(format!("helper exited with {status}"))
    } else {
        Error::PrivilegedWriteFailed(detail.to_string())
    };
    protocol.complete(status.success(), fallback)
}

fn ensure_authentication_agent() {
    AUTH_AGENT_START.call_once(|| {
        let Some(agent) = AUTHENTICATION_AGENTS
            .iter()
            .map(Path::new)
            .find(|path| path.is_file())
        else {
            return;
        };
        let child = Command::new(agent)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        if let Ok(mut child) = child {
            thread::spawn(move || {
                let _ = child.wait();
            });
            thread::sleep(Duration::from_millis(500));
        }
    });
}

fn helper_path() -> Result<PathBuf> {
    for candidate in SYSTEM_HELPERS.map(PathBuf::from) {
        if candidate.is_file() {
            validate_helper(&candidate)?;
            return Ok(candidate);
        }
    }
    let executable = env::current_exe().map_err(|error| io_error("current executable", error))?;
    let directory = executable.parent().ok_or_else(|| {
        Error::PrivilegedWriterUnavailable("application path has no parent directory".into())
    })?;
    let helper = directory.join(HELPER_NAME);
    helper.metadata().map_err(|error| {
        Error::PrivilegedWriterUnavailable(format!(
            "{} is missing ({error}); install Bootable's root-owned helper under /usr/libexec",
            helper.display()
        ))
    })?;
    validate_helper(&helper)?;
    Ok(helper)
}

fn validate_helper(helper: &Path) -> Result<()> {
    let metadata = helper.metadata().map_err(|error| {
        Error::PrivilegedWriterUnavailable(format!("{} is unavailable: {error}", helper.display()))
    })?;
    if !metadata.is_file() {
        return Err(Error::PrivilegedWriterUnavailable(format!(
            "{} is not a regular file",
            helper.display()
        )));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        if metadata.uid() != 0 || metadata.mode() & 0o022 != 0 || metadata.mode() & 0o111 == 0 {
            return Err(Error::PrivilegedWriterUnavailable(format!(
                "{} must be executable, owned by root, and not group/world-writable",
                helper.display()
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helper_name_is_stable_for_adjacent_packaging() {
        assert_eq!(
            Path::new(HELPER_NAME)
                .file_name()
                .and_then(|name| name.to_str()),
            Some(HELPER_NAME)
        );
    }

    #[test]
    fn installed_helper_paths_are_absolute_and_fixed() {
        assert!(
            SYSTEM_HELPERS
                .iter()
                .all(|path| Path::new(path).is_absolute())
        );
        assert_eq!(SYSTEM_HELPERS[0], "/usr/libexec/bootable-helper");
    }
}
