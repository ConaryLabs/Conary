// src/packages/rpm.rs

//! RPM package format parser

use crate::db::models::Trove;
use crate::error::{Error, Result};
use crate::packages::common::PackageMetadata;
use crate::packages::traits::{
    ConfigFileInfo, Dependency, DependencyType, ExtractedFile, PackageFile, PackageFormat,
    Scriptlet, ScriptletPhase,
};
use rpm::Package;
use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;
use tracing::debug;

/// RPM package representation
pub struct RpmPackage {
    /// Common package metadata
    meta: PackageMetadata,
    // RPM-specific provenance information
    source_rpm: Option<String>,
    build_host: Option<String>,
    vendor: Option<String>,
    license: Option<String>,
    url: Option<String>,
}

impl RpmPackage {
    /// Extract scriptlets from RPM package using rpm command
    fn extract_scriptlets(path: &std::path::Path) -> Vec<Scriptlet> {
        use std::process::Command;

        let output = match Command::new("rpm")
            .args(["-qp", "--scripts"])
            .arg(path)
            .output()
        {
            Ok(o) => o,
            Err(e) => {
                debug!("Failed to run rpm --scripts: {}", e);
                return Vec::new();
            }
        };

        if !output.status.success() {
            debug!(
                "rpm --scripts failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            return Vec::new();
        }

        let output_str = String::from_utf8_lossy(&output.stdout);
        Self::parse_rpm_scripts(&output_str)
    }

    /// Parse the output of `rpm -qp --scripts`
    fn parse_rpm_scripts(output: &str) -> Vec<Scriptlet> {
        let mut scriptlets = Vec::new();
        let mut current_phase: Option<ScriptletPhase> = None;
        let mut current_interpreter = String::from("/bin/sh");
        let mut current_content = String::new();

        for line in output.lines() {
            // Check for scriptlet header lines like:
            // "preinstall scriptlet (using /bin/sh):"
            // "postinstall program: /usr/bin/lua"
            if let Some((phase, interpreter)) = Self::parse_scriptlet_header(line) {
                // Save previous scriptlet if any
                if let Some(prev_phase) = current_phase.take() {
                    let content = current_content.trim().to_string();
                    if !content.is_empty() {
                        scriptlets.push(Scriptlet {
                            phase: prev_phase,
                            interpreter: current_interpreter.clone(),
                            content,
                            flags: None,
                        });
                    }
                }
                current_phase = Some(phase);
                current_interpreter = interpreter;
                current_content.clear();
            } else if current_phase.is_some() {
                // Accumulate script content
                current_content.push_str(line);
                current_content.push('\n');
            }
        }

        // Don't forget the last scriptlet
        if let Some(phase) = current_phase {
            let content = current_content.trim().to_string();
            if !content.is_empty() {
                scriptlets.push(Scriptlet {
                    phase,
                    interpreter: current_interpreter,
                    content,
                    flags: None,
                });
            }
        }

        scriptlets
    }

    /// Parse a scriptlet header line and return (phase, interpreter)
    fn parse_scriptlet_header(line: &str) -> Option<(ScriptletPhase, String)> {
        let line_lower = line.to_lowercase();

        // Determine the phase
        let phase = if line_lower.starts_with("preinstall") || line_lower.starts_with("prein ") {
            ScriptletPhase::PreInstall
        } else if line_lower.starts_with("postinstall") || line_lower.starts_with("postin ") {
            ScriptletPhase::PostInstall
        } else if line_lower.starts_with("preuninstall") || line_lower.starts_with("preun ") {
            ScriptletPhase::PreRemove
        } else if line_lower.starts_with("postuninstall") || line_lower.starts_with("postun ") {
            ScriptletPhase::PostRemove
        } else if line_lower.starts_with("pretrans") {
            ScriptletPhase::PreTransaction
        } else if line_lower.starts_with("posttrans") {
            ScriptletPhase::PostTransaction
        } else if line_lower.contains("trigger") {
            ScriptletPhase::Trigger
        } else {
            return None;
        };

        // Extract interpreter from "(using /path/to/interpreter):" or "program: /path"
        let interpreter = if let Some(start) = line.find("(using ") {
            let rest = &line[start + 7..];
            if let Some(end) = rest.find(')') {
                rest[..end].to_string()
            } else {
                "/bin/sh".to_string()
            }
        } else if let Some(start) = line.find("program: ") {
            line[start + 9..].trim_end_matches(':').trim().to_string()
        } else {
            "/bin/sh".to_string()
        };

        Some((phase, interpreter))
    }

    /// Extract file list from RPM package with detailed metadata
    fn extract_files(pkg: &Package) -> Vec<PackageFile> {
        let mut files = Vec::new();

        // Use get_file_entries() to get complete file metadata
        if let Ok(file_entries) = pkg.metadata.get_file_entries() {
            for entry in file_entries {
                // FileDigest can be formatted as hex string
                let sha256 = entry.digest.as_ref().map(|d| format!("{}", d));

                files.push(PackageFile {
                    path: entry.path.to_string_lossy().to_string(),
                    size: entry.size as i64,
                    mode: entry.mode.raw_mode() as i32,
                    sha256,
                });
            }
        }

        files
    }

    /// Extract config files from RPM package using rpm command
    ///
    /// Uses `rpm -qpc` to list config files with their flags.
    /// RPM config file flags:
    /// - %config - regular config file
    /// - %config(noreplace) - preserve user's version on upgrade
    /// - %config(missingok) - don't complain if missing
    /// - %ghost - file not in payload, just tracked
    fn extract_config_files(path: &std::path::Path) -> Vec<ConfigFileInfo> {
        use std::process::Command;

        // Query config files with flags: %{FILEFLAGS:fflags} gives us c=config, n=noreplace, g=ghost
        // Format: path|flags
        let output = match Command::new("rpm")
            .args(["-qpc", "--qf", "[%{FILENAMES}|%{FILEFLAGS:fflags}\n]"])
            .arg(path)
            .output()
        {
            Ok(o) => o,
            Err(e) => {
                debug!("Failed to run rpm -qpc: {}", e);
                return Vec::new();
            }
        };

        if !output.status.success() {
            // No config files or error - try simple listing
            let simple_output = match Command::new("rpm")
                .args(["-qpc"])
                .arg(path)
                .output()
            {
                Ok(o) => o,
                Err(_) => return Vec::new(),
            };

            if !simple_output.status.success() {
                return Vec::new();
            }

            // Simple listing without flags
            return String::from_utf8_lossy(&simple_output.stdout)
                .lines()
                .filter(|line| !line.is_empty() && line.starts_with('/'))
                .map(|path| ConfigFileInfo {
                    path: path.to_string(),
                    noreplace: false,
                    ghost: false,
                })
                .collect();
        }

        let output_str = String::from_utf8_lossy(&output.stdout);
        let mut config_files = Vec::new();

        for line in output_str.lines() {
            if line.is_empty() {
                continue;
            }

            // Parse "path|flags" format
            if let Some((path, flags)) = line.split_once('|') {
                if path.is_empty() || !path.starts_with('/') {
                    continue;
                }

                // Check if this is a config file (has 'c' flag)
                if flags.contains('c') {
                    config_files.push(ConfigFileInfo {
                        path: path.to_string(),
                        noreplace: flags.contains('n'),
                        ghost: flags.contains('g'),
                    });
                }
            } else if line.starts_with('/') {
                // Simple path without flags - assume regular config
                config_files.push(ConfigFileInfo {
                    path: line.to_string(),
                    noreplace: false,
                    ghost: false,
                });
            }
        }

        config_files
    }

    /// Extract dependencies from RPM package
    fn extract_dependencies(pkg: &Package) -> Vec<Dependency> {
        let mut deps = Vec::new();

        // Extract runtime dependencies (Requires)
        if let Ok(requires) = pkg.metadata.get_requires() {
            for req in requires {
                // Skip rpmlib dependencies and file paths
                if req.name.starts_with("rpmlib(") || req.name.starts_with('/') {
                    continue;
                }

                // Convert DependencyFlags to constraint string with operator
                let version = if !req.version.is_empty() {
                    let operator = flags_to_operator(req.flags);
                    Some(format!("{}{}", operator, req.version))
                } else {
                    None
                };

                deps.push(Dependency {
                    name: req.name.to_string(),
                    version,
                    dep_type: DependencyType::Runtime,
                    description: None,
                });
            }
        }

        deps
    }
}

/// Convert RPM DependencyFlags to constraint operator string
fn flags_to_operator(flags: rpm::DependencyFlags) -> &'static str {
    use rpm::DependencyFlags;

