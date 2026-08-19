use std::env;
use std::io::BufReader;
use std::os::unix::fs::MetadataExt;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

use crate::error::{Error, Result, io_error};
use crate::model::{Progress, WritePlan};
use crate::operation::{OperationControl, OperationState};
use crate::privileged_protocol::run_privileged_client;

const SYSTEM_HELPER: &str = "/Library/PrivilegedHelperTools/app.bootable.helper";

pub(crate) fn write_via_authorization(
    plan: &WritePlan,
    confirmation: &str,
    control: &OperationControl,
    progress: &mut dyn FnMut(Progress),
) -> Result<()> {
    let helper = PathBuf::from(SYSTEM_HELPER);
    validate_helper(&helper)?;
    let channel = tempfile::Builder::new()
        .prefix("bootable-authorize-")
        .tempdir()
        .map_err(|error| io_error(env::temp_dir(), error))?;
    let socket_path = channel.path().join("helper.sock");
    let listener =
        UnixListener::bind(&socket_path).map_err(|error| io_error(&socket_path, error))?;
    listener
        .set_nonblocking(true)
        .map_err(|error| io_error(&socket_path, error))?;

    let command = format!(
        "{} --unix-socket {}",
        shell_quote(&helper),
        shell_quote(&socket_path)
    );
    let script = format!(
        "do shell script \"{}\" with administrator privileges",
        applescript_escape(&command)
    );
    let mut authorization = Command::new("/usr/bin/osascript")
        .args(["-e", &script])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| io_error("/usr/bin/osascript", error))?;

    let stream = loop {
        if control.state() == OperationState::Cancelled {
            let _ = authorization.kill();
            let _ = authorization.wait();
            return Err(Error::OperationCancelled);
        }
        match listener.accept() {
            Ok((stream, _)) => break stream,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(error) => return Err(io_error(&socket_path, error)),
        }
        if let Some(status) = authorization
            .try_wait()
            .map_err(|error| io_error("/usr/bin/osascript", error))?
        {
            let detail = authorization
                .stderr
                .take()
                .map(|stderr| {
                    let mut detail = String::new();
                    let _ = std::io::Read::read_to_string(&mut BufReader::new(stderr), &mut detail);
                    detail
                })
                .unwrap_or_default();
            return if detail.to_ascii_lowercase().contains("user canceled") {
                Err(Error::PrivilegeDenied)
            } else {
                Err(Error::PrivilegedWriterUnavailable(format!(
                    "macOS authorization exited with {status}: {}",
                    detail.trim()
                )))
            };
        }
        thread::sleep(Duration::from_millis(50));
    };

    let command_stream = stream
        .try_clone()
        .map_err(|error| io_error("macOS privileged command channel", error))?;
    let protocol = run_privileged_client(
        stream,
        command_stream,
        plan,
        confirmation,
        control,
        progress,
        "macOS privileged channel",
    )?;
    let output = authorization
        .wait_with_output()
        .map_err(|error| io_error("/usr/bin/osascript", error))?;
    let detail = String::from_utf8_lossy(&output.stderr);
    let fallback = if detail.to_ascii_lowercase().contains("user canceled") {
        Error::PrivilegeDenied
    } else if detail.trim().is_empty() {
        Error::PrivilegedWriteFailed(format!("macOS helper exited with {}", output.status))
    } else {
        Error::PrivilegedWriteFailed(detail.trim().to_owned())
    };
    protocol.complete(output.status.success(), fallback)
}

fn validate_helper(helper: &Path) -> Result<()> {
    let metadata = helper.metadata().map_err(|error| {
        Error::PrivilegedWriterUnavailable(format!(
            "{} is unavailable ({error}); install the root-owned helper first",
            helper.display()
        ))
    })?;
    if !metadata.is_file()
        || metadata.uid() != 0
        || metadata.mode() & 0o022 != 0
        || metadata.mode() & 0o111 == 0
    {
        return Err(Error::PrivilegedWriterUnavailable(format!(
            "{} must be executable, owned by root, and not group/world-writable",
            helper.display()
        )));
    }
    Ok(())
}

fn shell_quote(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"))
}

fn applescript_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authorization_command_quotes_shell_and_applescript_metacharacters() {
        let quoted = shell_quote(Path::new("/tmp/it's helper"));
        assert_eq!(quoted, "'/tmp/it'\\''s helper'");
        assert_eq!(
            applescript_escape("helper \\\"quoted\\\""),
            "helper \\\\\\\"quoted\\\\\\\""
        );
    }

    #[test]
    fn privileged_helper_path_is_fixed_and_absolute() {
        assert_eq!(
            SYSTEM_HELPER,
            "/Library/PrivilegedHelperTools/app.bootable.helper"
        );
        assert!(Path::new(SYSTEM_HELPER).is_absolute());
    }
}
