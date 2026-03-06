// conary-core/src/ccs/legacy/mod.rs
//! Legacy package format generators
//!
//! This module provides generators to convert CCS packages to legacy formats
//! (DEB, RPM, Arch) for compatibility with existing package managers.

pub mod arch;
pub mod deb;
pub mod rpm;

use crate::ccs::manifest::Hooks;
use std::collections::HashMap;
use std::sync::LazyLock;

/// Information that may be lost when converting to legacy formats
#[derive(Debug, Default)]
pub struct LossReport {
    /// Features that couldn't be represented in the target format
    pub unsupported_features: Vec<String>,
    /// Hooks that were approximated or skipped
    pub hook_notes: Vec<String>,
    /// Dependency mappings that may be inaccurate
    pub dependency_notes: Vec<String>,
}

impl LossReport {
    pub fn is_empty(&self) -> bool {
        self.unsupported_features.is_empty()
            && self.hook_notes.is_empty()
            && self.dependency_notes.is_empty()
    }

    pub fn add_unsupported(&mut self, feature: &str) {
        self.unsupported_features.push(feature.to_string());
    }

    pub fn add_hook_note(&mut self, note: &str) {
        self.hook_notes.push(note.to_string());
    }

    pub fn add_dependency_note(&mut self, note: &str) {
        self.dependency_notes.push(note.to_string());
    }

    /// Print a summary of what was lost in conversion
    pub fn print_summary(&self, format_name: &str) {
        if self.is_empty() {
            return;
        }

        println!("  Conversion notes for {}:", format_name);

        for note in &self.unsupported_features {
            println!("    [UNSUPPORTED] {}", note);
        }

        for note in &self.hook_notes {
            println!("    [HOOK] {}", note);
        }

        for note in &self.dependency_notes {
            println!("    [DEPENDENCY] {}", note);
        }
    }
}

/// Result of a legacy format generation
#[derive(Debug)]
pub struct GenerationResult {
    /// Size of the generated package in bytes
    pub size: u64,
    /// Any lossy conversion notes
    pub loss_report: LossReport,
}

/// Convert CCS hooks to format-specific install scripts
pub trait HookConverter {
    /// Generate pre-install script content
    fn pre_install(&self, hooks: &Hooks) -> Option<String>;

    /// Generate post-install script content
    fn post_install(&self, hooks: &Hooks) -> Option<String>;

    /// Generate pre-remove script content
    fn pre_remove(&self, hooks: &Hooks) -> Option<String>;

    /// Generate post-remove script content
    fn post_remove(&self, hooks: &Hooks) -> Option<String>;
}

/// Common functionality for generating install scripts from CCS hooks
pub struct CommonHookGenerator;

impl CommonHookGenerator {
    /// Generate user creation commands
    pub fn user_creation_commands(hooks: &Hooks) -> Vec<String> {
        let mut commands = Vec::new();

        for group in &hooks.groups {
            let flags = if group.system { "--system " } else { "" };
            let name = shell_escape(&group.name);
            commands.push(format!(
                "getent group {name} >/dev/null || groupadd {flags}{name}"
            ));
        }

        for user in &hooks.users {
            let mut flags = Vec::new();
            if user.system {
                flags.push("--system".to_string());
            }
            if let Some(home) = &user.home {
                flags.push(format!("--home-dir {}", shell_escape(home)));
            }
            if let Some(shell) = &user.shell {
                flags.push(format!("--shell {}", shell_escape(shell)));
            } else if user.system {
                flags.push("--shell /usr/sbin/nologin".to_string());
            }
            if let Some(group) = &user.group {
                flags.push(format!("--gid {}", shell_escape(group)));
            }

            let flags_str = if flags.is_empty() {
                String::new()
            } else {
                format!("{} ", flags.join(" "))
            };

            let name = shell_escape(&user.name);
            commands.push(format!(
                "getent passwd {name} >/dev/null || useradd {flags_str}{name}"
            ));
        }

        commands
    }

    /// Generate directory creation commands
    pub fn directory_commands(hooks: &Hooks) -> Vec<String> {
        let mut commands = Vec::new();

        for dir in &hooks.directories {
            commands.push(format!(
                "install -d -m {} -o {} -g {} {}",
                shell_escape(&dir.mode),
                shell_escape(&dir.owner),
                shell_escape(&dir.group),
                shell_escape(&dir.path),
            ));
        }

        commands
    }

