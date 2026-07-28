// apps/conary/tests/static_repo_m1a.rs

use std::collections::HashMap;
use std::ffi::OsStr;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use conary_core::ccs::builder::{
    BuildResult, ComponentData, FileEntry as CcsFileEntry, write_signed_current_ccs_package,
};
use conary_core::ccs::manifest::{CcsManifest, Platform};
use conary_core::ccs::signing::SigningKeyPair;
use conary_core::db::models::{FileEntry as DatabaseFileEntry, InstallSource, Trove, TroveType};
use conary_core::payload::{
    PayloadContentAuthority, PayloadIdentity, PayloadNode, PayloadNodeKind, ResolvedPayloadNode,
};
use conary_core::repository::StaticIndex;
use conary_core::repository::static_repo::RepoLocation;
use conary_core::repository::static_repo::publish::{StaticPublishOptions, publish_static_repo};
use conary_core::repository::versioning::VersionScheme;
use conary_core::runtime_root::ConaryRuntimeRoot;
use conary_core::trust::generate::{generate_snapshot, generate_targets, generate_timestamp};
use conary_core::trust::metadata::{
    RootMetadata, Signed, SnapshotMetadata, TargetsMetadata, TimestampMetadata,
};

const PACKAGE_NAME: &str = "test-hello";
const REPO_NAME: &str = "local-static";
const WRONG_FINGERPRINT: &str = "1111111111111111111111111111111111111111111111111111111111111111";

struct StaticRepoFixture {
    _work: tempfile::TempDir,
    repo_dir: PathBuf,
    root: PathBuf,
    db_path: PathBuf,
    key_dir: PathBuf,
    fingerprint: String,
    initial_generation: i64,
}

impl StaticRepoFixture {
    fn publish() -> Self {
        let work = tempfile::tempdir().unwrap();
        let repo_dir = work.path().join("repo");
        let root = work.path().join("root");
        let db_path = work.path().join("conary.db");
        let key_dir = work.path().join("keys");
        let state_file = work.path().join("publish-state.toml");
        let package_path = work.path().join("dist").join("test-hello.ccs");

        conary_core::db::init(&db_path).unwrap();
        let initial_generation = seed_selected_root(&db_path);
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&key_dir).unwrap();
        let publish_key = SigningKeyPair::generate().with_key_id("publish");
        publish_key
            .save_to_files(
                &key_dir.join("publish.private"),
                &key_dir.join("publish.public"),
            )
            .unwrap();
        write_single_payload_ccs(&package_path, &publish_key);

        let outcome = publish_static_repo(StaticPublishOptions {
            repo_name: REPO_NAME.to_string(),
            repo_description: None,
            destination: RepoLocation::File {
                root: repo_dir.clone(),
            },
            key_dir: key_dir.clone(),
            state_file,
            package_paths: vec![package_path],
            refresh: false,
            rotate_publish_key: false,
            rotate_root_key: false,
            artifact_gate_context: None,
        })
        .expect("publish static repo fixture");
        assert_eq!(outcome.root_key_ids.len(), 1);
        let fingerprint = outcome.root_key_ids[0].clone();

        Self {
            _work: work,
            repo_dir,
            root,
            db_path,
            key_dir,
            fingerprint,
            initial_generation,
        }
    }

    fn add(&self) -> Output {
        add_static_repo(&self.repo_dir, &self.db_path, &self.fingerprint)
    }

    fn sync(&self) -> Output {
        sync_static_repo(&self.db_path)
    }

    fn install(&self) -> Output {
        install_test_hello(&self.db_path, &self.root)
    }
}

