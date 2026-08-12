// apps/conary/src/commands/adopt/convert.rs

//! Exact native-artifact conversion for already-adopted packages.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};
use conary_core::ccs::TrustPolicy;
use conary_core::db::models::{
    Changeset, ChangesetStatus, ConvertedPackage, PackagePayloadEntry, PackagePayloadOwnership,
    Repository, RepositoryPackage,
};
use conary_core::packages::{InstalledPackageIdentity, PackageFormat, PackageFormatType};
use conary_core::payload::{PayloadContentAuthority, PayloadNode, PayloadNodeKind};
use conary_core::repository::selector::PackageWithRepo;
use conary_core::repository::{
    DownloadOptions, PackageSelector, RepositoryFormat, download_package_verified,
    verify_cached_package_verified,
};
use tempfile::{NamedTempFile, TempDir};

use super::super::generation::selected_root::LockedRuntimeRoot;
use super::super::install::{convert_native_package_to_ccs, resolve_native_payload_nodes};
use super::super::{InstalledPackageSelector, detect_package_format, open_db};

#[derive(Debug, thiserror::Error)]
enum AdoptedConversionError {
    #[error("adopted package '{package}' has no exact native package identity")]
    MissingNativeIdentity { package: String },
    #[error("no enrolled native repository contains exact artifact {identity}")]
    ArtifactUnavailable { identity: String },
    #[error("exact artifact {identity} is ambiguous across repositories: {repositories}")]
    AmbiguousArtifact {
        identity: String,
        repositories: String,
    },
    #[error(
        "resolved artifact identity mismatch for {field}: expected {expected:?}, got {actual:?}"
    )]
    IdentityMismatch {
        field: &'static str,
        expected: String,
        actual: String,
    },
    #[error("resolved artifact is missing adopted payload path {path}")]
    MissingPayloadPath { path: String },
    #[error("resolved artifact has unexpected payload path {path}")]
    UnexpectedPayloadPath { path: String },
    #[error("resolved artifact payload mismatch at {path}: {detail}")]
    PayloadMismatch { path: String, detail: String },
}

struct AdoptedConversionPlan {
    trove_id: i64,
    identity: InstalledPackageIdentity,
    payload: PackagePayloadOwnership,
    source: PackageWithRepo,
}

struct AcquiredArtifact {
    path: PathBuf,
    _download: Option<TempDir>,
}

pub async fn cmd_adopt_convert(
    packages: &[String],
    version: Option<&str>,
    architecture: Option<&str>,
    db_path: &str,
    dry_run: bool,
) -> Result<()> {
    if packages.is_empty() {
        bail!("Specify at least one adopted package to convert");
    }
    if packages.len() > 1 && (version.is_some() || architecture.is_some()) {
        bail!("--version and --arch may be used only with one adopted package");
    }

    if dry_run {
        let conn = open_db(db_path)?;
        for package in packages {
            let plan = plan_conversion(&conn, package, version, architecture)?;
            render_plan(&plan);
        }
        crate::ui::note("Dry run: no artifacts were acquired, converted, or published");
        return Ok(());
    }

    let _locked_root = LockedRuntimeRoot::acquire(db_path)
        .context("failed to acquire the runtime mutation lock for adopted conversion")?;
    let mut conn = open_db(db_path)?;
    for package in packages {
        let plan = plan_conversion(&conn, package, version, architecture)?;
        if current_conversion_is_usable(&conn, &plan)? {
            crate::ui::note(&format!(
                "{} already has a verified current CCS artifact",
                plan.identity.selector()
            ));
            continue;
        }
        let acquired = acquire_exact_artifact(db_path, &plan.source).await?;
        let format = detect_package_format(
            acquired
                .path
                .to_str()
                .context("native artifact cache path is not UTF-8")?,
        )?;
        let parsed = conary_core::packages::parse_package(&acquired.path)?;
        validate_native_identity(parsed.as_ref(), format, &plan.identity)?;
        validate_payload_equivalence(parsed.as_ref(), format, &plan.payload)?;

        let source_profile = plan.source.package.source_profile.as_deref().or(plan
            .source
            .repository
            .source_profile
            .as_deref());
        let converted =
            convert_native_package_to_ccs(parsed.as_ref(), &acquired.path, format, source_profile)?;
        publish_conversion(&mut conn, db_path, &plan, converted)?;
        crate::ui::status(
            "Converted",
            &format!("{} to verified CCS", plan.identity.selector()),
        );
    }
    Ok(())
}

