// apps/remi/src/server/universe_publish/candidate.rs

pub(crate) fn build_candidate(
    base_sequence: u64,
    profiles: Vec<ProfileRevisionV2>,
    canonical_map_bytes: Vec<u8>,
    root: Signed<conary_core::trust::RootMetadata>,
    keys_root: &Path,
) -> Result<SignedUniverseCandidate> {
    let sequence = base_sequence
        .checked_add(1)
        .context("Remi universe sequence overflow")?;
    let now = Utc::now();
    let root_bytes = canonical_json(&root, "universe root")?;
    let metadata_root_sha256 = conary_core::hash::sha256(&root_bytes);
    let canonical_map = conary_core::canonical::parse_snapshot(&canonical_map_bytes)?;
    let canonical_map_sha256 = conary_core::hash::sha256(&canonical_map_bytes);
    let profile_descriptors = profiles
        .into_iter()
        .enumerate()
        .map(|(index, revision)| {
            let ordinal = u32::try_from(index).context("too many universe profiles")?;
            let profile_revision_sha256 = revision.manifest_sha256()?;
            Ok(RemiUniverseProfileV2 {
                ordinal,
                profile_revision_sha256,
                catalog: RemiUniverseCatalogObjectV2 {
                    schema_version: CATALOG_CONTENT_SCHEMA_V1,
                    sha256: revision.catalog.sha256.clone(),
                    size: revision.catalog.size,
                    logical_digest_sha256: revision.logical_digest_sha256.clone(),
                },
                revision,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let manifest = RemiUniverseManifestV2 {
        schema_version: REMI_UNIVERSE_SCHEMA_V2,
        sequence,
        metadata_root_sha256,
        generated_at: now,
        expires_at: now + Duration::days(7),
        profiles: profile_descriptors,
        canonical_map: RemiUniverseCanonicalMapObjectV2 {
            schema_version: canonical_map.schema_version,
            sha256: canonical_map_sha256,
            size: u64::try_from(canonical_map_bytes.len())
                .context("canonical-map object size exceeds u64")?,
            revision: canonical_map.revision,
            entry_count: u64::try_from(canonical_map.entries.len())
                .context("canonical-map entry count exceeds u64")?,
        },
    };
    manifest.validate().map_err(anyhow::Error::from)?;
    let manifest_sha256 = manifest.manifest_sha256()?;
    let manifest_bytes = canonical_json(&manifest, "universe manifest")?;

    let mut target_descriptions = BTreeMap::new();
    insert_target(
        &mut target_descriptions,
        manifest.target_path()?,
        manifest_sha256.clone(),
        u64::try_from(manifest_bytes.len()).context("universe manifest size exceeds u64")?,
    )?;
    for profile in &manifest.profiles {
        insert_target(
            &mut target_descriptions,
            profile.catalog.target_path(),
            profile.catalog.sha256.clone(),
            profile.catalog.size,
        )?;
    }
    insert_target(
        &mut target_descriptions,
        manifest.canonical_map.target_path(),
        manifest.canonical_map.sha256.clone(),
        manifest.canonical_map.size,
    )?;

    let targets_key = load_universe_role_key(keys_root, UniverseSigningRole::Targets)?;
    let targets = TargetsMetadata {
        type_field: "targets".to_string(),
        spec_version: TUF_SPEC_VERSION.to_string(),
        version: sequence,
        expires: now + Duration::days(30),
        targets: target_descriptions,
    };
    let targets = Signed {
        signatures: vec![sign_tuf_metadata(&targets_key, &targets)?],
        signed: targets,
    };
    let targets_bytes = canonical_json(&targets, "universe targets")?;

    let snapshot_key = load_universe_role_key(keys_root, UniverseSigningRole::Snapshot)?;
    let snapshot = SnapshotMetadata {
        type_field: "snapshot".to_string(),
        spec_version: TUF_SPEC_VERSION.to_string(),
        version: sequence,
        expires: now + Duration::days(7),
        meta: BTreeMap::from([
            (
                "root.json".to_string(),
                metadata_reference(root.signed.version, &root_bytes)?,
            ),
            (
                "targets.json".to_string(),
                metadata_reference(sequence, &targets_bytes)?,
            ),
        ]),
    };
    let snapshot = Signed {
        signatures: vec![sign_tuf_metadata(&snapshot_key, &snapshot)?],
        signed: snapshot,
    };
    let snapshot_bytes = canonical_json(&snapshot, "universe snapshot")?;

    let timestamp_key = load_universe_role_key(keys_root, UniverseSigningRole::Timestamp)?;
    let timestamp = TimestampMetadata {
        type_field: "timestamp".to_string(),
        spec_version: TUF_SPEC_VERSION.to_string(),
        version: sequence,
        expires: now + Duration::days(1),
        meta: BTreeMap::from([(
            "snapshot.json".to_string(),
            metadata_reference(sequence, &snapshot_bytes)?,
        )]),
    };
    let timestamp = Signed {
        signatures: vec![sign_tuf_metadata(&timestamp_key, &timestamp)?],
        signed: timestamp,
    };
    let timestamp_bytes = canonical_json(&timestamp, "universe timestamp")?;

    let candidate = SignedUniverseCandidate {
        manifest,
        manifest_sha256,
        manifest_bytes,
        canonical_map_bytes,
        root,
        root_bytes,
        targets,
        targets_bytes,
        snapshot,
        snapshot_bytes,
        timestamp,
        timestamp_bytes,
    };
    verify_candidate(&candidate)?;
    Ok(candidate)
}

fn insert_target(
    targets: &mut BTreeMap<String, TargetDescription>,
    path: String,
    sha256: String,
    size: u64,
) -> Result<()> {
    let prior = targets.insert(
        path.clone(),
        TargetDescription {
            length: size,
            hashes: BTreeMap::from([("sha256".to_string(), sha256)]),
        },
    );
    if prior.is_some() {
        bail!("universe repeats target path {path}");
    }
    Ok(())
}

fn metadata_reference(version: u64, bytes: &[u8]) -> Result<MetaFile> {
    Ok(MetaFile {
        version,
        length: Some(u64::try_from(bytes.len()).context("TUF metadata size exceeds u64")?),
        hashes: Some(BTreeMap::from([(
            "sha256".to_string(),
            conary_core::hash::sha256(bytes),
        )])),
    })
}

fn verify_candidate(candidate: &SignedUniverseCandidate) -> Result<()> {
    if conary_core::hash::sha256(&candidate.root_bytes) != candidate.manifest.metadata_root_sha256 {
        bail!("universe manifest metadata-root digest disagrees with root bytes");
    }
    let (root_keys, root_threshold) = extract_role_keys(&candidate.root.signed, Role::Root)?;
    verify_root(&candidate.root, &root_keys, root_threshold)?;
    for (role, expires) in [
        (Role::Root, candidate.root.signed.expires),
        (Role::Targets, candidate.targets.signed.expires),
        (Role::Snapshot, candidate.snapshot.signed.expires),
        (Role::Timestamp, candidate.timestamp.signed.expires),
    ] {
        verify_not_expired(role, &expires)?;
    }
    let (targets_keys, targets_threshold) =
        extract_role_keys(&candidate.root.signed, Role::Targets)?;
    verify_signatures(
        &candidate.targets,
        Role::Targets,
        &targets_keys,
        targets_threshold,
    )?;
    let (snapshot_keys, snapshot_threshold) =
        extract_role_keys(&candidate.root.signed, Role::Snapshot)?;
    verify_signatures(
        &candidate.snapshot,
        Role::Snapshot,
        &snapshot_keys,
        snapshot_threshold,
    )?;
    let (timestamp_keys, timestamp_threshold) =
        extract_role_keys(&candidate.root.signed, Role::Timestamp)?;
    verify_signatures(
        &candidate.timestamp,
        Role::Timestamp,
        &timestamp_keys,
        timestamp_threshold,
    )?;
    let targets_ref = candidate
        .snapshot
        .signed
        .meta
        .get("targets.json")
        .context("universe snapshot omits targets.json")?;
    verify_metadata_hash(targets_ref, &candidate.targets_bytes, true)?;
    let snapshot_ref = candidate
        .timestamp
        .signed
        .meta
        .get("snapshot.json")
        .context("universe timestamp omits snapshot.json")?;
    verify_metadata_hash(snapshot_ref, &candidate.snapshot_bytes, true)?;
    verify_static_snapshot_consistency(
        &candidate.snapshot.signed,
        candidate.root.signed.version,
        candidate.targets.signed.version,
    )?;
    verify_remi_universe_manifest_target(
        &candidate.manifest_bytes,
        &candidate.targets.signed.targets,
    )?;
    conary_core::canonical::parse_snapshot(&candidate.canonical_map_bytes)?;
    Ok(())
}