#[test]
fn m1a_publish_add_sync_install_from_local_static_repo() {
    let fixture = StaticRepoFixture::publish();

    assert_success(&fixture.add());
    assert_success(&fixture.sync());
    assert_success(&fixture.install());

    let conn = conary_core::db::open(&fixture.db_path).unwrap();
    let installed = DatabaseFileEntry::find_by_path(&conn, "/usr/share/test-hello/hello.txt")
        .unwrap()
        .expect("installed payload authority");
    let content = installed.content.expect("installed content authority");
    assert_eq!(
        content.sha256,
        conary_core::hash::sha256(b"hello from m1a\n")
    );

    let runtime_root = ConaryRuntimeRoot::from_db_path(fixture.db_path.clone());
    let generation = conary_core::generation::mount::current_generation(runtime_root.root())
        .unwrap()
        .expect("static CCS install should publish a generation");
    assert_ne!(generation, fixture.initial_generation);
    let artifact = conary_core::generation::artifact::load_generation_artifact(
        &runtime_root.generation_path(generation),
    )
    .unwrap();
    let generation_entry = artifact
        .generation_root
        .entries
        .iter()
        .find(|entry| entry.path == "/usr/share/test-hello/hello.txt")
        .expect("selected generation should contain installed payload");
    assert_eq!(generation_entry.content.as_ref(), Some(&content));
    assert_eq!(
        conary_core::filesystem::CasStore::new(runtime_root.objects_dir())
            .unwrap()
            .retrieve(&content.sha256)
            .unwrap(),
        b"hello from m1a\n"
    );
}

fn resolved_test_node(mut node: PayloadNode) -> ResolvedPayloadNode {
    node.user = PayloadIdentity::Numeric {
        id: u64::from(unsafe { libc::geteuid() }),
    };
    node.group = PayloadIdentity::Numeric {
        id: u64::from(unsafe { libc::getegid() }),
    };
    ResolvedPayloadNode::from_numeric_source(node).unwrap()
}

fn seed_selected_root(db_path: &Path) -> i64 {
    let runtime_root = ConaryRuntimeRoot::from_db_path(db_path.to_path_buf());
    stage_test_boot_assets(runtime_root.root());
    let conn = conary_core::db::open(db_path).unwrap();
    let mut base = Trove::new_with_source(
        "static-repo-base".to_string(),
        "1.0.0".to_string(),
        TroveType::Package,
        InstallSource::Repository,
        VersionScheme::Conary,
    );
    let base_id = base.insert(&conn).unwrap();
    for path in ["/sbin", "/usr", "/usr/share"] {
        let mut node = PayloadNode::regular(0o755);
        node.kind = PayloadNodeKind::Directory;
        node.mode = libc::S_IFDIR | 0o755;
        DatabaseFileEntry::new(path.to_string(), resolved_test_node(node), None, base_id)
            .insert(&conn)
            .unwrap();
    }
    let init = b"#!/bin/sh\nexec true\n";
    let init_hash = conary_core::filesystem::CasStore::new(runtime_root.objects_dir())
        .unwrap()
        .store(init)
        .unwrap();
    DatabaseFileEntry::new(
        "/sbin/init".to_string(),
        resolved_test_node(PayloadNode::regular(0o755)),
        Some(PayloadContentAuthority {
            sha256: init_hash,
            size: init.len() as u64,
        }),
        base_id,
    )
    .insert(&conn)
    .unwrap();
    conary_core::ccs::HostCapabilityInventory::discover()
        .persist(&conn)
        .unwrap();

    let selected_root = tempfile::tempdir().unwrap();
    conary_core::generation::builder::materialize_selected_root_from_db(
        &conn,
        &runtime_root.objects_dir(),
        selected_root.path(),
    )
    .unwrap();
    let captured = conary_core::generation::root_manifest::scan_selected_root(
        selected_root.path(),
        &conary_core::filesystem::CasStore::new(runtime_root.objects_dir()).unwrap(),
    )
    .unwrap();
    let (generation, _) =
        conary_core::generation::builder::build_generation_from_captured_root_with_boot_root_and_activation(
            &conn,
            &runtime_root.generations_dir(),
            "static repository selected-root base",
            &runtime_root.root().join("boot"),
            conary_core::generation::builder::GenerationActivation::Active,
            captured,
        )
        .unwrap();
    conary_core::generation::mount::update_current_symlink(runtime_root.root(), generation)
        .unwrap();
    generation
}