fn plan_conversion(
    conn: &rusqlite::Connection,
    package: &str,
    version: Option<&str>,
    architecture: Option<&str>,
) -> Result<AdoptedConversionPlan> {
    let installed = super::super::resolve_installed_package(
        conn,
        &InstalledPackageSelector::new(
            package.to_string(),
            version.map(str::to_string),
            architecture.map(str::to_string),
        ),
    )?;
    if !installed.trove.install_source.is_adopted() {
        bail!(
            "Package '{}' is not under adopted native authority",
            package
        );
    }
    let identity = installed
        .trove
        .native_package_identity
        .clone()
        .ok_or_else(|| AdoptedConversionError::MissingNativeIdentity {
            package: package.to_string(),
        })?;
    identity.validate()?;
    let payload = PackagePayloadOwnership::load(conn, installed.trove_id)?;
    let source = resolve_exact_repository_artifact(conn, &identity)?;
    Ok(AdoptedConversionPlan {
        trove_id: installed.trove_id,
        identity,
        payload,
        source,
    })
}

fn resolve_exact_repository_artifact(
    conn: &rusqlite::Connection,
    identity: &InstalledPackageIdentity,
) -> Result<PackageWithRepo> {
    let expected_format = repository_format(identity);
    let mut candidates = Vec::new();
    for package in RepositoryPackage::find_by_name(conn, identity.name())? {
        if package.version != identity.version()
            || package.architecture.as_deref() != Some(identity.architecture())
            || package.version_scheme != identity.version_scheme()
        {
            continue;
        }
        let repository = Repository::find_by_id(conn, package.repository_id)?
            .context("exact repository package references a missing repository")?;
        if !repository.enabled || repository.package_format != expected_format {
            continue;
        }
        repository.require_source_policy()?;
        repository.require_trust_policy()?;
        candidates.push(PackageWithRepo {
            package,
            repository,
        });
    }
    if candidates.is_empty() {
        return Err(AdoptedConversionError::ArtifactUnavailable {
            identity: identity.selector().to_string(),
        }
        .into());
    }
    PackageSelector::select_best(candidates).map_err(|error| {
        if matches!(error, conary_core::Error::AmbiguousPackageSelection { .. }) {
            AdoptedConversionError::AmbiguousArtifact {
                identity: identity.selector().to_string(),
                repositories: error.to_string(),
            }
            .into()
        } else {
            anyhow::Error::from(error)
        }
    })
}

fn repository_format(identity: &InstalledPackageIdentity) -> RepositoryFormat {
    match identity {
        InstalledPackageIdentity::Rpm { .. } => RepositoryFormat::Fedora,
        InstalledPackageIdentity::Dpkg { .. } => RepositoryFormat::Debian,
        InstalledPackageIdentity::Pacman { .. } => RepositoryFormat::Arch,
    }
}

fn package_format(identity: &InstalledPackageIdentity) -> PackageFormatType {
    match identity {
        InstalledPackageIdentity::Rpm { .. } => PackageFormatType::Rpm,
        InstalledPackageIdentity::Dpkg { .. } => PackageFormatType::Deb,
        InstalledPackageIdentity::Pacman { .. } => PackageFormatType::Arch,
    }
}

fn render_plan(plan: &AdoptedConversionPlan) {
    crate::ui::row(
        crate::ui::Status::Pending,
        &[
            plan.identity.selector(),
            &format!("repository={}", plan.source.repository.name),
            &format!("checksum={}", plan.source.package.checksum),
            &format!("payload_nodes={}", plan.payload.entries().len()),
        ],
    );
}

