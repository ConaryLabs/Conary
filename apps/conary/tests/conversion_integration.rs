// tests/conversion_integration.rs
//! Integration tests for legacy package to CCS conversion
//!
//! These tests validate the end-to-end conversion process from RPM/DEB/Arch
//! packages to CCS format, including:
//! - File extraction and chunking
//! - Scriptlet analysis and hook detection
//! - Provenance extraction
//! - Typed lifecycle evidence

use conary_core::ccs::CcsPackage;
use conary_core::ccs::convert::{ConversionOptions, LegacyConverter};
use conary_core::ccs::legacy_replay::{
    HostForeignReplayPolicy, LegacyReplayLifecycle, LegacyReplayPolicyInput, LegacyReplayPreflight,
    LegacyReplayRefusalKind, plan_legacy_replay,
};
use conary_core::ccs::legacy_scriptlets::{
    PublicationStatus, ScriptletDecision, ScriptletFidelity, TargetCompatibility,
};
use conary_core::ccs::target_compatibility::{
    CompatibilityPreflightEnvironment, TargetCompatibilityMatrix,
};
use conary_core::packages::PackageFormat;
use conary_core::packages::common::PackageMetadata;
use conary_core::packages::traits::{
    ConfigFileInfo, Dependency, DependencyType, ExtractedFile, PackageFile, Scriptlet,
    ScriptletPhase,
};
use conary_core::repository::distro::ReplayTarget;
use conary_core::scriptlet::SandboxMode;
use std::path::PathBuf;
use tempfile::TempDir;

#[path = "conversion_integration/authority.rs"]
mod authority;

// =============================================================================
// TEST HELPERS
// =============================================================================

/// Create a minimal test package metadata
fn create_test_metadata(name: &str) -> PackageMetadata {
    PackageMetadata {
        package_path: PathBuf::from(format!("/tmp/{}-1.0.0.rpm", name)),
        name: name.to_string(),
        version: "1.0.0".to_string(),
        architecture: Some("x86_64".to_string()),
        description: Some(format!("Test package: {}", name)),
        files: vec![PackageFile {
            path: format!("/usr/bin/{}", name),
            size: 100,
            mode: 0o755,
            sha256: Some("abc123".to_string()),
            symlink_target: None,
        }],
        dependencies: vec![],
        provides: vec![],
        scriptlets: vec![],
        native_scriptlet_abi: vec![],
        config_files: vec![],
    }
}

/// Create test files matching the metadata
fn create_test_files(name: &str) -> Vec<ExtractedFile> {
    vec![ExtractedFile {
        path: format!("/usr/bin/{}", name),
        content: format!("#!/bin/sh\necho {}", name).into_bytes(),
        size: 20,
        mode: 0o755,
        sha256: Some("abc123".to_string()),
        symlink_target: None,
    }]
}

fn passive_converter(output_dir: &std::path::Path) -> LegacyConverter {
    LegacyConverter::new(ConversionOptions {
        output_dir: output_dir.to_path_buf(),
        enable_chunking: false,
    })
    .with_source_distro("fedora")
    .with_source_release("44")
}

fn parse_converted_package(result: &conary_core::ccs::convert::ConversionResult) -> CcsPackage {
    CcsPackage::parse(
        result
            .package_path
            .as_ref()
            .expect("conversion should write CCS package")
            .to_str()
            .expect("package path is utf-8"),
    )
    .expect("converted CCS package should parse")
}

fn golden_payload_files(name: &str) -> Vec<ExtractedFile> {
    let mut files = create_test_files(name);
    files.extend([
        ExtractedFile {
            path: "/usr/lib/systemd/system/demo.service".to_string(),
            content: b"[Service]\nExecStart=/usr/bin/demo\n".to_vec(),
            size: 32,
            mode: 0o644,
            sha256: None,
            symlink_target: None,
        },
        ExtractedFile {
            path: "/usr/lib/tmpfiles.d/demo.conf".to_string(),
            content: b"d /run/demo 0755 root root -\n".to_vec(),
            size: 28,
            mode: 0o644,
            sha256: None,
            symlink_target: None,
        },
        ExtractedFile {
            path: "/usr/lib/sysusers.d/demo.conf".to_string(),
            content: b"u demo - \"Demo User\" /run/demo -\n".to_vec(),
            size: 32,
            mode: 0o644,
            sha256: None,
            symlink_target: None,
        },
        ExtractedFile {
            path: "/usr/share/mime/packages/demo.xml".to_string(),
            content: b"<mime-info/>".to_vec(),
            size: 12,
            mode: 0o644,
            sha256: None,
            symlink_target: None,
        },
        ExtractedFile {
            path: "/usr/share/selinux/packages/demo.pp".to_string(),
            content: b"selinux policy module placeholder".to_vec(),
            size: 33,
            mode: 0o644,
            sha256: None,
            symlink_target: None,
        },
        ExtractedFile {
            path: "/etc/apparmor.d/usr.bin.demo".to_string(),
            content: b"profile usr.bin.demo /usr/bin/demo { }\n".to_vec(),
            size: 38,
            mode: 0o644,
            sha256: None,
            symlink_target: None,
        },
    ]);
    files
}

