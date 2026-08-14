// apps/conary/src/commands/adopt/convert/tests/lifecycle.rs

use super::super::*;
use conary_core::ccs::builder::BuildResult;
use conary_core::ccs::manifest::{Authors, CcsManifest, Platform, ScriptHook};
use conary_core::db::models::{
    InstallSource, PackagePayloadOwnership, Repository, RepositoryPackage, Trove, TroveType,
};
use conary_core::repository::selector::PackageWithRepo;
use conary_core::repository::versioning::VersionScheme;
use sha1::Digest as _;
use std::collections::HashMap;
use std::io::{Cursor, Read, Write};
use zip::write::SimpleFileOptions;

fn generate_eopkg_fixture(path: &Path, version: &str) {
    let payload_path = "usr/share/exact-lifecycle/version";
    let payload = version.as_bytes();
    let mut tar = tar::Builder::new(Vec::new());
    let mut header = tar::Header::new_gnu();
    header.set_path(payload_path).unwrap();
    header.set_size(u64::try_from(payload.len()).unwrap());
    header.set_mode(0o644);
    header.set_uid(0);
    header.set_gid(0);
    header.set_mtime(0);
    header.set_cksum();
    tar.append(&header, Cursor::new(payload)).unwrap();
    let tar = tar.into_inner().unwrap();
    let stream =
        liblzma::stream::Stream::new_easy_encoder(6, liblzma::stream::Check::Crc64).unwrap();
    let mut encoder = liblzma::read::XzEncoder::new_stream(tar.as_slice(), stream);
    let mut compressed = Vec::new();
    encoder.read_to_end(&mut compressed).unwrap();

    let metadata = format!(
        "<PISI><Package><Name>exact-lifecycle</Name><Summary>exact lifecycle</Summary>\
         <History><Update release=\"1\"><Version>{version}</Version></Update></History>\
         <Distribution>Solus</Distribution><DistributionRelease>1</DistributionRelease>\
         <Architecture>x86_64</Architecture><PackageFormat>1.2</PackageFormat></Package></PISI>"
    );
    let files = format!(
        "<Files><File><Path>{payload_path}</Path><Type>data</Type><Size>{}</Size>\
         <Uid>0</Uid><Gid>0</Gid><Mode>0644</Mode><Hash>{:x}</Hash></File></Files>",
        payload.len(),
        sha1::Sha1::digest(payload)
    );
    let output = std::fs::File::create(path).unwrap();
    let mut zip = zip::ZipWriter::new(output);
    for (name, bytes) in [
        ("metadata.xml", metadata.as_bytes()),
        ("files.xml", files.as_bytes()),
        ("install.tar.xz", compressed.as_slice()),
    ] {
        zip.start_file(name, SimpleFileOptions::default()).unwrap();
        zip.write_all(bytes).unwrap();
    }
    zip.finish().unwrap();
}

fn generate_native_fixture(
    result: &BuildResult,
    format: PackageFormatType,
    path: &Path,
    version: &str,
) {
    match format {
        PackageFormatType::Rpm => {
            conary_core::ccs::native_export::rpm::generate(result, path).unwrap();
        }
        PackageFormatType::Deb => {
            conary_core::ccs::native_export::deb::generate(result, path).unwrap();
        }
        PackageFormatType::Arch => {
            conary_core::ccs::native_export::arch::generate(result, path).unwrap();
        }
        PackageFormatType::Eopkg => generate_eopkg_fixture(path, version),
    }
}

fn source_profile(format: PackageFormatType) -> &'static str {
    match format {
        PackageFormatType::Rpm => "fedora-44",
        PackageFormatType::Deb => "ubuntu-26.04",
        PackageFormatType::Arch => "arch",
        PackageFormatType::Eopkg => "solus",
    }
}

