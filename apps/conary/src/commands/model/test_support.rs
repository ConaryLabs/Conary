// src/commands/model/test_support.rs

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};

use conary_core::ccs::native_lifecycle::{
    LifecyclePath, NATIVE_LIFECYCLE_SCHEMA_V1, NativeInvocation, NativeLifecycleBundle,
    NativeLifecycleEntry, NativeLifecycleEntryKind, RpmCriticality, RpmProgram, RpmRuntimeMetadata,
    ScriptletFidelity, SourceFormat, TransactionOrder, VersionScheme,
};

pub(super) fn build_test_ccs_package(dir: &Path, name: &str, version: &str) -> PathBuf {
    build_test_ccs_package_with_bundle(dir, name, version, None)
}

pub(super) fn build_test_ccs_package_with_bundle(
    dir: &Path,
    name: &str,
    version: &str,
    native_lifecycle: Option<NativeLifecycleBundle>,
) -> PathBuf {
    use conary_core::ccs::builder::write_signed_current_ccs_package;
    use conary_core::ccs::manifest::Platform;
    use conary_core::ccs::{BuildResult, CcsManifest, ComponentData, FileEntry};
    use conary_core::hash;
    use conary_core::payload::{PayloadContentAuthority, PayloadNode};

    let binary_content = format!("#!/bin/sh\necho {name} {version}\n").into_bytes();
    let binary_hash = hash::sha256(&binary_content);
    let init_content = format!("#!/bin/sh\nexec /usr/bin/{name}\n").into_bytes();
    let init_hash = hash::sha256(&init_content);
    let files = vec![
        FileEntry {
            path: format!("/usr/bin/{name}"),
            node: PayloadNode::regular(0o755),
            content: Some(PayloadContentAuthority {
                sha256: binary_hash.clone(),
                size: binary_content.len() as u64,
            }),
            component: "runtime".to_string(),
            chunks: None,
        },
        FileEntry {
            path: "/usr/sbin/init".to_string(),
            node: PayloadNode::regular(0o755),
            content: Some(PayloadContentAuthority {
                sha256: init_hash.clone(),
                size: init_content.len() as u64,
            }),
            component: "runtime".to_string(),
            chunks: None,
        },
    ];
    let component_size = (binary_content.len() + init_content.len()) as u64;
    let package_path = dir.join(format!("{name}-{version}.ccs"));
    let mut manifest = CcsManifest::new_minimal(name, version);
    if native_lifecycle.is_some() {
        manifest.package.version_scheme = conary_core::repository::versioning::VersionScheme::Rpm;
    }
    manifest.package.platform = Some(Platform {
        os: "linux".to_string(),
        arch: Some("x86_64".to_string()),
        libc: "gnu".to_string(),
        abi: None,
    });
    manifest.native_lifecycle = native_lifecycle;
    let result = BuildResult {
        manifest,
        components: HashMap::from([(
            "runtime".to_string(),
            ComponentData {
                name: "runtime".to_string(),
                files: files.clone(),
                hash: format!("{name}-runtime"),
                size: component_size,
            },
        )]),
        files,
        blobs: HashMap::from([(binary_hash, binary_content), (init_hash, init_content)]),
        total_size: 0,
        chunked: false,
        chunk_stats: None,
    };
    let signing_key = crate::commands::ccs::load_or_create_local_dev_key().unwrap();
    write_signed_current_ccs_package(&result, &package_path, &signing_key, true).unwrap();
    package_path
}

pub(super) fn typed_rpm_replatform_upgrade_bundle(
    package: &str,
    version: &str,
) -> NativeLifecycleBundle {
    let entry = typed_rpm_replatform_upgrade_entry();
    NativeLifecycleBundle {
        schema: NATIVE_LIFECYCLE_SCHEMA_V1.to_string(),
        schema_revision: conary_core::ccs::native_lifecycle::NATIVE_LIFECYCLE_SCHEMA_REVISION,
        source_format: SourceFormat::Rpm,
        source_family: "fedora-rhel".to_string(),
        source_distro: Some("fedora".to_string()),
        source_release: Some("44".to_string()),
        source_arch: Some("x86_64".to_string()),
        source_package: package.to_string(),
        source_version: version.to_string(),
        source_checksum: None,
        version_scheme: VersionScheme::Rpm,
        conversion_tool: "remi".to_string(),
        conversion_tool_version: "0.8.0".to_string(),
        conversion_policy: "goal6-model-test".to_string(),
        evidence_digest: Some(conary_core::hash::sha256_prefixed(
            format!("{package}-{version}-typed-rpm-replatform").as_bytes(),
        )),
        scriptlet_fidelity: ScriptletFidelity::NativeLifecycle,
        entries: vec![entry],
    }
}

fn typed_rpm_replatform_upgrade_entry() -> NativeLifecycleEntry {
    let body = "print('replatform-upgrade-new-pre')\n";
    NativeLifecycleEntry {
        id: "rpm:%pre".to_string(),
        native_slot: "%pre".to_string(),
        kind: NativeLifecycleEntryKind::Executable,
        phase: LifecyclePath::PreUpgrade,
        lifecycle_paths: vec!["upgrade:new-pre".to_string()],
        interpreter: "<lua>".to_string(),
        interpreter_args: Vec::new(),
        body_sha256: conary_core::hash::sha256_prefixed(body.as_bytes()),
        body: body.to_string(),
        body_encoding: None,
        native_invocation: NativeInvocation::default(),
        transaction_order: TransactionOrder {
            position: "before-payload".to_string(),
            before: Vec::new(),
            after: Vec::new(),
        },
        timeout_ms: 30_000,
        sandbox: None,
        capabilities: Vec::new(),
        evidence_digest: Some(conary_core::hash::sha256_prefixed(
            b"rpm:%pre:print replatform-upgrade-new-pre",
        )),
        source_evidence_refs: vec!["capture:rpm:%pre".to_string()],
        rpm_trigger: None,
        rpm_runtime: Some(RpmRuntimeMetadata {
            program: RpmProgram::EmbeddedLua,
            body_transforms: Vec::new(),
            critical: true,
            criticality: RpmCriticality::Header,
            raw_flags: 0,
            unknown_flags: 0,
            install_prefixes: Vec::new(),
            macro_context: Default::default(),
            header_context: Default::default(),
            package_rpm_version: None,
        }),
        rpm_sysusers: None,
        deb_maintainer: None,
        arch_install: None,
        arch_hook: None,
        residual_lifecycle: None,
    }
}

pub(super) fn serve_test_file(file_path: PathBuf) -> (String, std::thread::JoinHandle<()>) {
    let filename = file_path.file_name().unwrap().to_string_lossy().to_string();
    let bytes = std::fs::read(&file_path).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0_u8; 1024];
        let _ = stream.read(&mut request);
        let headers = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/octet-stream\r\nConnection: close\r\n\r\n",
            bytes.len()
        );
        stream.write_all(headers.as_bytes()).unwrap();
        stream.write_all(&bytes).unwrap();
    });
    (format!("http://{addr}/{filename}"), handle)
}

pub(super) struct ReplatformMetadataFailpointReset;

impl Drop for ReplatformMetadataFailpointReset {
    fn drop(&mut self) {
        super::apply::set_replatform_metadata_failpoint_for_test(false);
    }
}
