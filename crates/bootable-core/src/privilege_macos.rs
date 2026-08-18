use std::env;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::MetadataExt;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use crate::error::{Error, Result, io_error};
use crate::model::{
    PrivilegedWriteCommand, PrivilegedWriteEvent, PrivilegedWriteRequest, Progress, WritePlan,
};
use crate::operation::{OperationControl, OperationState};

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

    let mut stream = loop {
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

    let request = PrivilegedWriteRequest {
        plan: plan.clone(),
        confirmation: confirmation.to_owned(),
    };
    serde_json::to_writer(&mut stream, &request).map_err(|error| {
        Error::PrivilegedWriterUnavailable(format!("could not encode reviewed plan: {error}"))
    })?;
    stream
        .write_all(b"\n")
        .and_then(|()| stream.flush())
        .map_err(|error| io_error("macOS privileged request channel", error))?;

    let mut command_stream = stream
        .try_clone()
        .map_err(|error| io_error("macOS privileged command channel", error))?;
    let writer_control = control.clone();
    let writer_finished = Arc::new(AtomicBool::new(false));
    let writer_done = Arc::clone(&writer_finished);
    let control_writer = thread::spawn(move || {
        while !writer_done.load(Ordering::Acquire) {
            if writer_control.state() == OperationState::Cancelled {
                if serde_json::to_writer(&mut command_stream, &PrivilegedWriteCommand::Cancel)
                    .is_ok()
                {
                    let _ = command_stream.write_all(b"\n");
                    let _ = command_stream.flush();
                }
                break;
            }
            thread::sleep(Duration::from_millis(50));
        }
    });

    let mut finished = false;
    let mut failure = None;
    for line in BufReader::new(stream).lines() {
        let line = line.map_err(|error| io_error("macOS privileged progress channel", error))?;
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<PrivilegedWriteEvent>(&line).map_err(|error| {
            Error::PrivilegedWriteFailed(format!("invalid helper response: {error}"))
        })? {
            PrivilegedWriteEvent::Progress(update) => progress(update),
            PrivilegedWriteEvent::Finished => finished = true,
            PrivilegedWriteEvent::Failed { message } => failure = Some(message),
        }
    }
    writer_finished.store(true, Ordering::Release);
    let _ = control_writer.join();
    let output = authorization
        .wait_with_output()
        .map_err(|error| io_error("/usr/bin/osascript", error))?;
    if let Some(message) = failure {
        return Err(Error::PrivilegedWriteFailed(message));
    }
    if output.status.success() && finished {
        return Ok(());
    }
    let detail = String::from_utf8_lossy(&output.stderr);
    if detail.to_ascii_lowercase().contains("user canceled") {
        return Err(Error::PrivilegeDenied);
    }
    Err(Error::PrivilegedWriteFailed(if detail.trim().is_empty() {
        format!("macOS helper exited with {}", output.status)
    } else {
        detail.trim().to_owned()
    }))
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