fn lifecycle_build_result(version: &str) -> BuildResult {
    let mut manifest = CcsManifest::new_minimal("exact-lifecycle", version);
    manifest.package.license = Some("MIT".to_string());
    manifest.package.homepage = Some("https://example.invalid/exact-lifecycle".to_string());
    manifest.package.authors = Some(Authors {
        maintainers: vec!["Exact Test <exact@example.invalid>".to_string()],
        upstream: None,
    });
    manifest.package.platform = Some(Platform {
        arch: Some("x86_64".to_string()),
        ..Default::default()
    });
    manifest.hooks.post_install = Some(ScriptHook {
        script: ":".to_string(),
        reversible: None,
    });
    manifest.hooks.pre_remove = Some(ScriptHook {
        script: ":".to_string(),
        reversible: None,
    });
    BuildResult {
        manifest,
        components: HashMap::new(),
        files: Vec::new(),
        payloads: Vec::new(),
        total_size: 0,
        chunked: false,
        chunk_stats: None,
    }
}

fn lifecycle_identity(
    scheme: VersionScheme,
    version: &str,
    architecture: &str,
) -> InstalledPackageIdentity {
    match scheme {
        VersionScheme::Rpm => {
            let (version, release) = version.rsplit_once('-').unwrap();
            InstalledPackageIdentity::rpm(
                format!("exact-lifecycle-{version}-{release}.{architecture}"),
                "exact-lifecycle",
                None,
                version,
                release,
                architecture,
            )
            .unwrap()
        }
        VersionScheme::Debian => InstalledPackageIdentity::dpkg(
            format!("exact-lifecycle:{architecture}"),
            "exact-lifecycle",
            version,
            architecture,
            conary_core::repository::dependency_model::DebianMultiArch::No,
        )
        .unwrap(),
        VersionScheme::Arch => InstalledPackageIdentity::pacman(
            "exact-lifecycle",
            "exact-lifecycle",
            version,
            architecture,
        )
        .unwrap(),
        VersionScheme::Eopkg => {
            let (version, release) = version.rsplit_once('-').unwrap();
            InstalledPackageIdentity::eopkg(
                "exact-lifecycle",
                "exact-lifecycle",
                version,
                release.parse().unwrap(),
                architecture,
            )
            .unwrap()
        }
        other => panic!("unsupported native test scheme {other:?}"),
    }
}

#[test]
fn canonical_conversion_preserves_lifecycle_for_all_native_formats() {
    let temp = tempfile::tempdir().unwrap();
    for (format, extension) in [
        (PackageFormatType::Rpm, "rpm"),
        (PackageFormatType::Deb, "deb"),
        (PackageFormatType::Arch, "pkg.tar.zst"),
        (PackageFormatType::Eopkg, "eopkg"),
    ] {
        let package_path = temp.path().join(format!("exact-lifecycle.{extension}"));
        let result = lifecycle_build_result("1.2.3");
        generate_native_fixture(&result, format, &package_path, "1.2.3");
        let parsed = conary_core::packages::parse_package(&package_path).unwrap();
        if format == PackageFormatType::Eopkg {
            assert!(parsed.native_scriptlet_abi().is_empty());
        } else {
            assert!(
                parsed.native_scriptlet_abi().len() >= 2,
                "{format:?} fixture lost its lifecycle ABI before conversion"
            );
        }

        let converted = convert_native_package_to_ccs(
            parsed.as_ref(),
            &package_path,
            format,
            Some(source_profile(format)),
        )
        .unwrap();
        assert!(converted.ccs_path.exists());
        let record = converted.pending_record.into_record(42).unwrap();
        let summary = record.scriptlet_summary().unwrap();
        if format == PackageFormatType::Eopkg {
            assert_eq!(summary.scriptlet_fidelity, "native-free");
        } else {
            assert_ne!(summary.scriptlet_fidelity, "native-free");
        }
        assert!(summary.evidence_digest.is_some());
    }
}

