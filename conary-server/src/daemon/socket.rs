// conary-server/src/daemon/socket.rs

//! Unix socket listener for conaryd
//!
//! Provides a Unix domain socket listener for the daemon. This is the primary
//! interface for CLI communication. TCP is optional and disabled by default.
//!
//! # Peer Credentials
//!
//! Unix sockets support `SO_PEERCRED` which allows the daemon to authenticate
//! the connecting process by its UID/GID. This is used for authorization
//! without requiring passwords or certificates.

use conary_core::Result;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::net::{TcpListener, UnixListener};

/// Socket configuration
#[derive(Debug, Clone)]
pub struct SocketConfig {
    /// Path to Unix socket
    pub unix_path: PathBuf,
    /// Unix socket file permissions
    pub unix_mode: u32,
    /// Optional group for socket ownership
    pub unix_group: Option<String>,
    /// Whether to enable TCP listener
    pub enable_tcp: bool,
    /// TCP bind address
    pub tcp_bind: Option<String>,
}

impl Default for SocketConfig {
    fn default() -> Self {
        Self {
            unix_path: PathBuf::from("/run/conary/conaryd.sock"),
            unix_mode: 0o660,
            unix_group: None,
            enable_tcp: false,
            tcp_bind: Some("127.0.0.1:7890".to_string()),
        }
    }
}

/// Manages socket listeners for the daemon
pub struct SocketManager {
    config: SocketConfig,
    unix_listener: Option<UnixListener>,
    tcp_listener: Option<TcpListener>,
}

impl SocketManager {
    /// Create a new socket manager
    pub fn new(config: SocketConfig) -> Self {
        Self {
            config,
            unix_listener: None,
            tcp_listener: None,
        }
    }

    /// Bind to configured sockets
    pub async fn bind(&mut self) -> Result<()> {
        // Clean up existing socket file
        if self.config.unix_path.exists() {
            std::fs::remove_file(&self.config.unix_path)?;
        }

        // Ensure parent directory exists
        if let Some(parent) = self.config.unix_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Bind Unix socket
        let unix_listener = UnixListener::bind(&self.config.unix_path).map_err(|e| {
            conary_core::Error::IoError(format!(
                "Failed to bind Unix socket at {:?}: {}",
                self.config.unix_path, e
            ))
        })?;

        // Set socket permissions
        let perms = std::fs::Permissions::from_mode(self.config.unix_mode);
        std::fs::set_permissions(&self.config.unix_path, perms)?;

        // Set group ownership if specified
        #[cfg(unix)]
        if let Some(ref group) = self.config.unix_group {
            set_socket_group(&self.config.unix_path, group)?;
        }

        log::info!(
            "Listening on Unix socket: {:?} (mode: {:o})",
            self.config.unix_path,
            self.config.unix_mode
        );

        self.unix_listener = Some(unix_listener);

        // Optionally bind TCP socket
        if self.config.enable_tcp
            && let Some(ref bind_addr) = self.config.tcp_bind
        {
            let tcp_listener = TcpListener::bind(bind_addr).await.map_err(|e| {
                conary_core::Error::IoError(format!(
                    "Failed to bind TCP socket at {}: {}",
                    bind_addr, e
                ))
            })?;

            log::info!("Listening on TCP: {}", bind_addr);
            self.tcp_listener = Some(tcp_listener);
        }

        Ok(())
    }

    /// Get the Unix listener (if bound)
    pub fn unix_listener(&self) -> Option<&UnixListener> {
        self.unix_listener.as_ref()
    }

    /// Get the TCP listener (if bound)
    pub fn tcp_listener(&self) -> Option<&TcpListener> {
        self.tcp_listener.as_ref()
    }

    /// Take ownership of the Unix listener
    pub fn take_unix_listener(&mut self) -> Option<UnixListener> {
        self.unix_listener.take()
    }

    /// Take ownership of the TCP listener
    pub fn take_tcp_listener(&mut self) -> Option<TcpListener> {
        self.tcp_listener.take()
    }

    /// Get the socket path
    pub fn socket_path(&self) -> &Path {
        &self.config.unix_path
    }

    /// Clean up socket file on shutdown
    pub fn cleanup(&self) {
        if self.config.unix_path.exists()
            && let Err(e) = std::fs::remove_file(&self.config.unix_path)
        {
            log::warn!("Failed to remove socket file: {}", e);
        }
    }
}

