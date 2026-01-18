// src/ccs/convert/converter.rs
//! Legacy package to CCS format converter
//!
//! Takes `PackageMetadata` + extracted files and builds a `CcsManifest` with
//! component classification, invoking `CcsBuilder` with optional CDC chunking.

use crate::ccs::builder::{write_ccs_package, BuildResult, CcsBuilder};
use crate::ccs::convert::analyzer::ScriptletAnalyzer;
use crate::ccs::convert::capture::ScriptletCapturer;
use crate::ccs::convert::fidelity::{FidelityLevel, FidelityReport};
use crate::ccs::convert::mock::CapturedIntent;
use crate::ccs::manifest::{
    Capability, CcsManifest, Components, Config, Hooks, Package, PackageDep, Platform, Provides,
    Requires, Suggests, User, Group, Service, ServiceAction,
};
use crate::ccs::policy::BuildPolicyConfig;
use crate::packages::common::PackageMetadata;
use crate::packages::traits::{DependencyType, ExtractedFile, ScriptletPhase};
use std::path::{Path, PathBuf};
use tempfile::TempDir;

/// Options for legacy package conversion
#[derive(Debug, Clone)]
pub struct ConversionOptions {
    /// Enable CDC chunking for file content (better dedup, slower)
    pub enable_chunking: bool,
    /// Output directory for the converted package
    pub output_dir: PathBuf,
    /// Automatically classify files into components
    pub auto_classify: bool,
    /// Minimum fidelity level to proceed (warns below this)
    pub min_fidelity: FidelityLevel,
    /// Enable scriptlet capture (unsafe execution in sandbox)
    pub capture_scriptlets: bool,
}

impl Default for ConversionOptions {
    fn default() -> Self {
        Self {
            enable_chunking: true,
            output_dir: PathBuf::from("./target/ccs"),
            auto_classify: true,
            min_fidelity: FidelityLevel::High,
            capture_scriptlets: true, // Default to capture mode for safety
        }
    }
}

/// Result of converting a legacy package
#[derive(Debug)]
pub struct ConversionResult {
    /// The build result from CcsBuilder
    pub build_result: BuildResult,
    /// Path to the output CCS package (if written)
    pub package_path: Option<PathBuf>,
    /// Fidelity report for the conversion
    pub fidelity: FidelityReport,
    /// Original package format
    pub original_format: String,
    /// Original package checksum (for dedup/skip)
    pub original_checksum: String,
    /// Detected hooks extracted from scriptlets
    pub detected_hooks: Hooks,
}

/// Converts legacy packages (RPM/DEB/Arch) to CCS format
pub struct LegacyConverter {
    options: ConversionOptions,
    analyzer: ScriptletAnalyzer,
}

impl LegacyConverter {
    /// Create a new converter with the given options
    pub fn new(options: ConversionOptions) -> Self {
        Self {
            options,
            analyzer: ScriptletAnalyzer::new(),
        }
    }

    /// Create a converter with default options
    pub fn with_defaults() -> Self {
        Self::new(ConversionOptions::default())
    }