    /// Generate systemd enable/disable commands
    pub fn systemd_commands(hooks: &Hooks, enable: bool) -> Vec<String> {
        let mut commands = Vec::new();

        for unit in &hooks.systemd {
            if unit.enable && enable {
                // Only enable if requested
                let unit_name = shell_escape(&unit.unit);
                commands.push(format!(
                    "if command -v systemctl >/dev/null 2>&1; then systemctl daemon-reload; systemctl enable {unit_name}; fi"
                ));
            } else if !enable {
                // Stop on removal
                let unit_name = shell_escape(&unit.unit);
                commands.push(format!(
                    "if command -v systemctl >/dev/null 2>&1; then systemctl stop {unit_name} 2>/dev/null || true; fi"
                ));
            }
        }

        commands
    }

    /// Generate tmpfiles.d commands
    pub fn tmpfiles_commands(hooks: &Hooks) -> Vec<String> {
        let mut commands = Vec::new();

        if !hooks.tmpfiles.is_empty() {
            // systemd-tmpfiles --create will read from /usr/lib/tmpfiles.d/
            commands.push(
                "if command -v systemd-tmpfiles >/dev/null 2>&1; then systemd-tmpfiles --create; fi"
                    .to_string(),
            );
        }

        commands
    }

    /// Generate sysctl commands
    ///
    /// Sysctl keys and values are validated to contain only safe characters
    /// (alphanumeric, dots, underscores, hyphens, digits) and emitted unquoted.
    /// This avoids breaking arithmetic comparisons and sysctl -w assignments
    /// that would fail with shell-escaped (single-quoted) values.
    pub fn sysctl_commands(hooks: &Hooks) -> Vec<String> {
        let mut commands = Vec::new();

        for sysctl in &hooks.sysctl {
            let key = &sysctl.key;
            let value = &sysctl.value;

            // Validate that key and value contain only safe characters
            let is_safe =
                |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-');

            if !is_safe(key) || !is_safe(value) {
                // Fall back to shell-escaped form for safety, but only for
                // non-arithmetic (unconditional) sysctl writes
                let escaped_key = shell_escape(key);
                let escaped_value = shell_escape(value);
                commands.push(format!("sysctl -w {escaped_key}={escaped_value}"));
                continue;
            }

            if sysctl.only_if_lower {
                commands.push(format!(
                    "current=$(sysctl -n {key} 2>/dev/null || echo 0); if [ \"$current\" -lt {value} ]; then sysctl -w {key}={value}; fi"
                ));
            } else {
                commands.push(format!("sysctl -w {key}={value}"));
            }
        }

        commands
    }
}