async fn acquire_exact_artifact(
    db_path: &str,
    source: &PackageWithRepo,
) -> Result<AcquiredArtifact> {
    let digest = exact_sha256(&source.package.checksum)?;
    let cache_dir = conary_core::db::paths::db_dir(db_path)
        .join("native-artifacts")
        .join("sha256");
    fs::create_dir_all(&cache_dir).with_context(|| {
        format!(
            "failed to create native artifact cache {}",
            cache_dir.display()
        )
    })?;
    let cache_path = cache_dir.join(digest);
    let trust = DownloadOptions::for_repository(
        &source.repository,
        &conary_core::db::paths::keyring_dir(db_path),
    )?;

    if cache_path.exists() {
        match verify_cached_package_verified(&source.package, &cache_path, &trust).await {
            Ok(()) => {
                return Ok(AcquiredArtifact {
                    path: cache_path,
                    _download: None,
                });
            }
            Err(error) => {
                fs::remove_file(&cache_path).with_context(|| {
                    format!(
                        "cached native artifact {} failed authority verification ({error}) and could not be removed",
                        cache_path.display()
                    )
                })?;
            }
        }
    }

    let download_dir = conary_core::db::paths::temp_dir(db_path);
    fs::create_dir_all(&download_dir).with_context(|| {
        format!(
            "failed to create native artifact download directory {}",
            download_dir.display()
        )
    })?;
    let download = tempfile::tempdir_in(download_dir)
        .context("failed to create native artifact download staging")?;
    let downloaded = download_package_verified(&source.package, download.path(), &trust).await?;
    let mut staged = NamedTempFile::new_in(&cache_dir)
        .context("failed to create native artifact cache staging")?;
    let mut input = File::open(&downloaded)?;
    std::io::copy(&mut input, staged.as_file_mut())?;
    staged.as_file_mut().flush()?;
    staged.as_file().sync_all()?;
    match staged.persist_noclobber(&cache_path) {
        Ok(_) => {}
        Err(error) if cache_path.exists() => {
            drop(error.file);
        }
        Err(error) => return Err(error.error.into()),
    }
    verify_cached_package_verified(&source.package, &cache_path, &trust).await?;
    Ok(AcquiredArtifact {
        path: cache_path,
        _download: Some(download),
    })
}

fn exact_sha256(checksum: &str) -> Result<String> {
    let value = checksum.strip_prefix("sha256:").unwrap_or(checksum);
    let hash = conary_core::hash::Hash::new(conary_core::hash::HashAlgorithm::Sha256, value)?;
    Ok(hash.value)
}

fn validate_native_identity(
    package: &dyn PackageFormat,
    format: PackageFormatType,
    identity: &InstalledPackageIdentity,
) -> Result<()> {
    for (field, expected, actual) in [
        (
            "name",
            identity.name().to_string(),
            package.name().to_string(),
        ),
        ("version", identity.version(), package.version().to_string()),
        (
            "architecture",
            identity.architecture().to_string(),
            package.architecture().unwrap_or("").to_string(),
        ),
        (
            "version scheme",
            identity.version_scheme().as_str().to_string(),
            package.version_scheme().as_str().to_string(),
        ),
        (
            "package format",
            format!("{:?}", package_format(identity)),
            format!("{format:?}"),
        ),
    ] {
        if expected != actual {
            return Err(AdoptedConversionError::IdentityMismatch {
                field,
                expected,
                actual,
            }
            .into());
        }
    }
    Ok(())
}

