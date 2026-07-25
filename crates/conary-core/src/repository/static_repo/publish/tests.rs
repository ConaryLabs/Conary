// conary-core/src/repository/static_repo/publish/tests.rs

use super::{
    ForcedRefreshForTest, StaticPublishOptions, prepare_static_key_dir, publish_static_repo,
    publish_static_repo_with_forced_refresh_for_test, try_acquire_publish_lock_for_test,
    unique_atomic_temp_path,
};
use crate::ccs::builder::{CcsBuilder, write_signed_current_ccs_package};
use crate::ccs::manifest::CcsManifest;
use crate::ccs::signing::SigningKeyPair;
use crate::ccs::verify::{TrustPolicy, verify_package};
use crate::packages::traits::PackageFormat;
use crate::repository::static_repo::package_staging::stage_packages;
use crate::repository::static_repo::publish_context::{
    ArtifactGateContext, STATIC_PUBLISH_POLICY_DIGEST_V1, save_key_pair,
};
use crate::repository::static_repo::publish_gate::AcceptedStaticSignerSet;
use crate::repository::static_repo::{
    PackageKeyEntry, PackageKeyStatus, PackageKeysFile, RepoIdentity, RepoLocation, StaticIndex,
};
use crate::trust::keys::sign_tuf_metadata;
use crate::trust::metadata::{
    RootMetadata, Signed, SnapshotMetadata, TargetsMetadata, TimestampMetadata,
};
use crate::trust::signing_keypair_to_tuf_key;
use chrono::{Duration, Utc};
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

const ROOT_IDENTITY_WARNING: &str = "the root key **is** the repo's identity — store `root.private` offline if possible, and back up the whole directory; losing it means clients must manually re-trust (§7.4).";
#[test]
#[cfg(unix)]
fn prepare_static_key_dir_creates_repo_key_dir_0700() {
    use std::os::unix::fs::PermissionsExt;

    let temp_dir = tempfile::tempdir().unwrap();
    let key_base = temp_dir.path().join(".config/conary/keys");

    let key_dir = prepare_static_key_dir(&key_base, "test-repo").unwrap();

    assert_eq!(key_dir, key_base.join("test-repo"));
    assert!(key_dir.is_dir());
    let mode = std::fs::metadata(&key_dir).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o700);
}
#[test]
fn prepare_static_key_dir_rejects_empty_repo_name() {
    let temp_dir = tempfile::tempdir().unwrap();

    let error = prepare_static_key_dir(temp_dir.path(), "").unwrap_err();

    assert!(
        error.to_string().contains("repo name must not be empty"),
        "unexpected error: {error}"
    );
}

#[test]
fn prepare_static_key_dir_rejects_unsafe_repo_name_segments() {
    let temp_dir = tempfile::tempdir().unwrap();
    let key_base = temp_dir.path().join(".config/conary/keys");
    let absolute_name = temp_dir.path().join("escape").display().to_string();

    for repo_name in [
        "nested/repo",
        "../escape",
        &absolute_name,
        r"nested\repo",
        "   ",
        ".",
        "..",
    ] {
        let error = prepare_static_key_dir(&key_base, repo_name).unwrap_err();
        assert!(
            error.to_string().contains("safe path segment"),
            "unexpected error for {repo_name:?}: {error}"
        );
    }
}

#[test]
fn initial_publish_creates_static_repo_layout_and_identity_warning() {
    let fixture = PublishFixture::new();
    let package = fixture.build_package("widget", "1.0.0", "x86_64", b"hello\n");

    let outcome = publish_static_repo(fixture.options(vec![package])).unwrap();

    assert_eq!(outcome.root_version, 1);
    assert_eq!(outcome.targets_version, 1);
    assert_eq!(outcome.snapshot_version, 1);
    assert_eq!(outcome.timestamp_version, 1);
    assert_eq!(outcome.package_count, 1);
    assert_eq!(outcome.preview_warning, ROOT_IDENTITY_WARNING);
    assert_eq!(outcome.root_key_ids.len(), 1);
    assert!(!outcome.publish_key_id.is_empty());

    for relative in [
        "conary-repo.toml",
        "metadata/1.root.json",
        "metadata/root.json",
        "metadata/targets.json",
        "metadata/snapshot.json",
        "metadata/timestamp.json",
        "index.json",
        "keys/package-keys.json",
        "packages/widget/widget-1.0.0-1-x86_64.ccs",
    ] {
        assert!(
            fixture.repo_path(relative).exists(),
            "expected {relative} to be published"
        );
    }

    assert_eq!(
        fs::read(fixture.repo_path("metadata/1.root.json")).unwrap(),
        fs::read(fixture.repo_path("metadata/root.json")).unwrap()
    );

    let identity = read_identity(&fixture.repo_path("conary-repo.toml"));
    let root = read_root(&fixture.repo_path("metadata/root.json"));
    assert_eq!(
        identity.trust.root_key_ids,
        root.signed.roles["root"].keyids
    );
    assert_eq!(identity.trust.root_key_ids, outcome.root_key_ids);
}

