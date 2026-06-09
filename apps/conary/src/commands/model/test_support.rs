// src/commands/model/test_support.rs

use std::collections::{BTreeMap, HashMap};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};

use conary_core::ccs::legacy_scriptlets::{
    DecisionCounts, ForeignReplayPolicy, LEGACY_SCRIPTLET_SCHEMA_V1, LegacyScriptletBundle,
    LegacyScriptletEntry, LifecyclePath, NativeInvocation, PublicationPolicy, PublicationStatus,
    ScriptletDecision, ScriptletFidelity, SourceFormat, TargetCompatibility, TransactionOrder,
    VersionScheme,
};

pub(super) fn build_test_ccs_package(dir: &Path, name: &str, version: &str) -> PathBuf {
    build_test_ccs_package_with_bundle(dir, name, version, None)
}

pub(super) fn build_test_ccs_package_with_bundle(
    dir: &Path,
    name: &str,
    version: &str,
    legacy_scriptlets: Option<LegacyScriptletBundle>,
) -> PathBuf {
    use conary_core::ccs::builder::write_ccs_package;
    use conary_core::ccs::manifest::Platform;
    use conary_core::ccs::{BuildResult, CcsManifest, ComponentData, FileEntry, FileType};
    use conary_core::hash;

    let binary_content = format!("#!/bin/sh\necho {name} {version}\n").into_bytes();
    let binary_hash = hash::sha256(&binary_content);
    let init_content = format!("#!/bin/sh\nexec /usr/bin/{name}\n").into_bytes();
    let init_hash = hash::sha256(&init_content);
    let files = vec![
        FileEntry {
            path: format!("/usr/bin/{name}"),
            hash: binary_hash.clone(),
            size: binary_content.len() as u64,
            mode: 0o100755,
            component: "runtime".to_string(),
            file_type: FileType::Regular,
            target: None,
            chunks: None,
        },
        FileEntry {
            path: "/usr/sbin/init".to_string(),
            hash: init_hash.clone(),
            size: init_content.len() as u64,
            mode: 0o100755,
            component: "runtime".to_string(),
            file_type: FileType::Regular,
            target: None,
            chunks: None,
        },
    ];
    let component_size = files.iter().map(|file| file.size).sum();
    let package_path = dir.join(format!("{name}-{version}.ccs"));
    let mut manifest = CcsManifest::new_minimal(name, version);
    manifest.package.platform = Some(Platform {
        os: "linux".to_string(),
        arch: Some("x86_64".to_string()),
        libc: "gnu".to_string(),
        abi: None,
    });
    manifest.legacy_scriptlets = legacy_scriptlets;
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
    write_ccs_package(&result, &package_path).unwrap();
    package_path
}

pub(super) fn legacy_replatform_upgrade_bundle(
    package: &str,
    version: &str,
) -> LegacyScriptletBundle {
    let entry = legacy_replatform_upgrade_entry();
    LegacyScriptletBundle {
        schema: LEGACY_SCRIPTLET_SCHEMA_V1.to_string(),
        schema_revision: 1,
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
        adapter_registry_digest: None,
        target_policy_digest: None,
        evidence_digest: Some(conary_core::hash::sha256_prefixed(
            format!("{package}-{version}-legacy-replatform").as_bytes(),
        )),
        target_compatibility: TargetCompatibility::SourceNative,
        allowed_targets: vec!["rpm/fedora/44/x86_64".to_string()],
        foreign_replay_policy: ForeignReplayPolicy::Deny,
        publication_policy: PublicationPolicy::LocalOnly,
        publication_status: PublicationStatus::Public,
        scriptlet_fidelity: ScriptletFidelity::LegacyReplay,
        decision_counts: DecisionCounts {
            replaced: 0,
            legacy: 1,
            blocked: 0,
            review: 0,
            extra: BTreeMap::new(),
        },
        unsupported_class_counts: BTreeMap::new(),
        entries: vec![entry],
        extra: BTreeMap::new(),
    }
}

fn legacy_replatform_upgrade_entry() -> LegacyScriptletEntry {
    let body = "echo replay-replatform-upgrade\n";
    LegacyScriptletEntry {
        id: "rpm:%pre".to_string(),
        native_slot: "%pre".to_string(),
        phase: LifecyclePath::PreUpgrade,
        lifecycle_paths: vec!["upgrade:new-pre".to_string()],
        interpreter: "/bin/sh".to_string(),
        interpreter_args: Vec::new(),
        body_sha256: conary_core::hash::sha256_prefixed(body.as_bytes()),
        body: body.to_string(),
        body_encoding: None,
        native_invocation: NativeInvocation::default(),
        transaction_order: TransactionOrder {
            position: "before-payload".to_string(),
            before: Vec::new(),
            after: Vec::new(),
            extra: BTreeMap::new(),
        },
        timeout_ms: 30_000,
        sandbox: None,
        capabilities: Vec::new(),
        decision: ScriptletDecision::Legacy,
        reason_code: "legacy-replay-required".to_string(),
        human_reason: Some("test fixture".to_string()),
        evidence_digest: Some(conary_core::hash::sha256_prefixed(
            b"rpm:%pre:echo replay-replatform-upgrade",
        )),
        source_evidence_refs: vec!["capture:rpm:%pre".to_string()],
        effects: Vec::new(),
        unknown_commands: Vec::new(),
        blocked_classes: Vec::new(),
        rpm_trigger: None,
        deb_maintainer: None,
        arch_install: None,
        residual_replay: None,
        extra: BTreeMap::new(),
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