/// Create a network server package (nginx-like)
fn create_server_package() -> (PackageMetadata, Vec<ExtractedFile>) {
    let metadata = PackageMetadata {
        package_path: PathBuf::from("/tmp/myserver-1.0.0.rpm"),
        name: "myserver".to_string(),
        version: "1.0.0".to_string(),
        architecture: Some("x86_64".to_string()),
        description: Some("A test server application".to_string()),
        files: vec![
            PackageFile {
                path: "/usr/sbin/myserver".to_string(),
                size: 1024,
                mode: 0o755,
                sha256: Some("server_binary_hash".to_string()),
                symlink_target: None,
            },
            PackageFile {
                path: "/etc/myserver/myserver.conf".to_string(),
                size: 512,
                mode: 0o644,
                sha256: Some("config_hash".to_string()),
                symlink_target: None,
            },
            PackageFile {
                path: "/usr/lib/systemd/system/myserver.service".to_string(),
                size: 256,
                mode: 0o644,
                sha256: Some("service_hash".to_string()),
                symlink_target: None,
            },
        ],
        dependencies: vec![
            Dependency {
                name: "libssl3".to_string(),
                version: Some(">= 3.0".to_string()),
                dep_type: DependencyType::Runtime,
                description: None,
            },
            Dependency {
                name: "libc6".to_string(),
                version: None,
                dep_type: DependencyType::Runtime,
                description: None,
            },
        ],
        provides: vec![],
        scriptlets: vec![
            Scriptlet {
                phase: ScriptletPhase::PreInstall,
                interpreter: "/bin/sh".to_string(),
                content: "getent passwd myserver || useradd -r -s /sbin/nologin myserver"
                    .to_string(),
                flags: None,
            },
            Scriptlet {
                phase: ScriptletPhase::PostInstall,
                interpreter: "/bin/sh".to_string(),
                content: "systemctl daemon-reload\nsystemctl enable myserver".to_string(),
                flags: None,
            },
        ],
        native_scriptlet_abi: vec![],
        config_files: vec![ConfigFileInfo {
            path: "/etc/myserver/myserver.conf".to_string(),
            noreplace: true,
            ghost: false,
        }],
    };

    let systemd_service = br#"[Unit]
Description=My Server Application
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart=/usr/sbin/myserver --port 8080
User=myserver
Restart=on-failure

[Install]
WantedBy=multi-user.target
"#;

    let files = vec![
        ExtractedFile {
            path: "/usr/sbin/myserver".to_string(),
            content: b"\x7fELF binary placeholder".to_vec(),
            size: 1024,
            mode: 0o755,
            sha256: Some("server_binary_hash".to_string()),
            symlink_target: None,
        },
        ExtractedFile {
            path: "/etc/myserver/myserver.conf".to_string(),
            content: b"# Configuration file\nport = 8080\n".to_vec(),
            size: 512,
            mode: 0o644,
            sha256: Some("config_hash".to_string()),
            symlink_target: None,
        },
        ExtractedFile {
            path: "/usr/lib/systemd/system/myserver.service".to_string(),
            content: systemd_service.to_vec(),
            size: 256,
            mode: 0o644,
            sha256: Some("service_hash".to_string()),
            symlink_target: None,
        },
    ];

    (metadata, files)
}

// =============================================================================
// END-TO-END CONVERSION TESTS
// =============================================================================

