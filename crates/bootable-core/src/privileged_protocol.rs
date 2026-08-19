use std::io::{BufRead, BufReader, BufWriter, Read, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use crate::Bootable;
use crate::error::{Error, Result, io_error};
use crate::model::{
    PrivilegedWriteCommand, PrivilegedWriteEvent, PrivilegedWriteRequest, Progress, WritePlan,
};
use crate::operation::{OperationControl, OperationState};

pub(crate) struct PrivilegedClientOutcome {
    finished: bool,
    failure: Option<String>,
    protocol_failure: Option<String>,
}

impl PrivilegedClientOutcome {
    pub(crate) fn complete(self, process_succeeded: bool, fallback: Error) -> Result<()> {
        if let Some(message) = self.failure {
            return Err(Error::PrivilegedWriteFailed(message));
        }
        if let Some(message) = self.protocol_failure {
            return Err(Error::PrivilegedWriteFailed(message));
        }
        if process_succeeded && self.finished {
            Ok(())
        } else {
            Err(fallback)
        }
    }
}

pub(crate) fn run_privileged_client<R, W>(
    reader: R,
    mut writer: W,
    plan: &WritePlan,
    confirmation: &str,
    control: &OperationControl,
    progress: &mut dyn FnMut(Progress),
    channel: &'static str,
) -> Result<PrivilegedClientOutcome>
where
    R: Read,
    W: Write + Send + 'static,
{
    let request = PrivilegedWriteRequest {
        plan: plan.clone(),
        confirmation: confirmation.to_owned(),
    };
    serde_json::to_writer(&mut writer, &request).map_err(|error| {
        Error::PrivilegedWriterUnavailable(format!("could not encode reviewed plan: {error}"))
    })?;
    writer
        .write_all(b"\n")
        .and_then(|()| writer.flush())
        .map_err(|error| io_error(channel, error))?;

    let writer_control = control.clone();
    let writer_finished = Arc::new(AtomicBool::new(false));
    let writer_done = Arc::clone(&writer_finished);
    let control_writer = thread::spawn(move || {
        while !writer_done.load(Ordering::Acquire) {
            if writer_control.state() == OperationState::Cancelled {
                if serde_json::to_writer(&mut writer, &PrivilegedWriteCommand::Cancel).is_ok() {
                    let _ = writer.write_all(b"\n");
                    let _ = writer.flush();
                }
                break;
            }
            thread::sleep(Duration::from_millis(50));
        }
    });

    let mut outcome = PrivilegedClientOutcome {
        finished: false,
        failure: None,
        protocol_failure: None,
    };
    let mut terminal_event = false;
    for line in BufReader::new(reader).lines() {
        let line = match line {
            Ok(line) => line,
            Err(error) => {
                outcome.protocol_failure = Some(format!("could not read helper progress: {error}"));
                break;
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        let event = match serde_json::from_str::<PrivilegedWriteEvent>(&line) {
            Ok(event) => event,
            Err(error) => {
                outcome.protocol_failure = Some(format!("invalid helper response: {error}"));
                break;
            }
        };
        if terminal_event {
            outcome.protocol_failure = Some("helper sent data after a terminal event".into());
            break;
        }
        match event {
            PrivilegedWriteEvent::Progress(update) => progress(update),
            PrivilegedWriteEvent::Finished => {
                outcome.finished = true;
                terminal_event = true;
            }
            PrivilegedWriteEvent::Failed { message } => {
                outcome.failure = Some(message);
                terminal_event = true;
            }
        }
    }
    writer_finished.store(true, Ordering::Release);
    let _ = control_writer.join();
    Ok(outcome)
}

fn emit(writer: &mut impl Write, event: &PrivilegedWriteEvent) -> bool {
    serde_json::to_writer(&mut *writer, event).is_ok()
        && writer.write_all(b"\n").is_ok()
        && writer.flush().is_ok()
}

pub fn serve_privileged_writer<R, W>(reader: R, writer: W) -> i32
where
    R: Read + Send + 'static,
    W: Write,
{
    let mut reader = BufReader::new(reader);
    let mut writer = BufWriter::new(writer);
    let mut request_line = String::new();
    let request = match reader
        .read_line(&mut request_line)
        .map_err(serde_json::Error::io)
        .and_then(|_| serde_json::from_str::<PrivilegedWriteRequest>(&request_line))
    {
        Ok(request) => request,
        Err(error) => {
            let _ = emit(
                &mut writer,
                &PrivilegedWriteEvent::Failed {
                    message: format!("invalid privileged write request: {error}"),
                },
            );
            return 2;
        }
    };

    let control = OperationControl::new();
    let command_control = control.clone();
    thread::spawn(move || {
        for line in reader.lines().map_while(std::result::Result::ok) {
            if matches!(
                serde_json::from_str::<PrivilegedWriteCommand>(&line),
                Ok(PrivilegedWriteCommand::Cancel)
            ) {
                command_control.cancel();
                break;
            }
        }
    });

    let result = Bootable::native().write_controlled(
        &request.plan,
        &request.confirmation,
        &control,
        |progress| {
            let _ = emit(&mut writer, &PrivilegedWriteEvent::Progress(progress));
        },
    );
    match result {
        Ok(()) if emit(&mut writer, &PrivilegedWriteEvent::Finished) => 0,
        Ok(()) => 3,
        Err(error) => {
            let _ = emit(
                &mut writer,
                &PrivilegedWriteEvent::Failed {
                    message: error.to_string(),
                },
            );
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Device, DeviceId, ImageKind, ImageReport, WriteOptions, WriteStrategy};
    use std::path::PathBuf;
    use std::sync::Mutex;

    #[derive(Clone, Default)]
    struct SharedWriter(Arc<Mutex<Vec<u8>>>);

    impl Write for SharedWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().expect("writer").write(bytes)
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

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
    fn client_accepts_exactly_one_finished_event() {
        let events = b"{\"event\":\"finished\"}\n".as_slice();
        let outcome = run_privileged_client(
            events,
            SharedWriter::default(),
            &plan(),
            "confirm",
            &OperationControl::new(),
            &mut |_| {},
            "test channel",
        )
        .expect("protocol exchange");
        assert!(
            outcome
                .complete(true, Error::PrivilegedWriteFailed("fallback".into()))
                .is_ok()
        );
    }

    #[test]
    fn client_rejects_truncated_and_post_terminal_protocols() {
        for events in [
            "{not-json}\n".to_owned(),
            "{\"event\":\"finished\"}\n{\"event\":\"finished\"}\n".to_owned(),
            String::new(),
        ] {
            let outcome = run_privileged_client(
                events.as_bytes(),
                SharedWriter::default(),
                &plan(),
                "confirm",
                &OperationControl::new(),
                &mut |_| {},
                "test channel",
            )
            .expect("protocol exchange");
            assert!(
                outcome
                    .complete(
                        true,
                        Error::PrivilegedWriteFailed("missing terminal event".into())
                    )
                    .is_err()
            );
        }
    }

    #[test]
    fn explicit_helper_failure_precedes_process_fallback() {
        let events = b"{\"event\":\"failed\",\"data\":{\"message\":\"target changed\"}}\n";
        let outcome = run_privileged_client(
            events.as_slice(),
            SharedWriter::default(),
            &plan(),
            "confirm",
            &OperationControl::new(),
            &mut |_| {},
            "test channel",
        )
        .expect("protocol exchange");
        let error = outcome
            .complete(false, Error::PrivilegeDenied)
            .expect_err("helper failure");
        assert!(error.to_string().contains("target changed"));
    }
}