#[test]
fn package_overwrite_with_different_bytes_fails() {
    let fixture = PublishFixture::new();
    let first = fixture.build_package("widget", "1.0.0", "x86_64", b"first\n");
    publish_static_repo(fixture.options(vec![first])).unwrap();
    let second = fixture.build_package("widget", "1.0.0", "x86_64", b"second\n");

    let error = publish_static_repo(fixture.options(vec![second])).unwrap_err();

    assert!(
        error.to_string().contains("immutable package artifact"),
        "unexpected error: {error}"
    );
}

#[test]
fn index_version_matches_targets_and_package_keys_include_active_publish_key() {
    let fixture = PublishFixture::new();
    let package = fixture.build_package("widget", "1.0.0", "x86_64", b"hello\n");
    let outcome = publish_static_repo(fixture.options(vec![package])).unwrap();

    let index = read_index(&fixture.repo_path("index.json"));
    let targets = read_targets(&fixture.repo_path("metadata/targets.json"));
    let package_keys = read_package_keys(&fixture.repo_path("keys/package-keys.json"));

    assert_eq!(index.index_version, targets.signed.version);
    assert!(targets.signed.targets.contains_key("index.json"));
    assert!(
        targets
            .signed
            .targets
            .contains_key("keys/package-keys.json")
    );
    assert_eq!(package_keys.keys.len(), 1);
    assert!(matches!(
        package_keys.keys[0].status,
        PackageKeyStatus::Active
    ));
    assert_eq!(package_keys.keys[0].key_id.as_deref(), Some("publish"));

    let publish_key = SigningKeyPair::load_from_file(&fixture.key_dir.join("publish.private"))
        .expect("publish key loads");
    let (publish_key_id, _) = signing_keypair_to_tuf_key(&publish_key).unwrap();
    assert_eq!(outcome.publish_key_id, publish_key_id);
    assert_eq!(
        package_keys.keys[0].public_key,
        publish_key.public_key_base64()
    );
}

#[test]
fn publish_preserves_package_verified_by_active_publish_key() {
    let fixture = PublishFixture::new();
    let package = fixture.build_package("widget", "1.0.0", "x86_64", b"hello\n");
    publish_static_repo(fixture.options(vec![package])).unwrap();

    let index = read_index(&fixture.repo_path("index.json"));
    let package_keys = read_package_keys(&fixture.repo_path("keys/package-keys.json"));
    let active_public_key = package_keys
        .keys
        .iter()
        .find(|key| matches!(key.status, PackageKeyStatus::Active))
        .expect("active package key")
        .public_key
        .clone();
    let published_package = fixture.repo_path(&index.packages[0].path);
    let verification = verify_package(
        &published_package,
        &TrustPolicy::strict(vec![active_public_key.clone()]),
    )
    .unwrap();

    assert_eq!(verification.signature().public_key, active_public_key);
}

#[test]
fn refresh_without_near_expiry_changes_only_timestamp() {
    let fixture = PublishFixture::new();
    let package = fixture.build_package("widget", "1.0.0", "x86_64", b"hello\n");
    publish_static_repo(fixture.options(vec![package])).unwrap();

    let before_index = fs::read(fixture.repo_path("index.json")).unwrap();
    let before_targets = fs::read(fixture.repo_path("metadata/targets.json")).unwrap();
    let before_snapshot = fs::read(fixture.repo_path("metadata/snapshot.json")).unwrap();
    let before_timestamp = read_timestamp(&fixture.repo_path("metadata/timestamp.json"));

    let mut options = fixture.options(Vec::new());
    options.refresh = true;
    let outcome = publish_static_repo(options).unwrap();

    assert_eq!(outcome.root_version, 1);
    assert_eq!(outcome.targets_version, 1);
    assert_eq!(outcome.snapshot_version, 1);
    assert_eq!(outcome.timestamp_version, 2);
    assert_eq!(
        fs::read(fixture.repo_path("index.json")).unwrap(),
        before_index
    );
    assert_eq!(
        fs::read(fixture.repo_path("metadata/targets.json")).unwrap(),
        before_targets
    );
    assert_eq!(
        fs::read(fixture.repo_path("metadata/snapshot.json")).unwrap(),
        before_snapshot
    );
    assert_eq!(
        read_timestamp(&fixture.repo_path("metadata/timestamp.json"))
            .signed
            .version,
        before_timestamp.signed.version + 1
    );
}