fn stage_test_boot_assets(runtime_root: &Path) {
    let boot_root = runtime_root.join("boot");
    fs::create_dir_all(boot_root.join("EFI/BOOT")).unwrap();
    fs::write(boot_root.join("vmlinuz-test-kernel"), b"test-kernel").unwrap();
    fs::write(
        boot_root.join("initramfs-test-kernel.img"),
        b"test-initramfs",
    )
    .unwrap();
    fs::write(boot_root.join("EFI/BOOT/BOOTX64.EFI"), b"test-efi").unwrap();
}

#[test]
fn m1a_repo_sync_force_twice_without_changes_succeeds() {
    let fixture = StaticRepoFixture::publish();

    assert_success(&fixture.add());
    assert_success(&fixture.sync());
    assert_success(&fixture.sync());
}

#[test]
fn m1a_tampered_index_json_fails_sync() {
    let fixture = StaticRepoFixture::publish();
    let index_path = fixture.repo_dir.join("index.json");
    let mut index = fs::read_to_string(&index_path).unwrap();
    index.push('\n');
    fs::write(&index_path, index).unwrap();

    assert_success(&fixture.add());
    let output = fixture.sync();
    assert_failure_contains(&output, &["index.json", "mismatch"]);
}

#[test]
fn m1a_unsigned_static_package_fails_install() {
    let fixture = StaticRepoFixture::publish();

    replace_published_package_with_unsigned_package(&fixture);
    assert_success(&fixture.add());
    assert_success(&fixture.sync());

    let output = fixture.install();
    assert_failure_contains(
        &output,
        &["CCS package authority verification", "not signed"],
    );
}

#[test]
fn m1a_root_fingerprint_mismatch_fails_add() {
    let fixture = StaticRepoFixture::publish();

    let output = add_static_repo(&fixture.repo_dir, &fixture.db_path, WRONG_FINGERPRINT);
    assert_failure_contains(&output, &["fingerprint", "does not match"]);
}

#[test]
fn m1a_non_interactive_add_without_fingerprint_fails() {
    let fixture = StaticRepoFixture::publish();

    let output = Command::new(env!("CARGO_BIN_EXE_conary"))
        .env("CONARY_NON_INTERACTIVE", "1")
        .args([
            "repo",
            "add",
            REPO_NAME,
            &path_arg(&fixture.repo_dir),
            "--db-path",
            &path_arg(&fixture.db_path),
        ])
        .output()
        .expect("failed to run conary");
    assert_failure_contains(&output, &["non-interactive", "--fingerprint"]);
}

fn run_conary_owned(args: &[String]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_conary"))
        .env("CONARY_TEST_SKIP_GENERATION_MOUNT", "1")
        .args(args)
        .output()
        .expect("failed to run conary")
}

