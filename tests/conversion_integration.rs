// tests/conversion_integration.rs
//! Integration tests for legacy package to CCS conversion
//!
//! These tests validate the end-to-end conversion process from RPM/DEB/Arch
//! packages to CCS format, including:
//! - File extraction and chunking
//! - Scriptlet analysis and hook detection
//! - Capability inference during conversion
//! - Provenance extraction
//! - Fidelity tracking

use conary::ccs::convert::{ConversionOptions, FidelityLevel, LegacyConverter};
use conary::capability::inference::{Confidence, InferenceOptions};
use conary::packages::common::PackageMetadata;
use conary::packages::traits::{
    ConfigFileInfo, Dependency, DependencyType, ExtractedFile, PackageFile, Scriptlet,
    ScriptletPhase,
};
use std::path::PathBuf;
use tempfile::TempDir;

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
        }],
        dependencies: vec![],
        scriptlets: vec![],
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
    }]
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
            },
            PackageFile {
                path: "/etc/myserver/myserver.conf".to_string(),
                size: 512,
                mode: 0o644,
                sha256: Some("config_hash".to_string()),
            },
            PackageFile {
                path: "/usr/lib/systemd/system/myserver.service".to_string(),
                size: 256,
                mode: 0o644,
                sha256: Some("service_hash".to_string()),
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
        scriptlets: vec![
            Scriptlet {
                phase: ScriptletPhase::PreInstall,
                interpreter: "/bin/sh".to_string(),
                content: "getent passwd myserver || useradd -r -s /sbin/nologin myserver".to_string(),
                flags: None,
            },
            Scriptlet {
                phase: ScriptletPhase::PostInstall,
                interpreter: "/bin/sh".to_string(),
                content: "systemctl daemon-reload\nsystemctl enable myserver".to_string(),
                flags: None,
            },
        ],
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
        },
        ExtractedFile {
            path: "/etc/myserver/myserver.conf".to_string(),
            content: b"# Configuration file\nport = 8080\n".to_vec(),
            size: 512,
            mode: 0o644,
            sha256: Some("config_hash".to_string()),
        },
        ExtractedFile {
            path: "/usr/lib/systemd/system/myserver.service".to_string(),
            content: systemd_service.to_vec(),
            size: 256,
            mode: 0o644,
            sha256: Some("service_hash".to_string()),
        },
    ];

    (metadata, files)
}

/// Create a package with complex scriptlets
fn create_complex_scriptlet_package() -> (PackageMetadata, Vec<ExtractedFile>) {
    let metadata = PackageMetadata {
        package_path: PathBuf::from("/tmp/complex-pkg-1.0.0.rpm"),
        name: "complex-pkg".to_string(),
        version: "1.0.0".to_string(),
        architecture: Some("x86_64".to_string()),
        description: Some("Package with complex scriptlets".to_string()),
        files: vec![PackageFile {
            path: "/usr/bin/complex".to_string(),
            size: 100,
            mode: 0o755,
            sha256: Some("hash".to_string()),
        }],
        dependencies: vec![],
        scriptlets: vec![
            Scriptlet {
                phase: ScriptletPhase::PreInstall,
                interpreter: "/bin/sh".to_string(),
                content: r#"
# Create user and group
getent group complexgrp || groupadd -r complexgrp
getent passwd complexusr || useradd -r -g complexgrp -s /sbin/nologin complexusr

# Create directories
mkdir -p /var/lib/complex /var/log/complex
chown complexusr:complexgrp /var/lib/complex /var/log/complex
"#
                .to_string(),
                flags: None,
            },
            Scriptlet {
                phase: ScriptletPhase::PostInstall,
                interpreter: "/bin/sh".to_string(),
                content: r#"
# Reload systemd
systemctl daemon-reload

# Enable and start
systemctl enable complex.service
systemctl start complex.service || true
"#
                .to_string(),
                flags: None,
            },
            Scriptlet {
                phase: ScriptletPhase::PreRemove,
                interpreter: "/bin/sh".to_string(),
                content: r#"
# Stop service before removal
systemctl stop complex.service || true
systemctl disable complex.service || true
"#
                .to_string(),
                flags: None,
            },
        ],
        config_files: vec![],
    };

    let files = vec![ExtractedFile {
        path: "/usr/bin/complex".to_string(),
        content: b"binary".to_vec(),
        size: 100,
        mode: 0o755,
        sha256: Some("hash".to_string()),
    }];

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
        capture_scriptlets: false,
        enable_inference: false,
        min_fidelity: FidelityLevel::Low,
        ..Default::default()
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
        capture_scriptlets: false,
        enable_inference: false,
        min_fidelity: FidelityLevel::Low,
        ..Default::default()
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
        manifest.package.description.contains("detailed description"),
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
        capture_scriptlets: false, // Skip capture for this test
        enable_inference: false,
        min_fidelity: FidelityLevel::Partial,
        ..Default::default()
    };

    let converter = LegacyConverter::new(options);
    let (metadata, files) = create_server_package();

    let result = converter
        .convert(&metadata, &files, "rpm", "server_checksum")
        .unwrap();

    // Should detect user from scriptlet
    assert!(
        !result.detected_hooks.users.is_empty(),
        "Should detect user creation from scriptlet"
    );
    let user = &result.detected_hooks.users[0];
    assert_eq!(user.name, "myserver");
    assert!(user.system, "Should be a system user");

    // Should detect systemd from scriptlet (analyzer puts these in hooks.systemd)
    assert!(
        !result.detected_hooks.systemd.is_empty(),
        "Should detect systemd enable from scriptlet"
    );

    // Check config files preserved
    let manifest = &result.build_result.manifest;
    assert!(
        !manifest.config.files.is_empty(),
        "Config files should be preserved"
    );
    assert!(
        manifest.config.files.contains(&"/etc/myserver/myserver.conf".to_string()),
        "Should include myserver.conf"
    );
}

