use std::io::{BufRead, BufReader, Read};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde::Deserialize;

use crate::error::{Error, Result, io_error};
use crate::model::{Progress, WritePlan};
use crate::operation::{OperationControl, OperationState};
use crate::privileged_protocol::run_privileged_client;

const SYSTEM_HELPER: &str = r"C:\Program Files\Bootable\bootable-helper.exe";
const POWERSHELL: &str = "powershell.exe";
const AUTHORIZATION_TIMEOUT: Duration = Duration::from_secs(300);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct HelperSecurity {
    exists: bool,
    regular_file: bool,
    reparse_point: bool,
    trusted_owner: bool,
    weak_write_access: bool,
}

pub(crate) fn write_via_uac(
    plan: &WritePlan,
    confirmation: &str,
    control: &OperationControl,
    progress: &mut dyn FnMut(Progress),
) -> Result<()> {
    let helper = PathBuf::from(SYSTEM_HELPER);
    validate_helper(&helper)?;

    let listener = TcpListener::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
        .map_err(|error| io_error("Windows privileged localhost channel", error))?;
    listener
        .set_nonblocking(true)
        .map_err(|error| io_error("Windows privileged localhost channel", error))?;
    let endpoint = listener
        .local_addr()
        .map_err(|error| io_error("Windows privileged localhost channel", error))?;
    let token = authorization_token()?;
    let mut authorization = launch_uac_helper(&helper, endpoint, &token)?;
    let started = Instant::now();

    let stream = loop {
        if control.state() == OperationState::Cancelled {
            let _ = authorization.kill();
            let _ = authorization.wait();
            return Err(Error::OperationCancelled);
        }
        if started.elapsed() >= AUTHORIZATION_TIMEOUT {
            let _ = authorization.kill();
            let _ = authorization.wait();
            return Err(Error::PrivilegedWriterUnavailable(
                "Windows administrator authentication timed out".into(),
            ));
        }
        match listener.accept() {
            Ok((mut candidate, address)) if address.ip().is_loopback() => {
                if authenticate_stream(&mut candidate, &token)? {
                    break candidate;
                }
            }
            Ok((_candidate, _address)) => {}
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(error) => return Err(io_error("Windows privileged localhost channel", error)),
        }
        if let Some(status) = authorization
            .try_wait()
            .map_err(|error| io_error(POWERSHELL, error))?
        {
            let detail = read_child_stderr(&mut authorization);
            return if uac_was_cancelled(&detail) {
                Err(Error::PrivilegeDenied)
            } else {
                Err(Error::PrivilegedWriterUnavailable(format!(
                    "Windows authorization exited with {status}: {}",
                    detail.trim()
                )))
            };
        }
        thread::sleep(Duration::from_millis(50));
    };

    let command_stream = stream
        .try_clone()
        .map_err(|error| io_error("Windows privileged command channel", error))?;
    let protocol = run_privileged_client(
        stream,
        command_stream,
        plan,
        confirmation,
        control,
        progress,
        "Windows privileged channel",
    )?;
    let output = authorization
        .wait_with_output()
        .map_err(|error| io_error(POWERSHELL, error))?;

    let detail = String::from_utf8_lossy(&output.stderr);
    let fallback = if uac_was_cancelled(&detail) {
        Error::PrivilegeDenied
    } else if detail.trim().is_empty() {
        Error::PrivilegedWriteFailed(format!("Windows helper exited with {}", output.status))
    } else {
        Error::PrivilegedWriteFailed(detail.trim().to_owned())
    };
    protocol.complete(output.status.success(), fallback)
}

fn authorization_token() -> Result<String> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).map_err(|error| {
        Error::PrivilegedWriterUnavailable(format!(
            "could not create a secure Windows authorization token: {error}"
        ))
    })?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn launch_uac_helper(helper: &Path, endpoint: SocketAddr, token: &str) -> Result<Child> {
    let arguments = format!(
        "@('--tcp',{},'--token',{})",
        powershell_quote(&endpoint.to_string()),
        powershell_quote(token)
    );
    let script = format!(
        "Start-Process -FilePath {} -ArgumentList {} -Verb RunAs -Wait",
        powershell_quote(&helper.to_string_lossy()),
        arguments
    );
    Command::new(POWERSHELL)
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            &script,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| io_error(POWERSHELL, error))
}

fn authenticate_stream(stream: &mut TcpStream, token: &str) -> Result<bool> {
    stream
        .set_read_timeout(Some(HANDSHAKE_TIMEOUT))
        .map_err(|error| io_error("Windows privileged handshake", error))?;
    let mut received = String::new();
    let authenticated = BufReader::new(&mut *stream)
        .read_line(&mut received)
        .map(|read| read > 0 && constant_time_eq(received.trim().as_bytes(), token.as_bytes()))
        .unwrap_or(false);
    stream
        .set_read_timeout(None)
        .map_err(|error| io_error("Windows privileged handshake", error))?;
    Ok(authenticated)
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    let mut difference = left.len() ^ right.len();
    for index in 0..left.len().max(right.len()) {
        let left_byte = left.get(index).copied().unwrap_or_default();
        let right_byte = right.get(index).copied().unwrap_or_default();
        difference |= usize::from(left_byte ^ right_byte);
    }
    difference == 0
}

