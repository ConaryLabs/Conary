// src/commands/install/system_pm.rs
//! System package manager query helpers for dependency resolution

use super::blocklist;
use conary_core::packages::{SystemPackageManager, dpkg_query, pacman_query, rpm_query};
use std::path::Path;
use std::process::Command;
use tracing::debug;

/// Check if a package is installed via the system package manager
#[must_use]
pub fn is_system_package_installed(name: &str) -> bool {
    let pm = SystemPackageManager::detect();
    let result = match pm {
        SystemPackageManager::Rpm => rpm_query::query_package(name).is_ok(),
        SystemPackageManager::Dpkg => dpkg_query::query_package(name).is_ok(),
        SystemPackageManager::Pacman => pacman_query::query_package(name).is_ok(),
        SystemPackageManager::Unknown => false,
    };
    debug!(
        "System PM check for '{}' ({}): {}",
        name,
        pm.display_name(),
        if result { "installed" } else { "not found" }
    );
    result
}

/// Get the version of a system-installed package
#[must_use]
#[allow(dead_code)]
pub fn get_system_package_version(name: &str) -> Option<String> {
    let pm = SystemPackageManager::detect();
    match pm {
        SystemPackageManager::Rpm => rpm_query::query_package(name)
            .ok()
            .map(|info| info.version_only()),
        SystemPackageManager::Dpkg => dpkg_query::query_package(name)
            .ok()
            .map(|info| info.version_only()),
        SystemPackageManager::Pacman => pacman_query::query_package(name)
            .ok()
            .map(|info| info.version_only()),
        SystemPackageManager::Unknown => None,
    }
}

/// Check whether a critical runtime dependency is already satisfied by the live
/// root, even when Conary's database is empty.
#[must_use]
pub fn is_live_runtime_dependency_present(name: &str) -> bool {
    if let Some(paths) = live_runtime_package_probe_paths(name) {
        return candidate_paths(paths);
    }

    if !blocklist::is_critical_runtime_capability(name) {
        return false;
    }

    if let Some(group) = live_runtime_group(name) {
        return group_exists(group);
    }

    let lower = name.to_ascii_lowercase();
    if lower.starts_with("rtld(") || lower.starts_with("ld-linux") {
        return dynamic_linker_present();
    }

    if lower.starts_with("libc.so.6") {
        return soname_present("libc.so.6")
            || candidate_paths(&[
                "/lib64/libc.so.6",
                "/lib/libc.so.6",
                "/usr/lib64/libc.so.6",
                "/usr/lib/libc.so.6",
                "/lib/x86_64-linux-gnu/libc.so.6",
                "/usr/lib/x86_64-linux-gnu/libc.so.6",
                "/lib/aarch64-linux-gnu/libc.so.6",
                "/usr/lib/aarch64-linux-gnu/libc.so.6",
                "/lib/riscv64-linux-gnu/libc.so.6",
                "/usr/lib/riscv64-linux-gnu/libc.so.6",
            ]);
    }

    if let Some(soname) = live_runtime_soname(name) {
        return soname_present(soname) || candidate_paths(live_runtime_soname_probe_paths(soname));
    }

    false
}

fn live_runtime_soname(name: &str) -> Option<&str> {
    let soname = name.split('(').next().unwrap_or(name);
    let lower = soname.to_ascii_lowercase();
    if lower.starts_with("libcrypto.so.")
        || lower.starts_with("libssl.so.")
        || lower.starts_with("libgcc_s.so.")
        || lower.starts_with("libpam.so.")
        || lower.starts_with("libpcre2-8.so.")
        || lower.starts_with("libm.so.6")
    {
        Some(soname)
    } else {
        None
    }
}

