// conary-core/src/repository/static_repo/package_staging.rs

use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result, anyhow, bail};

use crate::ccs::builder::{BuildResult, write_signed_ccs_package};
use crate::ccs::package::CcsPackage;
use crate::ccs::signing::SigningKeyPair;
use crate::hash;
use crate::packages::traits::PackageFormat;
use crate::repository::static_repo::{StaticPackageEntry, validate_repo_relative_path};

const ATOMIC_WRITE_TEMP_ATTEMPTS: usize = 1024;
static ATOMIC_WRITE_TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Default)]
pub(crate) struct PendingPackageWrites {
    pub(crate) writes: Vec<PendingPackageWrite>,
    committed: bool,
}

pub(crate) struct PendingPackageWrite {
    pub(crate) entry: StaticPackageEntry,
    pub(crate) pending_path: PathBuf,
    pub(crate) final_path: PathBuf,
    pub(crate) promoted: bool,
}

impl PendingPackageWrites {
    pub(crate) fn package_entries(&self) -> Vec<&StaticPackageEntry> {
        self.writes.iter().map(|write| &write.entry).collect()
    }

    pub(crate) fn target_entry(&self, relative: &str) -> Option<(u64, String)> {
        self.writes
            .iter()
            .find(|write| write.entry.path == relative)
            .map(|write| (write.entry.size, write.entry.sha256.clone()))
    }

    pub(crate) fn promote(&mut self) -> Result<()> {
        for write in &mut self.writes {
            if write.final_path.exists() {
                let existing = fs::read(&write.final_path)
                    .with_context(|| format!("read {}", write.final_path.display()))?;
                let pending = fs::read(&write.pending_path)
                    .with_context(|| format!("read {}", write.pending_path.display()))?;
                if existing == pending {
                    fs::remove_file(&write.pending_path)
                        .with_context(|| format!("remove {}", write.pending_path.display()))?;
                    continue;
                }
                bail!(
                    "immutable package artifact {} appeared during publish with different bytes",
                    write.entry.path
                );
            }
            if let Some(parent) = write.final_path.parent() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("create package directory {}", parent.display()))?;
            }
            fs::rename(&write.pending_path, &write.final_path).with_context(|| {
                format!(
                    "promote package {} to {}",
                    write.pending_path.display(),
                    write.final_path.display()
                )
            })?;
            write.promoted = true;
        }
        Ok(())
    }

    pub(crate) fn commit(&mut self) {
        self.committed = true;
    }
}

impl Drop for PendingPackageWrites {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        for write in &self.writes {
            let _ = fs::remove_file(&write.pending_path);
            if write.promoted {
                let _ = fs::remove_file(&write.final_path);
            }
        }
    }
}

pub(crate) fn stage_packages(
    repo_root: &Path,
    package_paths: &[PathBuf],
    publish_key: &SigningKeyPair,
) -> Result<PendingPackageWrites> {
    let mut pending = PendingPackageWrites::default();
    for package_path in package_paths {
        let package = CcsPackage::parse(package_path.to_str().ok_or_else(|| {
            anyhow!(
                "package path is not valid UTF-8: {}",
                package_path.display()
            )
        })?)
        .map_err(anyhow::Error::from)
        .with_context(|| format!("parse CCS package {}", package_path.display()))?;
        let signed_bytes = sign_package_bytes(&package, publish_key)
            .with_context(|| format!("sign CCS package {}", package_path.display()))?;
        let relative = package_relative_path(&package);
        let destination = repo_root.join(&relative);
        if let Some(existing) = read_optional(repo_root, &relative)? {
            if existing != signed_bytes {
                bail!("immutable package artifact {relative} already exists with different bytes");
            }
            continue;
        }
        let pending_path = write_pending_package(&destination, &signed_bytes)?;
        pending.writes.push(PendingPackageWrite {
            entry: package_entry_from_package(&relative, &package, &signed_bytes)?,
            pending_path,
            final_path: destination,
            promoted: false,
        });
    }

    Ok(pending)
}

pub(crate) fn collect_package_entries(repo_root: &Path) -> Result<Vec<StaticPackageEntry>> {
    let packages_root = repo_root.join("packages");
    if !packages_root.exists() {
        return Ok(Vec::new());
    }

    let mut package_paths = Vec::new();
    collect_ccs_paths(&packages_root, &mut package_paths)?;
    package_paths.sort();

    let mut entries = Vec::new();
    for path in package_paths {
        let relative = path
            .strip_prefix(repo_root)
            .map_err(|_| anyhow!("package path escaped repo root: {}", path.display()))?
            .to_string_lossy()
            .replace('\\', "/");
        validate_repo_relative_path(&relative)?;
        let package = CcsPackage::parse(
            path.to_str()
                .ok_or_else(|| anyhow!("package path is not valid UTF-8: {}", path.display()))?,
        )
        .map_err(anyhow::Error::from)
        .with_context(|| format!("parse published CCS package {}", path.display()))?;
        let bytes = fs::read(&path).with_context(|| format!("read package {}", path.display()))?;
        entries.push(package_entry_from_package(&relative, &package, &bytes)?);
    }

    Ok(entries)
}

