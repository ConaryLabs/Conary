// apps/conary/src/commands/install/native_events/debian_runtime.rs

//! Exact dpkg maintainer-script process environment.

use super::NativeBundleOwner;
use anyhow::{Context, Result, bail};
use conary_core::ccs::native_lifecycle::{DebControlMember, NativeLifecycleEntry, SourceFormat};

#[path = "debian_runtime/admin_projection.rs"]
mod admin_projection;
#[path = "debian_runtime/alternatives_state.rs"]
mod alternatives_state;
#[path = "debian_runtime/lifecycle_bridge.rs"]
mod lifecycle_bridge;
#[path = "debian_runtime/trigger_mutations.rs"]
mod trigger_mutations;

pub(super) use admin_projection::{DebianAdminSnapshot, DebianConfigSnapshot, config_snapshots};
pub(super) use lifecycle_bridge::DebianLifecycleBridge;

/// The upstream dpkg ABI version whose maintainer-script environment Conary
/// implements. Bumping this requires rechecking dpkg's `script.c` contract.
const DPKG_MAINTSCRIPT_ABI_VERSION: &str = "1.23.7";
const DPKG_ADMIN_COMPAT_DIRECTORY: &str = "/var/lib/conary/dpkg-compat";

#[derive(Clone, Copy, PartialEq, Eq)]
enum DpkgRuntimeEnvironmentKey {
    Root,
    AdminDirectory,
    Force,
    Package,
    PackageRefcount,
    Architecture,
    ScriptName,
    Debug,
    RunningVersion,
}

impl DpkgRuntimeEnvironmentKey {
    fn parse(key: &str) -> Option<Self> {
        Some(match key {
            "DPKG_ROOT" => Self::Root,
            "DPKG_ADMINDIR" => Self::AdminDirectory,
            "DPKG_FORCE" => Self::Force,
            "DPKG_MAINTSCRIPT_PACKAGE" => Self::Package,
            "DPKG_MAINTSCRIPT_PACKAGE_REFCOUNT" => Self::PackageRefcount,
            "DPKG_MAINTSCRIPT_ARCH" => Self::Architecture,
            "DPKG_MAINTSCRIPT_NAME" => Self::ScriptName,
            "DPKG_MAINTSCRIPT_DEBUG" => Self::Debug,
            "DPKG_RUNNING_VERSION" => Self::RunningVersion,
            _ => return None,
        })
    }
}

pub(super) struct DebianRuntimeContext {
    pub(super) environment: Vec<String>,
    pub(super) package_refcount: u32,
}

pub(super) fn for_entry(
    owner: &NativeBundleOwner,
    entry: &NativeLifecycleEntry,
    package_refcount: Option<u32>,
) -> Result<Option<DebianRuntimeContext>> {
    if owner.bundle.source_format != SourceFormat::Deb {
        return Ok(None);
    }

    let metadata = entry.deb_maintainer.as_ref().with_context(|| {
        format!(
            "Debian lifecycle entry '{}' has no typed maintainer-script metadata",
            entry.id
        )
    })?;
    let script_name = maintainer_script_name(metadata.control_member)?;
    let architecture = owner
        .bundle
        .source_arch
        .as_deref()
        .filter(|architecture| !architecture.is_empty())
        .with_context(|| {
            format!(
                "Debian lifecycle bundle for '{}' has no source architecture",
                owner.package_name
            )
        })?;
    let package_refcount = package_refcount
        .filter(|count| *count > 0)
        .with_context(|| {
            format!(
                "Debian lifecycle entry '{}' has no positive event-time package refcount",
                entry.id
            )
        })?;
    let environment = maintainer_environment(
        &entry.native_invocation.environment,
        &owner.package_name,
        architecture,
        script_name,
        package_refcount,
    )?;

    Ok(Some(DebianRuntimeContext {
        environment,
        package_refcount,
    }))
}

fn maintainer_script_name(control_member: DebControlMember) -> Result<&'static str> {
    match control_member {
        DebControlMember::Preinst => Ok("preinst"),
        DebControlMember::Postinst => Ok("postinst"),
        DebControlMember::Prerm => Ok("prerm"),
        DebControlMember::Postrm => Ok("postrm"),
        DebControlMember::Config => Ok("config"),
        DebControlMember::Triggers => {
            bail!("DEBIAN/triggers is a control artifact and cannot be executed")
        }
    }
}