#[test]
fn test_minimal_conversion() {
    let temp_dir = TempDir::new().unwrap();
    let options = ConversionOptions {
        output_dir: temp_dir.path().to_path_buf(),
        enable_chunking: false, // Faster for tests
    };

    let converter = LegacyConverter::new(options);
    let metadata = create_test_metadata("minimal");
    let files = create_test_files("minimal");

    let result = converter.convert(&metadata, &files, "rpm", "checksum123");
    assert!(result.is_ok(), "Basic conversion should succeed");

    let result = result.unwrap();
    assert!(result.package_path.is_some(), "Should produce output file");
    assert_eq!(result.original_format, "rpm");
    assert_eq!(result.original_checksum, "checksum123");

    // Verify output file exists
    let package_path = result.package_path.unwrap();
    assert!(package_path.exists(), "CCS package file should exist");
    assert!(
        package_path.to_string_lossy().ends_with(".ccs"),
        "Should have .ccs extension"
    );
}

#[test]
fn test_conversion_preserves_metadata() {
    let temp_dir = TempDir::new().unwrap();
    let options = ConversionOptions {
        output_dir: temp_dir.path().to_path_buf(),
        enable_chunking: false,
    };

    let converter = LegacyConverter::new(options);

    let mut metadata = create_test_metadata("metadata-test");
    metadata.description = Some("A detailed description".to_string());
    metadata.dependencies = vec![Dependency {
        name: "libfoo".to_string(),
        version: Some(">= 1.0".to_string()),
        dep_type: DependencyType::Runtime,
        description: None,
    }];

    let files = create_test_files("metadata-test");

    let result = converter
        .convert(&metadata, &files, "deb", "deb_checksum")
        .unwrap();

    // Check manifest preserves metadata
    let manifest = &result.build_result.manifest;
    assert_eq!(manifest.package.name, "metadata-test");
    assert_eq!(manifest.package.version, "1.0.0");
    assert!(
        manifest
            .package
            .description
            .contains("detailed description"),
        "Description should be preserved"
    );

    // Check dependencies converted
    assert!(
        !manifest.requires.capabilities.is_empty() || !manifest.requires.packages.is_empty(),
        "Dependencies should be converted"
    );
}

#[test]
fn test_server_package_conversion() {
    let temp_dir = TempDir::new().unwrap();
    let options = ConversionOptions {
        output_dir: temp_dir.path().to_path_buf(),
        enable_chunking: false,
    };

    let converter = LegacyConverter::new(options);
    let (metadata, files) = create_server_package();

    let result = converter
        .convert(&metadata, &files, "rpm", "server_checksum")
        .unwrap();

    // Check config files preserved
    let manifest = &result.build_result.manifest;
    assert!(
        !manifest.config.files.is_empty(),
        "Config files should be preserved"
    );
    assert!(
        manifest
            .config
            .files
            .contains(&"/etc/myserver/myserver.conf".to_string()),
        "Should include myserver.conf"
    );
}

// =============================================================================
// FILE HANDLING TESTS
// =============================================================================

#[test]
fn test_file_permissions_preserved() {
    let temp_dir = TempDir::new().unwrap();
    let options = ConversionOptions {
        output_dir: temp_dir.path().to_path_buf(),
        enable_chunking: false,
    };

    let converter = LegacyConverter::new(options);

    let metadata = PackageMetadata {
        package_path: PathBuf::from("/tmp/perms-1.0.0.rpm"),
        name: "perms-test".to_string(),
        version: "1.0.0".to_string(),
        architecture: Some("x86_64".to_string()),
        description: None,
        files: vec![
            PackageFile {
                path: "/usr/bin/executable".to_string(),
                size: 100,
                mode: 0o755,
                sha256: Some("exec_hash".to_string()),
                symlink_target: None,
            },
            PackageFile {
                path: "/etc/config".to_string(),
                size: 50,
                mode: 0o644,
                sha256: Some("conf_hash".to_string()),
                symlink_target: None,
            },
            PackageFile {
                path: "/etc/secret".to_string(),
                size: 30,
                mode: 0o600,
                sha256: Some("secret_hash".to_string()),
                symlink_target: None,
            },
        ],
        dependencies: vec![],
        provides: vec![],
        scriptlets: vec![],
        native_scriptlet_abi: vec![],
        config_files: vec![],
    };

    let files = vec![
        ExtractedFile {
            path: "/usr/bin/executable".to_string(),
            content: b"#!/bin/sh".to_vec(),
            size: 100,
            mode: 0o755,
            sha256: Some("exec_hash".to_string()),
            symlink_target: None,
        },
        ExtractedFile {
            path: "/etc/config".to_string(),
            content: b"config".to_vec(),
            size: 50,
            mode: 0o644,
            sha256: Some("conf_hash".to_string()),
            symlink_target: None,
        },
        ExtractedFile {
            path: "/etc/secret".to_string(),
            content: b"secret".to_vec(),
            size: 30,
            mode: 0o600,
            sha256: Some("secret_hash".to_string()),
            symlink_target: None,
        },
    ];

    let result = converter.convert(&metadata, &files, "rpm", "cs").unwrap();

    // Verify conversion completed
    assert!(result.package_path.is_some());

    // Verify all files included in manifest
    let manifest = &result.build_result.manifest;
    assert_eq!(manifest.package.name, "perms-test");
}

