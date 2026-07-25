// apps/conary/src/commands/install/native_events/debian_runtime/lifecycle_bridge.rs

//! Debian endpoint staging for Conary's private lifecycle transport.
//!
//! The shims in this module contain protocol plumbing and the documented
//! confmodule shell surface only. Typed argv grammars and state mutations live
//! in their owning handlers.

use conary_core::scriptlet::{
    LifecycleBridgeConfig, LifecycleBridgeEndpoint, LifecycleBridgeHandler,
    LifecycleBridgeHandlerError, LifecycleBridgeRequest, LifecycleBridgeResponse,
    executable_bridge_shim, lifecycle_bridge_shell_library,
};

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

pub(super) fn config() -> LifecycleBridgeConfig {
    let mut endpoints = EXECUTABLE_ENDPOINTS
        .iter()
        .map(|path| LifecycleBridgeEndpoint::new(path, executable_bridge_shim()))
        .collect::<Vec<_>>();
    endpoints.push(LifecycleBridgeEndpoint::new(
        CONFMODULE_PATH,
        confmodule_shim(),
    ));
    LifecycleBridgeConfig::new(endpoints, PendingTypedDebianHandler)
}

struct PendingTypedDebianHandler;

impl LifecycleBridgeHandler for PendingTypedDebianHandler {
    fn handle(
        &self,
        _request: &LifecycleBridgeRequest,
    ) -> std::result::Result<LifecycleBridgeResponse, LifecycleBridgeHandlerError> {
        Ok(LifecycleBridgeResponse::new(
            Vec::new(),
            b"Conary has no typed Debian lifecycle handler for this command\n".to_vec(),
            127,
        ))
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
mod tests {
    use super::*;

    #[test]
    fn every_debian_endpoint_is_conary_owned() {
        let config = config();
        let targets = config
            .endpoints()
            .iter()
            .map(|endpoint| endpoint.target().to_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            targets,
            [
                "/usr/bin/dpkg-maintscript-helper",
                "/usr/bin/dpkg-query",
                "/usr/bin/dpkg",
                "/usr/bin/update-alternatives",
                "/usr/bin/deb-systemd-helper",
                "/usr/bin/deb-systemd-invoke",
                "/usr/sbin/invoke-rc.d",
                "/usr/sbin/update-rc.d",
                "/usr/sbin/service",
                "/usr/share/debconf/confmodule",
            ]
        );
        assert!(confmodule_shim().starts_with(b"#!/bin/sh\n"));
    }

    #[test]
    fn confmodule_never_executes_a_target_frontend() {
        let shim = String::from_utf8(confmodule_shim()).unwrap();
        assert!(!shim.contains("/usr/share/debconf/frontend"));
        assert!(!shim.contains("/usr/lib/cdebconf"));
        assert!(shim.contains("DEBIAN_HAS_FRONTEND=1"));
        assert!(shim.contains("exec 3>&1"));
    }

    #[test]
    fn confmodule_is_valid_posix_shell_syntax() {
        use std::io::Write as _;
        use std::process::{Command, Stdio};

        let mut child = Command::new("/bin/sh")
            .args(["-n", "-s"])
            .stdin(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(&confmodule_shim())
            .unwrap();
        assert!(child.wait().unwrap().success());
    }
}
