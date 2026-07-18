// src/commands/install/system_pm.rs
//! System package manager query helpers for dependency resolution

use super::blocklist;
use conary_core::packages::{SystemPackageManager, dpkg_query, pacman_query, rpm_query};
use conary_core::version::VersionConstraint;
use std::fs::File;
use std::io::Read;
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

/// Check whether a runtime dependency is already satisfied by the live root,
/// even when Conary's database is empty.
#[must_use]
pub fn is_live_runtime_dependency_present(name: &str, constraint: &VersionConstraint) -> bool {
    is_live_runtime_dependency_present_with_probes(
        name,
        constraint,
        SystemPackageManager::detect(),
        pacman_dependency_satisfied,
        soname_present,
    )
}

fn is_live_runtime_dependency_present_with_probes<PacmanProbe, SonameProbe>(
    name: &str,
    constraint: &VersionConstraint,
    system_pm: SystemPackageManager,
    pacman_dependency_satisfied: PacmanProbe,
    soname_present: SonameProbe,
) -> bool
where
    PacmanProbe: Fn(&str, &VersionConstraint) -> bool,
    SonameProbe: Fn(&str, Option<ElfClass>) -> bool,
{
    if let Some(paths) = live_runtime_package_probe_paths(name) {
        return candidate_paths(paths);
    }

    if let Some(soname) = live_runtime_soname(name) {
        if system_pm == SystemPackageManager::Pacman {
            if pacman_dependency_satisfied(name, constraint) {
                return true;
            }
            // Arch split SONAME capabilities carry the ABI/version in their
            // constraint (for example `libcap.so=2-64`), so a failed pacman
            // check must not be bypassed by an unversioned filesystem entry.
            // An unconstrained foreign dependency whose name embeds the exact
            // SONAME version can still be proven directly from the live ELF.
            if !matches!(constraint, VersionConstraint::Any)
                || !soname.to_ascii_lowercase().contains(".so.")
            {
                return false;
            }
        }
        if soname_present(soname, required_elf_class(name)) {
            return true;
        }
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
        return candidate_elf_paths(
            &[
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
            ],
            required_elf_class(name),
        );
    }

    if let Some(soname) = live_runtime_soname(name) {
        return candidate_elf_paths(
            live_runtime_soname_probe_paths(soname),
            required_elf_class(name),
        );
    }

    false
}

fn live_runtime_soname(name: &str) -> Option<&str> {
    let soname = name.split('(').next().unwrap_or(name);
    let lower = soname.to_ascii_lowercase();
    if lower.starts_with("lib") && (lower.contains(".so.") || lower.ends_with(".so")) {
        Some(soname)
    } else {
        None
    }
}

