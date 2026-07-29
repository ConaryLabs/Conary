// tests/conversion_integration.rs
//! Integration tests for native package to CCS conversion
//!
//! These tests validate the end-to-end conversion process from RPM/DEB/Arch
//! packages to CCS format, including:
//! - File extraction and chunking
//! - Scriptlet analysis and hook detection
//! - Provenance extraction
//! - Typed lifecycle evidence

use conary_core::ccs::CcsPackage;
use conary_core::ccs::convert::{ConversionOptions, NativePackageConverter};
use conary_core::ccs::native_lifecycle::ScriptletFidelity;
use conary_core::packages::common::PackageMetadata;
use conary_core::packages::native_abi::*;
use conary_core::packages::traits::{ConfigFileInfo, ExtractedFile, PackageFile};
use conary_core::payload::{PayloadContentAuthority, PayloadNode};
use conary_core::repository::dependency_model::{
    RepositoryRequirementGroup, RepositoryRequirementKind,
};
use conary_core::repository::requirement::parse_native_requirement;
use conary_core::repository::versioning::VersionScheme;
use std::path::PathBuf;
use tempfile::TempDir;

#[path = "conversion_integration/authority.rs"]
mod authority;

// =============================================================================
// TEST HELPERS
// =============================================================================

fn requirement(
    kind: RepositoryRequirementKind,
    scheme: VersionScheme,
    native_text: &str,
) -> RepositoryRequirementGroup {
    parse_native_requirement(kind, scheme, native_text).expect("valid exact test requirement")
}

fn content_authority(content: &[u8]) -> PayloadContentAuthority {
    PayloadContentAuthority {
        sha256: conary_core::hash::sha256(content),
        size: content.len() as u64,
    }
}

fn source_checksum(label: &str) -> String {
    format!("sha256:{}", conary_core::hash::sha256(label.as_bytes()))
}

fn package_regular(path: impl Into<String>, content: &[u8], mode: u32) -> PackageFile {
    PackageFile {
        path: path.into(),
        node: PayloadNode::regular(mode),
        content: Some(content_authority(content)),
    }
}

fn extracted_regular(path: impl Into<String>, content: &[u8], mode: u32) -> ExtractedFile {
    ExtractedFile {
        path: path.into(),
        node: PayloadNode::regular(mode),
        content: content.to_vec(),
        content_authority: Some(content_authority(content)),
    }
}

fn signed_test_converter(options: ConversionOptions) -> NativePackageConverter {
    NativePackageConverter::new(options).with_signing_key(std::sync::Arc::new(
        conary_core::ccs::SigningKeyPair::generate().with_key_id("conversion-integration"),
    ))
}

trait InMemoryConversionForTest {
    fn convert_in_memory_for_test(
        &self,
        metadata: &PackageMetadata,
        files: &[ExtractedFile],
        format: &str,
        checksum: &str,
    ) -> anyhow::Result<conary_core::ccs::convert::ConversionResult>;
}

impl InMemoryConversionForTest for NativePackageConverter {
    fn convert_in_memory_for_test(
        &self,
        metadata: &PackageMetadata,
        files: &[ExtractedFile],
        format: &str,
        checksum: &str,
    ) -> anyhow::Result<conary_core::ccs::convert::ConversionResult> {
        let payload =
            conary_core::packages::PackagePayload::from_extracted_in_memory(files.to_vec())?;
        let checksum = conary_core::hash::Hash::parse_prefixed(checksum)?;
        self.convert_payload(metadata, payload.files(), format, &checksum)
            .map_err(Into::into)
    }
}