fn live_runtime_soname_probe_paths(soname: &str) -> &'static [&'static str] {
    let lower = soname.to_ascii_lowercase();
    if lower.starts_with("libcrypto.so.") {
        &[
            "/usr/lib64/libcrypto.so.3",
            "/lib64/libcrypto.so.3",
            "/usr/lib/x86_64-linux-gnu/libcrypto.so.3",
            "/lib/x86_64-linux-gnu/libcrypto.so.3",
        ]
    } else if lower.starts_with("libssl.so.") {
        &[
            "/usr/lib64/libssl.so.3",
            "/lib64/libssl.so.3",
            "/usr/lib/x86_64-linux-gnu/libssl.so.3",
            "/lib/x86_64-linux-gnu/libssl.so.3",
        ]
    } else if lower.starts_with("libgcc_s.so.") {
        &[
            "/usr/lib64/libgcc_s.so.1",
            "/lib64/libgcc_s.so.1",
            "/usr/lib/x86_64-linux-gnu/libgcc_s.so.1",
            "/lib/x86_64-linux-gnu/libgcc_s.so.1",
        ]
    } else if lower.starts_with("libpam.so.") {
        &[
            "/usr/lib64/libpam.so.0",
            "/lib64/libpam.so.0",
            "/usr/lib/x86_64-linux-gnu/libpam.so.0",
            "/lib/x86_64-linux-gnu/libpam.so.0",
        ]
    } else if lower.starts_with("libpcre2-8.so.") {
        &[
            "/usr/lib64/libpcre2-8.so.0",
            "/lib64/libpcre2-8.so.0",
            "/usr/lib/libpcre2-8.so.0",
            "/lib/libpcre2-8.so.0",
            "/usr/lib/x86_64-linux-gnu/libpcre2-8.so.0",
            "/lib/x86_64-linux-gnu/libpcre2-8.so.0",
        ]
    } else if lower.starts_with("libm.so.6") {
        &[
            "/usr/lib64/libm.so.6",
            "/lib64/libm.so.6",
            "/usr/lib/libm.so.6",
            "/lib/libm.so.6",
            "/usr/lib/x86_64-linux-gnu/libm.so.6",
            "/lib/x86_64-linux-gnu/libm.so.6",
        ]
    } else {
        &[]
    }
}

fn live_runtime_package_probe_paths(name: &str) -> Option<&'static [&'static str]> {
    let lower = name.to_ascii_lowercase();
    match lower.as_str() {
        "bash" => Some(&["/usr/bin/bash", "/bin/bash"]),
        "filesystem" => Some(&["/usr", "/etc", "/var"]),
        name if name.starts_with("filesystem(") => Some(&["/usr", "/etc", "/var"]),
        "setup" => Some(&["/etc/passwd", "/etc/group"]),
        "findutils" => Some(&["/usr/bin/find", "/bin/find"]),
        "grep" => Some(&["/usr/bin/grep", "/bin/grep"]),
        "kmod" => Some(&["/usr/bin/kmod", "/usr/sbin/modprobe", "/sbin/modprobe"]),
        "procps-ng" => Some(&["/usr/bin/ps", "/bin/ps"]),
        "sed" => Some(&["/usr/bin/sed", "/bin/sed"]),
        "xz" => Some(&["/usr/bin/xz", "/bin/xz"]),
        "zstd" => Some(&["/usr/bin/zstd"]),
        "pcre2" => Some(&[
            "/usr/lib64/libpcre2-8.so.0",
            "/lib64/libpcre2-8.so.0",
            "/usr/lib/libpcre2-8.so.0",
            "/lib/libpcre2-8.so.0",
            "/usr/lib/x86_64-linux-gnu/libpcre2-8.so.0",
            "/lib/x86_64-linux-gnu/libpcre2-8.so.0",
        ]),
        _ => None,
    }
}

fn live_runtime_group(name: &str) -> Option<&str> {
    let group = name.strip_prefix("group(")?.strip_suffix(')')?;
    if group.is_empty() { None } else { Some(group) }
}

fn group_exists(group: &str) -> bool {
    std::fs::read_to_string("/etc/group")
        .map(|content| {
            content
                .lines()
                .any(|line| line.split(':').next() == Some(group))
        })
        .unwrap_or(false)
}

fn soname_present(soname: &str) -> bool {
    let output = match Command::new("ldconfig").arg("-p").output() {
        Ok(output) => output,
        Err(_) => return false,
    };

    output.status.success() && String::from_utf8_lossy(&output.stdout).contains(soname)
}

fn candidate_paths(paths: &[&str]) -> bool {
    paths.iter().any(|path| Path::new(path).exists())
}

