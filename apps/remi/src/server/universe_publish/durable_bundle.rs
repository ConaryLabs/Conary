// apps/remi/src/server/universe_publish/durable_bundle.rs

pub(crate) fn publish_candidate_files(
    candidate_root: &Path,
    catalog_dir: &Path,
    candidate: &SignedUniverseCandidate,
    catalog_authority: Option<&super::catalog_authority::CatalogAuthority>,
) -> Result<PathBuf> {
    require_real_directory(candidate_root, "universe candidate root")?;
    require_real_directory(catalog_dir, "catalog root")?;
    if fs::metadata(candidate_root)?.dev() != fs::metadata(catalog_dir)?.dev() {
        bail!("universe candidate and catalog roots must share one filesystem");
    }
    let universes = ensure_real_subdirectory(catalog_dir, "universes")?;
    let destination = universes.join(&candidate.manifest_sha256);
    if destination.exists() {
        verify_published_bundle(
            catalog_dir,
            &candidate.manifest,
            &candidate.manifest_sha256,
            catalog_authority,
        )?;
        return Ok(destination);
    }

    let staged = tempfile::Builder::new()
        .prefix("universe-")
        .tempdir_in(candidate_root)?;
    for (name, bytes) in [
        (UNIVERSE_MANIFEST_FILE, candidate.manifest_bytes.as_slice()),
        (
            UNIVERSE_CANONICAL_MAP_FILE,
            candidate.canonical_map_bytes.as_slice(),
        ),
        (UNIVERSE_ROOT_FILE, candidate.root_bytes.as_slice()),
        (UNIVERSE_TARGETS_FILE, candidate.targets_bytes.as_slice()),
        (UNIVERSE_SNAPSHOT_FILE, candidate.snapshot_bytes.as_slice()),
        (
            UNIVERSE_TIMESTAMP_FILE,
            candidate.timestamp_bytes.as_slice(),
        ),
    ] {
        write_new_file(&staged.path().join(name), bytes)?;
    }
    File::open(staged.path())?.sync_all()?;
    let staged_path = staged.keep();
    fs::rename(&staged_path, &destination)
        .with_context(|| format!("publish signed universe {}", candidate.manifest_sha256))?;
    File::open(&universes)?.sync_all()?;
    verify_published_bundle(
        catalog_dir,
        &candidate.manifest,
        &candidate.manifest_sha256,
        catalog_authority,
    )?;
    Ok(destination)
}