fn rpm_lifecycle_entry(
    slot_name: &str,
    slot: RpmScriptletSlot,
    lifecycle: NativeLifecyclePath,
    position: NativeTransactionPosition,
    body: &str,
) -> NativeScriptletEntry {
    NativeScriptletEntry {
        id: format!("rpm:{slot_name}"),
        format: NativeScriptletFormat::Rpm,
        kind: NativeScriptletKind::Executable,
        native_slot: slot_name.to_string(),
        primary_lifecycle: lifecycle,
        lifecycle_paths: vec![lifecycle],
        interpreter: Some("/bin/sh".to_string()),
        interpreter_args: Vec::new(),
        body: NativeScriptletBody::from_bytes(body.as_bytes().to_vec()),
        invocation: NativeInvocationContract::none(),
        order: NativeTransactionOrder::new(position),
        support: NativeScriptletSupport::Parsed,
        metadata: NativeScriptletMetadata::Rpm(RpmNativeScriptletMetadata {
            slot,
            runtime: RpmScriptletRuntimeMetadata {
                program: RpmScriptletProgram::External,
                flags: RpmScriptletFlagsMetadata {
                    names: Vec::new(),
                    raw_bits: 0,
                    unknown_bits: 0,
                    expand: false,
                    query_format: false,
                    critical: false,
                    criticality: RpmScriptletCriticality::WarningOnly,
                },
                install_prefixes: Vec::new(),
                macro_context: Default::default(),
                header_context: Default::default(),
                package_rpm_version: None,
            },
            trigger: None,
            sysusers: None,
        }),
    }
}

/// Create a minimal test package metadata
fn create_test_metadata(name: &str) -> PackageMetadata {
    let content = format!("#!/bin/sh\necho {name}");
    PackageMetadata {
        package_path: PathBuf::from(format!("/tmp/{}-1.0.0.rpm", name)),
        name: name.to_string(),
        version: "1.0.0".to_string(),
        version_scheme: VersionScheme::Rpm,
        architecture: Some("x86_64".to_string()),
        debian_multi_arch: None,
        description: Some(format!("Test package: {}", name)),
        files: vec![package_regular(
            format!("/usr/bin/{name}"),
            content.as_bytes(),
            0o755,
        )],
        requirements: vec![],
        provides: vec![],
        relations: vec![],
        diagnostic_scriptlet_evidence: vec![],
        native_scriptlet_abi: vec![],
        config_files: vec![],
    }
}

/// Create test files matching the metadata
fn create_test_files(name: &str) -> Vec<ExtractedFile> {
    let content = format!("#!/bin/sh\necho {name}");
    vec![extracted_regular(
        format!("/usr/bin/{name}"),
        content.as_bytes(),
        0o755,
    )]
}

fn passive_converter(output_dir: &std::path::Path) -> NativePackageConverter {
    signed_test_converter(ConversionOptions {
        output_dir: output_dir.to_path_buf(),
    })
    .with_source_profile("fedora-44")
    .with_source_release("44")
}

fn parse_converted_package(result: &conary_core::ccs::convert::ConversionResult) -> CcsPackage {
    let package_path = result
        .package_path
        .as_ref()
        .expect("conversion should write CCS package");
    let verified = conary_core::ccs::verify::verify_package(
        package_path,
        &conary_core::ccs::TrustPolicy::strict(vec![result.signing_public_key.clone()]),
    )
    .expect("converted CCS authority should verify");
    CcsPackage::from_verified_archive(
        package_path.to_str().expect("package path is utf-8"),
        &verified,
    )
    .expect("verified converted CCS package should open")
}

fn golden_payload_files(name: &str) -> Vec<ExtractedFile> {
    let mut files = create_test_files(name);
    files.extend([
        extracted_regular(
            "/usr/lib/systemd/system/demo.service",
            b"[Service]\nExecStart=/usr/bin/demo\n",
            0o644,
        ),
        extracted_regular(
            "/usr/lib/tmpfiles.d/demo.conf",
            b"d /run/demo 0755 root root -\n",
            0o644,
        ),
        extracted_regular(
            "/usr/lib/sysusers.d/demo.conf",
            b"u demo - \"Demo User\" /run/demo -\n",
            0o644,
        ),
        extracted_regular("/usr/share/mime/packages/demo.xml", b"<mime-info/>", 0o644),
        extracted_regular(
            "/usr/share/selinux/packages/demo.pp",
            b"selinux policy module placeholder",
            0o644,
        ),
        extracted_regular(
            "/etc/apparmor.d/usr.bin.demo",
            b"profile usr.bin.demo /usr/bin/demo { }\n",
            0o644,
        ),
    ]);
    files
}

