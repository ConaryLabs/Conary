// apps/remi/src/server/native_oracle_input/tests.rs

use std::fs;

use super::*;
use crate::server::catalog_authority::test_support::ActiveCatalogFixture;

fn valid_manifest() -> NativeOracleInputSetV1 {
    let fixture = ActiveCatalogFixture::new();
    let object_bytes = b"x";
    let object_sha256 = conary_core::hash::sha256(object_bytes);
    let mut profiles = Vec::new();
    for (index, public) in conary_core::repository::supported_profiles::public_profiles()
        .iter()
        .enumerate()
    {
        let revision_sha256 = fixture.register(public.id(), index as i64 + 1, Vec::new());
        let pin = fixture
            .authority()
            .open_selected_profile(&ProfileRevisionSelection {
                source_profile: public.id().to_string(),
                profile_revision_sha256: revision_sha256,
            })
            .expect("open fixture profile");
        let mut revision = pin.manifest().clone();
        let mut sources = Vec::new();
        for member in &mut revision.members {
            let source = fixture
                .authority()
                .source_bundle_for_member(&pin, member.ordinal)
                .expect("open fixture source");
            let mut source = source.manifest;
            for object in &mut source.authenticated_objects {
                object.sha256 = object_sha256.clone();
                object.size = object_bytes.len() as u64;
            }
            member.source_snapshot_sha256 = source.manifest_sha256().expect("hash source manifest");
            sources.push(source);
        }
        let profile_revision_sha256 = revision.manifest_sha256().expect("hash profile revision");
        profiles.push(NativeOracleInputProfileV1 {
            profile_revision_sha256,
            revision,
            sources,
        });
    }
    NativeOracleInputSetV1 {
        schema_version: NATIVE_ORACLE_INPUT_SCHEMA_V1,
        profiles,
        objects: vec![NativeOracleInputObjectV1 {
            sha256: object_sha256,
            size: object_bytes.len() as u64,
        }],
    }
}

fn write_bundle(root: &Path, manifest: &NativeOracleInputSetV1) {
    fs::create_dir(root).expect("create bundle root");
    let objects = root.join(NATIVE_ORACLE_INPUT_OBJECT_DIRECTORY);
    fs::create_dir(&objects).expect("create objects root");
    let bytes = conary_core::json::canonical_json(manifest).expect("canonical manifest");
    fs::write(root.join(NATIVE_ORACLE_INPUT_MANIFEST_FILE), bytes).expect("write manifest");
    for object in &manifest.objects {
        fs::write(objects.join(&object.sha256), b"x").expect("write object");
    }
}

fn fixture_selections(
    fixture: &ActiveCatalogFixture,
    candidate: bool,
) -> Vec<ProfileRevisionSelection> {
    conary_core::repository::supported_profiles::public_profiles()
        .iter()
        .enumerate()
        .map(|(index, profile)| {
            let revision = if candidate {
                fixture.candidate(profile.id(), index as i64 + 1, Vec::new())
            } else {
                fixture.activate(profile.id(), index as i64 + 1, Vec::new())
            };
            ProfileRevisionSelection {
                source_profile: profile.id().to_string(),
                profile_revision_sha256: revision,
            }
        })
        .collect()
}

#[test]
fn complete_bundle_reopens() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("bundle");
    let manifest = valid_manifest();
    write_bundle(&root, &manifest);

    assert_eq!(reopen_native_oracle_input_bundle(&root).unwrap(), manifest);
}

#[test]
fn exact_private_candidates_are_required() {
    let fixture = ActiveCatalogFixture::new();
    let selections = fixture_selections(&fixture, true);
    let candidates = capture_current_candidates(fixture.db_path(), &selections).unwrap();
    assert_eq!(candidates.len(), selections.len());

    let active_only = ActiveCatalogFixture::new();
    let selections = fixture_selections(&active_only, false);
    let error = capture_current_candidates(active_only.db_path(), &selections).unwrap_err();
    assert!(error.to_string().contains("no current private candidate"));
}

#[test]
fn candidate_supersession_fails_the_final_fence() {
    let fixture = ActiveCatalogFixture::new();
    let selections = fixture_selections(&fixture, true);
    let initial = capture_current_candidates(fixture.db_path(), &selections).unwrap();
    let mut changed = initial.clone();
    changed[0].run_id = uuid::Uuid::new_v4().to_string();

    let error = require_unchanged_candidates(&initial, &changed).unwrap_err();
    assert!(error.to_string().contains("candidate set changed"));
}

#[test]
fn solus_and_noncanonical_digests_cannot_enter_candidate_set() {
    let digest = "a".repeat(64);
    let mut candidates = conary_core::repository::supported_profiles::public_profiles()
        .iter()
        .map(|profile| ProfileRevisionSelection {
            source_profile: profile.id().to_string(),
            profile_revision_sha256: digest.clone(),
        })
        .collect::<Vec<_>>();
    validate_candidate_selections(&candidates).unwrap();

    candidates[0].source_profile = "solus".to_string();
    assert!(
        validate_candidate_selections(&candidates)
            .unwrap_err()
            .to_string()
            .contains("at this ordinal")
    );
    candidates[0].source_profile = "fedora-44".to_string();
    candidates[0].profile_revision_sha256 = "A".repeat(64);
    assert!(
        validate_candidate_selections(&candidates)
            .unwrap_err()
            .to_string()
            .contains("lowercase SHA-256")
    );
}

