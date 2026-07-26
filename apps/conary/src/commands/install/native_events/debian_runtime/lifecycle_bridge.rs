// apps/conary/src/commands/install/native_events/debian_runtime/lifecycle_bridge.rs

//! Debian endpoint staging for Conary's private lifecycle transport.
//!
//! The shims in this module contain protocol plumbing and the documented
//! confmodule shell surface only. Typed argv grammars and state mutations live
//! in their owning handlers.

use anyhow::{Context, Result};
use conary_core::packages::deb::debconf::{
    DebconfEngine, DebconfExecution, DebconfSession, DebconfSessionPolicy, DebconfState,
    DebconfTemplateLoadError, DebconfTemplateLoader,
};
use conary_core::packages::deb::templates::{MAX_DEBCONF_TEMPLATES_SIZE, parse};
use conary_core::packages::native_abi::DebconfTemplateRecord;
use conary_core::scriptlet::{
    LifecycleBridgeConfig, LifecycleBridgeEndpoint, LifecycleBridgeHandler,
    LifecycleBridgeHandlerError, LifecycleBridgeRequest, LifecycleBridgeResponse,
    executable_bridge_shim, lifecycle_bridge_shell_library,
};
use nix::fcntl::{OFlag, OpenHow, ResolveFlag, open, openat2};
use nix::sys::stat::Mode;
use std::fs::File;
use std::io::Read;
use std::os::fd::OwnedFd;
use std::path::Path;
use std::sync::{Arc, Mutex};

const EXECUTABLE_ENDPOINTS: &[&str] = &[
    "/usr/bin/dpkg-maintscript-helper",
    "/usr/bin/dpkg-query",
    "/usr/bin/dpkg",
    "/usr/bin/update-alternatives",
    "/usr/bin/deb-systemd-helper",
    "/usr/bin/deb-systemd-invoke",
    "/usr/sbin/invoke-rc.d",
    "/usr/sbin/update-rc.d",
    "/usr/sbin/service",
];
const CONFMODULE_PATH: &str = "/usr/share/debconf/confmodule";

/// One in-memory debconf authority for one native lifecycle graph event.
///
/// The owning event thread loads and persists [`DebconfState`]. The bridge
/// server thread receives only this in-memory handler and never a database
/// connection.
pub(crate) struct DebianLifecycleBridge {
    handler: Arc<DebianLifecycleHandler>,
}

impl DebianLifecycleBridge {
    pub(crate) fn new(
        root: &Path,
        owner: &str,
        mut state: DebconfState,
        package_templates: &[DebconfTemplateRecord],
    ) -> Result<Self> {
        state
            .import_template_records(owner, package_templates)
            .with_context(|| {
                format!("cannot preload debconf templates for lifecycle owner {owner:?}")
            })?;
        let loader = SelectedRootTemplateLoader::new(root)?;
        Ok(Self {
            handler: Arc::new(DebianLifecycleHandler {
                inner: Mutex::new(DebianDebconfEvent {
                    engine: DebconfEngine::new(loader),
                    state,
                    session: DebconfSession::new(owner, DebconfSessionPolicy::default()),
                }),
            }),
        })
    }

    pub(crate) fn config(&self) -> LifecycleBridgeConfig {
        let handler: Arc<dyn LifecycleBridgeHandler> = self.handler.clone();
        LifecycleBridgeConfig::with_shared_handler(endpoints(), handler)
    }

    pub(crate) fn state(&self) -> Result<DebconfState> {
        let mut event = self
            .handler
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("Debian debconf lifecycle state lock is poisoned"))?;
        let DebianDebconfEvent { state, session, .. } = &mut *event;
        if !session.stopped {
            // EOF from a script that did not call db_stop still terminates the
            // per-event confmodule session. Commit its pending seen changes
            // before returning the state to the owning transaction thread.
            session.finish(state);
        }
        Ok(state.clone())
    }
}

fn endpoints() -> Vec<LifecycleBridgeEndpoint> {
    let mut endpoints = EXECUTABLE_ENDPOINTS
        .iter()
        .map(|path| LifecycleBridgeEndpoint::new(path, executable_bridge_shim()))
        .collect::<Vec<_>>();
    endpoints.push(LifecycleBridgeEndpoint::new(
        CONFMODULE_PATH,
        confmodule_shim(),
    ));
    endpoints
}

struct DebianLifecycleHandler {
    inner: Mutex<DebianDebconfEvent>,
}

struct DebianDebconfEvent {
    engine: DebconfEngine<SelectedRootTemplateLoader>,
    state: DebconfState,
    session: DebconfSession,
}

