// crates/conary-core/src/repository/catalog/parity/io.rs

//! Canonical streaming native parity artifacts and strict bundle reopen.

use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};

use super::super::ProfileRevisionV2;
use super::super::store::{hash_file, sync_parent, validate_candidate_path};
use super::contract::{
    NativeParityArtifactV1, NativeParityCountsV1, NativeParityImplementationV1,
    NativeParityOracleV1, NativeParityPackageV1,
};
use crate::error::{Error, Result};

pub const NATIVE_PARITY_PACKAGE_FILE_NAME: &str = "packages.jsonl";
pub const NATIVE_PARITY_MANIFEST_FILE_NAME: &str = "manifest.json";

const MAX_NATIVE_PARITY_MANIFEST_BYTES: u64 = 1024 * 1024;

/// Incremental canonical writer for one complete native package fact stream.
pub struct NativeParityOracleWriter {
    path: PathBuf,
    writer: BufWriter<File>,
    hasher: crate::hash::Hasher,
    bytes_written: u64,
    counts: NativeParityCountsV1,
    previous_package_key: Option<String>,
    profile: ProfileRevisionV2,
    implementation: NativeParityImplementationV1,
}

impl NativeParityOracleWriter {
    pub fn create(
        path: impl AsRef<Path>,
        profile: &ProfileRevisionV2,
        implementation: NativeParityImplementationV1,
    ) -> Result<Self> {
        profile.validate()?;
        let path = path.as_ref();
        validate_candidate_path(path)?;
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(path)?;
        Ok(Self {
            path: path.to_path_buf(),
            writer: BufWriter::new(file),
            hasher: crate::hash::Hasher::new(crate::hash::HashAlgorithm::Sha256),
            bytes_written: 0,
            counts: NativeParityCountsV1::default(),
            previous_package_key: None,
            profile: profile.clone(),
            implementation,
        })
    }

    /// Append one exact package in strictly increasing profile package-key
    /// order. Only this package and its relation arrays are retained.
    pub fn package(&mut self, package: &NativeParityPackageV1) -> Result<()> {
        package.validate_authority(
            &self.profile.profile,
            &self.profile.members,
            &self.implementation,
        )?;
        if self
            .previous_package_key
            .as_deref()
            .is_some_and(|previous| previous >= package.package_key_sha256.as_str())
        {
            return Err(Error::ConfigError(format!(
                "native parity package key '{}' is duplicated or noncanonical",
                package.package_key_sha256
            )));
        }
        let bytes = crate::json::canonical_json(package).map_err(|error| {
            Error::ParseError(format!("serialize native parity package: {error}"))
        })?;
        self.writer.write_all(&bytes)?;
        self.writer.write_all(b"\n")?;
        self.hasher.update(&bytes);
        self.hasher.update(b"\n");
        self.bytes_written = checked_add(
            self.bytes_written,
            u64::try_from(bytes.len() + 1).map_err(|_| {
                Error::InternalError("native parity row size exceeds u64".to_string())
            })?,
            "artifact bytes",
        )?;
        self.counts.packages = checked_add(self.counts.packages, 1, "packages")?;
        self.counts.provides = checked_add(
            self.counts.provides,
            u64::try_from(package.provides.len()).map_err(|_| {
                Error::InternalError("native parity provide count exceeds u64".to_string())
            })?,
            "provides",
        )?;
        self.counts.requirement_groups = checked_add(
            self.counts.requirement_groups,
            u64::try_from(package.requirement_groups.len()).map_err(|_| {
                Error::InternalError(
                    "native parity requirement-group count exceeds u64".to_string(),
                )
            })?,
            "requirement groups",
        )?;
        for group in &package.requirement_groups {
            self.counts.requirement_atoms = checked_add(
                self.counts.requirement_atoms,
                u64::try_from(group.atoms.len()).map_err(|_| {
                    Error::InternalError(
                        "native parity requirement-atom count exceeds u64".to_string(),
                    )
                })?,
                "requirement atoms",
            )?;
        }
        self.previous_package_key = Some(package.package_key_sha256.clone());
        Ok(())
    }

    pub fn finish(mut self) -> Result<NativeParityOracleV1> {
        self.writer.flush()?;
        self.writer.get_ref().sync_all()?;
        let metadata = self.writer.get_ref().metadata()?;
        if metadata.len() != self.bytes_written {
            return Err(Error::ConflictError(format!(
                "native parity artifact {} wrote {} bytes but filesystem reports {}",
                self.path.display(),
                self.bytes_written,
                metadata.len()
            )));
        }
        sync_parent(&self.path)?;
        NativeParityOracleV1::bind(
            &self.profile,
            self.implementation,
            NativeParityArtifactV1 {
                sha256: self.hasher.finalize().to_string(),
                size: self.bytes_written,
                counts: self.counts,
            },
        )
    }
}