fn live_runtime_soname_probe_paths(soname: &str) -> &'static [&'static str] {
    let lower = soname.to_ascii_lowercase();
    if lower == "libcrypto.so.3" {
        &[
            "/usr/lib64/libcrypto.so.3",
            "/lib64/libcrypto.so.3",
            "/usr/lib/x86_64-linux-gnu/libcrypto.so.3",
            "/lib/x86_64-linux-gnu/libcrypto.so.3",
        ]
    } else if lower == "libssl.so.3" {
        &[
            "/usr/lib64/libssl.so.3",
            "/lib64/libssl.so.3",
            "/usr/lib/x86_64-linux-gnu/libssl.so.3",
            "/lib/x86_64-linux-gnu/libssl.so.3",
        ]
    } else if lower == "libgcc_s.so.1" {
        &[
            "/usr/lib64/libgcc_s.so.1",
            "/lib64/libgcc_s.so.1",
            "/usr/lib/x86_64-linux-gnu/libgcc_s.so.1",
            "/lib/x86_64-linux-gnu/libgcc_s.so.1",
        ]
    } else if lower == "libpam.so.0" {
        &[
            "/usr/lib64/libpam.so.0",
            "/lib64/libpam.so.0",
            "/usr/lib/x86_64-linux-gnu/libpam.so.0",
            "/lib/x86_64-linux-gnu/libpam.so.0",
        ]
    } else if lower == "libudev.so.1" {
        &[
            "/usr/lib64/libudev.so.1",
            "/lib64/libudev.so.1",
            "/usr/lib/libudev.so.1",
            "/lib/libudev.so.1",
            "/usr/lib/x86_64-linux-gnu/libudev.so.1",
            "/lib/x86_64-linux-gnu/libudev.so.1",
        ]
    } else if lower == "libpcre2-8.so.0" {
        &[
            "/usr/lib64/libpcre2-8.so.0",
            "/lib64/libpcre2-8.so.0",
            "/usr/lib/libpcre2-8.so.0",
            "/lib/libpcre2-8.so.0",
            "/usr/lib/x86_64-linux-gnu/libpcre2-8.so.0",
            "/lib/x86_64-linux-gnu/libpcre2-8.so.0",
        ]
    } else if lower == "libm.so.6" {
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

fn pacman_dependency_satisfied(name: &str, constraint: &VersionConstraint) -> bool {
    let Some(spec) = pacman_dependency_spec(name, constraint) else {
        return false;
    };
    Command::new("pacman")
        .args(["-T", &spec])
        .output()
        .is_ok_and(|output| output.status.success())
}

fn pacman_dependency_spec(name: &str, constraint: &VersionConstraint) -> Option<String> {
    let versioned =
        |operator: &str, version: &dyn std::fmt::Display| format!("{name}{operator}{version}");
    match constraint {
        VersionConstraint::Any => Some(name.to_string()),
        VersionConstraint::Exact(version) => Some(versioned("=", version)),
        VersionConstraint::GreaterThan(version) => Some(versioned(">", version)),
        VersionConstraint::GreaterOrEqual(version) => Some(versioned(">=", version)),
        VersionConstraint::LessThan(version) => Some(versioned("<", version)),
        VersionConstraint::LessOrEqual(version) => Some(versioned("<=", version)),
        VersionConstraint::NotEqual(_) | VersionConstraint::And(_, _) => None,
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
        "systemd-udev" | "udev" => Some(&[
            "/usr/bin/udevadm",
            "/bin/udevadm",
            "/usr/sbin/udevadm",
            "/sbin/udevadm",
            "/usr/lib/systemd/systemd-udevd",
            "/lib/systemd/systemd-udevd",
        ]),
        "pcre2" => Some(&[
            "/usr/lib64/libpcre2-8.so.0",
            "/lib64/libpcre2-8.so.0",
            "/usr/lib/libpcre2-8.so.0",
            "/lib/libpcre2-8.so.0",
            "/usr/lib/x86_64-linux-gnu/libpcre2-8.so.0",
            "/lib/x86_64-linux-gnu/libpcre2-8.so.0",
        ]),
        "glibc" | "libc6" => Some(&[
            "/usr/lib64/libc.so.6",
            "/lib64/libc.so.6",
            "/usr/lib/libc.so.6",
            "/lib/libc.so.6",
            "/usr/lib/x86_64-linux-gnu/libc.so.6",
            "/lib/x86_64-linux-gnu/libc.so.6",
            "/usr/lib/aarch64-linux-gnu/libc.so.6",
            "/lib/aarch64-linux-gnu/libc.so.6",
            "/usr/lib/riscv64-linux-gnu/libc.so.6",
            "/lib/riscv64-linux-gnu/libc.so.6",
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ElfClass {
    Elf32,
    Elf64,
}

fn required_elf_class(name: &str) -> Option<ElfClass> {
    let lower = name.to_ascii_lowercase();
    if lower.ends_with("(64bit)") {
        Some(ElfClass::Elf64)
    } else if lower.ends_with("(32bit)") {
        Some(ElfClass::Elf32)
    } else {
        None
    }
}

fn soname_present(soname: &str, required_class: Option<ElfClass>) -> bool {
    let output = match Command::new("ldconfig").arg("-p").output() {
        Ok(output) => output,
        Err(_) => return false,
    };

    output.status.success()
        && ldconfig_output_has_soname(
            &String::from_utf8_lossy(&output.stdout),
            soname,
            required_class,
        )
}

fn ldconfig_output_has_soname(
    output: &str,
    soname: &str,
    required_class: Option<ElfClass>,
) -> bool {
    output.lines().any(|line| {
        let Some((entry, path)) = line.split_once("=>") else {
            return false;
        };
        if entry.split_whitespace().next() != Some(soname) {
            return false;
        }

        path_matches_elf_class(Path::new(path.trim()), required_class)
    })
}

fn elf_class(path: &Path) -> Option<ElfClass> {
    let mut header = [0_u8; 5];
    File::open(path).ok()?.read_exact(&mut header).ok()?;
    if header[..4] != *b"\x7fELF" {
        return None;
    }
    match header[4] {
        1 => Some(ElfClass::Elf32),
        2 => Some(ElfClass::Elf64),
        _ => None,
    }
}

fn path_matches_elf_class(path: &Path, required_class: Option<ElfClass>) -> bool {
    match required_class {
        Some(required_class) => elf_class(path) == Some(required_class),
        None => elf_class(path).is_some(),
    }
}

fn candidate_paths(paths: &[&str]) -> bool {
    paths.iter().any(|path| Path::new(path).exists())
}

fn candidate_elf_paths(paths: &[&str], required_class: Option<ElfClass>) -> bool {
    paths
        .iter()
        .any(|path| path_matches_elf_class(Path::new(path), required_class))
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
        assert!(!is_live_runtime_dependency_present(
            "tree",
            &VersionConstraint::Any
        ));
    }

    #[test]
    fn live_runtime_soname_extracts_generic_rpm_soname_dependencies() {
        assert_eq!(
            live_runtime_soname("libcap.so.2()(64bit)"),
            Some("libcap.so.2")
        );
        assert_eq!(
            live_runtime_soname("libhwloc.so.15()(64bit)"),
            Some("libhwloc.so.15")
        );
        assert_eq!(
            live_runtime_soname("libstdc++.so.6(GLIBCXX_3.4.20)(64bit)"),
            Some("libstdc++.so.6")
        );
        assert_eq!(live_runtime_soname("application()"), None);
        assert_eq!(live_runtime_soname("htop"), None);
    }

    #[test]
    fn generic_live_runtime_soname_uses_exact_compatible_cache_entry() {
        let temp = tempfile::tempdir().unwrap();
        let elf64 = temp.path().join("libcap.so.2");
        let elf32 = temp.path().join("libcap-32.so.2");
        std::fs::write(&elf64, b"\x7fELF\x02").unwrap();
        std::fs::write(&elf32, b"\x7fELF\x01").unwrap();

        let cache = format!(
            "\tlibcap.so.20 (libc6,x86-64) => {}\n\tlibcap.so.2 (libc6,x86-64) => {}\n",
            elf64.display(),
            elf64.display()
        );
        let present = |soname: &str, required_class: Option<ElfClass>| {
            ldconfig_output_has_soname(&cache, soname, required_class)
        };

        assert!(is_live_runtime_dependency_present_with_probes(
            "libcap.so.2()(64bit)",
            &VersionConstraint::Any,
            SystemPackageManager::Rpm,
            |_, _| panic!("RPM SONAME lookup must not call pacman"),
            present
        ));
        assert!(is_live_runtime_dependency_present_with_probes(
            "libcap.so.2()(64bit)",
            &VersionConstraint::Any,
            SystemPackageManager::Pacman,
            |_, _| false,
            present
        ));
        assert!(!is_live_runtime_dependency_present_with_probes(
            "libcap.so.200()(64bit)",
            &VersionConstraint::Any,
            SystemPackageManager::Pacman,
            |_, _| false,
            present
        ));

        let wrong_abi_cache = format!("\tlibcap.so.2 (libc6) => {}\n", elf32.display());
        assert!(!is_live_runtime_dependency_present_with_probes(
            "libcap.so.2()(64bit)",
            &VersionConstraint::Any,
            SystemPackageManager::Pacman,
            |_, _| false,
            |soname, required_class| {
                ldconfig_output_has_soname(&wrong_abi_cache, soname, required_class)
            }
        ));

        let not_elf = temp.path().join("libexample.so.1");
        std::fs::write(&not_elf, b"not an ELF file").unwrap();
        let non_elf_cache = format!("\tlibexample.so.1 (libc6) => {}\n", not_elf.display());
        assert!(!ldconfig_output_has_soname(
            &non_elf_cache,
            "libexample.so.1",
            None
        ));
    }

    #[test]
    fn arch_versioned_soname_uses_constraint_aware_pacman_probe() {
        let constraint = VersionConstraint::parse("= 2-64").unwrap();
        assert_eq!(
            pacman_dependency_spec("libcap.so", &constraint).as_deref(),
            Some("libcap.so=2-64")
        );
        assert!(is_live_runtime_dependency_present_with_probes(
            "libcap.so",
            &constraint,
            SystemPackageManager::Pacman,
            |name, constraint| {
                pacman_dependency_spec(name, constraint).as_deref() == Some("libcap.so=2-64")
            },
            |_, _| panic!("Arch SONAME lookup must use pacman's capability database")
        ));

        let wrong_version = VersionConstraint::parse("= 999-64").unwrap();
        assert!(!is_live_runtime_dependency_present_with_probes(
            "libcap.so",
            &wrong_version,
            SystemPackageManager::Pacman,
            |name, constraint| {
                pacman_dependency_spec(name, constraint).as_deref() == Some("libcap.so=2-64")
            },
            |_, _| panic!("Arch SONAME lookup must use pacman's capability database")
        ));
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
        assert_eq!(
            live_runtime_package_probe_paths("glibc"),
            Some(
                &[
                    "/usr/lib64/libc.so.6",
                    "/lib64/libc.so.6",
                    "/usr/lib/libc.so.6",
                    "/lib/libc.so.6",
                    "/usr/lib/x86_64-linux-gnu/libc.so.6",
                    "/lib/x86_64-linux-gnu/libc.so.6",
                    "/usr/lib/aarch64-linux-gnu/libc.so.6",
                    "/lib/aarch64-linux-gnu/libc.so.6",
                    "/usr/lib/riscv64-linux-gnu/libc.so.6",
                    "/lib/riscv64-linux-gnu/libc.so.6",
                ][..]
            )
        );
        assert_eq!(
            live_runtime_package_probe_paths("libc6"),
            live_runtime_package_probe_paths("glibc")
        );
        assert_eq!(
            live_runtime_package_probe_paths("udev"),
            Some(
                &[
                    "/usr/bin/udevadm",
                    "/bin/udevadm",
                    "/usr/sbin/udevadm",
                    "/sbin/udevadm",
                    "/usr/lib/systemd/systemd-udevd",
                    "/lib/systemd/systemd-udevd",
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
            live_runtime_soname("libudev.so.1()(64bit)"),
            Some("libudev.so.1")
        );
        assert_eq!(
            live_runtime_soname("libpcre2-8.so.0()(64bit)"),
            Some("libpcre2-8.so.0")
        );
        assert_eq!(
            live_runtime_soname("libm.so.6(GLIBC_2.2.5)(64bit)"),
            Some("libm.so.6")
        );
        assert_eq!(
            live_runtime_soname("libfoo.so.1()(64bit)"),
            Some("libfoo.so.1")
        );
        assert_eq!(live_runtime_soname("libfoo"), None);
        assert!(!live_runtime_soname_probe_paths("libcrypto.so.3").is_empty());
        assert!(!live_runtime_soname_probe_paths("libgcc_s.so.1").is_empty());
        assert!(!live_runtime_soname_probe_paths("libpam.so.0").is_empty());
        assert!(!live_runtime_soname_probe_paths("libudev.so.1").is_empty());
        assert!(!live_runtime_soname_probe_paths("libpcre2-8.so.0").is_empty());
        assert!(!live_runtime_soname_probe_paths("libm.so.6").is_empty());
        assert!(live_runtime_soname_probe_paths("libcrypto.so.30").is_empty());
        assert!(live_runtime_soname_probe_paths("libpam.so.1").is_empty());
    }
}