pub(crate) fn verify_published_bundle(
    catalog_dir: &Path,
    expected: &RemiUniverseManifestV2,
    expected_sha256: &str,
    catalog_authority: Option<&super::catalog_authority::CatalogAuthority>,
) -> Result<()> {
    expected.validate().map_err(anyhow::Error::from)?;
    if expected.manifest_sha256()? != expected_sha256 {
        bail!("published universe path identity disagrees with its manifest");
    }
    let directory = universe_bundle_path(catalog_dir, expected_sha256);
    require_real_directory(&directory, "published universe bundle")?;
    let mut names = fs::read_dir(&directory)?
        .map(|entry| entry.map(|entry| entry.file_name()))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    names.sort();
    let mut expected_names = UNIVERSE_FILES.map(std::ffi::OsString::from).to_vec();
    expected_names.sort();
    if names != expected_names {
        bail!("published universe bundle contains an unexpected file set");
    }
    let manifest_bytes = read_plain_file(&directory.join(UNIVERSE_MANIFEST_FILE))?;
    if manifest_bytes != canonical_json(expected, "expected universe manifest")? {
        bail!("published universe manifest bytes disagree with pointer authority");
    }
    let root_bytes = read_plain_file(&directory.join(UNIVERSE_ROOT_FILE))?;
    let targets_bytes = read_plain_file(&directory.join(UNIVERSE_TARGETS_FILE))?;
    let snapshot_bytes = read_plain_file(&directory.join(UNIVERSE_SNAPSHOT_FILE))?;
    let timestamp_bytes = read_plain_file(&directory.join(UNIVERSE_TIMESTAMP_FILE))?;
    let reopened = SignedUniverseCandidate {
        manifest: expected.clone(),
        manifest_sha256: expected_sha256.to_string(),
        manifest_bytes: manifest_bytes.clone(),
        canonical_map_bytes: read_plain_file(&directory.join(UNIVERSE_CANONICAL_MAP_FILE))?,
        root: serde_json::from_slice(&root_bytes).context("parse published universe root")?,
        root_bytes,
        targets: serde_json::from_slice(&targets_bytes)
            .context("parse published universe targets")?,
        targets_bytes,
        snapshot: serde_json::from_slice(&snapshot_bytes)
            .context("parse published universe snapshot")?,
        snapshot_bytes,
        timestamp: serde_json::from_slice(&timestamp_bytes)
            .context("parse published universe timestamp")?,
        timestamp_bytes,
    };
    verify_candidate(&reopened).context("reopen complete signed universe metadata chain")?;
    let targets: &Signed<TargetsMetadata> = &reopened.targets;
    verify_remi_universe_manifest_target(&manifest_bytes, &targets.signed.targets)?;
    let canonical_bytes = reopened.canonical_map_bytes;
    if canonical_bytes.len() as u64 != expected.canonical_map.size
        || conary_core::hash::sha256(&canonical_bytes) != expected.canonical_map.sha256
    {
        bail!("published universe canonical-map object disagrees with its manifest");
    }
    let canonical = conary_core::canonical::parse_snapshot(&canonical_bytes)?;
    if canonical.revision != expected.canonical_map.revision
        || canonical.entries.len() as u64 != expected.canonical_map.entry_count
    {
        bail!("published universe canonical-map facts disagree with its manifest");
    }
    let verified_profiles = validate_canonical_candidate(
        catalog_dir,
        &canonical,
        expected.profiles.iter().map(|profile| &profile.revision),
    )
    .context("revalidate canonical contracts in the published universe")?;
    for profile in verified_profiles {
        if let Some(authority) = catalog_authority {
            authority.remember_verified_profile_reader(
                &profile.profile,
                &profile.profile_revision_sha256,
                profile.reader,
            );
        }
    }
    Ok(())
}

pub(crate) fn canonical_bytes(snapshot: &CanonicalMapSnapshot) -> Result<Vec<u8>> {
    validate_canonical_map_snapshot(snapshot).map_err(anyhow::Error::from)?;
    canonical_json(snapshot, "canonical map")
}

fn canonical_json(value: &impl serde::Serialize, label: &str) -> Result<Vec<u8>> {
    conary_core::json::canonical_json(value)
        .map_err(anyhow::Error::msg)
        .with_context(|| format!("serialize {label}"))
}

pub(crate) fn universe_bundle_path(catalog_dir: &Path, manifest_sha256: &str) -> PathBuf {
    catalog_dir.join("universes").join(manifest_sha256)
}

fn ensure_real_subdirectory(parent: &Path, name: &str) -> Result<PathBuf> {
    require_real_directory(parent, "directory parent")?;
    let path = parent.join(name);
    match fs::create_dir(&path) {
        Ok(()) => File::open(parent)?.sync_all()?,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.into()),
    }
    require_real_directory(&path, name)?;
    Ok(path)
}

fn require_real_directory(path: &Path, label: &str) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("inspect {label} {}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
        bail!("{label} {} must be a real directory", path.display());
    }
    Ok(())
}

fn write_new_file(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o644)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

fn read_plain_file(path: &Path) -> Result<Vec<u8>> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
        bail!("universe file {} must be a plain file", path.display());
    }
    if metadata.permissions().mode() & 0o022 != 0 {
        bail!(
            "universe file {} must not be group/world writable",
            path.display()
        );
    }
    Ok(fs::read(path)?)
}
