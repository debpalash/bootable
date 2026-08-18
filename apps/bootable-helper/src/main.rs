use std::io::{self, BufRead, BufReader, BufWriter, Read, Write};
use std::net::{IpAddr, SocketAddr, TcpStream};

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
    Ok(serve(
        BufReader::new(io::stdin()),
        BufWriter::new(io::stdout()),
    ))
}

fn tcp_channel() -> io::Result<Option<(SocketAddr, String)>> {
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    if arguments.is_empty()
        || arguments
            .first()
            .is_some_and(|flag| flag == "--unix-socket")
    {
        return Ok(None);
    }
    if arguments.len() != 4 || arguments[0] != "--tcp" || arguments[2] != "--token" {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "expected --tcp 127.0.0.1:PORT --token 64_HEX_CHARACTERS",
        ));
    }
    let endpoint = arguments[1]
        .parse::<SocketAddr>()
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    if !matches!(endpoint.ip(), IpAddr::V4(address) if address.is_loopback()) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "privileged TCP channels must use IPv4 loopback",
        ));
    }
    let token = arguments[3].clone();
    if token.len() != 64 || !token.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "privileged TCP token must contain exactly 64 hexadecimal characters",
        ));
    }
    Ok(Some((endpoint, token)))
}

fn serve_tcp(endpoint: SocketAddr, token: &str) -> io::Result<i32> {
    let mut stream = TcpStream::connect_timeout(&endpoint, std::time::Duration::from_secs(10))?;
    stream.write_all(token.as_bytes())?;
    stream.write_all(b"\n")?;
    stream.flush()?;
    let reader = BufReader::new(stream.try_clone()?);
    let writer = BufWriter::new(stream);
    Ok(serve(reader, writer))
}

fn main() {
    let tcp = tcp_channel();
    if let Ok(Some((endpoint, token))) = &tcp {
        match serve_tcp(*endpoint, token) {
            Ok(code) => std::process::exit(code),
            Err(error) => {
                eprintln!("could not open privileged request channel: {error}");
                std::process::exit(2);
            }
        }
    }
    if let Err(error) = tcp {
        eprintln!("could not parse privileged request channel: {error}");
        std::process::exit(2);
    }

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
