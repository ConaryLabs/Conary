// conary-core/src/ccs/native_lifecycle/types.rs
//! Closed enums shared by native lifecycle contracts.

use serde::{Deserialize, Serialize};

macro_rules! string_enum {
    (
        $(#[$meta:meta])*
        pub enum $name:ident {
            $($variant:ident => $value:literal),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
        pub enum $name {
            $(
                #[serde(rename = $value)]
                $variant,
            )+
        }

        impl $name {
            pub fn as_str(&self) -> &str {
                match self {
                    $(Self::$variant => $value,)+
                }
            }
        }
    };
}

string_enum! {
    pub enum SourceFormat {
        Rpm => "rpm",
        Deb => "deb",
        Arch => "arch",
    }
}

string_enum! {
    pub enum VersionScheme {
        Rpm => "rpm",
        Deb => "deb",
        Arch => "arch",
        Semver => "semver",
    }
}

impl VersionScheme {
    /// Project lifecycle vocabulary into the package/repository version
    /// authority without comparing their deliberately different wire names.
    #[must_use]
    pub const fn repository_scheme(self) -> crate::repository::versioning::VersionScheme {
        match self {
            Self::Rpm => crate::repository::versioning::VersionScheme::Rpm,
            Self::Deb => crate::repository::versioning::VersionScheme::Debian,
            Self::Arch => crate::repository::versioning::VersionScheme::Arch,
            Self::Semver => crate::repository::versioning::VersionScheme::Conary,
        }
    }
}

string_enum! {
    pub enum ScriptletFidelity {
        NativeFree => "native-free",
        NativeLifecycle => "native-lifecycle",
    }
}

string_enum! {
    pub enum LifecyclePath {
        PackageControl => "package-control",
        PreInstall => "pre-install",
        RpmSysusers => "rpm-sysusers",
        PostInstall => "post-install",
        PreUpgrade => "pre-upgrade",
        PostUpgrade => "post-upgrade",
        PreRemove => "pre-remove",
        PostRemove => "post-remove",
        PreTransaction => "pre-transaction",
        PostTransaction => "post-transaction",
        PreUntransaction => "pre-untransaction",
        PostUntransaction => "post-untransaction",
        Verify => "verify",
        Trigger => "trigger",
        FileTrigger => "file-trigger",
    }
}

string_enum! {
    pub enum DebMaintainerMode {
        Install => "install",
        Configure => "configure",
        Reconfigure => "reconfigure",
        Upgrade => "upgrade",
        Remove => "remove",
        Purge => "purge",
        Triggered => "triggered",
        Disappear => "disappear",
        Deconfigure => "deconfigure",
        FailedUpgrade => "failed-upgrade",
        AbortInstall => "abort-install",
        AbortUpgrade => "abort-upgrade",
        AbortRemove => "abort-remove",
        AbortDeconfigure => "abort-deconfigure",
    }
}

string_enum! {
    pub enum DebMaintainerArgumentValue {
        Action => "action",
        OldVersion => "old-version",
        NewVersion => "new-version",
        PackageInstanceCount => "package-instance-count",
        PackageName => "package-name",
        InstallingPackageName => "installing-package-name",
        InstallingPackageVersion => "installing-package-version",
        ConflictingPackageMarker => "conflicting-package-marker",
        ConflictingPackageName => "conflicting-package-name",
        ConflictingPackageVersion => "conflicting-package-version",
        TriggerName => "trigger-name",
        TriggerNames => "trigger-names",
        TriggerCount => "trigger-count",
        FilePath => "file-path",
        InstalledVersion => "installed-version",
        MostRecentlyConfiguredVersion => "most-recently-configured-version",
        Literal => "literal",
    }
}

#[cfg(test)]
mod tests {
    use super::VersionScheme;
    use crate::repository::versioning::VersionScheme as RepositoryVersionScheme;

    #[test]
    fn lifecycle_version_schemes_project_to_typed_repository_authority() {
        assert_eq!(
            VersionScheme::Rpm.repository_scheme(),
            RepositoryVersionScheme::Rpm
        );
        assert_eq!(
            VersionScheme::Deb.repository_scheme(),
            RepositoryVersionScheme::Debian
        );
        assert_eq!(
            VersionScheme::Arch.repository_scheme(),
            RepositoryVersionScheme::Arch
        );
        assert_eq!(
            VersionScheme::Semver.repository_scheme(),
            RepositoryVersionScheme::Conary
        );
    }
}
