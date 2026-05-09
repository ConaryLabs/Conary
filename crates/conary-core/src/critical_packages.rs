// conary-core/src/critical_packages.rs

//! Critical package and runtime capability policy shared by Conary apps.

/// Packages that must never be overlaid, converted for live takeover, or taken
/// over by Conary.
pub const CRITICAL_PACKAGES: &[&str] = &[
    // Core C runtime
    "glibc",
    "glibc-common",
    "glibc-minimal-langpack",
    "glibc-all-langpacks",
    "glibc-devel",
    "libc6",
    "libc6-dev",
    "libc-bin",
    "gcc-libs",
    // Dynamic linker
    "ld-linux",
    "binutils",
    // Init system
    "systemd",
    "systemd-libs",
    "systemd-udev",
    "systemd-resolved",
    "udev",
    "libsystemd0",
    // Authentication
    "pam",
    "linux-pam",
    "libpam-modules",
    "libpam-runtime",
    "shadow-utils",
    // Core utilities
    "bash",
    "filesystem",
    "setup",
    "util-linux",
    "util-linux-core",
    "coreutils",
    // Crypto libraries
    "openssl-libs",
    "openssl",
    "libssl3",
    "libssl3t64",
    "libssl1.1",
    "libcrypto",
    // Kernel interface
    "linux-api-headers",
    "kernel-headers",
    "linux-libc-dev",
    // Privilege escalation
    "sudo",
    "polkit",
    "polkit-libs",
    // NSS/DNS
    "nss-softokn",
    "nspr",
    "ca-certificates",
];

/// Runtime capabilities that are expected to be satisfied by the live system
/// and should never trigger a converted replacement.
pub const CRITICAL_RUNTIME_CAPABILITY_PREFIXES: &[&str] = &[
    "libc.so.6",
    "ld-linux",
    "rtld(",
    "libcrypto.so.",
    "libssl.so.",
    "libgcc_s.so.",
    "libpam.so.",
    "libudev.so.",
    "libpcre2-8.so.",
    "libm.so.6",
    "filesystem(",
    "group(",
];

/// Check if a package name is on the critical package-name blocklist.
#[must_use]
pub fn is_critical_package_name(name: &str) -> bool {
    CRITICAL_PACKAGES
        .iter()
        .any(|package| package.eq_ignore_ascii_case(name))
}

/// Check if a dependency or provide string is a live runtime capability for a
/// critical package such as glibc, the dynamic linker, or base account files.
#[must_use]
pub fn is_critical_runtime_capability(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    CRITICAL_RUNTIME_CAPABILITY_PREFIXES
        .iter()
        .any(|prefix| lower.starts_with(prefix))
}

/// Check if a package/dependency/provide name must be treated as live-system
/// critical.
#[must_use]
pub fn is_blocked(name: &str) -> bool {
    is_critical_package_name(name) || is_critical_runtime_capability(name)
}

/// Return the critical package-name blocklist for display and reporting.
#[must_use]
pub fn blocked_packages() -> &'static [&'static str] {
    CRITICAL_PACKAGES
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn critical_package_names_are_blocked() {
        for name in ["glibc", "bash", "filesystem", "setup", "udev"] {
            assert!(is_critical_package_name(name), "{name} should be critical");
            assert!(is_blocked(name), "{name} should be blocked");
        }
    }

    #[test]
    fn critical_package_names_are_case_insensitive() {
        assert!(is_critical_package_name("GLIBC"));
        assert!(is_blocked("SystemD"));
    }

    #[test]
    fn critical_runtime_capabilities_are_blocked() {
        for name in [
            "libc.so.6()(64bit)",
            "ld-linux-x86-64.so.2()(64bit)",
            "libssl.so.3()(64bit)",
            "libudev.so.1()(64bit)",
            "group(mail)",
        ] {
            assert!(
                is_critical_runtime_capability(name),
                "{name} should be a critical runtime capability"
            );
            assert!(is_blocked(name), "{name} should be blocked");
        }
    }

    #[test]
    fn normal_packages_are_not_blocked() {
        for name in ["nginx", "curl", "vim"] {
            assert!(
                !is_critical_package_name(name),
                "{name} should not be critical"
            );
            assert!(!is_blocked(name), "{name} should not be blocked");
        }
    }

    #[test]
    fn blocked_package_list_is_exposed() {
        assert!(blocked_packages().contains(&"glibc"));
        assert!(blocked_packages().contains(&"bash"));
    }
}
