// conary-core/src/recipe/audit.rs

//! Recipe dependency audit -- static analysis and build-time tracing.
//!
//! Detects tools and libraries used in recipe build scripts that are not
//! declared in `makedepends` or `requires`.

use std::collections::HashSet;

use crate::recipe::Recipe;

/// Errors during recipe auditing.
#[derive(Debug, thiserror::Error)]
pub enum AuditError {
    #[error("recipe has no build scripts to audit")]
    NoBuildScripts,
    #[error("strace not found in PATH -- required for --trace")]
    StraceMissing,
    #[error("build failed during trace: {0}")]
    BuildFailed(String),
    #[error("no built sysroot available -- run a full pipeline build first")]
    NoSysroot,
    #[error("recipe parse error: {0}")]
    RecipeParse(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FindingKind {
    Missing,
    Verified,
    Ignored,
}

#[derive(Debug, Clone)]
pub struct AuditFinding {
    pub tool: String,
    pub package: String,
    pub context: String,
    pub kind: FindingKind,
}

#[derive(Debug)]
pub struct AuditReport {
    pub package_name: String,
    pub package_version: String,
    pub findings: Vec<AuditFinding>,
}

impl AuditReport {
    #[must_use]
    pub fn count(&self, kind: FindingKind) -> usize {
        self.findings.iter().filter(|f| f.kind == kind).count()
    }
}

const TOOL_PACKAGE_MAP: &[(&str, &str)] = &[
    ("pkg-config", "pkg-config"),
    ("cmake", "cmake"),
    ("meson", "meson"),
    ("ninja", "ninja"),
    ("scons", "scons"),
    ("python3", "python"),
    ("python", "python"),
    ("perl", "perl"),
    ("m4", "m4"),
    ("ruby", "ruby"),
    ("autoconf", "autoconf"),
    ("automake", "automake"),
    ("libtool", "libtool"),
    ("autoreconf", "autoconf"),
    ("bison", "bison"),
    ("flex", "flex"),
    ("yacc", "bison"),
    ("lex", "flex"),
    ("gettext", "gettext"),
    ("intltool-update", "intltool"),
    ("msgfmt", "gettext"),
    ("makeinfo", "texinfo"),
    ("install-info", "texinfo"),
    ("nasm", "nasm"),
    ("yasm", "yasm"),
    ("cargo", "rust"),
    ("rustc", "rust"),
    ("go", "go"),
];

const BASE_TOOLS: &[&str] = &[
    "make", "gcc", "g++", "cc", "c++", "ld", "ar", "as", "nm", "ranlib",
    "strip", "objdump", "objcopy", "readelf", "strings",
    "bash", "sh", "env", "test", "true", "false",
    "cat", "cp", "mv", "rm", "mkdir", "rmdir", "ln", "ls", "chmod",
    "chown", "touch", "head", "tail", "sort", "uniq", "wc", "tr",
    "cut", "paste", "comm", "diff", "find", "xargs",
    "sed", "awk", "grep", "egrep", "fgrep",
    "tar", "gzip", "gunzip", "bzip2", "xz", "zstd",
    "install", "dirname", "basename", "realpath", "readlink",
    "echo", "printf", "expr", "tee",
];

/// Statically audit a recipe's build scripts for undeclared tool and library dependencies.
///
/// Scans all build script fields (`configure`, `setup`, `make`, `install`, `check`)
/// for tool invocations (from [`TOOL_PACKAGE_MAP`]) and `-l<lib>` linker flags,
/// then cross-references each against the declared `makedepends` and `requires` lists.
///
/// # Errors
///
/// Returns [`AuditError::NoBuildScripts`] if the recipe has no non-empty build scripts.
pub fn static_audit(recipe: &Recipe) -> Result<AuditReport, AuditError> {
    let build = &recipe.build;

    let mut scripts = String::new();
    if let Some(ref s) = build.configure {
        scripts.push_str(s);
        scripts.push('\n');
    }
    if let Some(ref s) = build.setup {
        scripts.push_str(s);
        scripts.push('\n');
    }
    if let Some(ref s) = build.make {
        scripts.push_str(s);
        scripts.push('\n');
    }
    if let Some(ref s) = build.install {
        scripts.push_str(s);
        scripts.push('\n');
    }
    if let Some(ref s) = build.check {
        scripts.push_str(s);
        scripts.push('\n');
    }

    if scripts.trim().is_empty() {
        return Err(AuditError::NoBuildScripts);
    }

    let declared: HashSet<&str> = build
        .requires
        .iter()
        .chain(build.makedepends.iter())
        .map(|s| s.as_str())
        .collect();

    let mut findings = Vec::new();
    let mut seen_tools: HashSet<&str> = HashSet::new();
    let mut seen_libs: HashSet<String> = HashSet::new();

    // Build a set of tokens from the scripts for word-boundary-safe matching.
    // This avoids false positives like "go" matching inside "cargo".
    let tokens: HashSet<&str> = scripts.split_whitespace().collect();

    // Scan for tool invocations (word-boundary match via token set)
    for &(tool, package) in TOOL_PACKAGE_MAP {
        let found = tokens.contains(tool)
            || tokens.iter().any(|t| {
                // Also match "path/to/tool" patterns (e.g., "/usr/bin/cmake")
                t.ends_with(&format!("/{tool}"))
            });

        if found && seen_tools.insert(tool) {
            let kind = if BASE_TOOLS.contains(&tool) {
                FindingKind::Ignored
            } else if declared.contains(package) {
                FindingKind::Verified
            } else {
                FindingKind::Missing
            };

            findings.push(AuditFinding {
                tool: tool.to_owned(),
                package: package.to_owned(),
                context: "build scripts".to_owned(),
                kind,
            });
        }
    }

    // Scan for -l<lib> linker flags
    for word in &tokens {
        if let Some(lib) = word.strip_prefix("-l")
            && !lib.is_empty()
            && lib.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-')
            && seen_libs.insert(lib.to_owned())
        {
            // Use exact match against declared deps, not substring
            let kind = if declared.contains(lib) {
                FindingKind::Verified
            } else {
                FindingKind::Missing
            };

            findings.push(AuditFinding {
                tool: format!("-l{lib}"),
                package: lib.to_owned(),
                context: "linker flag".to_owned(),
                kind,
            });
        }
    }

    Ok(AuditReport {
        package_name: recipe.package.name.clone(),
        package_version: recipe.package.version.clone(),
        findings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recipe_with_scripts(configure: &str, makedepends: Vec<&str>) -> Recipe {
        let deps = makedepends
            .iter()
            .map(|d| format!("\"{d}\""))
            .collect::<Vec<_>>()
            .join(", ");
        let toml_str = format!(
            r#"
[package]
name = "test-pkg"
version = "1.0"

[source]
archive = "https://example.com/test-1.0.tar.gz"
checksum = "sha256:abc123"

[build]
makedepends = [{deps}]
configure = "{configure}"
make = "make"
install = "make install"
"#
        );
        toml::from_str(&toml_str).expect("test recipe must parse")
    }

    #[test]
    fn detects_missing_pkg_config() {
        let recipe = recipe_with_scripts("pkg-config --cflags foo", vec![]);
        let report = static_audit(&recipe).unwrap();
        assert!(report.count(FindingKind::Missing) >= 1);
        assert!(report
            .findings
            .iter()
            .any(|f| f.tool == "pkg-config" && f.kind == FindingKind::Missing));
    }

    #[test]
    fn verified_when_declared() {
        let recipe = recipe_with_scripts("pkg-config --cflags foo", vec!["pkg-config"]);
        let report = static_audit(&recipe).unwrap();
        assert!(report
            .findings
            .iter()
            .any(|f| f.tool == "pkg-config" && f.kind == FindingKind::Verified));
        assert_eq!(report.count(FindingKind::Missing), 0);
    }

    #[test]
    fn ignores_base_tools() {
        let recipe = recipe_with_scripts("sed -i 's/foo/bar/' file", vec![]);
        let report = static_audit(&recipe).unwrap();
        assert_eq!(report.count(FindingKind::Missing), 0);
    }

    #[test]
    fn detects_linker_flags() {
        let recipe = recipe_with_scripts("./configure -lssl -lcrypto", vec![]);
        let report = static_audit(&recipe).unwrap();
        assert!(report
            .findings
            .iter()
            .any(|f| f.tool == "-lssl" && f.kind == FindingKind::Missing));
    }

    #[test]
    fn empty_build_scripts_returns_error() {
        let toml_str = r#"
[package]
name = "empty"
version = "1.0"

[source]
archive = "https://example.com/empty.tar.gz"
checksum = "sha256:abc123"

[build]
"#;
        let recipe: Recipe = toml::from_str(toml_str).expect("parse");
        let result = static_audit(&recipe);
        assert!(matches!(result, Err(AuditError::NoBuildScripts)));
    }

    #[test]
    fn report_count_works() {
        let recipe = recipe_with_scripts("cmake . && pkg-config --libs bar", vec!["cmake"]);
        let report = static_audit(&recipe).unwrap();
        // cmake is verified (declared), pkg-config is missing
        assert!(report.count(FindingKind::Verified) >= 1);
        assert!(report.count(FindingKind::Missing) >= 1);
    }
}