fn validate_helper(helper: &Path) -> Result<()> {
    let expected = helper.to_string_lossy();
    let script = format!(
        r#"$p={}; $exists=Test-Path -LiteralPath $p -PathType Leaf; if (-not $exists) {{ [pscustomobject]@{{PascalCase=1;Exists=$false;RegularFile=$false;ReparsePoint=$false;TrustedOwner=$false;WeakWriteAccess=$false}} | ConvertTo-Json -Compress; exit }}; $i=Get-Item -LiteralPath $p -Force; $a=Get-Acl -LiteralPath $p; $owner=($a.Owner | ForEach-Object {{ (New-Object Security.Principal.NTAccount($_)).Translate([Security.Principal.SecurityIdentifier]).Value }}); $trusted=@('S-1-5-18','S-1-5-32-544','S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464'); $weak=@('S-1-1-0','S-1-5-11','S-1-5-32-545',[Security.Principal.WindowsIdentity]::GetCurrent().User.Value); $danger=[int][Security.AccessControl.FileSystemRights]::Write -bor [int][Security.AccessControl.FileSystemRights]::Modify -bor [int][Security.AccessControl.FileSystemRights]::FullControl -bor [int][Security.AccessControl.FileSystemRights]::Delete -bor [int][Security.AccessControl.FileSystemRights]::ChangePermissions -bor [int][Security.AccessControl.FileSystemRights]::TakeOwnership; $weakWrite=@($a.Access | Where-Object {{ $_.AccessControlType -eq 'Allow' -and $weak -contains $_.IdentityReference.Translate([Security.Principal.SecurityIdentifier]).Value -and (([int]$_.FileSystemRights -band $danger) -ne 0) }}).Count -gt 0; [pscustomobject]@{{Exists=$true;RegularFile=(-not $i.PSIsContainer);ReparsePoint=(($i.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0);TrustedOwner=($trusted -contains $owner);WeakWriteAccess=$weakWrite}} | ConvertTo-Json -Compress"#,
        powershell_quote(&expected)
    );
    let output = Command::new(POWERSHELL)
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            &script,
        ])
        .output()
        .map_err(|error| io_error(POWERSHELL, error))?;
    if !output.status.success() {
        return Err(Error::PrivilegedWriterUnavailable(format!(
            "could not validate {}: {}",
            helper.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let security: HelperSecurity = serde_json::from_slice(&output.stdout).map_err(|error| {
        Error::PrivilegedWriterUnavailable(format!(
            "could not validate {} security metadata: {error}",
            helper.display()
        ))
    })?;
    if !security.exists {
        return Err(Error::PrivilegedWriterUnavailable(format!(
            "{} is missing; install Bootable for all users first",
            helper.display()
        )));
    }
    if !security.regular_file
        || security.reparse_point
        || !security.trusted_owner
        || security.weak_write_access
    {
        return Err(Error::PrivilegedWriterUnavailable(format!(
            "{} must be a regular non-reparse file owned by SYSTEM, Administrators, or TrustedInstaller and not writable by the current user or broad user groups",
            helper.display()
        )));
    }
    Ok(())
}

fn read_child_stderr(child: &mut Child) -> String {
    child
        .stderr
        .take()
        .map(|mut stderr| {
            let mut detail = String::new();
            let _ = stderr.read_to_string(&mut detail);
            detail
        })
        .unwrap_or_default()
}

fn powershell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn uac_was_cancelled(detail: &str) -> bool {
    let detail = detail.to_ascii_lowercase();
    detail.contains("canceled by the user")
        || detail.contains("cancelled by the user")
        || detail.contains("operation was canceled")
        || detail.contains("operation was cancelled")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helper_path_is_fixed_under_program_files() {
        assert_eq!(
            SYSTEM_HELPER,
            r"C:\Program Files\Bootable\bootable-helper.exe"
        );
        assert!(SYSTEM_HELPER.starts_with(r"C:\Program Files\Bootable\"));
    }

    #[test]
    fn powershell_literals_escape_single_quotes() {
        assert_eq!(powershell_quote("it's fixed"), "'it''s fixed'");
    }

    #[test]
    fn handshake_comparison_rejects_prefixes_and_suffixes() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"ab", b"abc"));
        assert!(!constant_time_eq(b"abcd", b"abc"));
        assert!(!constant_time_eq(b"abd", b"abc"));
    }

    #[test]
    fn authorization_tokens_have_256_bits_of_hex_material() {
        let token = authorization_token().expect("token");
        assert_eq!(token.len(), 64);
        assert!(token.bytes().all(|byte| byte.is_ascii_hexdigit()));
    }
}