#[test]
fn persistence_failure_rolls_back_record_changeset_and_artifact_for_all_formats() {
    let temp = tempfile::tempdir().unwrap();
    for (format, scheme, extension) in [
        (PackageFormatType::Rpm, VersionScheme::Rpm, "rpm"),
        (PackageFormatType::Deb, VersionScheme::Debian, "deb"),
        (PackageFormatType::Arch, VersionScheme::Arch, "pkg.tar.zst"),
        (PackageFormatType::Eopkg, VersionScheme::Eopkg, "eopkg"),
    ] {
        let case_dir = temp.path().join(format!("failure-{}", scheme.as_str()));
        std::fs::create_dir_all(&case_dir).unwrap();
        let package_path = case_dir.join(format!("exact-lifecycle.{extension}"));
        let result = lifecycle_build_result("1.2.3");
        generate_native_fixture(&result, format, &package_path, "1.2.3");
        let parsed = conary_core::packages::parse_package(&package_path).unwrap();
        let converted = convert_native_package_to_ccs(
            parsed.as_ref(),
            &package_path,
            format,
            Some(source_profile(format)),
        )
        .unwrap();

        let db_path = case_dir.join("runtime/conary.db");
        conary_core::db::init(&db_path).unwrap();
        let mut conn = conary_core::db::open(&db_path).unwrap();
        let mut trove = Trove::new_with_source(
            "exact-lifecycle".to_string(),
            parsed.version().to_string(),
            TroveType::Package,
            InstallSource::AdoptedFull,
            scheme,
        );
        let architecture = parsed.architecture().unwrap();
        trove.architecture = Some(architecture.to_string());
        if scheme == VersionScheme::Debian {
            trove.debian_multi_arch =
                Some(conary_core::repository::dependency_model::DebianMultiArch::No);
        }
        let identity = lifecycle_identity(scheme, parsed.version(), architecture);
        trove.native_package_identity = Some(identity.clone());
        let trove_id = trove.insert(&conn).unwrap();
        conn.execute_batch(
            "CREATE TRIGGER fail_adopted_conversion_apply
             BEFORE UPDATE OF status ON changesets
             WHEN NEW.status = 'applied'
             BEGIN
                 SELECT RAISE(ABORT, 'forced conversion persistence failure');
             END;",
        )
        .unwrap();
        let mut package_file = std::fs::File::open(&package_path).unwrap();
        let checksum = conary_core::hash::hash_reader(
            conary_core::hash::HashAlgorithm::Sha256,
            &mut package_file,
        )
        .unwrap()
        .value;
        let mut package = RepositoryPackage::new(
            1,
            "exact-lifecycle".to_string(),
            parsed.version().to_string(),
            scheme,
            checksum,
            i64::try_from(std::fs::metadata(&package_path).unwrap().len()).unwrap(),
            "https://packages.example.invalid/exact".to_string(),
        );
        package.architecture = Some(architecture.to_string());
        let plan = AdoptedConversionPlan {
            trove_id,
            identity,
            payload: PackagePayloadOwnership::load(&conn, trove_id).unwrap(),
            source: PackageWithRepo {
                package,
                repository: Repository::new(
                    "exact-source".to_string(),
                    "https://metadata.example.invalid/exact".to_string(),
                ),
            },
        };

        let error =
            publish_conversion(&mut conn, db_path.to_str().unwrap(), &plan, converted).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("forced conversion persistence failure")
        );
        assert!(
            conary_core::db::models::ConvertedPackage::find_by_trove(&conn, trove_id)
                .unwrap()
                .is_none()
        );
        assert!(
            conary_core::db::models::Changeset::list_all(&conn)
                .unwrap()
                .is_empty()
        );
        let output_dir =
            conary_core::db::paths::db_dir(db_path.to_str().unwrap()).join("packages/adopted");
        assert!(
            !output_dir.exists() || std::fs::read_dir(output_dir).unwrap().next().is_none(),
            "{format:?} persistence failure retained a published CCS artifact"
        );

        conn.execute_batch("DROP TRIGGER fail_adopted_conversion_apply;")
            .unwrap();
        let converted = convert_native_package_to_ccs(
            parsed.as_ref(),
            &package_path,
            format,
            Some(source_profile(format)),
        )
        .unwrap();
        publish_conversion(&mut conn, db_path.to_str().unwrap(), &plan, converted).unwrap();
        let record = conary_core::db::models::ConvertedPackage::find_by_trove(&conn, trove_id)
            .unwrap()
            .unwrap();
        let usable = current_conversion_is_usable(&conn, db_path.to_str().unwrap(), &plan).unwrap();
        if !usable {
            let key = crate::commands::ccs::load_or_create_local_dev_key().unwrap();
            let path = record.ccs_path.as_deref().unwrap();
            let verified = conary_core::ccs::verify::verify_package(
                Path::new(path),
                &TrustPolicy::strict(vec![key.public_key_base64()]),
            )
            .unwrap();
            let package =
                conary_core::ccs::CcsPackage::from_verified_archive(path, &verified).unwrap();
            let manifest = package.manifest();
            panic!(
                "{format:?} current conversion mismatch: manifest={:?}/{:?}/{:?}/{:?} lifecycle_checksum={:?}, expected={:?}/{:?}/{:?}, record={:?}/{:?}",
                manifest.package.name,
                manifest.package.version,
                manifest.package.release,
                manifest.package.version_scheme,
                manifest
                    .native_lifecycle
                    .as_ref()
                    .and_then(|lifecycle| lifecycle.source_checksum.as_deref()),
                plan.identity.name(),
                plan.identity.version(),
                plan.identity.version_scheme(),
                record.original_format,
                record.original_checksum,
            );
        }
        assert!(Path::new(record.ccs_path.as_deref().unwrap()).exists());
        let changesets = conary_core::db::models::Changeset::list_all(&conn).unwrap();
        assert_eq!(changesets.len(), 1);
        assert_eq!(
            changesets[0].status,
            conary_core::db::models::ChangesetStatus::Applied
        );
    }
}