fn validate_payload_equivalence(
    package: &dyn PackageFormat,
    format: PackageFormatType,
    adopted: &PackagePayloadOwnership,
) -> Result<()> {
    let payload = package.package_payload()?;
    let resolved_nodes = resolve_native_payload_nodes(
        Path::new("/"),
        payload.files().iter().map(|file| file.node.clone()),
        format,
    )?;
    let mut artifact = BTreeMap::new();
    for (file, node) in payload.files().iter().zip(resolved_nodes) {
        let path = canonical_payload_path(&file.path)?;
        if artifact
            .insert(path.clone(), (node, file.content_authority.clone()))
            .is_some()
        {
            return Err(AdoptedConversionError::PayloadMismatch {
                path,
                detail: "source artifact declares the path more than once".to_string(),
            }
            .into());
        }
    }
    let installed = adopted
        .entries()
        .iter()
        .map(|entry| (entry.path.clone(), entry))
        .collect::<BTreeMap<_, _>>();

    for path in installed.keys().collect::<BTreeSet<_>>() {
        if !artifact.contains_key(path) {
            return Err(AdoptedConversionError::MissingPayloadPath {
                path: (*path).clone(),
            }
            .into());
        }
    }
    for path in artifact.keys().collect::<BTreeSet<_>>() {
        if !installed.contains_key(path) {
            return Err(AdoptedConversionError::UnexpectedPayloadPath {
                path: (*path).clone(),
            }
            .into());
        }
    }
    for (path, (node, content)) in artifact {
        compare_payload_entry(&path, node, content, installed[&path])?;
    }
    Ok(())
}

fn canonical_payload_path(value: &str) -> Result<String> {
    if value.is_empty() || value.contains('\0') {
        bail!("native artifact contains an empty or NUL-bearing payload path");
    }
    let relative = value.strip_prefix('/').unwrap_or(value);
    if relative.is_empty()
        || relative
            .split('/')
            .any(|component| component.is_empty() || matches!(component, "." | ".."))
    {
        bail!("native artifact payload path {value:?} is not canonical");
    }
    let absolute = if value.starts_with('/') {
        PathBuf::from(value)
    } else {
        Path::new("/").join(value)
    };
    if absolute.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::CurDir | Component::Prefix(_)
        )
    }) {
        bail!("native artifact payload path {value:?} is not canonical");
    }
    Ok(absolute.to_string_lossy().into_owned())
}