/// A byte-verified oracle whose normalized rows can be replayed repeatedly.
#[derive(Debug, Clone)]
pub struct NativeParityOracleReader {
    path: PathBuf,
    manifest: NativeParityOracleV1,
}

impl NativeParityOracleReader {
    pub fn open_verified(path: impl AsRef<Path>, manifest: &NativeParityOracleV1) -> Result<Self> {
        manifest.validate()?;
        let path = path.as_ref();
        let metadata = fs::symlink_metadata(path)?;
        if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
            return Err(Error::InvalidPath(format!(
                "native parity artifact {} must be a regular file, never a symlink",
                path.display()
            )));
        }
        if metadata.len() != manifest.artifact.size {
            return Err(Error::ChecksumMismatch {
                expected: format!("{} bytes", manifest.artifact.size),
                actual: format!("{} bytes", metadata.len()),
            });
        }
        let sha256 = hash_file(path)?;
        if sha256 != manifest.artifact.sha256 {
            return Err(Error::ChecksumMismatch {
                expected: manifest.artifact.sha256.clone(),
                actual: sha256,
            });
        }
        Ok(Self {
            path: path.canonicalize()?,
            manifest: manifest.clone(),
        })
    }

    #[must_use]
    pub fn manifest(&self) -> &NativeParityOracleV1 {
        &self.manifest
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Replay canonical rows with one complete package retained at a time.
    pub fn for_each_package(
        &self,
        mut visitor: impl FnMut(NativeParityPackageV1) -> Result<()>,
    ) -> Result<()> {
        let mut cursor = self.cursor()?;
        while let Some(package) = cursor.next_package()? {
            visitor(package)?;
        }
        Ok(())
    }

    pub fn verify_contents(&self) -> Result<()> {
        self.for_each_package(|_| Ok(()))
    }

    pub(super) fn cursor(&self) -> Result<NativeParityOracleCursor> {
        Ok(NativeParityOracleCursor {
            reader: BufReader::new(File::open(&self.path)?),
            path: self.path.clone(),
            manifest: self.manifest.clone(),
            line: Vec::new(),
            previous_package_key: None,
            counts: NativeParityCountsV1::default(),
            finished: false,
        })
    }
}

pub(super) struct NativeParityOracleCursor {
    reader: BufReader<File>,
    path: PathBuf,
    manifest: NativeParityOracleV1,
    line: Vec<u8>,
    previous_package_key: Option<String>,
    counts: NativeParityCountsV1,
    finished: bool,
}

impl NativeParityOracleCursor {
    pub(super) fn next_package(&mut self) -> Result<Option<NativeParityPackageV1>> {
        if self.finished {
            return Ok(None);
        }
        self.line.clear();
        let read = self.reader.read_until(b'\n', &mut self.line)?;
        if read == 0 {
            self.finished = true;
            if self.counts != self.manifest.artifact.counts {
                return Err(Error::ConflictError(format!(
                    "native parity artifact {} row counts do not match its manifest",
                    self.path.display()
                )));
            }
            return Ok(None);
        }
        if self.line.last() != Some(&b'\n') {
            return Err(Error::ParseError(format!(
                "native parity artifact {} has an unterminated final row",
                self.path.display()
            )));
        }
        self.line.pop();
        if self.line.is_empty() || self.line.last() == Some(&b'\r') {
            return Err(Error::ParseError(format!(
                "native parity artifact {} contains an empty or CRLF row",
                self.path.display()
            )));
        }
        let package: NativeParityPackageV1 =
            serde_json::from_slice(&self.line).map_err(|error| {
                Error::ParseError(format!(
                    "parse native parity package in {}: {error}",
                    self.path.display()
                ))
            })?;
        package.validate(&self.manifest)?;
        let canonical = crate::json::canonical_json(&package).map_err(|error| {
            Error::ParseError(format!("serialize native parity package: {error}"))
        })?;
        if canonical != self.line {
            return Err(Error::ConfigError(format!(
                "native parity package '{}' is not canonical JSON",
                package.name
            )));
        }
        if self
            .previous_package_key
            .as_deref()
            .is_some_and(|previous| previous >= package.package_key_sha256.as_str())
        {
            return Err(Error::ConfigError(format!(
                "native parity package key '{}' is duplicated or noncanonical",
                package.package_key_sha256
            )));
        }
        accumulate_counts(&mut self.counts, &package)?;
        self.previous_package_key = Some(package.package_key_sha256.clone());
        Ok(Some(package))
    }
}