fn output_text(output: &Output) -> String {
    format!(
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn assert_success(output: &Output) {
    assert!(output.status.success(), "{}", output_text(output));
}

fn assert_failure_contains(output: &Output, needles: &[&str]) {
    assert!(
        !output.status.success(),
        "command unexpectedly succeeded\n{}",
        output_text(output)
    );
    let text = output_text(output).to_lowercase();
    for needle in needles {
        assert!(
            text.contains(&needle.to_lowercase()),
            "expected output to contain {needle:?}\n{}",
            output_text(output)
        );
    }
}

fn add_static_repo(repo_dir: &Path, db_path: &Path, fingerprint: &str) -> Output {
    run_conary_owned(&[
        "repo".into(),
        "add".into(),
        REPO_NAME.into(),
        path_arg(repo_dir),
        "--fingerprint".into(),
        fingerprint.into(),
        "--db-path".into(),
        path_arg(db_path),
    ])
}

fn sync_static_repo(db_path: &Path) -> Output {
    run_conary_owned(&[
        "repo".into(),
        "sync".into(),
        REPO_NAME.into(),
        "--db-path".into(),
        path_arg(db_path),
        "--force".into(),
    ])
}

fn install_test_hello(db_path: &Path, root: &Path) -> Output {
    run_conary_owned(&[
        "install".into(),
        PACKAGE_NAME.into(),
        "--repo".into(),
        REPO_NAME.into(),
        "--db-path".into(),
        path_arg(db_path),
        "--root".into(),
        path_arg(root),
        "--sandbox".into(),
        "always".into(),
        "--yes".into(),
    ])
}

fn write_single_payload_ccs(package_path: &Path, signing_key: &SigningKeyPair) {
    fs::create_dir_all(package_path.parent().expect("package parent")).unwrap();
    let content = b"hello from m1a\n".to_vec();
    let content_size = content.len() as u64;
    let hash = conary_core::hash::sha256(&content);
    let mut node = PayloadNode::regular(0o644);
    node.user = PayloadIdentity::Numeric {
        id: u64::from(unsafe { libc::geteuid() }),
    };
    node.group = PayloadIdentity::Numeric {
        id: u64::from(unsafe { libc::getegid() }),
    };
    let file = CcsFileEntry {
        path: "/usr/share/test-hello/hello.txt".to_string(),
        node,
        content: Some(PayloadContentAuthority {
            sha256: hash.clone(),
            size: content_size,
        }),
        component: "runtime".to_string(),
        chunks: None,
    };
    let mut manifest = CcsManifest::new_minimal(PACKAGE_NAME, "1.0.0");
    manifest.package.platform = Some(Platform {
        os: "linux".to_string(),
        arch: Some(std::env::consts::ARCH.to_string()),
        libc: "gnu".to_string(),
        abi: None,
    });
    let files = vec![file.clone()];
    let result = BuildResult {
        manifest,
        components: HashMap::from([(
            "runtime".to_string(),
            ComponentData {
                name: "runtime".to_string(),
                files: vec![file.clone()],
                hash: "runtime".to_string(),
                size: content_size,
            },
        )]),
        files: files.clone(),
        payloads: conary_core::ccs::builder::payloads_from_bounded_memory_for_tests(
            &files,
            HashMap::from([(hash, content)]),
        )
        .unwrap(),
        total_size: content_size,
        chunked: false,
        chunk_stats: None,
    };
    write_signed_current_ccs_package(&result, package_path, signing_key, false).unwrap();
}

fn replace_published_package_with_unsigned_package(fixture: &StaticRepoFixture) {
    let published_package = find_published_package(&fixture.repo_dir);
    rewrite_ccs_without_manifest_signature(&published_package);
    refresh_static_metadata_for_package_mutation(&fixture.repo_dir, &fixture.key_dir);
}

fn rewrite_ccs_without_manifest_signature(package_path: &Path) {
    let unsigned_path = package_path.with_extension("unsigned.ccs");
    let input = fs::File::open(package_path).unwrap();
    let decoder = flate2::read::GzDecoder::new(input);
    let mut archive = tar::Archive::new(decoder);

    let output = fs::File::create(&unsigned_path).unwrap();
    let encoder = flate2::write::GzEncoder::new(output, flate2::Compression::default());
    let mut builder = tar::Builder::new(encoder);
    let mut removed_signature = false;

    for entry in archive.entries().unwrap() {
        let mut entry = entry.unwrap();
        let path = entry.path().unwrap().into_owned();
        let path_text = path.to_string_lossy();
        if path_text == "MANIFEST.sig" || path_text == "./MANIFEST.sig" {
            removed_signature = true;
            continue;
        }

        let mut header = entry.header().clone();
        let mut content = Vec::new();
        entry.read_to_end(&mut content).unwrap();
        header.set_size(content.len() as u64);
        header.set_cksum();
        builder
            .append_data(&mut header, path, content.as_slice())
            .unwrap();
    }

    assert!(removed_signature, "published package should be signed");
    let encoder = builder.into_inner().unwrap();
    encoder.finish().unwrap();
    fs::rename(unsigned_path, package_path).unwrap();
}

fn refresh_static_metadata_for_package_mutation(repo_dir: &Path, key_dir: &Path) {
    let index_path = repo_dir.join("index.json");
    let package_keys_path = repo_dir.join("keys/package-keys.json");
    let root_path = repo_dir.join("metadata/root.json");
    let targets_path = repo_dir.join("metadata/targets.json");
    let snapshot_path = repo_dir.join("metadata/snapshot.json");
    let timestamp_path = repo_dir.join("metadata/timestamp.json");

    let publish_key = SigningKeyPair::load_from_file(&key_dir.join("publish.private")).unwrap();
    let root: Signed<RootMetadata> =
        serde_json::from_slice(&fs::read(&root_path).unwrap()).unwrap();
    let old_targets: Signed<TargetsMetadata> =
        serde_json::from_slice(&fs::read(&targets_path).unwrap()).unwrap();
    let old_snapshot: Signed<SnapshotMetadata> =
        serde_json::from_slice(&fs::read(&snapshot_path).unwrap()).unwrap();
    let old_timestamp: Signed<TimestampMetadata> =
        serde_json::from_slice(&fs::read(&timestamp_path).unwrap()).unwrap();
    let mut index =
        StaticIndex::parse(std::str::from_utf8(&fs::read(&index_path).unwrap()).unwrap()).unwrap();

    let package = index
        .packages
        .iter_mut()
        .find(|package| package.name == PACKAGE_NAME)
        .expect("published index should contain test package");
    let package_bytes = fs::read(repo_dir.join(&package.path)).unwrap();
    package.size = package_bytes.len() as u64;
    package.sha256 = conary_core::hash::sha256(&package_bytes);
    index.index_version = old_targets.signed.version + 1;
    let index_bytes = serde_json::to_vec_pretty(&index).unwrap();
    fs::write(&index_path, &index_bytes).unwrap();

    let package_keys_bytes = fs::read(&package_keys_path).unwrap();
    let mut target_entries = index
        .packages
        .iter()
        .map(|package| (package.path.clone(), package.size, package.sha256.clone()))
        .collect::<Vec<_>>();
    target_entries.push((
        "index.json".to_string(),
        index_bytes.len() as u64,
        conary_core::hash::sha256(&index_bytes),
    ));
    target_entries.push((
        "keys/package-keys.json".to_string(),
        package_keys_bytes.len() as u64,
        conary_core::hash::sha256(&package_keys_bytes),
    ));
    target_entries.sort_by(|left, right| left.0.cmp(&right.0));

    let targets = generate_targets(
        &target_entries,
        &publish_key,
        old_targets.signed.version + 1,
        90,
    )
    .unwrap();
    let snapshot = generate_snapshot(
        root.signed.version,
        &targets,
        &publish_key,
        old_snapshot.signed.version + 1,
        90,
    )
    .unwrap();
    let timestamp = generate_timestamp(
        &snapshot,
        &publish_key,
        old_timestamp.signed.version + 1,
        720,
    )
    .unwrap();

    fs::write(targets_path, serde_json::to_vec(&targets).unwrap()).unwrap();
    fs::write(snapshot_path, serde_json::to_vec(&snapshot).unwrap()).unwrap();
    fs::write(timestamp_path, serde_json::to_vec(&timestamp).unwrap()).unwrap();
}

fn find_published_package(repo_dir: &Path) -> PathBuf {
    let package_dir = repo_dir.join("packages").join(PACKAGE_NAME);
    fs::read_dir(package_dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.extension() == Some(OsStr::new("ccs")))
        .expect("published static repo should contain a .ccs package")
}

fn path_arg(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}