fn compare_payload_entry(
    path: &str,
    artifact_node: conary_core::payload::ResolvedPayloadNode,
    artifact_content: Option<PayloadContentAuthority>,
    installed: &PackagePayloadEntry,
) -> Result<()> {
    let comparable = |node: &PayloadNode| {
        (
            comparable_kind(&node.kind),
            node.mode,
            node.mtime,
            node.xattrs.clone(),
        )
    };
    if comparable(&artifact_node.source) != comparable(&installed.node.source) {
        return Err(AdoptedConversionError::PayloadMismatch {
            path: path.to_string(),
            detail: "node kind, mode, timestamp, or xattrs differ".to_string(),
        }
        .into());
    }
    if artifact_node.uid != installed.node.uid || artifact_node.gid != installed.node.gid {
        return Err(AdoptedConversionError::PayloadMismatch {
            path: path.to_string(),
            detail: format!(
                "resolved ownership differs (artifact {}:{}, adopted {}:{})",
                artifact_node.uid, artifact_node.gid, installed.node.uid, installed.node.gid
            ),
        }
        .into());
    }
    if artifact_content != installed.content {
        return Err(AdoptedConversionError::PayloadMismatch {
            path: path.to_string(),
            detail: "content size or SHA-256 differs".to_string(),
        }
        .into());
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum ComparableNodeKind {
    Regular(bool),
    Directory,
    Symlink(String),
    Hardlink(String),
    BlockDevice(u64, u64),
    CharacterDevice(u64, u64),
    Fifo,
    Socket,
}

fn comparable_kind(kind: &PayloadNodeKind) -> ComparableNodeKind {
    match kind {
        PayloadNodeKind::Regular { hardlink_identity } => {
            ComparableNodeKind::Regular(hardlink_identity.is_some())
        }
        PayloadNodeKind::Directory => ComparableNodeKind::Directory,
        PayloadNodeKind::Symlink { target } => ComparableNodeKind::Symlink(target.clone()),
        PayloadNodeKind::Hardlink { target, .. } => ComparableNodeKind::Hardlink(target.clone()),
        PayloadNodeKind::BlockDevice { major, minor } => {
            ComparableNodeKind::BlockDevice(*major, *minor)
        }
        PayloadNodeKind::CharacterDevice { major, minor } => {
            ComparableNodeKind::CharacterDevice(*major, *minor)
        }
        PayloadNodeKind::Fifo => ComparableNodeKind::Fifo,
        PayloadNodeKind::Socket => ComparableNodeKind::Socket,
    }
}

fn current_conversion_is_usable(
    conn: &rusqlite::Connection,
    plan: &AdoptedConversionPlan,
) -> Result<bool> {
    let Some(existing) = ConvertedPackage::find_by_trove(conn, plan.trove_id)? else {
        return Ok(false);
    };
    if existing.needs_reconversion() {
        return Ok(false);
    }
    let Some(path) = existing.ccs_path.as_deref() else {
        return Ok(false);
    };
    let key = super::super::ccs::load_or_create_local_dev_key()?;
    Ok(conary_core::ccs::verify::verify_package(
        Path::new(path),
        &TrustPolicy::strict(vec![key.public_key_base64()]),
    )
    .is_ok())
}

fn publish_conversion(
    conn: &mut rusqlite::Connection,
    db_path: &str,
    plan: &AdoptedConversionPlan,
    converted: super::super::install::NativeCcsConversion,
) -> Result<()> {
    let output_dir = conary_core::db::paths::db_dir(db_path)
        .join("packages")
        .join("adopted");
    fs::create_dir_all(&output_dir)?;
    let checksum = exact_sha256(&plan.source.package.checksum)?;
    let file_name = conary_core::filesystem::path::sanitize_filename(&format!(
        "{}-{}-{}-v{}-{}.ccs",
        plan.identity.name(),
        plan.identity.version(),
        plan.identity.architecture(),
        conary_core::db::models::CONVERSION_VERSION,
        &checksum[..16]
    ))?;
    let final_path = output_dir.join(file_name);
    let mut staged = NamedTempFile::new_in(&output_dir)?;
    let mut input = File::open(&converted.ccs_path)?;
    std::io::copy(&mut input, staged.as_file_mut())?;
    staged.as_file_mut().flush()?;
    staged.as_file().sync_all()?;
    let policy = TrustPolicy::strict(vec![converted.signing_public_key.clone()]);
    conary_core::ccs::verify::verify_package(staged.path(), &policy)
        .context("same-directory adopted CCS staging failed verification")?;

    let published_new = if final_path.exists() {
        if file_sha256(staged.path())? != file_sha256(&final_path)? {
            bail!(
                "refusing to replace different adopted CCS artifact at {}",
                final_path.display()
            );
        }
        conary_core::ccs::verify::verify_package(&final_path, &policy)
            .context("existing adopted CCS output failed verification")?;
        false
    } else {
        staged
            .persist_noclobber(&final_path)
            .map_err(|error| error.error)?;
        sync_directory(&output_dir)?;
        true
    };

    let db_result = (|| -> Result<()> {
        let tx = conn.unchecked_transaction()?;
        ConvertedPackage::delete_installed_by_trove(&tx, plan.trove_id)?;
        let mut record = converted.pending_record.into_record(plan.trove_id)?;
        record.set_installed_ccs_path(final_path.to_string_lossy().into_owned())?;
        record.insert(&tx)?;
        let mut changeset = Changeset::new(format!(
            "Convert adopted package {} to exact CCS",
            plan.identity.selector()
        ));
        changeset.tx_uuid = Some(uuid::Uuid::new_v4().to_string());
        changeset.insert(&tx)?;
        changeset.update_status(&tx, ChangesetStatus::Applied)?;
        tx.commit()?;
        Ok(())
    })();
    if let Err(error) = db_result {
        if published_new {
            let _ = fs::remove_file(&final_path);
            let _ = sync_directory(&output_dir);
        }
        return Err(error);
    }
    Ok(())
}

fn file_sha256(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    Ok(conary_core::hash::hash_reader(conary_core::hash::HashAlgorithm::Sha256, &mut file)?.value)
}

fn sync_directory(path: &Path) -> Result<()> {
    File::open(path)?.sync_all()?;
    Ok(())
}

#[cfg(test)]
#[path = "convert/tests.rs"]
mod tests;
