//! SFTP session management module
//!
//! Wraps SSH2 connection establishment, authentication, and SFTP subsystem channel creation.
//! author: logic
//! date: 2026-05-31

use std::net::{TcpStream, ToSocketAddrs};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::error::SftpError;
use crate::sftp::Sftp;

/// Default connection timeout (10 seconds)
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

/// Authentication method
#[derive(Debug, Clone)]
pub enum AuthMethod {
    Password {
        password: String,
    },
    PublicKey {
        key_path: PathBuf,
        passphrase: Option<String>,
    },
}

/// SFTP session, wrapping an ssh2 connection
pub struct SftpSession {
    session: Arc<ssh2::Session>,
    _tcp: TcpStream,
    /// Marks whether the connection has been explicitly disconnected, to prevent a double disconnect in Drop
    disconnected: Arc<AtomicBool>,
}

impl SftpSession {
    /// Establish an SSH connection using the given parameters
    ///
    /// # Parameters
    /// - `host`: server address
    /// - `port`: server port
    /// - `username`: username
    /// - `auth`: authentication method
    /// - `timeout`: optional timeout; None uses the default of 10 seconds
    pub fn connect(
        host: &str,
        port: u16,
        username: &str,
        auth: AuthMethod,
        timeout: Option<Duration>,
    ) -> Result<Self, SftpError> {
        let effective_timeout = timeout.unwrap_or(DEFAULT_TIMEOUT);
        let addr = format!("{host}:{port}");

        // Perform DNS resolution via ToSocketAddrs, supporting both hostnames and IP addresses
        let socket_addr = addr
            .to_socket_addrs()
            .map_err(|e| SftpError::ConnectionFailed(format!("Address resolution failed: {e}")))?
            .next()
            .ok_or_else(|| {
                SftpError::ConnectionFailed(format!("DNS resolution returned no results: {addr}"))
            })?;

        // Use a TCP connection with a timeout
        let tcp = TcpStream::connect_timeout(&socket_addr, effective_timeout).map_err(|e| {
            if e.kind() == std::io::ErrorKind::TimedOut {
                SftpError::Timeout
            } else {
                SftpError::ConnectionFailed(format!("Failed to connect to {addr}: {e}"))
            }
        })?;

        let mut session = ssh2::Session::new().map_err(|e| {
            SftpError::ConnectionFailed(format!("Failed to create SSH session: {e}"))
        })?;

        let tcp_for_session = tcp
            .try_clone()
            .map_err(|e| SftpError::ConnectionFailed(format!("Failed to clone TCP stream: {e}")))?;
        session.set_tcp_stream(tcp_for_session);

        // Set the SSH session timeout (in milliseconds), affecting the handshake and all subsequent blocking operations
        session.set_timeout(effective_timeout.as_millis() as u32);

        session.handshake().map_err(|e| {
            if is_timeout_error(&e) {
                SftpError::Timeout
            } else {
                SftpError::ConnectionFailed(format!("SSH handshake failed: {e}"))
            }
        })?;

        // Verify the server's host key against ~/.ssh/known_hosts BEFORE sending
        // any credentials. Without this, a password sent right after the
        // handshake could be handed to a man-in-the-middle. Uses trust-on-first-
        // use (OpenSSH `StrictHostKeyChecking=accept-new`): unknown hosts are
        // recorded, but a later key change is rejected.
        verify_host_key(&session, host, port)?;

        match &auth {
            AuthMethod::Password { password } => {
                session.userauth_password(username, password).map_err(|e| {
                    if is_timeout_error(&e) {
                        SftpError::Timeout
                    } else {
                        SftpError::AuthFailed(format!("Password authentication failed: {e}"))
                    }
                })?;
            }
            AuthMethod::PublicKey {
                key_path,
                passphrase,
            } => {
                let pass = passphrase.as_deref();
                session
                    .userauth_pubkey_file(username, None, key_path, pass)
                    .map_err(|e| {
                        if is_timeout_error(&e) {
                            SftpError::Timeout
                        } else {
                            SftpError::AuthFailed(format!("Key authentication failed: {e}"))
                        }
                    })?;
            }
        }

        if !session.authenticated() {
            return Err(SftpError::AuthFailed("Authentication did not pass".into()));
        }

        // Set the operation timeout (30 seconds) to avoid operations blocking indefinitely on network failures
        session.set_timeout(30_000);

        Ok(Self {
            session: Arc::new(session),
            _tcp: tcp,
            disconnected: Arc::new(AtomicBool::new(false)),
        })
    }