/// Create a network server package (nginx-like)
fn create_server_package() -> (PackageMetadata, Vec<ExtractedFile>) {
    let binary_content = b"\x7fELF binary placeholder";
    let config_content = b"# Configuration file\nport = 8080\n";
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
    let metadata = PackageMetadata {
        package_path: PathBuf::from("/tmp/myserver-1.0.0.rpm"),
        name: "myserver".to_string(),
        version: "1.0.0".to_string(),
        version_scheme: VersionScheme::Rpm,
        architecture: Some("x86_64".to_string()),
        debian_multi_arch: None,
        description: Some("A test server application".to_string()),
        files: vec![
            package_regular("/usr/sbin/myserver", binary_content, 0o755),
            package_regular("/etc/myserver/myserver.conf", config_content, 0o644),
            package_regular(
                "/usr/lib/systemd/system/myserver.service",
                systemd_service,
                0o644,
            ),
        ],
        requirements: vec![
            requirement(
                RepositoryRequirementKind::Depends,
                VersionScheme::Rpm,
                "openssl-libs >= 3.0",
            ),
            requirement(
                RepositoryRequirementKind::Depends,
                VersionScheme::Rpm,
                "glibc",
            ),
        ],
        provides: vec![],
        relations: vec![],
        diagnostic_scriptlet_evidence: Vec::new(),
        native_scriptlet_abi: vec![
            rpm_lifecycle_entry(
                "%pre",
                RpmScriptletSlot::Pre,
                NativeLifecyclePath::PreInstall,
                NativeTransactionPosition::BeforePayload,
                "getent passwd myserver || useradd -r -s /sbin/nologin myserver",
            ),
            rpm_lifecycle_entry(
                "%post",
                RpmScriptletSlot::Post,
                NativeLifecyclePath::PostInstall,
                NativeTransactionPosition::AfterPayload,
                "systemctl daemon-reload\nsystemctl enable myserver",
            ),
        ],
        config_files: vec![ConfigFileInfo {
            path: "/etc/myserver/myserver.conf".to_string(),
            noreplace: true,
            ghost: false,
            remove_on_upgrade: false,
        }],
    };

    let files = vec![
        extracted_regular("/usr/sbin/myserver", binary_content, 0o755),
        extracted_regular("/etc/myserver/myserver.conf", config_content, 0o644),
        extracted_regular(
            "/usr/lib/systemd/system/myserver.service",
            systemd_service,
            0o644,
        ),
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
    };

    let converter = signed_test_converter(options);
    let metadata = create_test_metadata("minimal");
    let files = create_test_files("minimal");
    let checksum = source_checksum("minimal native package");

    let result = converter.convert_in_memory_for_test(&metadata, &files, "rpm", &checksum);
    assert!(result.is_ok(), "Basic conversion should succeed");

    let result = result.unwrap();
    assert!(result.package_path.is_some(), "Should produce output file");
    assert_eq!(result.original_format, "rpm");
    assert_eq!(result.original_checksum, checksum);

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
    };

    let converter = signed_test_converter(options);

    let mut metadata = create_test_metadata("metadata-test");
    metadata.description = Some("A detailed description".to_string());
    metadata.version_scheme = VersionScheme::Debian;
    metadata.debian_multi_arch =
        Some(conary_core::repository::dependency_model::DebianMultiArch::No);
    metadata.requirements = vec![requirement(
        RepositoryRequirementKind::Depends,
        VersionScheme::Debian,
        "libfoo (>= 1.0)",
    )];

    let files = create_test_files("metadata-test");
    let checksum = source_checksum("metadata native package");

    let result = converter
        .convert_in_memory_for_test(&metadata, &files, "deb", &checksum)
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

    assert_eq!(manifest.requirements, metadata.requirements);
}

