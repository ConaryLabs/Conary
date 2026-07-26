// conary-core/src/scriptlet/types.rs

/// Package format types for argument handling
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageFormat {
    Conary,
    Rpm,
    Deb,
    Arch,
}

/// Execution mode determines arguments passed to scriptlets
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecutionMode {
    /// Fresh install
    Install,
    /// Package removal
    Remove,
    /// Upgrade from old version (for NEW package scriptlets)
    Upgrade { old_version: String },
    /// Upgrade removal (for OLD package scriptlets during upgrade)
    /// RPM: $1=1 (not 0, signaling "another version remains")
    /// DEB: "upgrade <new_version>" (not "remove")
    /// Arch: Should NOT be used - Arch skips old package scripts during upgrade
    UpgradeRemoval { new_version: String },
}