    /// Convert a legacy package to CCS format
    ///
    /// # Arguments
    /// * `metadata` - Package metadata from the legacy parser
    /// * `files` - Extracted file contents
    /// * `format` - Original format ("rpm", "deb", "arch")
    /// * `checksum` - Original package file checksum
    ///
    /// # Returns
    /// A `ConversionResult` containing the CCS build result and fidelity report
    pub fn convert(
        &self,
        metadata: &PackageMetadata,
        files: &[ExtractedFile],
        format: &str,
        checksum: &str,
    ) -> Result<ConversionResult, ConversionError> {
        let mut final_metadata = metadata.clone();
        let mut final_files = files.to_vec();
        let mut captured_hooks = Hooks::default();
        let mut processed_scriptlets = Vec::new();

        // Step 0: Capture scriptlets if enabled
        if self.options.capture_scriptlets {
            let mut capturer = ScriptletCapturer::new()
                .map_err(|e| ConversionError::IoError(format!("Failed to init capturer: {}", e)))?;

            for script in &metadata.scriptlets {
                // We only capture post-install for now as it's the most common for state setup
                if script.phase == ScriptletPhase::PostInstall {
                    tracing::info!("Capturing PostInstall scriptlet for {}", metadata.name);
                    
                    let result = capturer.capture(&script.content, &script.interpreter, files)
                        .map_err(|e| ConversionError::BuildError(format!("Capture failed: {}", e)))?;

                    // Add new files
                    final_files.extend(result.new_files);

                    // Convert intents to hooks
                    for intent in result.intents {
                        match intent {
                            CapturedIntent::UserAdd(args) => {
                                // Simplified: assume args[0] is user name if no flags, or parse flags
                                // This is a placeholder for real arg parsing
                                if let Some(name) = args.last() {
                                     captured_hooks.users.push(User {
                                         name: name.clone(),
                                         uid: None,
                                         gid: None,
                                         groups: vec![],
                                         home: None,
                                         shell: None,
                                         system: true,
                                     });
                                }
                            },
                            CapturedIntent::SystemdEnable(svc) => {
                                captured_hooks.services.push(Service {
                                    name: svc,
                                    action: ServiceAction::Enable,
                                });
                            },
                            CapturedIntent::SystemdDisable(svc) => {
                                captured_hooks.services.push(Service {
                                    name: svc,
                                    action: ServiceAction::Disable,
                                });
                            },
                            _ => tracing::debug!("Ignored captured intent: {:?}", intent),
                        }
                    }
                    // Mark as processed (don't include in final metadata)
                    continue; 
                }
                processed_scriptlets.push(script.clone());
            }
            final_metadata.scriptlets = processed_scriptlets;
        }

        // Step 1: Analyze scriptlets (remaining ones) to extract declarative hooks
        let (detected_hooks_list, fidelity) = self.analyzer.analyze(&final_metadata.scriptlets);
        let mut detected_hooks = ScriptletAnalyzer::build_hooks(&detected_hooks_list);

        // Merge captured hooks
        detected_hooks.users.extend(captured_hooks.users);
        detected_hooks.services.extend(captured_hooks.services);
        // TODO: Merge other hooks

        // Step 2: Check fidelity threshold
        if fidelity.level < self.options.min_fidelity && fidelity.level.requires_warning() {
            tracing::warn!(
                "Conversion fidelity {} is below threshold {}. {}",
                fidelity.level,
                self.options.min_fidelity,
                fidelity.level.description()
            );
        }

        // Step 3: Build CCS manifest from metadata
        let manifest = self.build_manifest(&final_metadata, &detected_hooks)?;

        // Step 4: Create temporary directory with file structure
        let temp_dir = TempDir::new()
            .map_err(|e| ConversionError::IoError(format!("Failed to create temp dir: {}", e)))?;

        // Write files to temp directory
        self.write_files_to_temp(&final_files, temp_dir.path())?;

        // Write manifest
        let manifest_path = temp_dir.path().join("ccs.toml");
        let manifest_toml = toml::to_string_pretty(&manifest)
            .map_err(|e| ConversionError::ManifestError(format!("Failed to serialize manifest: {}", e)))?;
        std::fs::write(&manifest_path, manifest_toml)
            .map_err(|e| ConversionError::IoError(format!("Failed to write manifest: {}", e)))?;

        // Step 5: Build CCS package using CcsBuilder
        let mut builder = CcsBuilder::new(manifest.clone(), temp_dir.path());

        if self.options.enable_chunking {
            builder = builder.with_chunking();
        }

        let build_result = builder.build()
            .map_err(|e| ConversionError::BuildError(format!("CCS build failed: {}", e)))?;

        // Step 6: Write the package file
        std::fs::create_dir_all(&self.options.output_dir)
            .map_err(|e| ConversionError::IoError(format!("Failed to create output dir: {}", e)))?;

        let package_filename = format!(
            "{}-{}.ccs",
            build_result.manifest.package.name,
            build_result.manifest.package.version
        );
        let package_path = self.options.output_dir.join(&package_filename);

        write_ccs_package(&build_result, &package_path)
            .map_err(|e| ConversionError::BuildError(format!("Failed to write package: {}", e)))?;

        Ok(ConversionResult {
            build_result,
            package_path: Some(package_path),
            fidelity,
            original_format: format.to_string(),
            original_checksum: checksum.to_string(),
            detected_hooks,
        })
    }

    /// Build a CCS manifest from legacy package metadata
    fn build_manifest(
        &self,
        metadata: &PackageMetadata,
        hooks: &Hooks,
    ) -> Result<CcsManifest, ConversionError> {
        // Build platform info
        let platform = metadata.architecture.as_ref().map(|arch| Platform {
            os: "linux".to_string(),
            arch: Some(arch.clone()),
            libc: "gnu".to_string(),
            abi: None,
        });

        // Convert dependencies to capabilities and packages
        let mut capabilities = Vec::new();
        let mut packages = Vec::new();

        for dep in &metadata.dependencies {
            if dep.dep_type == DependencyType::Runtime {
                if let Some(ref ver) = dep.version {
                    capabilities.push(Capability::Versioned {
                        name: dep.name.clone(),
                        version: ver.clone(),
                    });
                } else {
                    packages.push(PackageDep {
                        name: dep.name.clone(),
                        version: None,
                    });
                }
            }
        }

        // Build config file list
        let config_files: Vec<String> = metadata.config_files
            .iter()
            .map(|c| c.path.clone())
            .collect();

        let manifest = CcsManifest {
            package: Package {
                name: metadata.name.clone(),
                version: metadata.version.clone(),
                description: metadata.description.clone().unwrap_or_else(|| {
                    format!("Converted from {} package", metadata.package_path.display())
                }),
                license: None,
                homepage: None,
                repository: None,
                platform,
                authors: None,
            },
            provides: Provides::default(),
            requires: Requires {
                capabilities,
                packages,
            },
            suggests: Suggests::default(),
            components: Components::default(),
            hooks: hooks.clone(),
            config: Config {
                files: config_files,
                noreplace: true,
            },
            build: None,
            legacy: None,
            policy: BuildPolicyConfig::default(),
        };

        Ok(manifest)
    }