fn maintainer_environment(
    persisted: &[String],
    package_name: &str,
    architecture: &str,
    script_name: &str,
    package_refcount: u32,
) -> Result<Vec<String>> {
    validate_environment_value("package name", package_name)?;
    validate_environment_value("architecture", architecture)?;
    validate_environment_value("maintainer script name", script_name)?;

    let mut environment = Vec::with_capacity(persisted.len() + 9);
    for assignment in persisted {
        let key = assignment
            .split_once('=')
            .map_or(assignment.as_str(), |(key, _)| key);
        match DpkgRuntimeEnvironmentKey::parse(key) {
            Some(_) => {
                // The runtime values below are transaction state, not
                // package-authored metadata.
            }
            None => environment.push(assignment.clone()),
        }
    }
    environment.extend([
        // dpkg leaves DPKG_ROOT empty for both the live root and its normal
        // chrooted target-root execution mode.
        "DPKG_ROOT=".to_string(),
        // Never allow dpkg-query or maintscript helpers to fall back to a
        // source package manager's host database. This path is reserved for
        // Conary's authoritative compatibility projection.
        format!("DPKG_ADMINDIR={DPKG_ADMIN_COMPAT_DIRECTORY}"),
        // Conary does not expose dpkg force switches. An explicitly empty
        // value is dpkg's documented contract for disabling every force flag.
        "DPKG_FORCE=".to_string(),
        format!("DPKG_MAINTSCRIPT_PACKAGE={package_name}"),
        format!("DPKG_MAINTSCRIPT_PACKAGE_REFCOUNT={package_refcount}"),
        format!("DPKG_MAINTSCRIPT_ARCH={architecture}"),
        format!("DPKG_MAINTSCRIPT_NAME={script_name}"),
        "DPKG_MAINTSCRIPT_DEBUG=0".to_string(),
        format!("DPKG_RUNNING_VERSION={DPKG_MAINTSCRIPT_ABI_VERSION}"),
    ]);
    Ok(environment)
}

fn validate_environment_value(label: &str, value: &str) -> Result<()> {
    if value.contains('\0') {
        bail!("Debian maintainer-script {label} contains NUL");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn environment_matches_dpkg_script_process_contract() {
        let environment = maintainer_environment(
            &[
                "PACKAGE_DEFINED=value".to_string(),
                "DPKG_MAINTSCRIPT_PACKAGE=wrong".to_string(),
            ],
            "demo",
            "amd64",
            "postinst",
            2,
        )
        .unwrap();

        assert_eq!(
            environment,
            vec![
                "PACKAGE_DEFINED=value",
                "DPKG_ROOT=",
                "DPKG_ADMINDIR=/var/lib/conary/dpkg-compat",
                "DPKG_FORCE=",
                "DPKG_MAINTSCRIPT_PACKAGE=demo",
                "DPKG_MAINTSCRIPT_PACKAGE_REFCOUNT=2",
                "DPKG_MAINTSCRIPT_ARCH=amd64",
                "DPKG_MAINTSCRIPT_NAME=postinst",
                "DPKG_MAINTSCRIPT_DEBUG=0",
                "DPKG_RUNNING_VERSION=1.23.7",
            ]
        );
    }

    #[test]
    fn config_has_its_exact_debconf_script_identity() {
        assert_eq!(
            maintainer_script_name(DebControlMember::Config).unwrap(),
            "config"
        );
        assert!(maintainer_script_name(DebControlMember::Triggers).is_err());
    }

    #[test]
    fn package_metadata_cannot_redirect_helpers_to_a_host_dpkg_database() {
        let environment = maintainer_environment(
            &["DPKG_ADMINDIR=/var/lib/dpkg".to_string()],
            "demo",
            "amd64",
            "postinst",
            1,
        )
        .unwrap();
        assert!(
            environment
                .iter()
                .any(|assignment| assignment == "DPKG_ADMINDIR=/var/lib/conary/dpkg-compat")
        );
        assert!(
            environment
                .iter()
                .all(|assignment| assignment != "DPKG_ADMINDIR=/var/lib/dpkg")
        );
    }
}