/// Escape a string for safe use in shell commands.
///
/// Wraps the value in single quotes and escapes any embedded single quotes
/// using the `'\''` idiom, which is safe against shell injection.
pub fn shell_escape(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Static capability-to-package mapping table, initialized once
static CAPABILITY_MAPPINGS: LazyLock<HashMap<&'static str, HashMap<&'static str, &'static str>>> =
    LazyLock::new(|| {
        HashMap::from([
        // Core system libraries
        (
            "glibc",
            HashMap::from([("deb", "libc6"), ("rpm", "glibc"), ("arch", "glibc")]),
        ),
        (
            "libgcc",
            HashMap::from([
                ("deb", "libgcc-s1"),
                ("rpm", "libgcc"),
                ("arch", "gcc-libs"),
            ]),
        ),
        (
            "libstdc++",
            HashMap::from([
                ("deb", "libstdc++6"),
                ("rpm", "libstdc++"),
                ("arch", "gcc-libs"),
            ]),
        ),
        // Cryptography and security
        (
            "openssl",
            HashMap::from([
                ("deb", "libssl3"),
                ("rpm", "openssl-libs"),
                ("arch", "openssl"),
            ]),
        ),
        (
            "libcrypto",
            HashMap::from([
                ("deb", "libssl3"),
                ("rpm", "openssl-libs"),
                ("arch", "openssl"),
            ]),
        ),
        (
            "gnutls",
            HashMap::from([
                ("deb", "libgnutls30"),
                ("rpm", "gnutls"),
                ("arch", "gnutls"),
            ]),
        ),
        // Compression
        (
            "zlib",
            HashMap::from([("deb", "zlib1g"), ("rpm", "zlib"), ("arch", "zlib")]),
        ),
        (
            "bzip2",
            HashMap::from([
                ("deb", "libbz2-1.0"),
                ("rpm", "bzip2-libs"),
                ("arch", "bzip2"),
            ]),
        ),
        (
            "xz",
            HashMap::from([("deb", "liblzma5"), ("rpm", "xz-libs"), ("arch", "xz")]),
        ),
        (
            "lz4",
            HashMap::from([("deb", "liblz4-1"), ("rpm", "lz4-libs"), ("arch", "lz4")]),
        ),
        (
            "zstd",
            HashMap::from([("deb", "libzstd1"), ("rpm", "libzstd"), ("arch", "zstd")]),
        ),
        // Networking
        (
            "libcurl",
            HashMap::from([("deb", "libcurl4"), ("rpm", "libcurl"), ("arch", "curl")]),
        ),
        (
            "libssh2",
            HashMap::from([
                ("deb", "libssh2-1"),
                ("rpm", "libssh2"),
                ("arch", "libssh2"),
            ]),
        ),
        (
            "nghttp2",
            HashMap::from([
                ("deb", "libnghttp2-14"),
                ("rpm", "libnghttp2"),
                ("arch", "libnghttp2"),
            ]),
        ),
        // XML/JSON/YAML parsing
        (
            "libxml2",
            HashMap::from([("deb", "libxml2"), ("rpm", "libxml2"), ("arch", "libxml2")]),
        ),
        (
            "libxslt",
            HashMap::from([
                ("deb", "libxslt1.1"),
                ("rpm", "libxslt"),
                ("arch", "libxslt"),
            ]),
        ),
        (
            "libyaml",
            HashMap::from([
                ("deb", "libyaml-0-2"),
                ("rpm", "libyaml"),
                ("arch", "libyaml"),
            ]),
        ),
        // Database
        (
            "sqlite3",
            HashMap::from([
                ("deb", "libsqlite3-0"),
                ("rpm", "sqlite-libs"),
                ("arch", "sqlite"),
            ]),
        ),
        (
            "libpq",
            HashMap::from([
                ("deb", "libpq5"),
                ("rpm", "postgresql-libs"),
                ("arch", "postgresql-libs"),
            ]),
        ),
        (
            "libmysqlclient",
            HashMap::from([
                ("deb", "libmysqlclient21"),
                ("rpm", "mysql-libs"),
                ("arch", "mariadb-libs"),
            ]),
        ),
        // Math/science
        (
            "fftw",
            HashMap::from([
                ("deb", "libfftw3-3"),
                ("rpm", "fftw-libs"),
                ("arch", "fftw"),
            ]),
        ),
        (
            "lapack",
            HashMap::from([("deb", "liblapack3"), ("rpm", "lapack"), ("arch", "lapack")]),
        ),
        // Image formats
        (
            "libpng",
            HashMap::from([
                ("deb", "libpng16-16"),
                ("rpm", "libpng"),
                ("arch", "libpng"),
            ]),
        ),
        (
            "libjpeg",
            HashMap::from([
                ("deb", "libjpeg62-turbo"),
                ("rpm", "libjpeg-turbo"),
                ("arch", "libjpeg-turbo"),
            ]),
        ),
        (
            "libwebp",
            HashMap::from([("deb", "libwebp7"), ("rpm", "libwebp"), ("arch", "libwebp")]),
        ),
        // Text/fonts
        (
            "freetype",
            HashMap::from([
                ("deb", "libfreetype6"),
                ("rpm", "freetype"),
                ("arch", "freetype2"),
            ]),
        ),
        (
            "fontconfig",
            HashMap::from([
                ("deb", "libfontconfig1"),
                ("rpm", "fontconfig"),
                ("arch", "fontconfig"),
            ]),
        ),
        (
            "pcre2",
            HashMap::from([("deb", "libpcre2-8-0"), ("rpm", "pcre2"), ("arch", "pcre2")]),
        ),
        // System utilities
        (
            "systemd",
            HashMap::from([
                ("deb", "libsystemd0"),
                ("rpm", "systemd-libs"),
                ("arch", "systemd-libs"),
            ]),
        ),
        (
            "dbus",
            HashMap::from([
                ("deb", "libdbus-1-3"),
                ("rpm", "dbus-libs"),
                ("arch", "dbus"),
            ]),
        ),
        (
            "udev",
            HashMap::from([
                ("deb", "libudev1"),
                ("rpm", "systemd-libs"),
                ("arch", "systemd-libs"),
            ]),
        ),
        // Python/Perl/Ruby runtimes (for extensions)
        (
            "python3",
            HashMap::from([
                ("deb", "libpython3.11"),
                ("rpm", "python3-libs"),
                ("arch", "python"),
            ]),
        ),
        (
            "perl",
            HashMap::from([
                ("deb", "libperl5.36"),
                ("rpm", "perl-libs"),
                ("arch", "perl"),
            ]),
        ),
        (
            "ruby",
            HashMap::from([
                ("deb", "libruby3.1"),
                ("rpm", "ruby-libs"),
                ("arch", "ruby"),
            ]),
        ),
    ])
});

/// Map a CCS dependency to a format-specific package name
/// This is a best-effort mapping and may not be accurate for all cases
pub fn map_capability_to_package(capability: &str, format: &str) -> Option<String> {
    CAPABILITY_MAPPINGS
        .get(capability)
        .and_then(|m| m.get(format))
        .map(|s| (*s).to_string())
}

/// Map a capability to a distro-specific package name via canonical DB.
/// Falls back to hardcoded mappings if DB has no entry.
pub fn map_capability_to_package_db(
    conn: &rusqlite::Connection,
    capability: &str,
    format: &str,
) -> crate::error::Result<Option<String>> {
    use crate::db::models::{CanonicalPackage, PackageImplementation};

    // Map format string to distro prefix for matching
    let distro_prefix = match format {
        "deb" => "ubuntu",
        "rpm" => "fedora",
        "arch" => "arch",
        _ => return Ok(map_capability_to_package(capability, format)),
    };

    // Try canonical lookup first
    if let Some(canonical) = CanonicalPackage::resolve_name(conn, capability)?
        && let Some(can_id) = canonical.id
    {
        let impls = PackageImplementation::find_by_canonical(conn, can_id)?;
        if let Some(impl_pkg) = impls.iter().find(|i| i.distro.starts_with(distro_prefix)) {
            return Ok(Some(impl_pkg.distro_name.clone()));
        }
    }

    // Fall back to hardcoded mappings
    Ok(map_capability_to_package(capability, format))
}

/// Get the architecture string for a format
pub fn arch_for_format(arch: Option<&str>, format: &str) -> String {
    let arch = arch.unwrap_or("x86_64");

    match format {
        "deb" => match arch {
            "x86_64" => "amd64",
            "aarch64" => "arm64",
            "i686" | "i386" => "i386",
            "armv7l" | "armhf" => "armhf",
            _ => arch,
        },
        "rpm" => match arch {
            "amd64" => "x86_64",
            "arm64" => "aarch64",
            _ => arch,
        },
        "arch" => match arch {
            "amd64" => "x86_64",
            _ => arch,
        },
        _ => arch,
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_arch_mapping() {
        assert_eq!(arch_for_format(Some("x86_64"), "deb"), "amd64");
        assert_eq!(arch_for_format(Some("aarch64"), "deb"), "arm64");
        assert_eq!(arch_for_format(Some("amd64"), "rpm"), "x86_64");
        assert_eq!(arch_for_format(None, "arch"), "x86_64");
    }

    #[test]
    fn test_capability_mapping() {
        assert_eq!(
            map_capability_to_package("glibc", "deb"),
            Some("libc6".to_string())
        );
        assert_eq!(
            map_capability_to_package("openssl", "rpm"),
            Some("openssl-libs".to_string())
        );
        assert_eq!(map_capability_to_package("unknown", "deb"), None);
    }

    #[test]
    fn test_map_capability_from_db() {
        use crate::db::models::{CanonicalPackage, PackageImplementation};
        use crate::db::schema;
        use rusqlite::Connection;
        use tempfile::NamedTempFile;

        let temp_file = NamedTempFile::new().unwrap();
        let conn = Connection::open(temp_file.path()).unwrap();
        conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
        schema::migrate(&conn).unwrap();

        // Populate canonical data
        let mut pkg = CanonicalPackage::new("glibc".to_string(), "package".to_string());
        let cid = pkg.insert(&conn).unwrap();
        let mut i1 = PackageImplementation::new(
            cid,
            "ubuntu-noble".into(),
            "libc6".into(),
            "curated".into(),
        );
        i1.insert_or_ignore(&conn).unwrap();
        let mut i2 = PackageImplementation::new(
            cid,
            "fedora-41".into(),
            "glibc".into(),
            "curated".into(),
        );
        i2.insert_or_ignore(&conn).unwrap();
        let mut i3 =
            PackageImplementation::new(cid, "arch".into(), "glibc".into(), "curated".into());
        i3.insert_or_ignore(&conn).unwrap();

        // DB lookup should work
        let result = map_capability_to_package_db(&conn, "glibc", "deb").unwrap();
        assert_eq!(result, Some("libc6".to_string()));

        let result = map_capability_to_package_db(&conn, "glibc", "rpm").unwrap();
        assert_eq!(result, Some("glibc".to_string()));

        // Unknown capability falls through to hardcoded (which also returns None for truly unknown)
        let result = map_capability_to_package_db(&conn, "totally-unknown-pkg", "deb").unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn test_loss_report() {
        let mut report = LossReport::default();
        assert!(report.is_empty());

        report.add_unsupported("merkle tree verification");
        assert!(!report.is_empty());
        assert_eq!(report.unsupported_features.len(), 1);
    }
}