#[test]
fn serialized_source_authority_must_be_public_and_credential_free() {
    let manifest = valid_manifest();
    let mut source = manifest.profiles[0].sources[0].clone();
    source.provenance.content_url = Some("http://packages.example.test/content".to_string());
    assert!(
        require_public_snapshot(&source)
            .unwrap_err()
            .to_string()
            .contains("non-public or credential-bearing URL")
    );

    source.provenance.content_url =
        Some("https://operator:secret@packages.example.test/content".to_string());
    assert!(
        require_public_snapshot(&source)
            .unwrap_err()
            .to_string()
            .contains("non-public or credential-bearing URL")
    );

    source.provenance.content_url = Some("https://127.0.0.1/content".to_string());
    assert!(
        require_public_snapshot(&source)
            .unwrap_err()
            .to_string()
            .contains("non-public or credential-bearing URL")
    );

    source.provenance.content_url = Some("https://8.8.8.8/content".to_string());
    require_public_snapshot(&source).unwrap();
}

#[test]
fn publisher_copies_retained_bytes_and_reopens_without_network() {
    let temp = tempfile::tempdir().unwrap();
    let output = temp.path().join("bundle");
    let manifest = valid_manifest();
    let source = temp.path().join("retained-object");
    fs::write(&source, b"x").unwrap();
    let sources = vec![ObjectSource {
        object: manifest.objects[0].clone(),
        path: source,
    }];

    publish_bundle(&output, &manifest, sources).unwrap();
    assert_eq!(
        reopen_native_oracle_input_bundle(&output).unwrap(),
        manifest
    );
}

#[test]
fn object_tamper_fails_reopen() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("bundle");
    let manifest = valid_manifest();
    write_bundle(&root, &manifest);
    fs::write(
        root.join(NATIVE_ORACLE_INPUT_OBJECT_DIRECTORY)
            .join(&manifest.objects[0].sha256),
        b"y",
    )
    .unwrap();

    let error = reopen_native_oracle_input_bundle(&root).unwrap_err();
    assert!(error.to_string().contains("reopen native metadata object"));
}

#[test]
fn truncated_object_fails_reopen() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("bundle");
    let manifest = valid_manifest();
    write_bundle(&root, &manifest);
    fs::write(
        root.join(NATIVE_ORACLE_INPUT_OBJECT_DIRECTORY)
            .join(&manifest.objects[0].sha256),
        b"",
    )
    .unwrap();

    let error = reopen_native_oracle_input_bundle(&root).unwrap_err();
    assert!(error.to_string().contains("size or file type drifted"));
}

#[test]
fn extra_entry_fails_reopen() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("bundle");
    let manifest = valid_manifest();
    write_bundle(&root, &manifest);
    fs::write(root.join("surprise"), b"nope").unwrap();

    let error = reopen_native_oracle_input_bundle(&root).unwrap_err();
    assert!(error.to_string().contains("incomplete or unexpected"));
}

#[test]
fn unknown_manifest_field_fails_reopen() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("bundle");
    let manifest = valid_manifest();
    write_bundle(&root, &manifest);
    let mut value = serde_json::to_value(&manifest).unwrap();
    value
        .as_object_mut()
        .unwrap()
        .insert("unexpected".to_string(), serde_json::Value::Bool(true));
    fs::write(
        root.join(NATIVE_ORACLE_INPUT_MANIFEST_FILE),
        serde_json::to_vec(&value).unwrap(),
    )
    .unwrap();

    let error = reopen_native_oracle_input_bundle(&root).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("parse native-oracle input manifest")
    );
}

#[test]
fn noncanonical_manifest_json_fails_reopen() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("bundle");
    let manifest = valid_manifest();
    write_bundle(&root, &manifest);
    fs::write(
        root.join(NATIVE_ORACLE_INPUT_MANIFEST_FILE),
        serde_json::to_vec_pretty(&manifest).unwrap(),
    )
    .unwrap();

    let error = reopen_native_oracle_input_bundle(&root).unwrap_err();
    assert!(error.to_string().contains("not canonical JSON"));
}

#[test]
fn reordered_profiles_fail_reopen() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("bundle");
    let mut manifest = valid_manifest();
    manifest.profiles.swap(0, 1);
    write_bundle(&root, &manifest);

    let error = reopen_native_oracle_input_bundle(&root).unwrap_err();
    assert!(error.to_string().contains("at this ordinal"));
}

#[test]
fn explicit_profile_revision_digest_drift_fails_reopen() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("bundle");
    let mut manifest = valid_manifest();
    manifest.profiles[0].profile_revision_sha256 = "f".repeat(64);
    write_bundle(&root, &manifest);

    let error = reopen_native_oracle_input_bundle(&root).unwrap_err();
    assert!(error.to_string().contains("revision digest drifted"));
}

#[cfg(unix)]
#[test]
fn symlinked_object_fails_reopen() {
    use std::os::unix::fs::symlink;

    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("bundle");
    let manifest = valid_manifest();
    write_bundle(&root, &manifest);
    let object = root
        .join(NATIVE_ORACLE_INPUT_OBJECT_DIRECTORY)
        .join(&manifest.objects[0].sha256);
    fs::remove_file(&object).unwrap();
    symlink(root.join(NATIVE_ORACLE_INPUT_MANIFEST_FILE), &object).unwrap();

    let error = reopen_native_oracle_input_bundle(&root).unwrap_err();
    assert!(error.to_string().contains("size or file type drifted"));
}