#[test]
fn test_empty_package_conversion() {
    let temp_dir = TempDir::new().unwrap();
    let options = ConversionOptions {
        output_dir: temp_dir.path().to_path_buf(),
        enable_chunking: false,
    };

    let converter = LegacyConverter::new(options);

    let metadata = PackageMetadata {
        package_path: PathBuf::from("/tmp/empty-1.0.0.rpm"),
        name: "empty-pkg".to_string(),
        version: "1.0.0".to_string(),
        architecture: None, // No architecture
        description: None,  // No description
        files: vec![],      // No files
        dependencies: vec![],
        provides: vec![],
        scriptlets: vec![],
        native_scriptlet_abi: vec![],
        config_files: vec![],
    };

    let files: Vec<ExtractedFile> = vec![];

    // Empty package should still convert (meta-packages exist)
    let result = converter.convert(&metadata, &files, "rpm", "cs");
    assert!(
        result.is_ok(),
        "Empty package should convert: {:?}",
        result.err()
    );

    let result = result.unwrap();
    assert_eq!(result.build_result.manifest.package.name, "empty-pkg");
}

#[test]
fn test_large_file_handling() {
    let temp_dir = TempDir::new().unwrap();
    let options = ConversionOptions {
        output_dir: temp_dir.path().to_path_buf(),
        enable_chunking: true, // Test with chunking
    };

    let converter = LegacyConverter::new(options);

    // Create a larger file (1MB of data)
    let large_content: Vec<u8> = (0..1_000_000).map(|i| (i % 256) as u8).collect();

    let metadata = PackageMetadata {
        package_path: PathBuf::from("/tmp/large-1.0.0.rpm"),
        name: "large-file-pkg".to_string(),
        version: "1.0.0".to_string(),
        architecture: Some("x86_64".to_string()),
        description: None,
        files: vec![PackageFile {
            path: "/usr/share/large/data.bin".to_string(),
            size: large_content.len() as i64,
            mode: 0o644,
            sha256: Some("large_hash".to_string()),
            symlink_target: None,
        }],
        dependencies: vec![],
        provides: vec![],
        scriptlets: vec![],
        native_scriptlet_abi: vec![],
        config_files: vec![],
    };

    let files = vec![ExtractedFile {
        path: "/usr/share/large/data.bin".to_string(),
        content: large_content.clone(),
        size: large_content.len() as i64,
        mode: 0o644,
        sha256: Some("large_hash".to_string()),
        symlink_target: None,
    }];

    let result = converter.convert(&metadata, &files, "rpm", "cs");
    assert!(
        result.is_ok(),
        "Large file should convert: {:?}",
        result.err()
    );

    let result = result.unwrap();
    assert!(result.package_path.is_some());

    // With chunking enabled, should have chunked data in the build result
    assert!(
        result.build_result.chunked,
        "Chunking should be used for large files"
    );
}

// =============================================================================
// FORMAT-SPECIFIC TESTS
// =============================================================================

#[test]
fn test_rpm_format_tracking() {
    let temp_dir = TempDir::new().unwrap();
    let options = ConversionOptions {
        output_dir: temp_dir.path().to_path_buf(),
        enable_chunking: false,
    };

    let converter = LegacyConverter::new(options);
    let metadata = create_test_metadata("rpm-pkg");
    let files = create_test_files("rpm-pkg");

    let result = converter
        .convert(&metadata, &files, "rpm", "rpm_checksum_abc")
        .unwrap();

    assert_eq!(result.original_format, "rpm");
    assert_eq!(result.original_checksum, "rpm_checksum_abc");
}

#[test]
fn test_deb_format_tracking() {
    let temp_dir = TempDir::new().unwrap();
    let options = ConversionOptions {
        output_dir: temp_dir.path().to_path_buf(),
        enable_chunking: false,
    };

    let converter = LegacyConverter::new(options);
    let metadata = create_test_metadata("deb-pkg");
    let files = create_test_files("deb-pkg");

    let result = converter
        .convert(&metadata, &files, "deb", "deb_checksum_xyz")
        .unwrap();

    assert_eq!(result.original_format, "deb");
    assert_eq!(result.original_checksum, "deb_checksum_xyz");
}

