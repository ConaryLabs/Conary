// apps/conary/tests/static_repo_m1a.rs

use std::collections::HashMap;
use std::ffi::OsStr;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use conary_core::ccs::builder::{
    BuildResult, ComponentData, FileEntry, FileType, write_ccs_package,
};
use conary_core::ccs::manifest::CcsManifest;
use conary_core::ccs::signing::SigningKeyPair;
use conary_core::repository::StaticIndex;
use conary_core::repository::static_repo::RepoLocation;
use conary_core::repository::static_repo::publish::{StaticPublishOptions, publish_static_repo};
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
        fs::create_dir_all(&root).unwrap();
        write_single_payload_ccs(&package_path);

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
            force_reinit: false,
            accept_destination_state: false,
            rotate_publish_key: false,
            rotate_root_key: false,
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

    let installed = fixture.root.join("usr/share/test-hello/hello.txt");
    assert_eq!(fs::read_to_string(installed).unwrap(), "hello from m1a\n");
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
        &["Static repository package signature", "not signed"],
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
        "never".into(),
        "--yes".into(),
    ])
}

fn write_single_payload_ccs(package_path: &Path) {
    fs::create_dir_all(package_path.parent().expect("package parent")).unwrap();
    let content = b"hello from m1a\n".to_vec();
    let hash = conary_core::hash::sha256(&content);
    let file = FileEntry {
        path: "/usr/share/test-hello/hello.txt".to_string(),
        hash: hash.clone(),
        size: content.len() as u64,
        mode: 0o100644,
        component: "runtime".to_string(),
        file_type: FileType::Regular,
        target: None,
        chunks: None,
    };
    let result = BuildResult {
        manifest: CcsManifest::new_minimal(PACKAGE_NAME, "1.0.0"),
        components: HashMap::from([(
            "runtime".to_string(),
            ComponentData {
                name: "runtime".to_string(),
                files: vec![file.clone()],
                hash: "runtime".to_string(),
                size: file.size,
            },
        )]),
        files: vec![file],
        blobs: HashMap::from([(hash, content)]),
        total_size: b"hello from m1a\n".len() as u64,
        chunked: false,
        chunk_stats: None,
    };
    write_ccs_package(&result, package_path).unwrap();
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