fn publish_lifecycle_conversion(
    case_dir: &Path,
    format: PackageFormatType,
    scheme: VersionScheme,
    extension: &str,
    version: &str,
) -> (PathBuf, String, String) {
    std::fs::create_dir_all(case_dir).unwrap();
    let package_path = case_dir.join(format!("exact-lifecycle-{version}.{extension}"));
    let result = lifecycle_build_result(version);
    generate_native_fixture(&result, format, &package_path, version);
    let parsed = conary_core::packages::parse_package(&package_path).unwrap();
    let converted = convert_native_package_to_ccs(
        parsed.as_ref(),
        &package_path,
        format,
        Some(source_profile(format)),
    )
    .unwrap();
    let signing_public_key = converted.signing_public_key.clone();

    let db_path = case_dir.join("source/conary.db");
    conary_core::db::init(&db_path).unwrap();
    let mut conn = conary_core::db::open(&db_path).unwrap();
    let architecture = parsed.architecture().unwrap();
    let mut trove = Trove::new_with_source(
        "exact-lifecycle".to_string(),
        parsed.version().to_string(),
        TroveType::Package,
        InstallSource::AdoptedFull,
        scheme,
    );
    trove.architecture = Some(architecture.to_string());
    if scheme == VersionScheme::Debian {
        trove.debian_multi_arch =
            Some(conary_core::repository::dependency_model::DebianMultiArch::No);
    }
    let identity = lifecycle_identity(scheme, parsed.version(), architecture);
    trove.native_package_identity = Some(identity.clone());
    let trove_id = trove.insert(&conn).unwrap();
    let mut package_file = std::fs::File::open(&package_path).unwrap();
    let checksum =
        conary_core::hash::hash_reader(conary_core::hash::HashAlgorithm::Sha256, &mut package_file)
            .unwrap()
            .value;
    let mut package = RepositoryPackage::new(
        1,
        "exact-lifecycle".to_string(),
        parsed.version().to_string(),
        scheme,
        checksum,
        i64::try_from(std::fs::metadata(&package_path).unwrap().len()).unwrap(),
        "https://packages.example.invalid/exact".to_string(),
    );
    package.architecture = Some(architecture.to_string());
    let plan = AdoptedConversionPlan {
        trove_id,
        identity,
        payload: PackagePayloadOwnership::load(&conn, trove_id).unwrap(),
        source: PackageWithRepo {
            package,
            repository: Repository::new(
                "exact-source".to_string(),
                "https://metadata.example.invalid/exact".to_string(),
            ),
        },
    };
    publish_conversion(&mut conn, db_path.to_str().unwrap(), &plan, converted).unwrap();
    let record = conary_core::db::models::ConvertedPackage::find_by_trove(&conn, trove_id)
        .unwrap()
        .unwrap();
    (
        PathBuf::from(record.ccs_path.unwrap()),
        signing_public_key,
        parsed.version().to_string(),
    )
}