#[test]
fn publish_rejects_tampered_package_keys_before_refresh() {
    let fixture = PublishFixture::new();
    let package = fixture.build_package("widget", "1.0.0", "x86_64", b"hello\n");
    publish_static_repo(fixture.options(vec![package])).unwrap();

    let extra_key = SigningKeyPair::generate().with_key_id("extra");
    let mut package_keys = read_package_keys(&fixture.repo_path("keys/package-keys.json"));
    package_keys.keys.push(PackageKeyEntry {
        algorithm: "ed25519".to_string(),
        public_key: extra_key.public_key_base64(),
        key_id: Some("extra".to_string()),
        status: PackageKeyStatus::Active,
        comment: Some("unauthorized injected key".to_string()),
    });
    fs::write(
        fixture.repo_path("keys/package-keys.json"),
        serde_json::to_vec_pretty(&package_keys).unwrap(),
    )
    .unwrap();

    let mut options = fixture.options(Vec::new());
    options.refresh = true;
    let error = publish_static_repo_with_forced_refresh_for_test(
        options,
        ForcedRefreshForTest {
            root: false,
            targets: true,
            snapshot: false,
        },
    )
    .unwrap_err();

    assert!(
        error.to_string().contains("keys/package-keys.json"),
        "expected package-keys target verification failure, got: {error:?}"
    );
}

#[test]
fn publish_rejects_timestamp_that_does_not_pin_current_snapshot() {
    let fixture = PublishFixture::new();
    let package = fixture.build_package("widget", "1.0.0", "x86_64", b"hello\n");
    publish_static_repo(fixture.options(vec![package])).unwrap();

    let publish_key =
        SigningKeyPair::load_from_file(&fixture.key_dir.join("publish.private")).unwrap();
    let mut timestamp = read_timestamp(&fixture.repo_path("metadata/timestamp.json"));
    let snapshot_ref = timestamp
        .signed
        .meta
        .get_mut("snapshot.json")
        .expect("timestamp pins snapshot");
    snapshot_ref.version += 1;
    timestamp.signatures = vec![sign_tuf_metadata(&publish_key, &timestamp.signed).unwrap()];
    write_json(&fixture.repo_path("metadata/timestamp.json"), &timestamp);

    let error = publish_static_repo(fixture.options(Vec::new())).unwrap_err();

    assert!(
        error.to_string().contains("timestamp") && error.to_string().contains("snapshot.json"),
        "expected timestamp/snapshot consistency failure, got: {error:?}"
    );
}

#[test]
fn publish_rejects_timestamp_that_omits_snapshot_length() {
    let fixture = PublishFixture::new();
    let package = fixture.build_package("widget", "1.0.0", "x86_64", b"hello\n");
    publish_static_repo(fixture.options(vec![package])).unwrap();

    let publish_key =
        SigningKeyPair::load_from_file(&fixture.key_dir.join("publish.private")).unwrap();
    let mut timestamp = read_timestamp(&fixture.repo_path("metadata/timestamp.json"));
    let snapshot_ref = timestamp
        .signed
        .meta
        .get_mut("snapshot.json")
        .expect("timestamp pins snapshot");
    snapshot_ref.length = None;
    timestamp.signatures = vec![sign_tuf_metadata(&publish_key, &timestamp.signed).unwrap()];
    write_json(&fixture.repo_path("metadata/timestamp.json"), &timestamp);

    let error = publish_static_repo(fixture.options(Vec::new())).unwrap_err();

    assert!(
        error.to_string().contains("timestamp")
            && error.to_string().contains("snapshot.json")
            && error.to_string().contains("length"),
        "expected missing snapshot length failure, got: {error:?}"
    );
}

#[test]
fn forced_targets_refresh_cascades_to_snapshot_timestamp_and_preserves_packages() {
    let fixture = PublishFixture::new();
    let package = fixture.build_package("widget", "1.0.0", "x86_64", b"hello\n");
    publish_static_repo(fixture.options(vec![package])).unwrap();

    let before_index = read_index(&fixture.repo_path("index.json"));
    let before_targets = read_targets(&fixture.repo_path("metadata/targets.json"));
    let before_snapshot = read_snapshot(&fixture.repo_path("metadata/snapshot.json"));

    let mut options = fixture.options(Vec::new());
    options.refresh = true;
    let outcome = publish_static_repo_with_forced_refresh_for_test(
        options,
        ForcedRefreshForTest {
            root: false,
            targets: true,
            snapshot: false,
        },
    )
    .unwrap();

    let after_index = read_index(&fixture.repo_path("index.json"));
    let after_targets = read_targets(&fixture.repo_path("metadata/targets.json"));
    let after_snapshot = read_snapshot(&fixture.repo_path("metadata/snapshot.json"));

    assert_eq!(outcome.root_version, 1);
    assert_eq!(outcome.targets_version, before_targets.signed.version + 1);
    assert_eq!(outcome.snapshot_version, before_snapshot.signed.version + 1);
    assert_eq!(outcome.timestamp_version, 2);
    assert_eq!(after_index.index_version, after_targets.signed.version);
    assert_ne!(after_index.generated, before_index.generated);
    assert_eq!(after_index.packages.len(), before_index.packages.len());
    assert_eq!(after_index.packages[0].path, before_index.packages[0].path);
    assert_eq!(
        after_index.packages[0].sha256,
        before_index.packages[0].sha256
    );
    assert_eq!(
        after_snapshot.signed.meta["targets.json"].version,
        after_targets.signed.version
    );
}