impl LifecycleBridgeHandler for DebianLifecycleHandler {
    fn handle(
        &self,
        request: &LifecycleBridgeRequest,
    ) -> std::result::Result<LifecycleBridgeResponse, LifecycleBridgeHandlerError> {
        let endpoint = DebianLifecycleEndpoint::parse(request.command()).ok_or_else(|| {
            LifecycleBridgeHandlerError::new(format!(
                "request uses an undeclared Debian lifecycle endpoint {:?}",
                String::from_utf8_lossy(request.command())
            ))
        })?;
        match endpoint {
            DebianLifecycleEndpoint::Confmodule => self.handle_confmodule(request.arguments()),
            unsupported => Ok(unsupported.response()),
        }
    }
}

impl DebianLifecycleHandler {
    fn handle_confmodule(
        &self,
        argv: &[Vec<u8>],
    ) -> std::result::Result<LifecycleBridgeResponse, LifecycleBridgeHandlerError> {
        let mut event = self.inner.lock().map_err(|_| {
            LifecycleBridgeHandlerError::new("Debian debconf lifecycle state lock is poisoned")
        })?;
        let (command, arguments) = argv
            .split_first()
            .map_or((b"".as_slice(), [].as_slice()), |(command, arguments)| {
                (command.as_slice(), arguments)
            });
        let DebianDebconfEvent {
            engine,
            state,
            session,
        } = &mut *event;
        match engine.process_argv(state, session, command, arguments) {
            DebconfExecution::Reply(reply) => {
                let caller = reply.caller_result();
                let exit_code = u8::try_from(caller.status.as_u16())
                    .expect("debconf caller result codes fit the private transport");
                Ok(LifecycleBridgeResponse::new(
                    caller.ret,
                    Vec::new(),
                    exit_code,
                ))
            }
            // The real shell confmodule sends STOP without reading a reply.
            // Our private request/response transport still needs a terminal
            // frame so the sourced shell function can return without hanging.
            DebconfExecution::Stopped => {
                Ok(LifecycleBridgeResponse::new(Vec::new(), Vec::new(), 0))
            }
        }
    }
}

#[derive(Clone, Copy)]
enum DebianLifecycleEndpoint {
    DpkgMaintscriptHelper,
    DpkgQuery,
    Dpkg,
    UpdateAlternatives,
    DebSystemdHelper,
    DebSystemdInvoke,
    InvokeRcD,
    UpdateRcD,
    Service,
    Confmodule,
}

impl DebianLifecycleEndpoint {
    fn parse(path: &[u8]) -> Option<Self> {
        Some(match path {
            b"/usr/bin/dpkg-maintscript-helper" => Self::DpkgMaintscriptHelper,
            b"/usr/bin/dpkg-query" => Self::DpkgQuery,
            b"/usr/bin/dpkg" => Self::Dpkg,
            b"/usr/bin/update-alternatives" => Self::UpdateAlternatives,
            b"/usr/bin/deb-systemd-helper" => Self::DebSystemdHelper,
            b"/usr/bin/deb-systemd-invoke" => Self::DebSystemdInvoke,
            b"/usr/sbin/invoke-rc.d" => Self::InvokeRcD,
            b"/usr/sbin/update-rc.d" => Self::UpdateRcD,
            b"/usr/sbin/service" => Self::Service,
            b"/usr/share/debconf/confmodule" => Self::Confmodule,
            _ => return None,
        })
    }

    fn path(self) -> &'static str {
        match self {
            Self::DpkgMaintscriptHelper => "/usr/bin/dpkg-maintscript-helper",
            Self::DpkgQuery => "/usr/bin/dpkg-query",
            Self::Dpkg => "/usr/bin/dpkg",
            Self::UpdateAlternatives => "/usr/bin/update-alternatives",
            Self::DebSystemdHelper => "/usr/bin/deb-systemd-helper",
            Self::DebSystemdInvoke => "/usr/bin/deb-systemd-invoke",
            Self::InvokeRcD => "/usr/sbin/invoke-rc.d",
            Self::UpdateRcD => "/usr/sbin/update-rc.d",
            Self::Service => "/usr/sbin/service",
            Self::Confmodule => "/usr/share/debconf/confmodule",
        }
    }

    fn response(self) -> LifecycleBridgeResponse {
        LifecycleBridgeResponse::new(
            Vec::new(),
            format!(
                "{} has no typed Debian lifecycle implementation\n",
                self.path()
            )
            .into_bytes(),
            127,
        )
    }
}

struct SelectedRootTemplateLoader {
    root: OwnedFd,
}

impl SelectedRootTemplateLoader {
    fn new(root: &Path) -> Result<Self> {
        let root = open(
            root,
            OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
            Mode::empty(),
        )
        .map_err(std::io::Error::from)
        .with_context(|| {
            format!(
                "cannot open selected root {} for debconf template loading",
                root.display()
            )
        })?;
        Ok(Self { root })
    }

    fn open_error(message: impl Into<String>) -> DebconfTemplateLoadError {
        DebconfTemplateLoadError::open(message.into().into_bytes())
    }
}