    // Check for combined flags first
    if flags.contains(DependencyFlags::LESS) && flags.contains(DependencyFlags::EQUAL) {
        "<= "
    } else if flags.contains(DependencyFlags::GREATER) && flags.contains(DependencyFlags::EQUAL) {
        ">= "
    } else if flags.contains(DependencyFlags::LESS) {
        "< "
    } else if flags.contains(DependencyFlags::GREATER) {
        "> "
    } else if flags.contains(DependencyFlags::EQUAL) {
        "= "
    } else {
        // No comparison flags (ANY) - return empty
        ""
    }
}

impl PackageFormat for RpmPackage {
    fn parse(path: &str) -> Result<Self> {
        debug!("Parsing RPM package: {}", path);

        let file = File::open(path)
            .map_err(|e| Error::InitError(format!("Failed to open RPM file: {}", e)))?;

        let mut buf_reader = BufReader::new(file);

        let pkg = Package::parse(&mut buf_reader)
            .map_err(|e| Error::InitError(format!("Failed to parse RPM: {}", e)))?;

        // Extract basic metadata
        let name = pkg
            .metadata
            .get_name()
            .map_err(|e| Error::InitError(format!("Failed to get package name: {}", e)))?
            .to_string();

        let version = pkg
            .metadata
            .get_version()
            .map_err(|e| Error::InitError(format!("Failed to get package version: {}", e)))?
            .to_string();

        let architecture = pkg.metadata.get_arch().ok().map(|s| s.to_string());
        let description = pkg.metadata.get_description().ok().map(|s| s.to_string());

        // Extract provenance information
        let source_rpm = pkg.metadata.get_source_rpm().ok().map(|s| s.to_string());
        let build_host = pkg.metadata.get_build_host().ok().map(|s| s.to_string());
        let vendor = pkg.metadata.get_vendor().ok().map(|s| s.to_string());
        let license = pkg.metadata.get_license().ok().map(|s| s.to_string());
        let url = pkg.metadata.get_url().ok().map(|s| s.to_string());

        let files = Self::extract_files(&pkg);
        let dependencies = Self::extract_dependencies(&pkg);

        // Extract scriptlets and config files using rpm command
        let scriptlets = Self::extract_scriptlets(std::path::Path::new(path));
        let config_files = Self::extract_config_files(std::path::Path::new(path));

        debug!(
            "Parsed RPM: {} version {} ({} files, {} dependencies, {} scriptlets, {} config files)",
            name,
            version,
            files.len(),
            dependencies.len(),
            scriptlets.len(),
            config_files.len()
        );

        let meta = PackageMetadata {
            package_path: PathBuf::from(path),
            name,
            version,
            architecture,
            description,
            files,
            dependencies,
            scriptlets,
            config_files,
        };

        Ok(Self {
            meta,
            source_rpm,
            build_host,
            vendor,
            license,
            url,
        })
    }