#[test]
fn refresh_selects_near_expiry_targets_and_preserves_packages() {
    let fixture = PublishFixture::new();
    let package = fixture.build_package("widget", "1.0.0", "x86_64", b"hello\n");
    publish_static_repo(fixture.options(vec![package])).unwrap();

    let before_index = read_index(&fixture.repo_path("index.json"));
    let before_root = read_root(&fixture.repo_path("metadata/root.json"));
    let before_targets = read_targets(&fixture.repo_path("metadata/targets.json"));
    let before_snapshot = read_snapshot(&fixture.repo_path("metadata/snapshot.json"));
    set_targets_expiry(&fixture, Utc::now() + Duration::days(10));

    let mut options = fixture.options(Vec::new());
    options.refresh = true;
    let outcome = publish_static_repo(options).unwrap();

    let after_index = read_index(&fixture.repo_path("index.json"));
    let after_root = read_root(&fixture.repo_path("metadata/root.json"));
    let after_targets = read_targets(&fixture.repo_path("metadata/targets.json"));
    let after_snapshot = read_snapshot(&fixture.repo_path("metadata/snapshot.json"));

    assert_eq!(outcome.root_version, before_root.signed.version);
    assert_eq!(outcome.targets_version, before_targets.signed.version + 1);
    assert_eq!(outcome.snapshot_version, before_snapshot.signed.version + 1);
    assert_eq!(outcome.timestamp_version, 2);
    assert_eq!(after_root.signed.version, before_root.signed.version);
    assert_eq!(after_index.index_version, after_targets.signed.version);
    assert_ne!(after_index.generated, before_index.generated);
    assert_eq!(
        serde_json::to_value(&after_index.packages).unwrap(),
        serde_json::to_value(&before_index.packages).unwrap()
    );
    assert_eq!(
        after_snapshot.signed.meta["targets.json"].version,
        after_targets.signed.version
    );
}

#[test]
fn forced_root_refresh_cascades_to_snapshot_and_timestamp() {
    let fixture = PublishFixture::new();
    let package = fixture.build_package("widget", "1.0.0", "x86_64", b"hello\n");
    publish_static_repo(fixture.options(vec![package])).unwrap();

    let before_root = read_root(&fixture.repo_path("metadata/root.json"));
    let before_targets = read_targets(&fixture.repo_path("metadata/targets.json"));
    let before_snapshot = read_snapshot(&fixture.repo_path("metadata/snapshot.json"));

    let mut options = fixture.options(Vec::new());
    options.refresh = true;
    let outcome = publish_static_repo_with_forced_refresh_for_test(
        options,
        ForcedRefreshForTest {
            root: true,
            targets: false,
            snapshot: false,
        },
    )
    .unwrap();

    let after_root = read_root(&fixture.repo_path("metadata/root.json"));
    let after_targets = read_targets(&fixture.repo_path("metadata/targets.json"));
    let after_snapshot = read_snapshot(&fixture.repo_path("metadata/snapshot.json"));

    assert_eq!(outcome.root_version, before_root.signed.version + 1);
    assert_eq!(outcome.targets_version, before_targets.signed.version);
    assert_eq!(outcome.snapshot_version, before_snapshot.signed.version + 1);
    assert_eq!(outcome.timestamp_version, 2);
    assert_eq!(
        after_root.signed.roles["root"].keyids,
        before_root.signed.roles["root"].keyids
    );
    assert_eq!(after_targets.signed.version, before_targets.signed.version);
    assert_eq!(
        after_snapshot.signed.meta["root.json"].version,
        after_root.signed.version
    );
}