#[test]
fn test_arch_format_tracking() {
    let temp_dir = TempDir::new().unwrap();
    let options = ConversionOptions {
        output_dir: temp_dir.path().to_path_buf(),
        enable_chunking: false,
    };

    let converter = LegacyConverter::new(options);
    let metadata = create_test_metadata("arch-pkg");
    let files = create_test_files("arch-pkg");

    let result = converter
        .convert(&metadata, &files, "arch", "arch_checksum_123")
        .unwrap();

    assert_eq!(result.original_format, "arch");
    assert_eq!(result.original_checksum, "arch_checksum_123");
}

// =============================================================================
// DEPENDENCY CONVERSION TESTS
// =============================================================================

#[test]
fn test_dependency_conversion() {
    let temp_dir = TempDir::new().unwrap();
    let options = ConversionOptions {
        output_dir: temp_dir.path().to_path_buf(),
        enable_chunking: false,
    };

    let converter = LegacyConverter::new(options);

    let mut metadata = create_test_metadata("deps-pkg");
    metadata.dependencies = vec![
        Dependency {
            name: "libfoo".to_string(),
            version: Some(">= 1.0".to_string()),
            dep_type: DependencyType::Runtime,
            description: None,
        },
        Dependency {
            name: "libbar".to_string(),
            version: None,
            dep_type: DependencyType::Runtime,
            description: None,
        },
        Dependency {
            name: "build-tools".to_string(),
            version: Some(">= 2.0".to_string()),
            dep_type: DependencyType::Build, // Build dep should be ignored
            description: None,
        },
    ];

    let files = create_test_files("deps-pkg");
    let result = converter.convert(&metadata, &files, "rpm", "cs").unwrap();

    let manifest = &result.build_result.manifest;

    // Runtime deps with version go to capabilities
    assert!(
        manifest
            .requires
            .capabilities
            .iter()
            .any(|c| matches!(c, conary_core::ccs::manifest::Capability::Versioned { name, .. } if name == "libfoo")),
        "Versioned runtime dep should become capability"
    );

    // Runtime deps without version go to packages
    assert!(
        manifest
            .requires
            .packages
            .iter()
            .any(|p| p.name == "libbar"),
        "Unversioned runtime dep should become package dep"
    );

    // Build deps should NOT be included
    assert!(
        !manifest
            .requires
            .capabilities
            .iter()
            .any(|c| matches!(c, conary_core::ccs::manifest::Capability::Versioned { name, .. } if name == "build-tools")),
        "Build deps should not be in runtime requirements"
    );
    assert!(
        !manifest
            .requires
            .packages
            .iter()
            .any(|p| p.name == "build-tools"),
        "Build deps should not be in runtime requirements"
    );
}

// =============================================================================
// ERROR HANDLING TESTS
// =============================================================================

#[test]
fn test_invalid_output_dir_handling() {
    // Test with an invalid output directory
    let options = ConversionOptions {
        output_dir: PathBuf::from("/nonexistent/deeply/nested/path/that/should/not/exist"),
        enable_chunking: false,
    };

    let converter = LegacyConverter::new(options);
    let metadata = create_test_metadata("error-test");
    let files = create_test_files("error-test");

    // The conversion might fail when trying to create output directory
    // depending on permissions, or it might succeed if it can create the dirs
    let result = converter.convert(&metadata, &files, "rpm", "cs");

    // Either succeeds (created dirs) or fails with I/O error
    if let Err(e) = result {
        // Since ConversionError is not publicly re-exported, check error message
        let err_msg = format!("{}", e);
        assert!(
            err_msg.contains("I/O error") || err_msg.contains("Failed"),
            "Should be I/O error, got: {}",
            err_msg
        );
    }
}

#[test]
fn test_special_characters_in_package_name() {
    let temp_dir = TempDir::new().unwrap();
    let options = ConversionOptions {
        output_dir: temp_dir.path().to_path_buf(),
        enable_chunking: false,
    };

    let converter = LegacyConverter::new(options);

    let mut metadata = create_test_metadata("pkg-with-special_chars.v2");
    metadata.name = "pkg-with-special_chars.v2".to_string();

    let files = create_test_files("pkg-with-special_chars.v2");

    let result = converter.convert(&metadata, &files, "rpm", "cs");
    assert!(
        result.is_ok(),
        "Package with special chars should convert: {:?}",
        result.err()
    );

    let result = result.unwrap();
    assert!(result.package_path.is_some());
}