    fn name(&self) -> &str {
        self.meta.name()
    }

    fn version(&self) -> &str {
        self.meta.version()
    }

    fn architecture(&self) -> Option<&str> {
        self.meta.architecture()
    }

    fn description(&self) -> Option<&str> {
        self.meta.description()
    }

    fn files(&self) -> &[PackageFile] {
        self.meta.files()
    }

    fn dependencies(&self) -> &[Dependency] {
        self.meta.dependencies()
    }

    fn extract_file_contents(&self) -> Result<Vec<ExtractedFile>> {
        use std::process::Command;
        use tempfile::TempDir;

        debug!(
            "Extracting file contents from RPM: {:?}",
            self.meta.package_path()
        );

        // Create temp directory for extraction
        let temp_dir = TempDir::new()
            .map_err(|e| Error::InitError(format!("Failed to create temp dir: {}", e)))?;

        // Extract RPM to temp directory using rpm2cpio | cpio
        // rpm2cpio package.rpm | cpio -idmv -D /tmp/extract
        let rpm2cpio_output = Command::new("rpm2cpio")
            .arg(self.meta.package_path())
            .output()
            .map_err(|e| {
                Error::InitError(format!(
                    "Failed to run rpm2cpio: {}. Is rpm2cpio installed?",
                    e
                ))
            })?;

        if !rpm2cpio_output.status.success() {
            return Err(Error::InitError(format!(
                "rpm2cpio failed: {}",
                String::from_utf8_lossy(&rpm2cpio_output.stderr)
            )));
        }

        // Extract cpio archive
        let cpio_status = Command::new("cpio")
            .args(["-idm", "--quiet"])
            .current_dir(temp_dir.path())
            .stdin(std::process::Stdio::piped())
            .spawn()
            .and_then(|mut child| {
                use std::io::Write;
                if let Some(stdin) = child.stdin.as_mut() {
                    stdin.write_all(&rpm2cpio_output.stdout)?;
                }
                child.wait()
            })
            .map_err(|e| {
                Error::InitError(format!("Failed to run cpio: {}. Is cpio installed?", e))
            })?;

        if !cpio_status.success() {
            return Err(Error::InitError("cpio extraction failed".to_string()));
        }

        // Read extracted files
        let mut extracted_files = Vec::new();

        for file_meta in self.meta.files() {
            let full_path = temp_dir.path().join(file_meta.path.trim_start_matches('/'));

            // Skip if not a regular file (directory, symlink, etc.)
            if !full_path.is_file() {
                continue;
            }

            // Read file content
            let content = std::fs::read(&full_path)
                .map_err(|e| Error::InitError(format!("Failed to read {}: {}", file_meta.path, e)))?;

            extracted_files.push(ExtractedFile {
                path: file_meta.path.clone(),
                content,
                size: file_meta.size,
                mode: file_meta.mode,
                sha256: file_meta.sha256.clone(),
            });
        }

        debug!("Extracted {} files from RPM", extracted_files.len());
        Ok(extracted_files)
    }