#[test]
fn refresh_selects_near_expiry_root_and_cascades_without_targets_bump() {
    let fixture = PublishFixture::new();
    let package = fixture.build_package("widget", "1.0.0", "x86_64", b"hello\n");
    publish_static_repo(fixture.options(vec![package])).unwrap();

    let before_root = read_root(&fixture.repo_path("metadata/root.json"));
    let before_targets = read_targets(&fixture.repo_path("metadata/targets.json"));
    let before_snapshot = read_snapshot(&fixture.repo_path("metadata/snapshot.json"));
    set_root_expiry(&fixture, Utc::now() + Duration::days(80));

    let mut options = fixture.options(Vec::new());
    options.refresh = true;
    let outcome = publish_static_repo(options).unwrap();

    let after_root = read_root(&fixture.repo_path("metadata/root.json"));
    let after_targets = read_targets(&fixture.repo_path("metadata/targets.json"));
    let after_snapshot = read_snapshot(&fixture.repo_path("metadata/snapshot.json"));

    assert_eq!(outcome.root_version, before_root.signed.version + 1);
    assert_eq!(outcome.targets_version, before_targets.signed.version);
    assert_eq!(outcome.snapshot_version, before_snapshot.signed.version + 1);
    assert_eq!(outcome.timestamp_version, 2);
    assert_eq!(after_targets.signed.version, before_targets.signed.version);
    assert_eq!(
        after_snapshot.signed.meta["root.json"].version,
        after_root.signed.version
    );
}

#[test]
fn refresh_repairs_expired_snapshot_destination_metadata() {
    let fixture = PublishFixture::new();
    let package = fixture.build_package("widget", "1.0.0", "x86_64", b"hello\n");
    publish_static_repo(fixture.options(vec![package])).unwrap();
    let before_snapshot = read_snapshot(&fixture.repo_path("metadata/snapshot.json"));
    set_snapshot_expiry(&fixture, Utc::now() - Duration::hours(1));

    let mut options = fixture.options(Vec::new());
    options.refresh = true;
    let outcome = publish_static_repo(options).unwrap();

    let after_snapshot = read_snapshot(&fixture.repo_path("metadata/snapshot.json"));
    let after_timestamp = read_timestamp(&fixture.repo_path("metadata/timestamp.json"));

    assert_eq!(outcome.snapshot_version, before_snapshot.signed.version + 1);
    assert!(after_snapshot.signed.expires > Utc::now());
    assert!(after_timestamp.signed.expires > Utc::now());
}

#[test]
fn state_file_watermark_rejects_destination_version_regression() {
    let fixture = PublishFixture::new();
    let package = fixture.build_package("widget", "1.0.0", "x86_64", b"hello\n");
    publish_static_repo(fixture.options(vec![package])).unwrap();
    fs::write(
        &fixture.state_file,
        "root_version = 1\ntargets_version = 5\nsnapshot_version = 5\ntimestamp_version = 5\n",
    )
    .unwrap();

    let mut options = fixture.options(Vec::new());
    options.refresh = true;
    let error = publish_static_repo(options).unwrap_err();

    assert!(
        error.to_string().contains("destination versions")
            && error.to_string().contains("watermark"),
        "unexpected error: {error}"
    );
}

#[test]
fn damaged_destination_fails_closed_without_identity_reset_path() {
    let fixture = PublishFixture::new();
    fs::create_dir_all(fixture.repo_path("metadata")).unwrap();
    fs::write(fixture.repo_path("metadata/timestamp.json"), b"not json").unwrap();
    let package = fixture.build_package("widget", "1.0.0", "x86_64", b"hello\n");

    let error = publish_static_repo(fixture.options(vec![package])).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("damaged or partially initialized"),
        "unexpected error: {error}"
    );
}

#[test]
fn publish_key_rotation_updates_roles_and_retires_old_package_key() {
    let fixture = PublishFixture::new();
    let package = fixture.build_package("widget", "1.0.0", "x86_64", b"hello\n");
    publish_static_repo(fixture.options(vec![package])).unwrap();
    let before_root = read_root(&fixture.repo_path("metadata/root.json"));
    let before_keys = read_package_keys(&fixture.repo_path("keys/package-keys.json"));
    let old_public_key = before_keys.keys[0].public_key.clone();

    let mut options = fixture.options(Vec::new());
    options.rotate_publish_key = true;
    let outcome = publish_static_repo(options).unwrap();

    let after_root = read_root(&fixture.repo_path("metadata/root.json"));
    let after_keys = read_package_keys(&fixture.repo_path("keys/package-keys.json"));

    assert_eq!(outcome.root_version, before_root.signed.version + 1);
    assert_eq!(
        after_root.signed.roles["root"].keyids,
        before_root.signed.roles["root"].keyids
    );
    for role in ["targets", "snapshot", "timestamp"] {
        assert_ne!(
            after_root.signed.roles[role].keyids,
            before_root.signed.roles[role].keyids
        );
        assert_eq!(
            after_root.signed.roles[role].keyids,
            after_root.signed.roles["targets"].keyids
        );
    }
    assert!(after_keys.keys.iter().any(|key| {
        key.public_key == old_public_key && matches!(key.status, PackageKeyStatus::Retired)
    }));
    assert!(after_keys.keys.iter().any(|key| {
        key.public_key != old_public_key && matches!(key.status, PackageKeyStatus::Active)
    }));
}