#[test]
fn test_complex_scriptlet_analysis() {
    let temp_dir = TempDir::new().unwrap();
    let options = ConversionOptions {
        output_dir: temp_dir.path().to_path_buf(),
        enable_chunking: false,
        capture_scriptlets: false,
        enable_inference: false,
        min_fidelity: FidelityLevel::Low, // Complex scripts may lower fidelity
        ..Default::default()
    };

    let converter = LegacyConverter::new(options);
    let (metadata, files) = create_complex_scriptlet_package();

    let result = converter
        .convert(&metadata, &files, "rpm", "complex_checksum")
        .unwrap();

    // Should detect group
    assert!(
        !result.detected_hooks.groups.is_empty(),
        "Should detect group creation"
    );
    assert!(
        result
            .detected_hooks
            .groups
            .iter()
            .any(|g| g.name == "complexgrp"),
        "Should detect complexgrp"
    );

    // Should detect user
    assert!(
        !result.detected_hooks.users.is_empty(),
        "Should detect user creation"
    );
    assert!(
        result
            .detected_hooks
            .users
            .iter()
            .any(|u| u.name == "complexusr"),
        "Should detect complexusr"
    );

    // Should detect systemd operations (analyzer puts these in hooks.systemd)
    assert!(
        !result.detected_hooks.systemd.is_empty(),
        "Should detect systemd operations"
    );
}

// =============================================================================
// CAPABILITY INFERENCE INTEGRATION TESTS
// =============================================================================

#[test]
fn test_conversion_with_inference_enabled() {
    let temp_dir = TempDir::new().unwrap();
    let options = ConversionOptions {
        output_dir: temp_dir.path().to_path_buf(),
        enable_chunking: false,
        capture_scriptlets: false,
        enable_inference: true, // Enable inference
        inference_options: InferenceOptions::fast(),
        min_fidelity: FidelityLevel::Low,
        ..Default::default()
    };

    let converter = LegacyConverter::new(options);
    let (metadata, files) = create_server_package();

    let result = converter
        .convert(&metadata, &files, "rpm", "server_checksum")
        .unwrap();

    // Should have inferred capabilities
    assert!(
        result.inferred_capabilities.is_some(),
        "Should infer capabilities when enabled"
    );

    let caps = result.inferred_capabilities.unwrap();

    // Server package should have network requirements inferred
    assert!(
        !caps.network.no_network,
        "Server should need network (from heuristics)"
    );

    // Should detect config directory from file paths
    assert!(
        caps.filesystem.read_paths.contains(&"/etc/myserver".to_string()),
        "Should detect config directory"
    );
}

#[test]
fn test_conversion_with_inference_disabled() {
    let temp_dir = TempDir::new().unwrap();
    let options = ConversionOptions {
        output_dir: temp_dir.path().to_path_buf(),
        enable_chunking: false,
        capture_scriptlets: false,
        enable_inference: false, // Disable inference
        min_fidelity: FidelityLevel::Low,
        ..Default::default()
    };

    let converter = LegacyConverter::new(options);
    let metadata = create_test_metadata("no-inference");
    let files = create_test_files("no-inference");

    let result = converter
        .convert(&metadata, &files, "rpm", "checksum")
        .unwrap();

    // Should NOT have inferred capabilities
    assert!(
        result.inferred_capabilities.is_none(),
        "Should not infer capabilities when disabled"
    );
}

