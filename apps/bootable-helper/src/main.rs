use std::io::{self, BufWriter, Write};
use std::net::{IpAddr, SocketAddr, TcpStream};

use bootable_core::{PrivilegedWriteEvent, serve_privileged_writer};

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
    Ok(serve_privileged_writer(stream.try_clone()?, stream))
}

fn serve_standard_io() -> io::Result<i32> {
    Ok(serve_privileged_writer(io::stdin(), io::stdout()))
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
    Ok(serve_privileged_writer(stream.try_clone()?, stream))
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
            let event = PrivilegedWriteEvent::Failed {
                message: format!("could not open privileged request channel: {error}"),
            };
            let _ = serde_json::to_writer(&mut writer, &event);
            let _ = writer.write_all(b"\n");
            let _ = writer.flush();
            std::process::exit(2);
        }
    }
}