#[test]
fn failed_publish_key_rotation_keeps_active_key_files_unchanged() {
    let fixture = PublishFixture::new();
    let first = fixture.build_package("widget", "1.0.0", "x86_64", b"first\n");
    publish_static_repo(fixture.options(vec![first])).unwrap();
    let before_private = fs::read(fixture.key_dir.join("publish.private")).unwrap();
    let before_public = fs::read(fixture.key_dir.join("publish.public")).unwrap();
    let before_key =
        SigningKeyPair::load_from_file(&fixture.key_dir.join("publish.private")).unwrap();

    let conflicting = fixture.build_package("widget", "1.0.0", "x86_64", b"second\n");
    let mut options = fixture.options(vec![conflicting]);
    options.rotate_publish_key = true;
    let error = publish_static_repo(options).unwrap_err();

    assert!(
        error.to_string().contains("immutable package artifact"),
        "unexpected error: {error}"
    );
    assert_eq!(
        fs::read(fixture.key_dir.join("publish.private")).unwrap(),
        before_private
    );
    assert_eq!(
        fs::read(fixture.key_dir.join("publish.public")).unwrap(),
        before_public
    );
    let after_key =
        SigningKeyPair::load_from_file(&fixture.key_dir.join("publish.private")).unwrap();
    assert_eq!(
        after_key.public_key_base64(),
        before_key.public_key_base64()
    );
}

#[test]
fn package_staging_keeps_rotation_artifact_pending_until_promoted() {
    let fixture = PublishFixture::new();
    let first = fixture.build_package("widget", "1.0.0", "x86_64", b"first\n");
    publish_static_repo(fixture.options(vec![first])).unwrap();

    let pending_key = SigningKeyPair::generate().with_key_id("publish");
    let staged =
        fixture.build_package_with_key("gadget", "1.0.0", "x86_64", b"pending\n", &pending_key);
    let accepted_signers =
        AcceptedStaticSignerSet::from_initial_key("publish", pending_key.public_key_base64());
    let mut pending = stage_packages(
        &fixture.repo_dir,
        std::slice::from_ref(&staged),
        &accepted_signers,
        &pending_key,
    )
    .unwrap();
    let entries = pending.package_entries();
    let relative = entries[0].path.clone();
    let pending_path = pending.writes[0].pending_path.clone();
    let final_path = fixture.repo_path(&relative);

    assert!(pending_path.exists());
    assert!(
        !final_path.exists(),
        "package staging must not expose final immutable path before commit"
    );

    pending.promote().unwrap();
    assert!(final_path.exists());
    pending.commit();
}

#[test]
fn publish_rejects_pending_rotation_state_before_timestamp_upload() {
    let fixture = PublishFixture::new();
    let package = fixture.build_package("widget", "1.0.0", "x86_64", b"first\n");
    publish_static_repo(fixture.options(vec![package])).unwrap();
    let old_private = fs::read(fixture.key_dir.join("publish.private")).unwrap();
    let old_public = fs::read(fixture.key_dir.join("publish.public")).unwrap();
    let old_timestamp = fs::read(fixture.repo_path("metadata/timestamp.json")).unwrap();
    let old_watermark = fs::read(&fixture.state_file).unwrap();

    let mut rotate = fixture.options(Vec::new());
    rotate.rotate_publish_key = true;
    publish_static_repo(rotate).unwrap();
    let rotated_key =
        SigningKeyPair::load_from_file(&fixture.key_dir.join("publish.private")).unwrap();
    save_key_pair(&rotated_key, &fixture.key_dir, "publish.pending").unwrap();
    fs::write(fixture.key_dir.join("publish.private"), old_private).unwrap();
    fs::write(fixture.key_dir.join("publish.public"), old_public).unwrap();
    fs::write(fixture.repo_path("metadata/timestamp.json"), old_timestamp).unwrap();
    fs::write(&fixture.state_file, old_watermark).unwrap();

    let mut retry = fixture.options(Vec::new());
    retry.rotate_publish_key = true;
    let error = publish_static_repo(retry).unwrap_err();

    let active_key =
        SigningKeyPair::load_from_file(&fixture.key_dir.join("publish.private")).unwrap();
    assert_ne!(
        active_key.public_key_base64(),
        rotated_key.public_key_base64()
    );
    assert!(
        error.to_string().contains("timestamp"),
        "expected torn timestamp/root state to fail closed, got: {error}"
    );
    assert!(fixture.key_dir.join("publish.pending.private").exists());
    assert!(fixture.key_dir.join("publish.pending.public").exists());
}