fn write_pending_package(final_path: &Path, bytes: &[u8]) -> Result<PathBuf> {
    if let Some(parent) = final_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create package directory {}", parent.display()))?;
    }
    let (pending_path, mut file) = create_atomic_temp_file(final_path)?;
    if let Err(error) = write_atomic_temp_file(&pending_path, &mut file, bytes) {
        drop(file);
        let _ = fs::remove_file(&pending_path);
        return Err(error);
    }
    drop(file);
    Ok(pending_path)
}

fn sign_package_bytes(package: &CcsPackage, publish_key: &SigningKeyPair) -> Result<Vec<u8>> {
    let build_result = BuildResult {
        manifest: package.manifest().clone(),
        components: package.components().clone(),
        files: package.file_entries().to_vec(),
        blobs: package.extract_all_content().map_err(anyhow::Error::from)?,
        total_size: package.file_entries().iter().map(|entry| entry.size).sum(),
        chunked: package
            .file_entries()
            .iter()
            .any(|entry| entry.chunks.is_some()),
        chunk_stats: None,
    };
    let signed_package = tempfile::NamedTempFile::new()?;
    write_signed_ccs_package(&build_result, signed_package.path(), publish_key)?;
    fs::read(signed_package.path())
        .with_context(|| format!("read signed package {}", signed_package.path().display()))
}

fn package_relative_path(package: &CcsPackage) -> String {
    let arch = package.architecture().unwrap_or("noarch");
    format!(
        "packages/{}/{}-{}-1-{}.ccs",
        package.name(),
        package.name(),
        package.version(),
        arch
    )
}

fn package_entry_from_package(
    relative: &str,
    package: &CcsPackage,
    bytes: &[u8],
) -> Result<StaticPackageEntry> {
    let (name, version, release, arch) = parse_package_filename(relative)?;
    if package.name() != name || package.version() != version {
        bail!(
            "package metadata {}-{} does not match artifact path {}-{}",
            package.name(),
            package.version(),
            name,
            version
        );
    }
    if package.architecture().unwrap_or("noarch") != arch {
        bail!(
            "package architecture {:?} does not match artifact path {arch}",
            package.architecture()
        );
    }
    Ok(StaticPackageEntry {
        name,
        version,
        release,
        arch,
        path: relative.to_string(),
        sha256: hash::sha256(bytes),
        size: bytes.len() as u64,
        description: package.description().map(str::to_string),
        dependencies: package
            .dependencies()
            .iter()
            .map(|dep| match &dep.version {
                Some(version) => format!("{} {}", dep.name, version),
                None => dep.name.clone(),
            })
            .collect(),
    })
}

fn collect_ccs_paths(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(dir).with_context(|| format!("read directory {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            collect_ccs_paths(&path, out)?;
        } else if file_type.is_file() && path.extension().is_some_and(|ext| ext == "ccs") {
            out.push(path);
        }
    }
    Ok(())
}

fn parse_package_filename(relative: &str) -> Result<(String, String, String, String)> {
    let filename = relative
        .rsplit('/')
        .next()
        .ok_or_else(|| anyhow!("package path has no filename: {relative}"))?;
    let stem = filename
        .strip_suffix(".ccs")
        .ok_or_else(|| anyhow!("package path is not a .ccs artifact: {relative}"))?;
    let mut parts = stem.rsplitn(4, '-').collect::<Vec<_>>();
    if parts.len() != 4 {
        bail!("package filename must be <name>-<version>-<release>-<arch>.ccs: {relative}");
    }
    parts.reverse();
    Ok((
        parts[0].to_string(),
        parts[1].to_string(),
        parts[2].to_string(),
        parts[3].to_string(),
    ))
}

fn read_optional(root: &Path, relative: &str) -> Result<Option<Vec<u8>>> {
    validate_repo_relative_path(relative)?;
    let path = root.join(relative);
    match fs::read(&path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("read {}", path.display())),
    }
}

fn create_atomic_temp_file(path: &Path) -> Result<(PathBuf, File)> {
    for _ in 0..ATOMIC_WRITE_TEMP_ATTEMPTS {
        let tmp = unique_atomic_temp_path(path);
        match OpenOptions::new().write(true).create_new(true).open(&tmp) {
            Ok(file) => return Ok((tmp, file)),
            Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(error).with_context(|| format!("create temp file {}", tmp.display()));
            }
        }
    }

    bail!(
        "failed to create unique temp file next to {} after {} attempts",
        path.display(),
        ATOMIC_WRITE_TEMP_ATTEMPTS
    )
}

fn unique_atomic_temp_path(path: &Path) -> PathBuf {
    let suffix = ATOMIC_WRITE_TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("atomic-write");
    path.with_file_name(format!(".{filename}.tmp.{}.{}", std::process::id(), suffix))
}

fn write_atomic_temp_file(path: &Path, file: &mut File, bytes: &[u8]) -> Result<()> {
    file.write_all(bytes)
        .with_context(|| format!("write temp file {}", path.display()))?;
    file.sync_all()
        .with_context(|| format!("sync temp file {}", path.display()))
}