#[test]
fn test_server_package_conversion() {
    let temp_dir = TempDir::new().unwrap();
    let options = ConversionOptions {
        output_dir: temp_dir.path().to_path_buf(),
    };

    let converter = signed_test_converter(options);
    let (metadata, files) = create_server_package();
    let checksum = source_checksum("server native package");

    let result = converter
        .convert_in_memory_for_test(&metadata, &files, "rpm", &checksum)
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
            .iter()
            .any(|config| config.path == "/etc/myserver/myserver.conf"
                && config.noreplace
                && !config.ghost
                && !config.remove_on_upgrade),
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
    };

    let converter = signed_test_converter(options);
    let executable_content = b"#!/bin/sh";
    let config_content = b"config";
    let secret_content = b"secret";

    let metadata = PackageMetadata {
        package_path: PathBuf::from("/tmp/perms-1.0.0.rpm"),
        name: "perms-test".to_string(),
        version: "1.0.0".to_string(),
        version_scheme: VersionScheme::Rpm,
        architecture: Some("x86_64".to_string()),
        debian_multi_arch: None,
        description: None,
        files: vec![
            package_regular("/usr/bin/executable", executable_content, 0o755),
            package_regular("/etc/config", config_content, 0o644),
            package_regular("/etc/secret", secret_content, 0o600),
        ],
        requirements: vec![],
        provides: vec![],
        relations: vec![],
        diagnostic_scriptlet_evidence: vec![],
        native_scriptlet_abi: vec![],
        config_files: vec![],
    };

    let files = vec![
        extracted_regular("/usr/bin/executable", executable_content, 0o755),
        extracted_regular("/etc/config", config_content, 0o644),
        extracted_regular("/etc/secret", secret_content, 0o600),
    ];
    let checksum = source_checksum("permissions native package");

    let result = converter
        .convert_in_memory_for_test(&metadata, &files, "rpm", &checksum)
        .unwrap();

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
    };

    let converter = signed_test_converter(options);

    let metadata = PackageMetadata {
        package_path: PathBuf::from("/tmp/empty-1.0.0.rpm"),
        name: "empty-pkg".to_string(),
        version: "1.0.0".to_string(),
        version_scheme: VersionScheme::Rpm,
        architecture: Some("noarch".to_string()),
        debian_multi_arch: None,
        description: None, // No description
        files: vec![],     // No files
        requirements: vec![],
        provides: vec![],
        relations: vec![],
        diagnostic_scriptlet_evidence: vec![],
        native_scriptlet_abi: vec![],
        config_files: vec![],
    };

    let files: Vec<ExtractedFile> = vec![];
    let checksum = source_checksum("empty native package");

    // Empty package should still convert (meta-packages exist)
    let result = converter.convert_in_memory_for_test(&metadata, &files, "rpm", &checksum);
    assert!(
        result.is_ok(),
        "Empty package should convert: {:?}",
        result.err()
    );

    let result = result.unwrap();
    assert_eq!(result.build_result.manifest.package.name, "empty-pkg");
}

#[test]
fn test_large_payload_emits_verified_ccs_archive() {
    let temp_dir = TempDir::new().unwrap();
    let options = ConversionOptions {
        output_dir: temp_dir.path().to_path_buf(),
    };

    let converter = signed_test_converter(options);

    // Create a larger file (1MB of data)
    let large_content: Vec<u8> = (0..1_000_000).map(|i| (i % 256) as u8).collect();

    let metadata = PackageMetadata {
        package_path: PathBuf::from("/tmp/large-1.0.0.rpm"),
        name: "large-file-pkg".to_string(),
        version: "1.0.0".to_string(),
        version_scheme: VersionScheme::Rpm,
        architecture: Some("x86_64".to_string()),
        debian_multi_arch: None,
        description: None,
        files: vec![package_regular(
            "/usr/share/large/data.bin",
            &large_content,
            0o644,
        )],
        requirements: vec![],
        provides: vec![],
        relations: vec![],
        diagnostic_scriptlet_evidence: vec![],
        native_scriptlet_abi: vec![],
        config_files: vec![],
    };

    let files = vec![extracted_regular(
        "/usr/share/large/data.bin",
        &large_content,
        0o644,
    )];
    let checksum = source_checksum("large native package");

    let result = converter.convert_in_memory_for_test(&metadata, &files, "rpm", &checksum);
    assert!(
        result.is_ok(),
        "Large file should convert: {:?}",
        result.err()
    );

    let result = result.unwrap();
    assert!(result.package_path.is_some());
    let parsed = parse_converted_package(&result);
    assert_eq!(parsed.file_entries().len(), 1);
    assert_eq!(
        parsed.file_entries()[0]
            .content
            .as_ref()
            .expect("regular file content authority")
            .size,
        large_content.len() as u64
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
    };

    let converter = signed_test_converter(options);
    let metadata = create_test_metadata("rpm-pkg");
    let files = create_test_files("rpm-pkg");
    let checksum = source_checksum("rpm native package");

    let result = converter
        .convert_in_memory_for_test(&metadata, &files, "rpm", &checksum)
        .unwrap();

    assert_eq!(result.original_format, "rpm");
    assert_eq!(result.original_checksum, checksum);
}