#[test]
fn publish_lock_rejects_second_local_holder() {
    let fixture = PublishFixture::new();
    fs::create_dir_all(&fixture.repo_dir).unwrap();
    let _first = try_acquire_publish_lock_for_test(&fixture.repo_dir).unwrap();

    let error = try_acquire_publish_lock_for_test(&fixture.repo_dir).unwrap_err();

    assert!(
        error
            .to_string()
            .contains("another static repo publish is already running"),
        "unexpected error: {error}"
    );
}

#[test]
fn artifact_gate_context_rejects_before_package_staging() {
    let fixture = PublishFixture::new();
    let package = fixture.build_package("widget", "1.0.0", "x86_64", b"hello\n");
    let mut options = fixture.options(vec![package]);
    let unrelated_key = SigningKeyPair::generate();
    options.artifact_gate_context = Some(ArtifactGateContext {
        accepted_signers: AcceptedStaticSignerSet::from_initial_key(
            "publish",
            unrelated_key.public_key_base64(),
        ),
        publish_policy_digest: STATIC_PUBLISH_POLICY_DIGEST_V1.to_string(),
    });

    let error = format!("{:#}", publish_static_repo(options).unwrap_err());

    assert!(
        error.contains("verify current CCS authority for static publication")
            && error.contains("package signer is not trusted"),
        "{error}"
    );
    assert!(!fixture.repo_path("index.json").exists());
}

#[test]
fn atomic_temp_paths_are_unique_next_to_destination() {
    let temp_dir = tempfile::tempdir().unwrap();
    let destination = temp_dir.path().join("metadata/timestamp.json");

    let first = unique_atomic_temp_path(&destination);
    let second = unique_atomic_temp_path(&destination);

    assert_ne!(first, second);
    assert_eq!(first.parent(), destination.parent());
    assert_eq!(second.parent(), destination.parent());
}

struct PublishFixture {
    _temp: TempDir,
    repo_dir: PathBuf,
    key_dir: PathBuf,
    state_file: PathBuf,
    package_dir: PathBuf,
}

impl PublishFixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let repo_dir = temp.path().join("repo");
        let key_dir = temp.path().join("keys");
        let state_file = temp.path().join("last-published.toml");
        let package_dir = temp.path().join("packages");
        fs::create_dir_all(&package_dir).unwrap();
        let publish_key = SigningKeyPair::generate().with_key_id("publish");
        publish_key
            .save_to_files(
                &key_dir.join("publish.private"),
                &key_dir.join("publish.public"),
            )
            .unwrap();

        Self {
            _temp: temp,
            repo_dir,
            key_dir,
            state_file,
            package_dir,
        }
    }

    fn options(&self, package_paths: Vec<PathBuf>) -> StaticPublishOptions {
        StaticPublishOptions {
            repo_name: "test-repo".to_string(),
            repo_description: Some("test static repo".to_string()),
            destination: RepoLocation::File {
                root: self.repo_dir.clone(),
            },
            key_dir: self.key_dir.clone(),
            state_file: self.state_file.clone(),
            package_paths,
            refresh: false,
            rotate_publish_key: false,
            rotate_root_key: false,
            artifact_gate_context: None,
        }
    }

    fn repo_path(&self, relative: &str) -> PathBuf {
        self.repo_dir.join(relative)
    }

    fn build_package(&self, name: &str, version: &str, arch: &str, content: &[u8]) -> PathBuf {
        let key = SigningKeyPair::load_from_file(&self.key_dir.join("publish.private")).unwrap();
        self.build_package_with_key(name, version, arch, content, &key)
    }

    fn build_package_with_key(
        &self,
        name: &str,
        version: &str,
        arch: &str,
        content: &[u8],
        key: &SigningKeyPair,
    ) -> PathBuf {
        let source_dir = self
            .package_dir
            .join(format!("{name}-{version}-{arch}-src"));
        fs::create_dir_all(source_dir.join("usr/share")).unwrap();
        fs::write(source_dir.join("usr/share/payload"), content).unwrap();
        let manifest = CcsManifest::parse(&format!(
            r#"
[package]
name = "{name}"
version = "{version}"
version_scheme = "conary"
release = "1"
kind = "package"
description = "fixture package"
license = "MIT"

[package.platform]
arch = "{arch}"
"#
        ))
        .unwrap();
        let result = CcsBuilder::new(manifest, &source_dir).build().unwrap();
        let package_path = self
            .package_dir
            .join(format!("{name}-{version}-{arch}-input.ccs"));
        write_signed_current_ccs_package(&result, &package_path, key, false).unwrap();

        let verified = verify_package(
            &package_path,
            &TrustPolicy::strict(vec![key.public_key_base64()]),
        )
        .unwrap();
        let parsed = crate::ccs::package::CcsPackage::from_verified_archive(
            package_path.to_str().unwrap(),
            &verified,
        )
        .unwrap();
        assert_eq!(parsed.name(), name);
        assert_eq!(parsed.version(), version);
        assert_eq!(parsed.architecture(), Some(arch));

        package_path
    }
}

