// conary-core/src/ccs/convert/converter/tests/manifest.rs

use super::*;

#[test]
fn test_conversion_options_default() {
    let options = ConversionOptions::default();
    assert!(options.enable_chunking);
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
    };
    let converter = LegacyConverter::new(options);

    let metadata = make_test_metadata();

    let manifest = converter
        .build_manifest(&metadata, &make_test_files(), &Hooks::default())
        .unwrap();

    assert_eq!(manifest.package.name, "test-package");
    assert_eq!(manifest.package.version, "1.0.0");
    assert_eq!(manifest.provides.binaries, vec!["test".to_string()]);

    assert!(
        manifest.hooks.users.is_empty(),
        "shell text must not synthesize authoritative user hooks"
    );
}

#[test]
fn test_build_manifest_derives_sonames_and_pkgconfig() {
    let converter = LegacyConverter::with_defaults();
    let mut metadata = make_test_metadata();
    metadata.files = vec![];
    let files = vec![
        ExtractedFile {
            path: "/usr/lib64/libjq.so.1.0.4".to_string(),
            content: vec![],
            size: 0,
            mode: 0o755,
            sha256: Some("abc".to_string()),
            symlink_target: None,
        },
        ExtractedFile {
            path: "/usr/share/pkgconfig/jq.pc".to_string(),
            content: vec![],
            size: 0,
            mode: 0o644,
            sha256: Some("def".to_string()),
            symlink_target: None,
        },
    ];

    let manifest = converter
        .build_manifest(&metadata, &files, &Hooks::default())
        .unwrap();

    assert_eq!(manifest.provides.sonames, vec!["libjq.so.1".to_string()]);
    assert_eq!(manifest.provides.pkgconfig, vec!["jq".to_string()]);
}

#[test]
fn build_manifest_preserves_native_virtual_provides_from_package_metadata() {
    let converter = LegacyConverter::with_defaults();
    let mut metadata = make_test_metadata();
    metadata.provides = vec![Dependency {
        name: "kernel-uname-r".to_string(),
        version: Some("= 6.17.1-300.fc44.x86_64".to_string()),
        dep_type: DependencyType::Runtime,
        description: None,
    }];

    let manifest = converter
        .build_manifest(&metadata, &make_test_files(), &Hooks::default())
        .unwrap();

    assert!(
        manifest
            .provides
            .capabilities
            .contains(&"kernel-uname-r".to_string()),
        "native virtual provides must survive conversion without file-path heuristics"
    );
    assert!(
        manifest
            .provides
            .capabilities
            .contains(&"kernel-uname-r = 6.17.1-300.fc44.x86_64".to_string()),
        "versioned native provides should retain their native constraint text"
    );
}

#[cfg(unix)]
#[test]
fn write_files_to_temp_preserves_symlinks() {
    let converter = LegacyConverter::with_defaults();
    let temp_dir = tempfile::tempdir().unwrap();
    let files = vec![ExtractedFile {
        path: "/usr/bin/sh".to_string(),
        content: Vec::new(),
        size: 4,
        mode: 0o120777,
        sha256: None,
        symlink_target: Some("bash".to_string()),
    }];

    converter
        .write_files_to_temp(&files, temp_dir.path())
        .unwrap();

    let staged_path = temp_dir.path().join("usr/bin/sh");
    let metadata = std::fs::symlink_metadata(&staged_path).unwrap();
    assert!(metadata.file_type().is_symlink());
    assert_eq!(
        std::fs::read_link(staged_path).unwrap(),
        PathBuf::from("bash")
    );
}

#[test]
fn test_write_files_to_temp() {
    let converter = LegacyConverter::with_defaults();
    let files = make_test_files();

    let temp_dir = TempDir::new().unwrap();
    converter
        .write_files_to_temp(&files, temp_dir.path())
        .unwrap();

    // Check files were written
    assert!(temp_dir.path().join("usr/bin/test").exists());

    // Check content
    let content = std::fs::read(temp_dir.path().join("usr/bin/test")).unwrap();
    assert_eq!(content, b"#!/bin/sh\necho test");
}