fn seed_target_shell_runtime(db_path: &Path) {
    let trove_id = crate::commands::test_helpers::seed_test_bootable_runtime(db_path);
    let mut paths = std::collections::BTreeSet::new();
    for executable in ["/bin/sh", "/bin/bash", "/usr/bin/bash"] {
        if !Path::new(executable).exists() {
            continue;
        }
        let output = std::process::Command::new("ldd")
            .arg(executable)
            .output()
            .unwrap();
        assert!(output.status.success());
        paths.extend(
            String::from_utf8(output.stdout)
                .unwrap()
                .split_whitespace()
                .filter(|field| field.starts_with('/'))
                .map(|field| field.trim_end_matches(':').to_string()),
        );
        paths.insert(executable.to_string());
    }
    paths.insert("/lib64/ld-linux-x86-64.so.2".to_string());

    let conn = conary_core::db::open(db_path).unwrap();
    for destination in paths {
        if conary_core::db::models::FileEntry::find_by_path(&conn, &destination)
            .unwrap()
            .is_some()
        {
            continue;
        }
        let source = std::fs::canonicalize(&destination).unwrap();
        let bytes = std::fs::read(&source).unwrap();
        let mode = std::os::unix::fs::MetadataExt::mode(&std::fs::metadata(&source).unwrap());
        crate::commands::test_helpers::insert_test_regular_file_with_parents(
            &conn,
            db_path,
            &destination,
            &bytes,
            mode,
            trove_id,
            None,
        );
    }
    crate::commands::test_helpers::insert_test_regular_file_with_parents(
        &conn,
        db_path,
        "/usr/sbin/usysconf",
        b"#!/bin/sh\nexit 0\n",
        0o100755,
        trove_id,
        None,
    );
}

fn run_target_root_lifecycle_test(test_name: &str) -> bool {
    const CHILD_MARKER: &str = "CONARY_ADOPTED_CCS_LIFECYCLE_TEST_MARKER";
    if nix::unistd::geteuid().is_root() {
        if let Some(marker) = std::env::var_os(CHILD_MARKER) {
            std::fs::write(marker, test_name).unwrap();
        }
        return true;
    }
    assert!(
        std::env::var_os(CHILD_MARKER).is_none(),
        "adopted CCS lifecycle child did not gain root in its user namespace"
    );
    let namespace_args = [
        "--user",
        "--map-root-user",
        "--mount",
        "--propagation",
        "private",
    ];
    let probe = std::process::Command::new("unshare")
        .args(namespace_args)
        .arg("/bin/true")
        .status();
    assert!(
        probe.is_ok_and(|status| status.success()),
        "full adopted CCS lifecycle proof requires a root or unprivileged user namespace"
    );
    let marker = tempfile::NamedTempFile::new().unwrap();
    let status = std::process::Command::new("unshare")
        .args(namespace_args)
        .arg(std::env::current_exe().unwrap())
        .arg(test_name)
        .args(["--exact", "--nocapture"])
        .env(CHILD_MARKER, marker.path())
        .status()
        .unwrap();
    assert!(status.success(), "adopted CCS lifecycle child failed");
    assert_eq!(std::fs::read_to_string(marker.path()).unwrap(), test_name);
    false
}

