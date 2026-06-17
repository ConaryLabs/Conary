// apps/conary/src/commands/record_mode/fanotify_backend.rs

use std::ffi::CString;
use std::io;
use std::os::fd::RawFd;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use conary_core::recipe::recording::{
    ScopeRoot, SelectedBackend, TraceOperation, TraceScope as ReportScope,
};

use super::trace::{
    RawTraceEvent, TraceBackend, TraceBackendStatus, TraceDrain, TraceScope, TraceSession,
};
use super::types::RequestedRecordBackend;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FanotifyProbe {
    Available,
    PermissionDenied,
    Unsupported,
}

pub(crate) struct FanotifyTraceBackend {
    probe_override: Option<FanotifyProbe>,
}

impl FanotifyTraceBackend {
    pub(crate) fn new() -> Self {
        Self {
            probe_override: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn with_probe(probe: FanotifyProbe) -> Self {
        Self {
            probe_override: Some(probe),
        }
    }

    pub(crate) fn probe_without_scope(
        &self,
        _requested: RequestedRecordBackend,
    ) -> Result<TraceBackendStatus> {
        match self.probe_override.unwrap_or_else(probe_fanotify_support) {
            FanotifyProbe::Available => Ok(TraceBackendStatus::selected(
                SelectedBackend::Fanotify,
                Vec::new(),
            )),
            FanotifyProbe::PermissionDenied => Ok(TraceBackendStatus::unavailable(
                SelectedBackend::Fanotify,
                "fanotify requires CAP_SYS_ADMIN for scoped marks in this environment",
            )),
            FanotifyProbe::Unsupported => Ok(TraceBackendStatus::unavailable(
                SelectedBackend::Fanotify,
                "fanotify is not supported by this kernel",
            )),
        }
    }
}

impl TraceBackend for FanotifyTraceBackend {
    fn probe(
        &self,
        _scope: &TraceScope,
        requested: RequestedRecordBackend,
    ) -> Result<TraceBackendStatus> {
        self.probe_without_scope(requested)
    }

    fn start(&self, scope: TraceScope) -> Result<Box<dyn TraceSession>> {
        let fd = fanotify_init()?;
        if let Err(error) = mark_scope_roots(fd, &scope) {
            close_fd(fd);
            return Err(error);
        }
        Ok(Box::new(FanotifyTraceSession {
            fd,
            scope,
            buffer: vec![0; 64 * 1024],
            closed: false,
        }))
    }
}

struct FanotifyTraceSession {
    fd: RawFd,
    scope: TraceScope,
    buffer: Vec<u8>,
    closed: bool,
}

impl TraceSession for FanotifyTraceSession {
    fn drain_events(&mut self) -> Result<TraceDrain> {
        let mut drain = TraceDrain::default();
        loop {
            // SAFETY: `buffer` points to valid writable memory for its length,
            // and `fd` is an open fanotify descriptor owned by this session.
            let read = unsafe {
                libc::read(
                    self.fd,
                    self.buffer.as_mut_ptr().cast::<libc::c_void>(),
                    self.buffer.len(),
                )
            };
            if read == 0 {
                break;
            }
            if read < 0 {
                let error = io::Error::last_os_error();
                match error.raw_os_error() {
                    Some(libc::EAGAIN) => break,
                    _ => return Err(error).context("failed to read fanotify events"),
                }
            }
            drain.events.extend(parse_fanotify_events(
                &self.buffer[..read as usize],
                &self.scope,
            )?);
        }
        Ok(drain)
    }

    fn finish(&mut self) -> Result<TraceDrain> {
        let drain = self.drain_events()?;
        self.close();
        Ok(drain)
    }
}

impl FanotifyTraceSession {
    fn close(&mut self) {
        if !self.closed {
            close_fd(self.fd);
            self.closed = true;
        }
    }
}

impl Drop for FanotifyTraceSession {
    fn drop(&mut self) {
        self.close();
    }
}

fn probe_fanotify_support() -> FanotifyProbe {
    match fanotify_init() {
        Ok(fd) => {
            close_fd(fd);
            FanotifyProbe::Available
        }
        Err(error) => match error
            .downcast_ref::<io::Error>()
            .and_then(|err| err.raw_os_error())
        {
            Some(libc::EPERM) | Some(libc::EACCES) => FanotifyProbe::PermissionDenied,
            _ => FanotifyProbe::Unsupported,
        },
    }
}

fn fanotify_init() -> Result<RawFd> {
    // SAFETY: fanotify_init is called with constant flags and no pointer
    // arguments. On success it returns a new fd owned by the caller.
    let fd = unsafe {
        libc::fanotify_init(
            libc::FAN_CLASS_NOTIF | libc::FAN_CLOEXEC | libc::FAN_NONBLOCK,
            (libc::O_RDONLY | libc::O_CLOEXEC) as u32,
        )
    };
    if fd >= 0 {
        Ok(fd)
    } else {
        Err(io::Error::last_os_error()).context("fanotify_init failed")
    }
}

fn mark_scope_roots(fd: RawFd, scope: &TraceScope) -> Result<()> {
    for root in scope.roots() {
        mark_root(fd, root)?;
    }
    Ok(())
}

fn mark_root(fd: RawFd, root: &ScopeRoot) -> Result<()> {
    let path = cstring_path(&root.root)?;
    let mask = libc::FAN_CREATE
        | libc::FAN_MODIFY
        | libc::FAN_DELETE
        | libc::FAN_MOVED_FROM
        | libc::FAN_MOVED_TO
        | libc::FAN_EVENT_ON_CHILD;
    // SAFETY: `path` is a valid NUL-terminated path buffer, `fd` is expected to
    // be a fanotify descriptor, and the call does not retain the pointer.
    let result = unsafe {
        libc::fanotify_mark(
            fd,
            libc::FAN_MARK_ADD,
            mask as u64,
            libc::AT_FDCWD,
            path.as_ptr(),
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
            .with_context(|| format!("failed to add fanotify mark for {}", root.root.display()))
    }
}

fn cstring_path(path: &Path) -> Result<CString> {
    CString::new(path.as_os_str().as_bytes())
        .with_context(|| format!("path contains NUL byte: {}", path.display()))
}

fn close_fd(fd: RawFd) {
    // SAFETY: closing an fd is safe; errors are intentionally ignored because
    // cleanup cannot be recovered here.
    unsafe {
        libc::close(fd);
    }
}

fn parse_fanotify_events(bytes: &[u8], scope: &TraceScope) -> Result<Vec<RawTraceEvent>> {
    let mut events = Vec::new();
    let mut offset = 0;
    while offset + std::mem::size_of::<libc::fanotify_event_metadata>() <= bytes.len() {
        // SAFETY: bounds above guarantee the metadata-sized read is within
        // `bytes`; read_unaligned avoids alignment requirements.
        let metadata = unsafe {
            std::ptr::read_unaligned(
                bytes[offset..]
                    .as_ptr()
                    .cast::<libc::fanotify_event_metadata>(),
            )
        };
        if metadata.event_len == 0 {
            break;
        }
        if metadata.fd >= 0 {
            let path = fanotify_event_path(metadata.fd)?;
            close_fd(metadata.fd);
            if let Some(event) = classify_fanotify_path(&path, metadata.mask, scope)? {
                events.push(event);
            }
        }
        offset += metadata.event_len as usize;
    }
    Ok(events)
}

fn fanotify_event_path(fd: RawFd) -> Result<PathBuf> {
    fs_read_link(Path::new("/proc/self/fd").join(fd.to_string()))
}

fn fs_read_link(path: PathBuf) -> Result<PathBuf> {
    std::fs::read_link(&path).with_context(|| format!("failed to read {}", path.display()))
}

fn classify_fanotify_path(
    path: &Path,
    mask: u64,
    scope: &TraceScope,
) -> Result<Option<RawTraceEvent>> {
    for root in scope.roots() {
        if path.starts_with(&root.root) {
            let operation = fanotify_operation(mask, root.scope);
            let observed = root.scope_path(path, operation)?;
            return Ok(Some(RawTraceEvent {
                path: path.to_path_buf(),
                observed,
            }));
        }
    }
    Ok(None)
}

fn fanotify_operation(mask: u64, scope: ReportScope) -> TraceOperation {
    if mask & (libc::FAN_DELETE | libc::FAN_MOVED_FROM) as u64 != 0 {
        return match scope {
            ReportScope::Install => TraceOperation::InstallDelete,
            ReportScope::Source => TraceOperation::SourceWrite,
            ReportScope::Work => TraceOperation::WorkWrite,
        };
    }
    if mask & (libc::FAN_CREATE | libc::FAN_MOVED_TO | libc::FAN_MODIFY | libc::FAN_ATTRIB) as u64
        != 0
    {
        return match scope {
            ReportScope::Install => {
                if mask & (libc::FAN_CREATE | libc::FAN_MOVED_TO) as u64 != 0 {
                    TraceOperation::InstallCreate
                } else {
                    TraceOperation::InstallModify
                }
            }
            ReportScope::Source => TraceOperation::SourceWrite,
            ReportScope::Work => TraceOperation::WorkWrite,
        };
    }
    TraceOperation::OutOfScope
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_fanotify_fails_closed_without_capability() {
        let backend = FanotifyTraceBackend::with_probe(FanotifyProbe::PermissionDenied);
        let status = backend
            .probe_without_scope(RequestedRecordBackend::Fanotify)
            .unwrap();
        assert!(!status.is_usable());
        assert!(
            status
                .unavailable_reason
                .as_deref()
                .unwrap()
                .contains("CAP_SYS_ADMIN")
        );
    }

    #[test]
    fn auto_can_report_fanotify_unavailable_for_fallback() {
        let backend = FanotifyTraceBackend::with_probe(FanotifyProbe::PermissionDenied);
        let status = backend
            .probe_without_scope(RequestedRecordBackend::Auto)
            .unwrap();
        assert!(!status.is_usable());
        assert_eq!(status.backend, SelectedBackend::Fanotify);
    }

    #[test]
    fn default_probe_reports_current_environment_status() {
        let backend = FanotifyTraceBackend::new();
        let status = backend
            .probe_without_scope(RequestedRecordBackend::Auto)
            .unwrap();

        assert_eq!(status.backend, SelectedBackend::Fanotify);
    }

    #[test]
    fn successful_probe_closes_descriptor() {
        let fd = fanotify_init();
        let Ok(fd) = fd else {
            return;
        };
        close_fd(fd);
        assert_eq!(probe_fanotify_support(), FanotifyProbe::Available);
    }

    #[test]
    fn mark_failure_closes_descriptor() {
        let fd = fanotify_init();
        let Ok(fd) = fd else {
            return;
        };
        let bad_scope = TraceScope {
            source: ScopeRoot {
                scope: ReportScope::Source,
                root: PathBuf::from("/definitely/missing/conary-record-source"),
            },
            work: ScopeRoot {
                scope: ReportScope::Work,
                root: PathBuf::from("/definitely/missing/conary-record-work"),
            },
            install: ScopeRoot {
                scope: ReportScope::Install,
                root: PathBuf::from("/definitely/missing/conary-record-install"),
            },
        };

        let error = mark_scope_roots(fd, &bad_scope).unwrap_err();
        close_fd(fd);

        assert!(error.to_string().contains("fanotify mark"));
    }
}