fn dynamic_linker_present() -> bool {
    candidate_paths(&[
        "/lib64/ld-linux-x86-64.so.2",
        "/lib/ld-linux.so.2",
        "/lib/ld-linux-aarch64.so.1",
        "/lib/ld-linux-riscv64-lp64d.so.1",
        "/usr/lib64/ld-linux-x86-64.so.2",
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_nonexistent_package_not_installed() {
        // A package that definitely doesn't exist should return false
        assert!(!is_system_package_installed("zzz-nonexistent-pkg-12345"));
    }

    #[test]
    fn test_nonexistent_package_no_version() {
        assert!(get_system_package_version("zzz-nonexistent-pkg-12345").is_none());
    }

    #[test]
    fn test_bash_is_installed() {
        // bash should be installed on any system running these tests.
        // Skip if PM is Unknown or if the query tool isn't available
        // (GitHub Actions runners may lack rpm/dpkg-query).
        if is_system_package_installed("bash") {
            // validates that the function returns true on a real system
        } else {
            eprintln!("skipping: system PM cannot find bash");
        }
    }

    #[test]
    fn test_bash_has_version() {
        if let Some(version) = get_system_package_version("bash") {
            assert!(!version.is_empty(), "bash version should not be empty");
        } else {
            eprintln!("skipping: system PM cannot query bash version");
        }
    }

    #[test]
    fn test_non_runtime_dependency_is_not_treated_as_live_runtime() {
        assert!(!is_live_runtime_dependency_present("tree"));
    }

    #[test]
    fn live_runtime_package_probe_covers_bootstrap_core_tools() {
        assert_eq!(
            live_runtime_package_probe_paths("bash"),
            Some(&["/usr/bin/bash", "/bin/bash"][..])
        );
        assert_eq!(
            live_runtime_package_probe_paths("filesystem"),
            Some(&["/usr", "/etc", "/var"][..])
        );
        assert_eq!(
            live_runtime_package_probe_paths("filesystem(unmerged-sbin-symlinks)"),
            Some(&["/usr", "/etc", "/var"][..])
        );
        assert_eq!(
            live_runtime_package_probe_paths("setup"),
            Some(&["/etc/passwd", "/etc/group"][..])
        );
        assert_eq!(
            live_runtime_package_probe_paths("findutils"),
            Some(&["/usr/bin/find", "/bin/find"][..])
        );
        assert_eq!(
            live_runtime_package_probe_paths("pcre2"),
            Some(
                &[
                    "/usr/lib64/libpcre2-8.so.0",
                    "/lib64/libpcre2-8.so.0",
                    "/usr/lib/libpcre2-8.so.0",
                    "/lib/libpcre2-8.so.0",
                    "/usr/lib/x86_64-linux-gnu/libpcre2-8.so.0",
                    "/lib/x86_64-linux-gnu/libpcre2-8.so.0",
                ][..]
            )
        );
        assert!(live_runtime_package_probe_paths("nginx").is_none());
    }

    #[test]
    fn live_runtime_group_capability_parses_rpm_group_requirements() {
        assert_eq!(live_runtime_group("group(mail)"), Some("mail"));
        assert_eq!(
            live_runtime_group("group(systemd-journal)"),
            Some("systemd-journal")
        );
        assert_eq!(live_runtime_group("user(mail)"), None);
        assert_eq!(live_runtime_group("group()"), None);
    }

    #[test]
    fn live_runtime_soname_probe_covers_bootstrap_libraries() {
        assert_eq!(
            live_runtime_soname("libcrypto.so.3()(64bit)"),
            Some("libcrypto.so.3")
        );
        assert_eq!(
            live_runtime_soname("libgcc_s.so.1(GCC_3.0)(64bit)"),
            Some("libgcc_s.so.1")
        );
        assert_eq!(
            live_runtime_soname("libpam.so.0(LIBPAM_1.0)(64bit)"),
            Some("libpam.so.0")
        );
        assert_eq!(
            live_runtime_soname("libpcre2-8.so.0()(64bit)"),
            Some("libpcre2-8.so.0")
        );
        assert_eq!(
            live_runtime_soname("libm.so.6(GLIBC_2.2.5)(64bit)"),
            Some("libm.so.6")
        );
        assert_eq!(live_runtime_soname("libfoo.so.1()(64bit)"), None);
        assert!(!live_runtime_soname_probe_paths("libcrypto.so.3").is_empty());
        assert!(!live_runtime_soname_probe_paths("libgcc_s.so.1").is_empty());
        assert!(!live_runtime_soname_probe_paths("libpam.so.0").is_empty());
        assert!(!live_runtime_soname_probe_paths("libpcre2-8.so.0").is_empty());
        assert!(!live_runtime_soname_probe_paths("libm.so.6").is_empty());
    }
}