pub fn write_native_parity_oracle_manifest(
    directory: impl AsRef<Path>,
    manifest: &NativeParityOracleV1,
) -> Result<()> {
    let directory = directory.as_ref();
    require_real_directory(directory)?;
    let reader = NativeParityOracleReader::open_verified(
        directory.join(NATIVE_PARITY_PACKAGE_FILE_NAME),
        manifest,
    )?;
    reader.verify_contents()?;
    let path = directory.join(NATIVE_PARITY_MANIFEST_FILE_NAME);
    let bytes = crate::json::canonical_json(manifest)
        .map_err(|error| Error::ParseError(format!("serialize native parity manifest: {error}")))?;
    if path.exists() {
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
            return Err(Error::InvalidPath(format!(
                "native parity manifest {} must be a regular file, never a symlink",
                path.display()
            )));
        }
        if fs::read(&path)? == bytes {
            File::open(&path)?.sync_all()?;
            sync_parent(&path)?;
            return Ok(());
        }
        return Err(Error::ConflictError(format!(
            "native parity manifest {} already exists with different bytes",
            path.display()
        )));
    }
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&path)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    sync_parent(&path)
}

pub fn verify_native_parity_oracle_bundle(
    directory: impl AsRef<Path>,
    profile: &ProfileRevisionV2,
) -> Result<NativeParityOracleReader> {
    let directory = directory.as_ref();
    verify_exact_directory(directory)?;
    let manifest_path = directory.join(NATIVE_PARITY_MANIFEST_FILE_NAME);
    let metadata = fs::symlink_metadata(&manifest_path)?;
    if metadata.file_type().is_symlink()
        || !metadata.file_type().is_file()
        || metadata.len() > MAX_NATIVE_PARITY_MANIFEST_BYTES
    {
        return Err(Error::InvalidPath(format!(
            "native parity manifest {} must be a bounded regular file",
            manifest_path.display()
        )));
    }
    let bytes = fs::read(&manifest_path)?;
    let manifest: NativeParityOracleV1 = serde_json::from_slice(&bytes).map_err(|error| {
        Error::ParseError(format!(
            "parse native parity manifest {}: {error}",
            manifest_path.display()
        ))
    })?;
    let canonical = crate::json::canonical_json(&manifest)
        .map_err(|error| Error::ParseError(format!("serialize native parity manifest: {error}")))?;
    if canonical != bytes {
        return Err(Error::ConfigError(format!(
            "native parity manifest {} is not canonical JSON",
            manifest_path.display()
        )));
    }
    manifest.validate_profile(profile)?;
    let reader = NativeParityOracleReader::open_verified(
        directory.join(NATIVE_PARITY_PACKAGE_FILE_NAME),
        &manifest,
    )?;
    reader.verify_contents()?;
    Ok(reader)
}

fn accumulate_counts(
    counts: &mut NativeParityCountsV1,
    package: &NativeParityPackageV1,
) -> Result<()> {
    counts.packages = checked_add(counts.packages, 1, "packages")?;
    counts.provides = checked_add(
        counts.provides,
        u64::try_from(package.provides.len())
            .map_err(|_| Error::InternalError("native parity provides exceed u64".to_string()))?,
        "provides",
    )?;
    counts.requirement_groups = checked_add(
        counts.requirement_groups,
        u64::try_from(package.requirement_groups.len()).map_err(|_| {
            Error::InternalError("native parity requirement groups exceed u64".to_string())
        })?,
        "requirement groups",
    )?;
    for group in &package.requirement_groups {
        counts.requirement_atoms = checked_add(
            counts.requirement_atoms,
            u64::try_from(group.atoms.len()).map_err(|_| {
                Error::InternalError("native parity requirement atoms exceed u64".to_string())
            })?,
            "requirement atoms",
        )?;
    }
    Ok(())
}

fn checked_add(current: u64, increment: u64, label: &str) -> Result<u64> {
    current
        .checked_add(increment)
        .ok_or_else(|| Error::InternalError(format!("native parity {label} count exceeds u64")))
}

fn require_real_directory(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
        return Err(Error::InvalidPath(format!(
            "native parity bundle {} must be a real directory",
            path.display()
        )));
    }
    Ok(())
}

fn verify_exact_directory(path: &Path) -> Result<()> {
    require_real_directory(path)?;
    let mut names = Vec::new();
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if file_type.is_symlink() || !file_type.is_file() {
            return Err(Error::InvalidPath(format!(
                "native parity bundle entry {} must be a regular file",
                entry.path().display()
            )));
        }
        names.push(entry.file_name());
    }
    names.sort();
    let mut expected: Vec<OsString> = vec![
        NATIVE_PARITY_MANIFEST_FILE_NAME.into(),
        NATIVE_PARITY_PACKAGE_FILE_NAME.into(),
    ];
    expected.sort();
    if names != expected {
        return Err(Error::ConflictError(format!(
            "native parity bundle {} must contain exactly {} and {}",
            path.display(),
            NATIVE_PARITY_MANIFEST_FILE_NAME,
            NATIVE_PARITY_PACKAGE_FILE_NAME
        )));
    }
    Ok(())
}
