// src/commands/install/semantics.rs

use super::{PackageFormatType, prepare, scriptlets::to_scriptlet_format};
use conary_core::repository::versioning::VersionScheme;
use conary_core::scriptlet::PackageFormat as ScriptletPackageFormat;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PreparedSourceKind {
    Legacy { format: PackageFormatType },
    Ccs,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct InstallSemantics {
    pub(super) source: PreparedSourceKind,
    pub(super) version_scheme: VersionScheme,
    pub(super) scriptlet_format: ScriptletPackageFormat,
}

impl InstallSemantics {
    pub(super) fn legacy(format: PackageFormatType) -> Self {
        Self {
            source: PreparedSourceKind::Legacy { format },
            version_scheme: prepare::version_scheme_for_format(format),
            scriptlet_format: to_scriptlet_format(format),
        }
    }

    pub(super) fn ccs() -> Self {
        Self {
            source: PreparedSourceKind::Ccs,
            // CCS is the native artifact shape, but the current install/rollback
            // metadata still expects a version-scheme and scriptlet-family.
            // Until CCS carries an explicit scheme, keep the existing RPM
            // fallback for mixed-version comparisons and upgrade scriptlets.
            version_scheme: VersionScheme::Rpm,
            scriptlet_format: ScriptletPackageFormat::Rpm,
        }
    }
}

pub(super) fn scheme_to_string(scheme: VersionScheme) -> String {
    match scheme {
        VersionScheme::Rpm => "rpm".to_string(),
        VersionScheme::Debian => "debian".to_string(),
        VersionScheme::Arch => "arch".to_string(),
    }
}
