use std::fs::File;
use std::io::{self, BufRead, BufReader, BufWriter, Read, Write};

use bootable_core::{
    Bootable, OperationControl, PrivilegedWriteCommand, PrivilegedWriteEvent,
    PrivilegedWriteRequest,
};

fn emit(writer: &mut impl Write, event: &PrivilegedWriteEvent) -> bool {
    serde_json::to_writer(&mut *writer, event).is_ok()
        && writer.write_all(b"\n").is_ok()
        && writer.flush().is_ok()
}

fn serve<R, W>(mut reader: BufReader<R>, mut writer: BufWriter<W>) -> i32
where
    R: Read + Send + 'static,
    W: Write,
{
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

#[cfg(unix)]
fn unix_socket_path() -> Option<std::path::PathBuf> {
    let mut arguments = std::env::args_os().skip(1);
    match (arguments.next(), arguments.next(), arguments.next()) {
        (Some(flag), Some(path), None) if flag == "--unix-socket" => Some(path.into()),
        _ => None,
    }
}

#[cfg(unix)]
fn serve_unix_socket(path: &std::path::Path) -> io::Result<i32> {
    use std::os::unix::net::UnixStream;

    let stream = UnixStream::connect(path)?;
    let reader = BufReader::new(stream.try_clone()?);
    let writer = BufWriter::new(stream);
    Ok(serve(reader, writer))
}

fn serve_standard_io() -> io::Result<i32> {
    let input = File::open("/dev/stdin")?;
    Ok(serve(BufReader::new(input), BufWriter::new(io::stdout())))
}

fn main() {
    #[cfg(unix)]
    let result = match unix_socket_path() {
        Some(path) => serve_unix_socket(&path),
        None => serve_standard_io(),
    };
    #[cfg(not(unix))]
    let result = serve_standard_io();

    match result {
        Ok(code) => std::process::exit(code),
        Err(error) => {
            let mut writer = BufWriter::new(io::stdout());
            let _ = emit(
                &mut writer,
                &PrivilegedWriteEvent::Failed {
                    message: format!("could not open privileged request channel: {error}"),
                },
            );
            std::process::exit(2);
        }
    }
}