#[tokio::test]
async fn published_adopted_ccs_drives_full_lifecycle_without_native_manager_runtime() {
    const TEST_NAME: &str = "commands::adopt::convert::tests::lifecycle::published_adopted_ccs_drives_full_lifecycle_without_native_manager_runtime";
    if !run_target_root_lifecycle_test(TEST_NAME) {
        return;
    }
    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let temp = tempfile::tempdir().unwrap();
    for (format, scheme, extension) in [
        (PackageFormatType::Rpm, VersionScheme::Rpm, "rpm"),
        (PackageFormatType::Deb, VersionScheme::Debian, "deb"),
        (PackageFormatType::Arch, VersionScheme::Arch, "pkg.tar.zst"),
        (PackageFormatType::Eopkg, VersionScheme::Eopkg, "eopkg"),
    ] {
        let format_dir = temp.path().join(scheme.as_str());
        let (v1_ccs, public_key, v1_version) = publish_lifecycle_conversion(
            &format_dir.join("v1"),
            format,
            scheme,
            extension,
            "1.2.3",
        );
        let (v2_ccs, v2_public_key, v2_version) = publish_lifecycle_conversion(
            &format_dir.join("v2"),
            format,
            scheme,
            extension,
            "1.2.4",
        );
        assert_eq!(public_key, v2_public_key);

        let target = format_dir.join("target");
        let root = target.join("root");
        let db_path = target.join("conary.db");
        std::fs::create_dir_all(&root).unwrap();
        conary_core::db::init(&db_path).unwrap();
        seed_target_shell_runtime(&db_path);
        let policy = target.join("trust-policy.toml");
        std::fs::write(
            &policy,
            format!("trusted_keys = [\"{public_key}\"]\nrequire_timestamp = false\n"),
        )
        .unwrap();
        let db_path = db_path.to_string_lossy().into_owned();
        let root = root.to_string_lossy().into_owned();
        let policy = policy.to_string_lossy().into_owned();

        crate::commands::ccs::cmd_ccs_install(
            v1_ccs.to_str().unwrap(),
            &db_path,
            &root,
            false,
            Some(policy.clone()),
            None,
            crate::commands::SandboxMode::Always,
            true,
            false,
        )
        .await
        .unwrap();
        let conn = conary_core::db::open(&db_path).unwrap();
        let installed = Trove::find_by_name(&conn, "exact-lifecycle").unwrap();
        assert_eq!(installed.len(), 1);
        assert_eq!(installed[0].version, v1_version);
        drop(conn);

        crate::commands::ccs::cmd_ccs_install(
            v2_ccs.to_str().unwrap(),
            &db_path,
            &root,
            false,
            Some(policy.clone()),
            None,
            crate::commands::SandboxMode::Always,
            true,
            false,
        )
        .await
        .unwrap();
        let conn = conary_core::db::open(&db_path).unwrap();
        let installed = Trove::find_by_name(&conn, "exact-lifecycle").unwrap();
        assert_eq!(installed.len(), 1);
        assert_eq!(installed[0].version, v2_version);
        let update_changeset = installed[0].installed_by_changeset_id.unwrap();
        drop(conn);

        crate::commands::cmd_rollback(update_changeset, &db_path)
            .await
            .unwrap();
        let conn = conary_core::db::open(&db_path).unwrap();
        let installed = Trove::find_by_name(&conn, "exact-lifecycle").unwrap();
        assert_eq!(installed.len(), 1);
        assert_eq!(installed[0].version, v1_version);
        let trove_id = installed[0].id.unwrap();
        assert!(
            conary_core::db::models::InstalledNativeLifecycleBundle::find_by_trove(&conn, trove_id)
                .unwrap()
                .is_some()
        );
        drop(conn);

        crate::commands::cmd_remove(
            "exact-lifecycle",
            &db_path,
            None,
            None,
            crate::commands::SandboxMode::Always,
            false,
        )
        .await
        .unwrap();
        let conn = conary_core::db::open(&db_path).unwrap();
        assert!(
            Trove::find_by_name(&conn, "exact-lifecycle")
                .unwrap()
                .is_empty()
        );
    }
}
