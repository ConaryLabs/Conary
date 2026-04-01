// src/commands/install/blocklist.rs
//! Critical system package blocklist
//!
//! Packages on this list cannot be overlaid or taken over by Conary,
//! regardless of `--dep-mode`. These are packages where CCS conversion
//! fidelity loss can render the system unbootable.

/// Packages that must never be overlaid or taken over.
///
/// In `satisfy` and `adopt` modes, these are always treated as satisfied
/// by the system. In `takeover` mode, Conary refuses to take ownership.
const CRITICAL_PACKAGES: &[&str] = &[
    // Core C runtime — replacing these can brick the system
    "glibc",
    "glibc-common",
    "glibc-minimal-langpack",
    "glibc-all-langpacks",
    "glibc-devel",
    "libc6",     // Debian/Ubuntu
    "libc6-dev", // Debian/Ubuntu
    "libc-bin",  // Debian/Ubuntu
    "gcc-libs",  // Arch
    // Dynamic linker
    "ld-linux",
    "binutils",
    // Init system
    "systemd",
    "systemd-libs",
    "systemd-udev",
    "systemd-resolved",
    "libsystemd0", // Debian/Ubuntu
    // Authentication
    "pam",
    "linux-pam",      // Arch
    "libpam-modules", // Debian/Ubuntu
    "libpam-runtime", // Debian/Ubuntu
    "shadow-utils",
    // Core utilities (running system depends on these)
    "util-linux",
    "util-linux-core",
    "coreutils",
    // Crypto libraries (in-use by running processes including conary itself)
    "openssl-libs",
    "openssl",
    "libssl3",    // Debian/Ubuntu
    "libssl3t64", // Debian/Ubuntu (time64)
    "libssl1.1",  // Older Debian
    "libcrypto",
    // Kernel interface
    "linux-api-headers", // Arch
    "kernel-headers",    // Fedora
    "linux-libc-dev",    // Debian/Ubuntu
    // Privilege escalation
    "sudo",
    "polkit",
    "polkit-libs",
    // NSS/DNS (system DNS resolution)
    "nss-softokn",
    "nspr",
    "ca-certificates",
];

/// Check if a package name is on the critical blocklist.
///
/// Blocked packages cannot be overlaid or taken over by Conary.
/// They are always treated as satisfied by the system package manager.
#[must_use]
pub fn is_blocked(name: &str) -> bool {
    CRITICAL_PACKAGES
        .iter()
        .any(|p| p.eq_ignore_ascii_case(name))
}

/// Return the full blocklist for display purposes.
#[must_use]
#[allow(dead_code)]
pub fn blocked_packages() -> &'static [&'static str] {
    CRITICAL_PACKAGES
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
}
