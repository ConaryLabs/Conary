// conary-core/src/scriptlet/types.rs

/// Package format types for argument handling
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageFormat {
    Rpm,
    Deb,
    Arch,
}

impl PackageFormat {
    /// Parse from string representation
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "rpm" => Some(Self::Rpm),
            "deb" => Some(Self::Deb),
            "arch" => Some(Self::Arch),
            _ => None,
        }
    }

    /// Convert to string representation
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Rpm => "rpm",
            Self::Deb => "deb",
            Self::Arch => "arch",
        }
    }
}

/// Execution mode determines arguments passed to scriptlets
#[derive(Debug, Clone)]
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

#[cfg(test)]
mod tests {
    use super::PackageFormat;

    #[test]
    fn test_package_format_from_str() {
        assert_eq!(PackageFormat::parse("rpm"), Some(PackageFormat::Rpm));
        assert_eq!(PackageFormat::parse("deb"), Some(PackageFormat::Deb));
        assert_eq!(PackageFormat::parse("arch"), Some(PackageFormat::Arch));
        assert_eq!(PackageFormat::parse("unknown"), None);
    }
}
