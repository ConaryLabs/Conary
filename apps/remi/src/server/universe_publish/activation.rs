// apps/remi/src/server/universe_publish/activation.rs

fn activate_candidate(
    db_path: &Path,
    inputs: &UniverseInputs,
    candidate: &SignedUniverseCandidate,
    _bundle: &Path,
) -> Result<()> {
    let conn = conary_core::db::open_fast(db_path)?;
    let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate)?;
    let current = tx
        .query_row(
            "SELECT manifest_sha256, sequence FROM remi_active_universe_revision
             WHERE singleton = 1",
            [],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()?;
    let expected_current = inputs.base_manifest_sha256.as_ref().map(|sha256| {
        (
            sha256.as_str(),
            i64::try_from(inputs.base_sequence).expect("stored sequence already fit i64"),
        )
    });
    if current
        .as_ref()
        .map(|(sha256, sequence)| (sha256.as_str(), *sequence))
        != expected_current
    {
        bail!("active Remi universe changed while a replacement was being built");
    }
    require_profile_inputs_unchanged(&tx, &candidate.manifest)?;
    let current_canonical = load_canonical_map_snapshot(&tx)?;
    let current_canonical_bytes = canonical_bytes(&current_canonical)?;
    if conary_core::hash::sha256(&current_canonical_bytes)
        != candidate.manifest.canonical_map.sha256
    {
        bail!("canonical map changed while a Remi universe was being built");
    }

    let sequence = i64::try_from(candidate.manifest.sequence)
        .context("universe sequence exceeds SQLite integer range")?;
    tx.execute(
        "INSERT INTO remi_universe_revisions (
             manifest_sha256, sequence, promotion_evidence_sha256,
             conversion_crawl_sha256, metadata_root_sha256,
             canonical_map_sha256, canonical_map_size, targets_version,
             snapshot_version, timestamp_version, manifest_json, durable, created_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 1, ?12)",
        params![
            &candidate.manifest_sha256,
            sequence,
            inputs
                .base_promotion_evidence_sha256
                .as_deref()
                .context("active universe has no promotion-evidence binding")?,
            inputs
                .base_conversion_crawl_sha256
                .as_deref()
                .context("active universe has no conversion-crawl binding")?,
            &candidate.manifest.metadata_root_sha256,
            &candidate.manifest.canonical_map.sha256,
            i64::try_from(candidate.manifest.canonical_map.size)
                .context("canonical-map size exceeds SQLite integer range")?,
            i64::try_from(candidate.targets.signed.version)
                .context("targets version exceeds SQLite integer range")?,
            i64::try_from(candidate.snapshot.signed.version)
                .context("snapshot version exceeds SQLite integer range")?,
            i64::try_from(candidate.timestamp.signed.version)
                .context("timestamp version exceeds SQLite integer range")?,
            String::from_utf8(candidate.manifest_bytes.clone())?,
            candidate.manifest.generated_at.timestamp(),
        ],
    )?;
    for profile in &candidate.manifest.profiles {
        tx.execute(
            "INSERT INTO remi_universe_profile_revisions (
                 manifest_sha256, ordinal, source_profile, profile_revision_sha256,
                 catalog_sha256, catalog_size
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                &candidate.manifest_sha256,
                i64::from(profile.ordinal),
                &profile.revision.profile,
                &profile.profile_revision_sha256,
                &profile.catalog.sha256,
                i64::try_from(profile.catalog.size)
                    .context("profile catalog size exceeds SQLite integer range")?,
            ],
        )?;
    }
    tx.execute(
        "INSERT INTO remi_active_universe_revision (
             singleton, manifest_sha256, sequence, activated_at
         ) VALUES (1, ?1, ?2, ?3)
         ON CONFLICT(singleton) DO UPDATE SET
             manifest_sha256 = excluded.manifest_sha256,
             sequence = excluded.sequence,
             activated_at = excluded.activated_at",
        params![&candidate.manifest_sha256, sequence, Utc::now().timestamp(),],
    )?;
    tx.commit()?;
    Ok(())
}

fn require_profile_inputs_unchanged(
    tx: &Transaction<'_>,
    manifest: &RemiUniverseManifestV2,
) -> Result<()> {
    let mut statement = tx.prepare(
        "SELECT source_profile, profile_revision_sha256
         FROM remi_active_profile_revisions
         ORDER BY source_profile COLLATE BINARY",
    )?;
    let active = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let expected = manifest
        .profiles
        .iter()
        .map(|profile| {
            (
                profile.revision.profile.clone(),
                profile.profile_revision_sha256.clone(),
            )
        })
        .collect::<Vec<_>>();
    if active != expected {
        bail!("active profile set changed while a Remi universe was being built");
    }
    Ok(())
}