fn read_identity(path: &Path) -> RepoIdentity {
    RepoIdentity::parse(&fs::read_to_string(path).unwrap()).unwrap()
}

fn read_index(path: &Path) -> StaticIndex {
    StaticIndex::parse(&fs::read_to_string(path).unwrap()).unwrap()
}

fn read_package_keys(path: &Path) -> PackageKeysFile {
    PackageKeysFile::parse(&fs::read_to_string(path).unwrap()).unwrap()
}

fn read_root(path: &Path) -> Signed<RootMetadata> {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn read_targets(path: &Path) -> Signed<TargetsMetadata> {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn read_snapshot(path: &Path) -> Signed<SnapshotMetadata> {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn read_timestamp(path: &Path) -> Signed<TimestampMetadata> {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn set_root_expiry(fixture: &PublishFixture, expires: chrono::DateTime<Utc>) {
    let root_key = SigningKeyPair::load_from_file(&fixture.key_dir.join("root.private")).unwrap();
    let mut root = read_root(&fixture.repo_path("metadata/root.json"));
    root.signed.expires = expires;
    root.signatures = vec![sign_tuf_metadata(&root_key, &root.signed).unwrap()];
    write_json(&fixture.repo_path("metadata/root.json"), &root);
    write_json(
        &fixture.repo_path(&format!("metadata/{}.root.json", root.signed.version)),
        &root,
    );
}

fn set_targets_expiry(fixture: &PublishFixture, expires: chrono::DateTime<Utc>) {
    let publish_key =
        SigningKeyPair::load_from_file(&fixture.key_dir.join("publish.private")).unwrap();
    let mut targets = read_targets(&fixture.repo_path("metadata/targets.json"));
    targets.signed.expires = expires;
    targets.signatures = vec![sign_tuf_metadata(&publish_key, &targets.signed).unwrap()];
    write_json(&fixture.repo_path("metadata/targets.json"), &targets);
    repin_snapshot_targets_and_timestamp(fixture, &publish_key);
}

fn set_snapshot_expiry(fixture: &PublishFixture, expires: chrono::DateTime<Utc>) {
    let publish_key =
        SigningKeyPair::load_from_file(&fixture.key_dir.join("publish.private")).unwrap();
    let mut snapshot = read_snapshot(&fixture.repo_path("metadata/snapshot.json"));
    snapshot.signed.expires = expires;
    snapshot.signatures = vec![sign_tuf_metadata(&publish_key, &snapshot.signed).unwrap()];
    write_json(&fixture.repo_path("metadata/snapshot.json"), &snapshot);
    repin_timestamp_snapshot(fixture, &publish_key);
}

fn repin_snapshot_targets_and_timestamp(fixture: &PublishFixture, publish_key: &SigningKeyPair) {
    let targets_bytes = fs::read(fixture.repo_path("metadata/targets.json")).unwrap();
    let mut snapshot = read_snapshot(&fixture.repo_path("metadata/snapshot.json"));
    let targets_ref = snapshot
        .signed
        .meta
        .get_mut("targets.json")
        .expect("snapshot pins targets");
    targets_ref.length = Some(targets_bytes.len() as u64);
    targets_ref.hashes = Some({
        let mut hashes = std::collections::BTreeMap::new();
        hashes.insert("sha256".to_string(), crate::hash::sha256(&targets_bytes));
        hashes
    });
    snapshot.signatures = vec![sign_tuf_metadata(publish_key, &snapshot.signed).unwrap()];
    write_json(&fixture.repo_path("metadata/snapshot.json"), &snapshot);
    repin_timestamp_snapshot(fixture, publish_key);
}

fn repin_timestamp_snapshot(fixture: &PublishFixture, publish_key: &SigningKeyPair) {
    let snapshot_bytes = fs::read(fixture.repo_path("metadata/snapshot.json")).unwrap();
    let mut timestamp = read_timestamp(&fixture.repo_path("metadata/timestamp.json"));
    let snapshot_ref = timestamp
        .signed
        .meta
        .get_mut("snapshot.json")
        .expect("timestamp pins snapshot");
    snapshot_ref.length = Some(snapshot_bytes.len() as u64);
    snapshot_ref.hashes = Some({
        let mut hashes = std::collections::BTreeMap::new();
        hashes.insert("sha256".to_string(), crate::hash::sha256(&snapshot_bytes));
        hashes
    });
    timestamp.signatures = vec![sign_tuf_metadata(publish_key, &timestamp.signed).unwrap()];
    write_json(&fixture.repo_path("metadata/timestamp.json"), &timestamp);
}

fn write_json<T: serde::Serialize>(path: &Path, value: &T) {
    fs::write(path, serde_json::to_vec(value).unwrap()).unwrap();
}