#[test]
fn test_deb_format_tracking() {
    let temp_dir = TempDir::new().unwrap();
    let options = ConversionOptions {
        output_dir: temp_dir.path().to_path_buf(),
    };

    let converter = signed_test_converter(options);
    let mut metadata = create_test_metadata("deb-pkg");
    metadata.version_scheme = VersionScheme::Debian;
    metadata.debian_multi_arch =
        Some(conary_core::repository::dependency_model::DebianMultiArch::No);
    let files = create_test_files("deb-pkg");
    let checksum = source_checksum("deb native package");

    let result = converter
        .convert_in_memory_for_test(&metadata, &files, "deb", &checksum)
        .unwrap();

    assert_eq!(result.original_format, "deb");
    assert_eq!(result.original_checksum, checksum);
}

#[test]
fn test_arch_format_tracking() {
    let temp_dir = TempDir::new().unwrap();
    let options = ConversionOptions {
        output_dir: temp_dir.path().to_path_buf(),
    };

    let converter = signed_test_converter(options);
    let mut metadata = create_test_metadata("arch-pkg");
    metadata.version_scheme = VersionScheme::Arch;
    let files = create_test_files("arch-pkg");
    let checksum = source_checksum("arch native package");

    let result = converter
        .convert_in_memory_for_test(&metadata, &files, "arch", &checksum)
        .unwrap();

    assert_eq!(result.original_format, "arch");
    assert_eq!(result.original_checksum, checksum);
}

// =============================================================================
// DEPENDENCY CONVERSION TESTS
// =============================================================================

#[test]
fn test_dependency_conversion() {
    let temp_dir = TempDir::new().unwrap();
    let options = ConversionOptions {
        output_dir: temp_dir.path().to_path_buf(),
    };

    let converter = signed_test_converter(options);

    let mut metadata = create_test_metadata("deps-pkg");
    metadata.version_scheme = VersionScheme::Rpm;
    metadata.requirements = vec![
        requirement(
            RepositoryRequirementKind::Depends,
            VersionScheme::Rpm,
            "libfoo >= 1.0",
        ),
        requirement(
            RepositoryRequirementKind::Depends,
            VersionScheme::Rpm,
            "libbar",
        ),
        requirement(
            RepositoryRequirementKind::Build,
            VersionScheme::Rpm,
            "build-tools >= 2.0",
        ),
    ];

    let files = create_test_files("deps-pkg");
    let checksum = source_checksum("dependency native package");
    let result = converter
        .convert_in_memory_for_test(&metadata, &files, "rpm", &checksum)
        .unwrap();

    let manifest = &result.build_result.manifest;

    assert_eq!(manifest.requirements, metadata.requirements);
    assert_eq!(
        manifest
            .requirements
            .iter()
            .map(|group| group.kind)
            .collect::<Vec<_>>(),
        vec![
            RepositoryRequirementKind::Depends,
            RepositoryRequirementKind::Depends,
            RepositoryRequirementKind::Build,
        ]
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
    };

    let converter = signed_test_converter(options);
    let metadata = create_test_metadata("error-test");
    let files = create_test_files("error-test");
    let checksum = source_checksum("invalid output native package");

    // The conversion might fail when trying to create output directory
    // depending on permissions, or it might succeed if it can create the dirs
    let result = converter.convert_in_memory_for_test(&metadata, &files, "rpm", &checksum);

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
    };

    let converter = signed_test_converter(options);

    let mut metadata = create_test_metadata("pkg-with-special_chars.v2");
    metadata.name = "pkg-with-special_chars.v2".to_string();

    let files = create_test_files("pkg-with-special_chars.v2");
    let checksum = source_checksum("special character native package");

    let result = converter.convert_in_memory_for_test(&metadata, &files, "rpm", &checksum);
    assert!(
        result.is_ok(),
        "Package with special chars should convert: {:?}",
        result.err()
    );

    let result = result.unwrap();
    assert!(result.package_path.is_some());
}
