// apps/conary/src/commands/install/blocklist.rs
//! Critical system package blocklist
//!
//! Packages on this list cannot be overlaid or taken over by Conary,
//! regardless of `--dep-mode`. These are packages where CCS conversion
//! fidelity loss can render the system unbootable.

/// Check if a dependency string is a live runtime capability for a blocked
/// system package such as glibc or the dynamic linker.
#[must_use]
pub fn is_critical_runtime_capability(name: &str) -> bool {
    conary_core::critical_packages::is_critical_runtime_capability(name)
}

/// Check if a package name is on the critical blocklist.
///
/// Blocked packages cannot be overlaid or taken over by Conary.
/// They are always treated as satisfied by the system package manager.
#[must_use]
pub fn is_blocked(name: &str) -> bool {
    conary_core::critical_packages::is_blocked(name)
}

/// Return the full blocklist for display purposes.
#[must_use]
#[allow(dead_code)]
pub fn blocked_packages() -> &'static [&'static str] {
    conary_core::critical_packages::blocked_packages()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_known_critical_packages_blocked() {
        assert!(is_blocked("glibc"));
        assert!(is_blocked("systemd"));
        assert!(is_blocked("openssl-libs"));
        assert!(is_blocked("sudo"));
        assert!(is_blocked("pam"));
        assert!(is_blocked("libc6"));
        assert!(is_blocked("linux-pam"));
        assert!(is_blocked("bash"));
        assert!(is_blocked("filesystem"));
        assert!(is_blocked("systemd-udev"));
        assert!(is_blocked("udev"));
    }

    #[test]
    fn test_normal_packages_not_blocked() {
        assert!(!is_blocked("nginx"));
        assert!(!is_blocked("tree"));
        assert!(!is_blocked("curl"));
        assert!(!is_blocked("jq"));
        assert!(!is_blocked("which"));
        assert!(!is_blocked("vim"));
    }

    #[test]
    fn test_blocklist_not_empty() {
        assert!(!blocked_packages().is_empty());
    }

    #[test]
    fn test_glibc_runtime_capabilities_are_blocked() {
        assert!(is_blocked("libc.so.6()(64bit)"));
        assert!(is_blocked("libc.so.6(GLIBC_2.34)(64bit)"));
        assert!(is_blocked("libcrypto.so.3()(64bit)"));
        assert!(is_blocked("libgcc_s.so.1(GCC_3.0)(64bit)"));
        assert!(is_blocked("libpam.so.0(LIBPAM_1.0)(64bit)"));
        assert!(is_blocked("libpcre2-8.so.0()(64bit)"));
        assert!(is_blocked("libm.so.6(GLIBC_2.2.5)(64bit)"));
        assert!(is_blocked("rtld(GNU_HASH)"));
        assert!(is_critical_runtime_capability(
            "ld-linux-x86-64.so.2()(64bit)"
        ));
    }

    #[test]
    fn test_base_filesystem_and_account_capabilities_are_blocked() {
        assert!(is_blocked("setup"));
        assert!(is_blocked("filesystem(unmerged-sbin-symlinks)"));
        assert!(is_blocked("group(mail)"));
    }
}