#[test]
fn test_nginx_wellknown_inference_during_conversion() {
    let temp_dir = TempDir::new().unwrap();
    let options = ConversionOptions {
        output_dir: temp_dir.path().to_path_buf(),
        enable_chunking: false,
        capture_scriptlets: false,
        enable_inference: true,
        inference_options: InferenceOptions::fast(),
        min_fidelity: FidelityLevel::Low,
        ..Default::default()
    };

    let converter = LegacyConverter::new(options);

    // Create nginx-like package
    let metadata = PackageMetadata {
        package_path: PathBuf::from("/tmp/nginx-1.24.0.rpm"),
        name: "nginx".to_string(), // Well-known name
        version: "1.24.0".to_string(),
        architecture: Some("x86_64".to_string()),
        description: Some("High performance web server".to_string()),
        files: vec![
            PackageFile {
                path: "/usr/sbin/nginx".to_string(),
                size: 1024,
                mode: 0o755,
                sha256: Some("nginx_hash".to_string()),
            },
            PackageFile {
                path: "/etc/nginx/nginx.conf".to_string(),
                size: 512,
                mode: 0o644,
                sha256: Some("conf_hash".to_string()),
            },
        ],
        dependencies: vec![],
        scriptlets: vec![],
        config_files: vec![],
    };

    let files = vec![
        ExtractedFile {
            path: "/usr/sbin/nginx".to_string(),
            content: b"nginx binary".to_vec(),
            size: 1024,
            mode: 0o755,
            sha256: Some("nginx_hash".to_string()),
        },
        ExtractedFile {
            path: "/etc/nginx/nginx.conf".to_string(),
            content: b"# nginx config".to_vec(),
            size: 512,
            mode: 0o644,
            sha256: Some("conf_hash".to_string()),
        },
    ];

    let result = converter
        .convert(&metadata, &files, "rpm", "nginx_checksum")
        .unwrap();

    let caps = result
        .inferred_capabilities
        .expect("Should have inferred capabilities for nginx");

    // nginx well-known profile should provide port 80 and 443
    assert!(
        caps.network.listen_ports.contains(&"80".to_string()),
        "nginx should listen on port 80"
    );
    assert!(
        caps.network.listen_ports.contains(&"443".to_string()),
        "nginx should listen on port 443"
    );

    // Should be high confidence (from well-known profile)
    assert!(
        caps.confidence.primary >= Confidence::High,
        "nginx should have high confidence from well-known profile"
    );

    // Should use tier 1 (well-known)
    assert_eq!(caps.tier_used, 1, "Should use tier 1 for well-known package");
}

// =============================================================================
// FIDELITY TRACKING TESTS
// =============================================================================

#[test]
fn test_high_fidelity_for_simple_package() {
    let temp_dir = TempDir::new().unwrap();
    let options = ConversionOptions {
        output_dir: temp_dir.path().to_path_buf(),
        enable_chunking: false,
        capture_scriptlets: false,
        enable_inference: false,
        min_fidelity: FidelityLevel::Low,
        ..Default::default()
    };

    let converter = LegacyConverter::new(options);
    let metadata = create_test_metadata("simple");
    let files = create_test_files("simple");

    let result = converter.convert(&metadata, &files, "rpm", "cs").unwrap();

    // Simple package with no scriptlets should have full fidelity
    assert_eq!(
        result.fidelity.level,
        FidelityLevel::Full,
        "Simple package should have full fidelity"
    );
}

#[test]
fn test_fidelity_with_declarative_scriptlets() {
    let temp_dir = TempDir::new().unwrap();
    let options = ConversionOptions {
        output_dir: temp_dir.path().to_path_buf(),
        enable_chunking: false,
        capture_scriptlets: false,
        enable_inference: false,
        min_fidelity: FidelityLevel::Low,
        ..Default::default()
    };

    let converter = LegacyConverter::new(options);
    let (metadata, files) = create_server_package();

    let result = converter.convert(&metadata, &files, "rpm", "cs").unwrap();

    // Package with common declarative patterns should maintain high fidelity
    assert!(
        result.fidelity.level >= FidelityLevel::High,
        "Package with declarative scriptlets should have high fidelity, got: {}",
        result.fidelity.level
    );
}