impl Drop for SocketManager {
    fn drop(&mut self) {
        self.cleanup();
    }
}

/// Set group ownership on a socket file
#[cfg(unix)]
fn set_socket_group(path: &Path, group_name: &str) -> Result<()> {
    use nix::unistd::{Gid, chown};
    use std::ffi::CString;

    // Look up group ID
    let group_cstr = CString::new(group_name).map_err(|_| {
        conary_core::Error::ConfigError(format!("Invalid group name: {}", group_name))
    })?;

    let gid = unsafe {
        let grp = libc::getgrnam(group_cstr.as_ptr());
        if grp.is_null() {
            // Try common alternatives
            let alternatives = ["wheel", "sudo", "adm"];
            let mut found_gid = None;

            for alt in &alternatives {
                let alt_cstr = CString::new(*alt).unwrap();
                let alt_grp = libc::getgrnam(alt_cstr.as_ptr());
                if !alt_grp.is_null() {
                    found_gid = Some((*alt_grp).gr_gid);
                    log::info!("Group '{}' not found, using '{}' instead", group_name, alt);
                    break;
                }
            }

            match found_gid {
                Some(gid) => gid,
                None => {
                    log::warn!("Could not find any suitable group for socket ownership");
                    return Ok(());
                }
            }
        } else {
            (*grp).gr_gid
        }
    };

    chown(path, None, Some(Gid::from_raw(gid)))
        .map_err(|e| conary_core::Error::IoError(format!("Failed to set socket group: {}", e)))?;

    Ok(())
}

/// Extract peer credentials from a Unix socket connection
///
/// Returns `auth::PeerCredentials` directly to avoid a duplicate struct.
#[cfg(unix)]
pub fn get_peer_credentials(
    stream: &tokio::net::UnixStream,
) -> Option<crate::daemon::auth::PeerCredentials> {
    use std::os::unix::io::AsRawFd;

    let fd = stream.as_raw_fd();

    unsafe {
        let mut cred: libc::ucred = std::mem::zeroed();
        let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;

        let result = libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            &mut cred as *mut _ as *mut libc::c_void,
            &mut len,
        );

        if result == 0 {
            Some(crate::daemon::auth::PeerCredentials {
                pid: cred.pid as u32,
                uid: cred.uid,
                gid: cred.gid,
            })
        } else {
            None
        }
    }
}

#[cfg(not(unix))]
pub fn get_peer_credentials(
    _stream: &tokio::net::UnixStream,
) -> Option<crate::daemon::auth::PeerCredentials> {
    None
}

/// Create an Arc wrapper for shared socket state
pub type SharedSocketManager = Arc<SocketManager>;

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_socket_manager_bind() {
        let temp_dir = TempDir::new().unwrap();
        let socket_path = temp_dir.path().join("test.sock");

        let config = SocketConfig {
            unix_path: socket_path.clone(),
            unix_mode: 0o660,
            unix_group: None,
            enable_tcp: false,
            tcp_bind: None,
        };

        let mut manager = SocketManager::new(config);
        manager.bind().await.unwrap();

        assert!(socket_path.exists());
        assert!(manager.unix_listener().is_some());
        assert!(manager.tcp_listener().is_none());
    }

    #[tokio::test]
    async fn test_socket_manager_cleanup() {
        let temp_dir = TempDir::new().unwrap();
        let socket_path = temp_dir.path().join("test.sock");

        let config = SocketConfig {
            unix_path: socket_path.clone(),
            unix_mode: 0o660,
            unix_group: None,
            enable_tcp: false,
            tcp_bind: None,
        };

        {
            let mut manager = SocketManager::new(config);
            manager.bind().await.unwrap();
            assert!(socket_path.exists());
        } // manager dropped here

        // Socket file should be cleaned up
        assert!(!socket_path.exists());
    }

    #[test]
    fn test_peer_credentials_is_root() {
        use crate::daemon::auth::PeerCredentials;

        let root_creds = PeerCredentials {
            pid: 1,
            uid: 0,
            gid: 0,
        };
        assert!(root_creds.is_root());

        let user_creds = PeerCredentials {
            pid: 1234,
            uid: 1000,
            gid: 1000,
        };
        assert!(!user_creds.is_root());
    }
}
