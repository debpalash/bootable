use std::fs::File;
use std::io::{self, BufRead, BufReader, BufWriter, Write};

use bootable_core::{
    Bootable, OperationControl, PrivilegedWriteCommand, PrivilegedWriteEvent,
    PrivilegedWriteRequest,
};

fn emit(writer: &mut BufWriter<io::StdoutLock<'_>>, event: &PrivilegedWriteEvent) -> bool {
    serde_json::to_writer(&mut *writer, event).is_ok()
        && writer.write_all(b"\n").is_ok()
        && writer.flush().is_ok()
}

fn main() {
    let input = match File::open("/dev/stdin") {
        Ok(input) => input,
        Err(error) => {
            let mut stdout = BufWriter::new(io::stdout().lock());
            let _ = emit(
                &mut stdout,
                &PrivilegedWriteEvent::Failed {
                    message: format!("could not open privileged request pipe: {error}"),
                },
            );
            std::process::exit(2);
        }
    };
    let mut reader = BufReader::new(input);
    let mut request_line = String::new();
    let request = match reader
        .read_line(&mut request_line)
        .map_err(serde_json::Error::io)
        .and_then(|_| serde_json::from_str::<PrivilegedWriteRequest>(&request_line))
    {
        Ok(request) => request,
        Err(error) => {
            let mut stdout = BufWriter::new(io::stdout().lock());
            let _ = emit(
                &mut stdout,
                &PrivilegedWriteEvent::Failed {
                    message: format!("invalid privileged write request: {error}"),
                },
            );
            std::process::exit(2);
        }
    };

    let control = OperationControl::new();
    let command_control = control.clone();
    std::thread::spawn(move || {
        for line in reader.lines().map_while(Result::ok) {
            if matches!(
                serde_json::from_str::<PrivilegedWriteCommand>(&line),
                Ok(PrivilegedWriteCommand::Cancel)
            ) {
                command_control.cancel();
                break;
            }
        }
    });

    let stdout = io::stdout();
    let mut writer = BufWriter::new(stdout.lock());
    let result = Bootable::native().write_controlled(
        &request.plan,
        &request.confirmation,
        &control,
        |progress| {
            let _ = emit(&mut writer, &PrivilegedWriteEvent::Progress(progress));
        },
    );
    match result {
        Ok(()) => {
            if !emit(&mut writer, &PrivilegedWriteEvent::Finished) {
                std::process::exit(3);
            }
        }
        Err(error) => {
            let _ = emit(
                &mut writer,
                &PrivilegedWriteEvent::Failed {
                    message: error.to_string(),
                },
            );
            std::process::exit(1);
        }
    }
}