    /// Write extracted files to a temporary directory
    fn write_files_to_temp(&self, files: &[ExtractedFile], temp_dir: &Path) -> Result<(), ConversionError> {
        for file in files {
            // Create relative path from absolute path
            let rel_path = file.path.strip_prefix('/').unwrap_or(&file.path);
            let full_path = temp_dir.join(rel_path);

            // Create parent directories
            if let Some(parent) = full_path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| ConversionError::IoError(format!("Failed to create directory: {}", e)))?;
            }

            // Write file content
            std::fs::write(&full_path, &file.content)
                .map_err(|e| ConversionError::IoError(format!("Failed to write file {}: {}", file.path, e)))?;

            // Set permissions (best effort on Unix)
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&full_path, std::fs::Permissions::from_mode(file.mode as u32));
            }
        }

        Ok(())
    }
}

/// Errors that can occur during conversion
#[derive(Debug, thiserror::Error)]
pub enum ConversionError {
    #[error("I/O error: {0}")]
    IoError(String),

    #[error("Manifest error: {0}")]
    ManifestError(String),

    #[error("Build error: {0}")]
    BuildError(String),

    #[error("Fidelity too low: {0}")]
    FidelityTooLow(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packages::traits::{Dependency, PackageFile, Scriptlet, ScriptletPhase};

    fn make_test_metadata() -> PackageMetadata {
        PackageMetadata {
            package_path: PathBuf::from("/tmp/test-1.0.0.rpm"),
            name: "test-package".to_string(),
            version: "1.0.0".to_string(),
            architecture: Some("x86_64".to_string()),
            description: Some("Test package".to_string()),
            files: vec![
                PackageFile {
                    path: "/usr/bin/test".to_string(),
                    size: 100,
                    mode: 0o755,
                    sha256: Some("abc123".to_string()),
                },
            ],
            dependencies: vec![
                Dependency {
                    name: "libc".to_string(),
                    version: Some(">= 2.17".to_string()),
                    dep_type: DependencyType::Runtime,
                    description: None,
                },
            ],
            scriptlets: vec![
                Scriptlet {
                    phase: ScriptletPhase::PreInstall,
                    interpreter: "/bin/sh".to_string(),
                    content: "getent passwd testuser || useradd -r testuser".to_string(),
                    flags: None,
                },
            ],
            config_files: vec![],
        }
    }

    fn make_test_files() -> Vec<ExtractedFile> {
        vec![
            ExtractedFile {
                path: "/usr/bin/test".to_string(),
                content: b"#!/bin/sh\necho test".to_vec(),
                size: 20,
                mode: 0o755,
                sha256: Some("abc123".to_string()),
            },
        ]
    }

    #[test]
    fn test_conversion_options_default() {
        let options = ConversionOptions::default();
        assert!(options.enable_chunking);
        assert!(options.auto_classify);
        assert_eq!(options.min_fidelity, FidelityLevel::High);
    }

    #[test]
    fn test_converter_creation() {
        let converter = LegacyConverter::with_defaults();
        assert!(converter.options.enable_chunking);
    }

    #[test]
    fn test_build_manifest() {
        let options = ConversionOptions {
            enable_chunking: false,
            output_dir: PathBuf::from("/tmp/test"),
            auto_classify: true,
            min_fidelity: FidelityLevel::Low,
        };
        let converter = LegacyConverter::new(options);

        let metadata = make_test_metadata();

        let (detected, _) = converter.analyzer.analyze(&metadata.scriptlets);
        let hooks = ScriptletAnalyzer::build_hooks(&detected);

        let manifest = converter.build_manifest(&metadata, &hooks).unwrap();

        assert_eq!(manifest.package.name, "test-package");
        assert_eq!(manifest.package.version, "1.0.0");

        // Should have extracted user hook
        assert!(!manifest.hooks.users.is_empty());
        assert_eq!(manifest.hooks.users[0].name, "testuser");
    }

    #[test]
    fn test_scriptlet_analysis_for_conversion() {
        let converter = LegacyConverter::with_defaults();
        let metadata = make_test_metadata();

        let (hooks, report) = converter.analyzer.analyze(&metadata.scriptlets);

        // Should detect useradd
        assert!(hooks.iter().any(|h| matches!(h, super::super::analyzer::DetectedHook::User(u) if u.name == "testuser")));

        // Should have good fidelity (simple scripts)
        assert!(report.level >= FidelityLevel::High);

        // Should preserve scriptlet
        assert_eq!(report.scriptlets_preserved, 1);
    }

    #[test]
    fn test_write_files_to_temp() {
        let converter = LegacyConverter::with_defaults();
        let files = make_test_files();

        let temp_dir = TempDir::new().unwrap();
        converter.write_files_to_temp(&files, temp_dir.path()).unwrap();

        // Check files were written
        assert!(temp_dir.path().join("usr/bin/test").exists());

        // Check content
        let content = std::fs::read(temp_dir.path().join("usr/bin/test")).unwrap();
        assert_eq!(content, b"#!/bin/sh\necho test");
    }
}
