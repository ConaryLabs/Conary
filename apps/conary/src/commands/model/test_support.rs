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

pub(super) fn build_test_ccs_package(
    dir: &Path,
    name: &str,
    version: &str,
    version_scheme: conary_core::repository::versioning::VersionScheme,
    native_lifecycle: Option<NativeLifecycleBundle>,
) -> PathBuf {
    build_test_ccs_package_with_payloads(
        dir,
        name,
        version,
        version_scheme,
        native_lifecycle,
        vec![
            (
                format!("/usr/bin/{name}"),
                format!("#!/bin/sh\necho {name} {version}\n").into_bytes(),
                0o755,
            ),
            (
                "/usr/sbin/init".to_string(),
                format!("#!/bin/sh\nexec /usr/bin/{name}\n").into_bytes(),
                0o755,
            ),
        ],
    )
}

pub(super) fn build_test_ccs_package_with_payloads(
    dir: &Path,
    name: &str,
    version: &str,
    version_scheme: conary_core::repository::versioning::VersionScheme,
    native_lifecycle: Option<NativeLifecycleBundle>,
    payloads: Vec<(String, Vec<u8>, u32)>,
) -> PathBuf {
    build_test_ccs_package_with_payloads_and_relations(
        dir,
        name,
        version,
        version_scheme,
        native_lifecycle,
        payloads,
        Vec::new(),
        Vec::new(),
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn build_test_ccs_package_with_payloads_and_relations(
    dir: &Path,
    name: &str,
    version: &str,
    version_scheme: conary_core::repository::versioning::VersionScheme,
    native_lifecycle: Option<NativeLifecycleBundle>,
    payloads: Vec<(String, Vec<u8>, u32)>,
    requirements: Vec<conary_core::repository::dependency_model::RepositoryRequirementGroup>,
    provided_sonames: Vec<String>,
) -> PathBuf {
    use conary_core::ccs::builder::write_signed_current_ccs_package;
    use conary_core::ccs::manifest::Platform;
    use conary_core::ccs::{BuildResult, CcsManifest, ComponentData, FileEntry};
    use conary_core::hash;
    use conary_core::payload::{PayloadContentAuthority, PayloadNode};

    let mut objects = HashMap::new();
    let files = payloads
        .into_iter()
        .map(|(path, content, mode)| {
            let content_hash = hash::sha256(&content);
            objects.insert(content_hash.clone(), content.clone());
            FileEntry {
                path,
                node: PayloadNode::regular(mode),
                content: Some(PayloadContentAuthority {
                    sha256: content_hash,
                    size: content.len() as u64,
                }),
                component: "runtime".to_string(),
                chunks: None,
            }
        })
        .collect::<Vec<_>>();
    let component_size = files
        .iter()
        .filter_map(|file| file.content.as_ref().map(|content| content.size))
        .sum();
    let package_path = dir.join(format!("{name}-{version}.ccs"));
    let mut manifest = CcsManifest::new_minimal(name, version);
    manifest.package.version_scheme = version_scheme;
    manifest.requirements = requirements;
    manifest.provides.sonames = provided_sonames;
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
        files: files.clone(),
        payloads: conary_core::ccs::builder::payloads_from_bounded_memory_for_tests(
            &files, objects,
        )
        .unwrap(),
        total_size: 0,
        chunked: false,
        chunk_stats: None,
    };
    let signing_key = crate::commands::ccs::load_or_create_local_dev_key().unwrap();
    write_signed_current_ccs_package(&result, &package_path, &signing_key, true).unwrap();
    package_path
}

pub(super) fn typed_rpm_model_post_helper_bundle(
    package: &str,
    version: &str,
) -> NativeLifecycleBundle {
    let body = r#"local helper = io.open("/usr/lib/systemd/systemd-update-helper", "r")
if helper then
    helper:close()
    local marker = assert(io.open("/model-post-observed", "w"))
    marker:write("observed")
    marker:close()
end
"#;
    let entry = NativeLifecycleEntry {
        id: "rpm:%post".to_string(),
        native_slot: Some(conary_core::packages::native_abi::RpmScriptletSlot::Post),
        kind: NativeLifecycleEntryKind::Executable,
        phase: LifecyclePath::PostInstall,
        lifecycle_paths: vec!["install:post".to_string()],
        interpreter: "<lua>".to_string(),
        interpreter_args: Vec::new(),
        body_sha256: conary_core::hash::sha256_prefixed(body.as_bytes()),
        body: body.to_string(),
        body_encoding: None,
        native_invocation: NativeInvocation {
            args: vec!["1".to_string()],
            chroot: Some("install-root".to_string()),
            ..Default::default()
        },
        transaction_order: TransactionOrder {
            position: "after-payload".to_string(),
            before: Vec::new(),
            after: vec!["payload".to_string()],
        },
        timeout_ms: 30_000,
        sandbox: None,
        capabilities: Vec::new(),
        evidence_digest: Some(conary_core::hash::sha256_prefixed(
            b"model package-set post helper fixture",
        )),
        source_evidence_refs: vec!["capture:rpm:%post".to_string()],
        rpm_trigger: None,
        rpm_runtime: Some(RpmRuntimeMetadata {
            program: RpmProgram::EmbeddedLua,
            body_transforms: Vec::new(),
            criticality: RpmCriticality::Header,
            raw_flags: conary_core::ccs::native_lifecycle::RPM_SCRIPTLET_FLAG_CRITICAL,
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
    };
    NativeLifecycleBundle {
        schema: NATIVE_LIFECYCLE_SCHEMA_V1.to_string(),
        schema_revision: conary_core::ccs::native_lifecycle::NATIVE_LIFECYCLE_SCHEMA_REVISION,
        source_format: SourceFormat::Rpm,
        source_family: "fedora-rhel".to_string(),
        source_profile: Some("fedora-44".to_string()),
        source_release: Some("44".to_string()),
        source_arch: Some("x86_64".to_string()),
        source_package: package.to_string(),
        source_version: version.to_string(),
        source_checksum: None,
        version_scheme: VersionScheme::Rpm,
        conversion_tool: "remi".to_string(),
        conversion_tool_version: "0.9.5".to_string(),
        conversion_policy: "model-package-set-test".to_string(),
        evidence_digest: Some(conary_core::hash::sha256_prefixed(
            format!("{package}-{version}-model-package-set").as_bytes(),
        )),
        scriptlet_fidelity: ScriptletFidelity::NativeLifecycle,
        entries: vec![entry],
    }
}

pub(super) fn insert_test_static_ccs_repository(
    conn: &rusqlite::Connection,
    name: &str,
    url: &str,
    source_profile: &str,
) -> i64 {
    let repository_id =
        crate::commands::test_helpers::insert_test_static_ccs_repository(conn, name, url);
    let mut repository = conary_core::db::models::Repository::find_by_id(conn, repository_id)
        .unwrap()
        .expect("inserted static repository");
    repository.source_profile = Some(source_profile.to_string());
    repository.update(conn).unwrap();
    repository_id
}

pub(super) fn insert_test_repository_package_resolution(
    conn: &rusqlite::Connection,
    repository_id: i64,
    repository_package_id: i64,
    package: &str,
    version: &str,
) {
    use conary_core::db::models::{PackageResolution, PrimaryStrategy, ResolutionStrategy};

    let mut resolution = PackageResolution::new(
        repository_id,
        package.to_string(),
        vec![ResolutionStrategy::RepositoryPackage {
            repository_package_id,
        }],
    );
    resolution.version = Some(version.to_string());
    resolution.primary_strategy = PrimaryStrategy::RepositoryPackage;
    resolution.insert(conn).unwrap();
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
        source_profile: Some("fedora-44".to_string()),
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
        native_slot: Some(conary_core::packages::native_abi::RpmScriptletSlot::Pre),
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
            criticality: RpmCriticality::Header,
            raw_flags: conary_core::ccs::native_lifecycle::RPM_SCRIPTLET_FLAG_CRITICAL,
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