    /// Get the SFTP channel
    pub fn sftp(&self) -> Result<Sftp, SftpError> {
        let sftp = self.session.sftp()?;
        Ok(Sftp::new(sftp))
    }

    /// Disconnect
    pub fn disconnect(&self) -> Result<(), SftpError> {
        if self.disconnected.swap(true, Ordering::SeqCst) {
            // Already disconnected, skip
            return Ok(());
        }
        self.session.disconnect(None, "bye", None)?;
        Ok(())
    }

    /// Check whether the connection is alive
    pub fn is_authenticated(&self) -> bool {
        self.session.authenticated()
    }
}

impl Drop for SftpSession {
    fn drop(&mut self) {
        if !self.disconnected.swap(true, Ordering::SeqCst) {
            let _ = self.session.disconnect(None, "bye", None);
        }
    }
}

/// Determine whether an ssh2 error is a timeout error
fn is_timeout_error(error: &ssh2::Error) -> bool {
    // ssh2 error code Session(-37) corresponds to LIBSSH2_ERROR_SOCKET_TIMEOUT
    error.code() == ssh2::ErrorCode::Session(-37)
}

/// Verify the server's host key against `~/.ssh/known_hosts` before
/// authenticating, using trust-on-first-use semantics (equivalent to OpenSSH's
/// `StrictHostKeyChecking=accept-new`):
///
/// - key matches a known entry -> proceed;
/// - key differs from a known entry -> reject (possible MITM);
/// - host is unknown -> record the key and proceed, so a later change is caught.
///
/// The known_hosts entry is keyed on `host` for the standard port and
/// `[host]:port` otherwise, matching OpenSSH's on-disk format.
fn verify_host_key(session: &ssh2::Session, host: &str, port: u16) -> Result<(), SftpError> {
    use ssh2::{CheckResult, KnownHostFileKind, KnownHostKeyFormat};

    let (key, key_type) = session.host_key().ok_or_else(|| {
        SftpError::HostKeyVerificationFailed("server did not present a host key".into())
    })?;

    let mut known_hosts = session.known_hosts().map_err(|e| {
        SftpError::HostKeyVerificationFailed(format!("failed to initialize known_hosts: {e}"))
    })?;

    let known_hosts_path = dirs::home_dir()
        .map(|h| h.join(".ssh").join("known_hosts"))
        .ok_or_else(|| {
            SftpError::HostKeyVerificationFailed("could not determine home directory".into())
        })?;

    // An absent known_hosts file is fine — every host is then trust-on-first-use.
    if known_hosts_path.exists() {
        known_hosts
            .read_file(&known_hosts_path, KnownHostFileKind::OpenSSH)
            .map_err(|e| {
                SftpError::HostKeyVerificationFailed(format!(
                    "failed to read {}: {e}",
                    known_hosts_path.display()
                ))
            })?;
    }

    match known_hosts.check_port(host, port, key) {
        CheckResult::Match => Ok(()),
        CheckResult::Mismatch => Err(SftpError::HostKeyVerificationFailed(format!(
            "host key for {host}:{port} does not match the entry in {} — possible \
             man-in-the-middle attack; refusing to connect. If the host key legitimately \
             changed, remove the stale entry from known_hosts and reconnect.",
            known_hosts_path.display()
        ))),
        CheckResult::NotFound => {
            let host_entry = if port == 22 {
                host.to_string()
            } else {
                format!("[{host}]:{port}")
            };
            let fmt: KnownHostKeyFormat = key_type.into();
            known_hosts
                .add(&host_entry, key, "zap-sftp (trust on first use)", fmt)
                .and_then(|()| {
                    if let Some(parent) = known_hosts_path.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    known_hosts.write_file(&known_hosts_path, KnownHostFileKind::OpenSSH)
                })
                .map_err(|e| {
                    SftpError::HostKeyVerificationFailed(format!(
                        "failed to record new host key for {host}:{port}: {e}"
                    ))
                })?;
            log::info!(
                "zap_sftp: recorded new host key for {host}:{port} in {} (trust on first use)",
                known_hosts_path.display()
            );
            Ok(())
        }
        CheckResult::Failure => Err(SftpError::HostKeyVerificationFailed(
            "libssh2 failed to check the host key against known_hosts".into(),
        )),
    }
}