    fn to_trove(&self) -> Trove {
        self.meta.to_trove()
    }

    fn scriptlets(&self) -> Vec<Scriptlet> {
        self.meta.scriptlets()
    }

    fn config_files(&self) -> Vec<ConfigFileInfo> {
        self.meta.config_files()
    }
}

impl RpmPackage {
    /// Get source RPM name (for provenance tracking)
    pub fn source_rpm(&self) -> Option<&str> {
        self.source_rpm.as_deref()
    }

    /// Get build host (for provenance tracking)
    pub fn build_host(&self) -> Option<&str> {
        self.build_host.as_deref()
    }

    /// Get vendor information
    pub fn vendor(&self) -> Option<&str> {
        self.vendor.as_deref()
    }

    /// Get license information
    pub fn license(&self) -> Option<&str> {
        self.license.as_deref()
    }

    /// Get upstream URL
    pub fn url(&self) -> Option<&str> {
        self.url.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rpm_package_structure() {
        // Verify the struct is properly defined
        assert!(std::mem::size_of::<RpmPackage>() > 0);
    }

    #[test]
    fn test_package_format_trait_implemented() {
        // Verify RpmPackage implements PackageFormat trait
        // This test ensures the trait is correctly implemented at compile time
        fn assert_implements_package_format<T: PackageFormat>() {}
        assert_implements_package_format::<RpmPackage>();
    }

    #[test]
    fn test_to_trove_conversion() {
        // Create a minimal RpmPackage for testing
        let rpm = RpmPackage {
            meta: PackageMetadata::new(
                PathBuf::from("/fake/path.rpm"),
                "test-package".to_string(),
                "1.0.0".to_string(),
            ),
            source_rpm: Some("test-package-1.0.0.src.rpm".to_string()),
            build_host: Some("buildhost.example.com".to_string()),
            vendor: Some("Test Vendor".to_string()),
            license: Some("MIT".to_string()),
            url: Some("https://example.com".to_string()),
        };

        let trove = rpm.to_trove();

        assert_eq!(trove.name, "test-package");
        assert_eq!(trove.version, "1.0.0");
    }

    #[test]
    fn test_provenance_accessors() {
        let rpm = RpmPackage {
            meta: PackageMetadata::new(
                PathBuf::from("/fake/test.rpm"),
                "test".to_string(),
                "1.0".to_string(),
            ),
            source_rpm: Some("test-1.0.src.rpm".to_string()),
            build_host: Some("builder".to_string()),
            vendor: Some("Vendor".to_string()),
            license: Some("GPL".to_string()),
            url: Some("https://test.com".to_string()),
        };

        assert_eq!(rpm.source_rpm(), Some("test-1.0.src.rpm"));
        assert_eq!(rpm.build_host(), Some("builder"));
        assert_eq!(rpm.vendor(), Some("Vendor"));
        assert_eq!(rpm.license(), Some("GPL"));
        assert_eq!(rpm.url(), Some("https://test.com"));
    }

    #[test]
    fn test_parse_nonexistent_file() {
        // Test that parsing a nonexistent file returns an error
        let result = RpmPackage::parse("/nonexistent/file.rpm");
        assert!(result.is_err());
    }

    #[test]
    fn test_dependency_type_variants() {
        // Ensure all DependencyType variants are accessible
        let runtime = DependencyType::Runtime;
        let build = DependencyType::Build;
        let optional = DependencyType::Optional;

        assert_eq!(runtime, DependencyType::Runtime);
        assert_eq!(build, DependencyType::Build);
        assert_eq!(optional, DependencyType::Optional);
    }

    #[test]
    fn test_parse_scriptlet_header() {
        // Test preinstall with shell
        let result = RpmPackage::parse_scriptlet_header("preinstall scriptlet (using /bin/sh):");
        assert!(result.is_some());
        let (phase, interp) = result.unwrap();
        assert_eq!(phase, ScriptletPhase::PreInstall);
        assert_eq!(interp, "/bin/sh");

        // Test postinstall with lua
        let result = RpmPackage::parse_scriptlet_header("postinstall program: /usr/bin/lua");
        assert!(result.is_some());
        let (phase, interp) = result.unwrap();
        assert_eq!(phase, ScriptletPhase::PostInstall);
        assert_eq!(interp, "/usr/bin/lua");

        // Test preuninstall
        let result = RpmPackage::parse_scriptlet_header("preuninstall scriptlet (using /bin/bash):");
        assert!(result.is_some());
        let (phase, interp) = result.unwrap();
        assert_eq!(phase, ScriptletPhase::PreRemove);
        assert_eq!(interp, "/bin/bash");

        // Test postuninstall
        let result = RpmPackage::parse_scriptlet_header("postuninstall scriptlet (using /bin/sh):");
        assert!(result.is_some());
        let (phase, interp) = result.unwrap();
        assert_eq!(phase, ScriptletPhase::PostRemove);
        assert_eq!(interp, "/bin/sh");

        // Test non-scriptlet line
        let result = RpmPackage::parse_scriptlet_header("echo hello");
        assert!(result.is_none());
    }

    #[test]
    fn test_parse_rpm_scripts() {
        let output = r#"preinstall scriptlet (using /bin/sh):
echo "Installing package"
mkdir -p /var/lib/myapp

postinstall scriptlet (using /bin/sh):
systemctl daemon-reload
systemctl enable myapp

preuninstall scriptlet (using /bin/sh):
systemctl stop myapp
systemctl disable myapp
"#;

        let scriptlets = RpmPackage::parse_rpm_scripts(output);
        assert_eq!(scriptlets.len(), 3);

        // Check preinstall
        assert_eq!(scriptlets[0].phase, ScriptletPhase::PreInstall);
        assert_eq!(scriptlets[0].interpreter, "/bin/sh");
        assert!(scriptlets[0].content.contains("Installing package"));
        assert!(scriptlets[0].content.contains("mkdir -p"));

        // Check postinstall
        assert_eq!(scriptlets[1].phase, ScriptletPhase::PostInstall);
        assert!(scriptlets[1].content.contains("daemon-reload"));

        // Check preuninstall
        assert_eq!(scriptlets[2].phase, ScriptletPhase::PreRemove);
        assert!(scriptlets[2].content.contains("stop myapp"));
    }
}