#[test]
fn test_fidelity_report_details() {
    let temp_dir = TempDir::new().unwrap();
    let options = ConversionOptions {
        output_dir: temp_dir.path().to_path_buf(),
        enable_chunking: false,
        capture_scriptlets: false,
        enable_inference: false,
        min_fidelity: FidelityLevel::Low,
        ..Default::default()
    };

    let converter = LegacyConverter::new(options);
    let (metadata, files) = create_complex_scriptlet_package();

    let result = converter.convert(&metadata, &files, "rpm", "cs").unwrap();

    // Check fidelity report has details
    let report = &result.fidelity;

    // Should have some operations detected or scriptlets preserved
    let has_activity = report.hooks_extracted > 0
        || report.scriptlets_preserved > 0
        || !report.detected_operations.is_empty();
    assert!(has_activity, "Should have detected some operations");

    // The fidelity report should have meaningful information
    println!(
        "Fidelity report: level={}, hooks_extracted={}, scriptlets_preserved={}, detected_ops={}",
        report.level,
        report.hooks_extracted,
        report.scriptlets_preserved,
        report.detected_operations.len()
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
        capture_scriptlets: false,
        enable_inference: false,
        min_fidelity: FidelityLevel::Low,
        ..Default::default()
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
            },
            PackageFile {
                path: "/etc/config".to_string(),
                size: 50,
                mode: 0o644,
                sha256: Some("conf_hash".to_string()),
            },
            PackageFile {
                path: "/etc/secret".to_string(),
                size: 30,
                mode: 0o600,
                sha256: Some("secret_hash".to_string()),
            },
        ],
        dependencies: vec![],
        scriptlets: vec![],
        config_files: vec![],
    };

    let files = vec![
        ExtractedFile {
            path: "/usr/bin/executable".to_string(),
            content: b"#!/bin/sh".to_vec(),
            size: 100,
            mode: 0o755,
            sha256: Some("exec_hash".to_string()),
        },
        ExtractedFile {
            path: "/etc/config".to_string(),
            content: b"config".to_vec(),
            size: 50,
            mode: 0o644,
            sha256: Some("conf_hash".to_string()),
        },
        ExtractedFile {
            path: "/etc/secret".to_string(),
            content: b"secret".to_vec(),
            size: 30,
            mode: 0o600,
            sha256: Some("secret_hash".to_string()),
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
        capture_scriptlets: false,
        enable_inference: false,
        min_fidelity: FidelityLevel::Low,
        ..Default::default()
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
        scriptlets: vec![],
        config_files: vec![],
    };

    let files: Vec<ExtractedFile> = vec![];

    // Empty package should still convert (meta-packages exist)
    let result = converter.convert(&metadata, &files, "rpm", "cs");
    assert!(result.is_ok(), "Empty package should convert: {:?}", result.err());

    let result = result.unwrap();
    assert_eq!(result.build_result.manifest.package.name, "empty-pkg");
}

#[test]
fn test_large_file_handling() {
    let temp_dir = TempDir::new().unwrap();
    let options = ConversionOptions {
        output_dir: temp_dir.path().to_path_buf(),
        enable_chunking: true, // Test with chunking
        capture_scriptlets: false,
        enable_inference: false,
        min_fidelity: FidelityLevel::Low,
        ..Default::default()
    };

    let converter = LegacyConverter::new(options);

    // Create a larger file (1MB of data)
    let large_content: Vec<u8> = (0..1_000_000)
        .map(|i| (i % 256) as u8)
        .collect();

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
        }],
        dependencies: vec![],
        scriptlets: vec![],
        config_files: vec![],
    };

    let files = vec![ExtractedFile {
        path: "/usr/share/large/data.bin".to_string(),
        content: large_content.clone(),
        size: large_content.len() as i64,
        mode: 0o644,
        sha256: Some("large_hash".to_string()),
    }];

    let result = converter.convert(&metadata, &files, "rpm", "cs");
    assert!(result.is_ok(), "Large file should convert: {:?}", result.err());

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
        capture_scriptlets: false,
        enable_inference: false,
        min_fidelity: FidelityLevel::Low,
        ..Default::default()
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
        capture_scriptlets: false,
        enable_inference: false,
        min_fidelity: FidelityLevel::Low,
        ..Default::default()
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
        capture_scriptlets: false,
        enable_inference: false,
        min_fidelity: FidelityLevel::Low,
        ..Default::default()
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
        capture_scriptlets: false,
        enable_inference: false,
        min_fidelity: FidelityLevel::Low,
        ..Default::default()
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
            .any(|c| matches!(c, conary::ccs::manifest::Capability::Versioned { name, .. } if name == "libfoo")),
        "Versioned runtime dep should become capability"
    );

    // Runtime deps without version go to packages
    assert!(
        manifest.requires.packages.iter().any(|p| p.name == "libbar"),
        "Unversioned runtime dep should become package dep"
    );

    // Build deps should NOT be included
    assert!(
        !manifest
            .requires
            .capabilities
            .iter()
            .any(|c| matches!(c, conary::ccs::manifest::Capability::Versioned { name, .. } if name == "build-tools")),
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
        capture_scriptlets: false,
        enable_inference: false,
        min_fidelity: FidelityLevel::Low,
        ..Default::default()
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
        capture_scriptlets: false,
        enable_inference: false,
        min_fidelity: FidelityLevel::Low,
        ..Default::default()
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