impl DebconfTemplateLoader for SelectedRootTemplateLoader {
    fn load(
        &mut self,
        path: &[u8],
    ) -> std::result::Result<Vec<DebconfTemplateRecord>, DebconfTemplateLoadError> {
        if path.is_empty() {
            return Err(Self::open_error("template path is empty"));
        }
        let how = OpenHow::new()
            .flags(OFlag::O_RDONLY | OFlag::O_CLOEXEC | OFlag::O_NONBLOCK | OFlag::O_NOCTTY)
            .resolve(ResolveFlag::RESOLVE_IN_ROOT | ResolveFlag::RESOLVE_NO_MAGICLINKS);
        let file = openat2(&self.root, path, how).map_err(|error| {
            Self::open_error(format!("selected-root path resolution failed: {error}"))
        })?;
        let mut file = File::from(file);
        let metadata = file
            .metadata()
            .map_err(|error| Self::open_error(format!("cannot inspect template file: {error}")))?;
        if !metadata.is_file() {
            return Err(Self::open_error("selected-root path is not a regular file"));
        }
        if metadata.len() > MAX_DEBCONF_TEMPLATES_SIZE {
            return Err(Self::open_error(format!(
                "template file is {} bytes; maximum is {MAX_DEBCONF_TEMPLATES_SIZE}",
                metadata.len()
            )));
        }

        let mut body = Vec::with_capacity(metadata.len() as usize);
        file.by_ref()
            .take(MAX_DEBCONF_TEMPLATES_SIZE + 1)
            .read_to_end(&mut body)
            .map_err(|error| Self::open_error(format!("cannot read template file: {error}")))?;
        if body.len() as u64 > MAX_DEBCONF_TEMPLATES_SIZE {
            return Err(Self::open_error(format!(
                "template file grew beyond the {MAX_DEBCONF_TEMPLATES_SIZE}-byte maximum"
            )));
        }
        parse(&body)
            .map(|metadata| metadata.records)
            .map_err(|error| DebconfTemplateLoadError::parse(error.to_string().into_bytes()))
    }
}

fn confmodule_shim() -> Vec<u8> {
    let mut shim = String::from("#!/bin/sh\n");
    shim.push_str(lifecycle_bridge_shell_library());
    shim.push_str(CONFMODULE_SURFACE);
    shim.into_bytes()
}

const CONFMODULE_SURFACE: &str = r#"
DEBIAN_HAS_FRONTEND=1
export DEBIAN_HAS_FRONTEND

if [ -z "${DEBCONF_REDIR:-}" ]; then
    exec 3>&1
    exec 1>&2
    DEBCONF_REDIR=1
    export DEBCONF_REDIR
fi

DEBCONF_OLD_FD_BASE=4
export DEBCONF_OLD_FD_BASE

_db_cmd () {
    _conary_bridge_exchange '/usr/share/debconf/confmodule' "$@" || return $?
    RET="$(printf '%b' "$_CONARY_BRIDGE_STDOUT")"
    printf '%b' "$_CONARY_BRIDGE_STDERR" >&2
    return "$_CONARY_BRIDGE_STATUS"
}

db_beginblock () { _db_cmd "BEGINBLOCK" "$@"; }
db_capb () { _db_cmd "CAPB" "$@"; }
db_clear () { _db_cmd "CLEAR" "$@"; }
db_data () { _db_cmd "DATA" "$@"; }
db_endblock () { _db_cmd "ENDBLOCK" "$@"; }
db_fget () { _db_cmd "FGET" "$@"; }
db_fset () { _db_cmd "FSET" "$@"; }
db_get () { _db_cmd "GET" "$@"; }
db_go () { _db_cmd "GO" "$@"; }
db_info () { _db_cmd "INFO" "$@"; }
db_input () { _db_cmd "INPUT" "$@"; }
db_metaget () { _db_cmd "METAGET" "$@"; }
db_progress () { _db_cmd "PROGRESS" "$@"; }
db_purge () { _db_cmd "PURGE" "$@"; }
db_register () { _db_cmd "REGISTER" "$@"; }
db_reset () { _db_cmd "RESET" "$@"; }
db_set () { _db_cmd "SET" "$@"; }
db_settitle () { _db_cmd "SETTITLE" "$@"; }
db_subst () { _db_cmd "SUBST" "$@"; }
db_title () { _db_cmd "TITLE" "$@"; }
db_unregister () { _db_cmd "UNREGISTER" "$@"; }
db_version () { _db_cmd "VERSION" "$@"; }
db_x_loadtemplatefile () { _db_cmd "X_LOADTEMPLATEFILE" "$@"; }
db_text () { db_input "$@"; }
db_stop () { _db_cmd "STOP"; }
"#;

#[cfg(test)]
#[path = "lifecycle_bridge/tests.rs"]
mod tests;
