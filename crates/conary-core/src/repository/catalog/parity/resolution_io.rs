// crates/conary-core/src/repository/catalog/parity/resolution_io.rs

//! Canonical native-resolution artifacts and independent complete reopen.

use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};

use rusqlite::{Connection, params};

use super::super::ProfileRevisionV2;
use super::super::store::{hash_file, sync_parent, validate_candidate_path};
use super::contract::{NativeParityImplementationV1, NativeParityOracleV1};
use super::io::NativeParityOracleReader;
use super::resolution_contract::{
    NativeResolutionArtifactV1, NativeResolutionCountsV1, NativeResolutionOracleV1,
    NativeResolutionOutcomeV1, NativeResolutionPolicyV1, NativeResolutionRootV1,
    native_requirement_group_sha256,
};
use crate::error::{Error, Result};
use crate::repository::dependency_model::RepositoryRequirementKind;

pub const NATIVE_RESOLUTION_ROOT_FILE_NAME: &str = "roots.jsonl";
pub const NATIVE_RESOLUTION_MANIFEST_FILE_NAME: &str = "manifest.json";

const MAX_NATIVE_RESOLUTION_MANIFEST_BYTES: u64 = 1024 * 1024;

/// Incremental writer retaining one complete root outcome at a time.
pub struct NativeResolutionOracleWriter {
    path: PathBuf,
    writer: BufWriter<File>,
    hasher: crate::hash::Hasher,
    bytes_written: u64,
    counts: NativeResolutionCountsV1,
    previous_root_key: Option<String>,
    profile: ProfileRevisionV2,
    package_oracle: NativeParityOracleV1,
    implementation: NativeParityImplementationV1,
    policy: NativeResolutionPolicyV1,
}

impl NativeResolutionOracleWriter {
    pub fn create(
        path: impl AsRef<Path>,
        profile: &ProfileRevisionV2,
        package_oracle: &NativeParityOracleV1,
        implementation: NativeParityImplementationV1,
        policy: NativeResolutionPolicyV1,
    ) -> Result<Self> {
        package_oracle.validate_profile(profile)?;
        policy.validate()?;
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
            counts: NativeResolutionCountsV1::default(),
            previous_root_key: None,
            profile: profile.clone(),
            package_oracle: package_oracle.clone(),
            implementation,
            policy,
        })
    }

    pub fn root(&mut self, root: &NativeResolutionRootV1) -> Result<()> {
        root.validate()?;
        if self
            .previous_root_key
            .as_deref()
            .is_some_and(|previous| previous >= root.root_package_key_sha256.as_str())
        {
            return Err(Error::ConfigError(format!(
                "native resolution root key '{}' is duplicated or noncanonical",
                root.root_package_key_sha256
            )));
        }
        let bytes = crate::json::canonical_json(root).map_err(|error| {
            Error::ParseError(format!("serialize native resolution root: {error}"))
        })?;
        self.writer.write_all(&bytes)?;
        self.writer.write_all(b"\n")?;
        self.hasher.update(&bytes);
        self.hasher.update(b"\n");
        self.bytes_written = checked_add(
            self.bytes_written,
            u64::try_from(bytes.len() + 1).map_err(|_| {
                Error::InternalError("native resolution row size exceeds u64".to_string())
            })?,
            "artifact bytes",
        )?;
        accumulate_counts(&mut self.counts, root)?;
        self.previous_root_key = Some(root.root_package_key_sha256.clone());
        Ok(())
    }

    pub fn finish(mut self) -> Result<NativeResolutionOracleV1> {
        self.writer.flush()?;
        self.writer.get_ref().sync_all()?;
        let metadata = self.writer.get_ref().metadata()?;
        if metadata.len() != self.bytes_written {
            return Err(Error::ConflictError(format!(
                "native resolution artifact {} wrote {} bytes but filesystem reports {}",
                self.path.display(),
                self.bytes_written,
                metadata.len()
            )));
        }
        sync_parent(&self.path)?;
        NativeResolutionOracleV1::bind(
            &self.profile,
            &self.package_oracle,
            self.implementation,
            self.policy,
            NativeResolutionArtifactV1 {
                sha256: self.hasher.finalize().to_string(),
                size: self.bytes_written,
                counts: self.counts,
            },
        )
    }
}

/// Byte-verified resolution evidence replayed one root outcome at a time.
#[derive(Debug, Clone)]
pub struct NativeResolutionOracleReader {
    path: PathBuf,
    manifest: NativeResolutionOracleV1,
}

impl NativeResolutionOracleReader {
    pub fn open_verified(
        path: impl AsRef<Path>,
        manifest: &NativeResolutionOracleV1,
    ) -> Result<Self> {
        manifest.validate()?;
        let path = path.as_ref();
        let metadata = fs::symlink_metadata(path)?;
        if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
            return Err(Error::InvalidPath(format!(
                "native resolution artifact {} must be a regular file, never a symlink",
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
    pub fn manifest(&self) -> &NativeResolutionOracleV1 {
        &self.manifest
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn for_each_root(
        &self,
        mut visitor: impl FnMut(NativeResolutionRootV1) -> Result<()>,
    ) -> Result<()> {
        let mut cursor = self.cursor()?;
        while let Some(root) = cursor.next_root()? {
            visitor(root)?;
        }
        Ok(())
    }

    pub fn verify_contents(&self) -> Result<()> {
        self.for_each_root(|_| Ok(()))
    }

    pub fn verify_package_oracle(&self, package_oracle: &NativeParityOracleReader) -> Result<()> {
        if self.manifest.package_oracle_manifest_sha256
            != package_oracle.manifest().manifest_sha256()?
        {
            return Err(Error::ConflictError(
                "native resolution evidence binds a different package oracle".to_string(),
            ));
        }
        if self.manifest.profile != package_oracle.manifest().profile
            || self.manifest.profile_revision_sha256
                != package_oracle.manifest().profile_revision_sha256
            || self.manifest.profile_logical_digest_sha256
                != package_oracle.manifest().profile_logical_digest_sha256
            || self.manifest.members != package_oracle.manifest().members
            || self.manifest.implementation.ecosystem
                != package_oracle.manifest().implementation.ecosystem
        {
            return Err(Error::ConflictError(
                "native resolution evidence and package oracle bindings disagree".to_string(),
            ));
        }
        validate_package_references(self, package_oracle)?;
        validate_root_coverage(self, package_oracle)
    }

    pub(super) fn cursor(&self) -> Result<NativeResolutionOracleCursor> {
        Ok(NativeResolutionOracleCursor {
            reader: BufReader::new(File::open(&self.path)?),
            path: self.path.clone(),
            manifest: self.manifest.clone(),
            line: Vec::new(),
            previous_root_key: None,
            counts: NativeResolutionCountsV1::default(),
            finished: false,
        })
    }
}

pub(super) struct NativeResolutionOracleCursor {
    reader: BufReader<File>,
    path: PathBuf,
    manifest: NativeResolutionOracleV1,
    line: Vec<u8>,
    previous_root_key: Option<String>,
    counts: NativeResolutionCountsV1,
    finished: bool,
}

impl NativeResolutionOracleCursor {
    pub(super) fn next_root(&mut self) -> Result<Option<NativeResolutionRootV1>> {
        if self.finished {
            return Ok(None);
        }
        self.line.clear();
        let read = self.reader.read_until(b'\n', &mut self.line)?;
        if read == 0 {
            self.finished = true;
            if self.counts != self.manifest.artifact.counts {
                return Err(Error::ConflictError(format!(
                    "native resolution artifact {} row counts do not match its manifest",
                    self.path.display()
                )));
            }
            return Ok(None);
        }
        if self.line.last() != Some(&b'\n') {
            return Err(Error::ParseError(format!(
                "native resolution artifact {} has an unterminated final row",
                self.path.display()
            )));
        }
        self.line.pop();
        if self.line.is_empty() || self.line.last() == Some(&b'\r') {
            return Err(Error::ParseError(format!(
                "native resolution artifact {} contains an empty or CRLF row",
                self.path.display()
            )));
        }
        let root: NativeResolutionRootV1 = serde_json::from_slice(&self.line).map_err(|error| {
            Error::ParseError(format!(
                "parse native resolution root in {}: {error}",
                self.path.display()
            ))
        })?;
        root.validate()?;
        let canonical = crate::json::canonical_json(&root).map_err(|error| {
            Error::ParseError(format!("serialize native resolution root: {error}"))
        })?;
        if canonical != self.line {
            return Err(Error::ConfigError(format!(
                "native resolution root '{}' is not canonical JSON",
                root.root_package_key_sha256
            )));
        }
        if self
            .previous_root_key
            .as_deref()
            .is_some_and(|previous| previous >= root.root_package_key_sha256.as_str())
        {
            return Err(Error::ConfigError(format!(
                "native resolution root key '{}' is duplicated or noncanonical",
                root.root_package_key_sha256
            )));
        }
        accumulate_counts(&mut self.counts, &root)?;
        self.previous_root_key = Some(root.root_package_key_sha256.clone());
        Ok(Some(root))
    }
}

pub fn write_native_resolution_oracle_manifest(
    directory: impl AsRef<Path>,
    manifest: &NativeResolutionOracleV1,
) -> Result<()> {
    let directory = directory.as_ref();
    require_real_directory(directory)?;
    let reader = NativeResolutionOracleReader::open_verified(
        directory.join(NATIVE_RESOLUTION_ROOT_FILE_NAME),
        manifest,
    )?;
    reader.verify_contents()?;
    let path = directory.join(NATIVE_RESOLUTION_MANIFEST_FILE_NAME);
    let bytes = crate::json::canonical_json(manifest).map_err(|error| {
        Error::ParseError(format!("serialize native resolution manifest: {error}"))
    })?;
    if path.exists() {
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
            return Err(Error::InvalidPath(format!(
                "native resolution manifest {} must be a regular file, never a symlink",
                path.display()
            )));
        }
        if fs::read(&path)? == bytes {
            File::open(&path)?.sync_all()?;
            sync_parent(&path)?;
            return Ok(());
        }
        return Err(Error::ConflictError(format!(
            "native resolution manifest {} already exists with different bytes",
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

pub fn verify_native_resolution_oracle_bundle(
    directory: impl AsRef<Path>,
    profile: &ProfileRevisionV2,
    package_oracle: &NativeParityOracleReader,
) -> Result<NativeResolutionOracleReader> {
    let directory = directory.as_ref();
    verify_exact_directory(directory)?;
    let manifest_path = directory.join(NATIVE_RESOLUTION_MANIFEST_FILE_NAME);
    let metadata = fs::symlink_metadata(&manifest_path)?;
    if metadata.file_type().is_symlink()
        || !metadata.file_type().is_file()
        || metadata.len() > MAX_NATIVE_RESOLUTION_MANIFEST_BYTES
    {
        return Err(Error::InvalidPath(format!(
            "native resolution manifest {} must be a bounded regular file",
            manifest_path.display()
        )));
    }
    let bytes = fs::read(&manifest_path)?;
    let manifest: NativeResolutionOracleV1 = serde_json::from_slice(&bytes).map_err(|error| {
        Error::ParseError(format!(
            "parse native resolution manifest {}: {error}",
            manifest_path.display()
        ))
    })?;
    let canonical = crate::json::canonical_json(&manifest).map_err(|error| {
        Error::ParseError(format!("serialize native resolution manifest: {error}"))
    })?;
    if canonical != bytes {
        return Err(Error::ConfigError(format!(
            "native resolution manifest {} is not canonical JSON",
            manifest_path.display()
        )));
    }
    manifest.validate_binding(profile, package_oracle.manifest())?;
    let reader = NativeResolutionOracleReader::open_verified(
        directory.join(NATIVE_RESOLUTION_ROOT_FILE_NAME),
        &manifest,
    )?;
    reader.verify_contents()?;
    reader.verify_package_oracle(package_oracle)?;
    Ok(reader)
}

fn validate_package_references(
    resolution: &NativeResolutionOracleReader,
    package_oracle: &NativeParityOracleReader,
) -> Result<()> {
    let scratch = tempfile::Builder::new()
        .prefix("conary-resolution-membership-")
        .tempdir()?;
    let mut connection = Connection::open(scratch.path().join("membership.sqlite3"))?;
    connection.execute_batch(
        "PRAGMA journal_mode = OFF;
         PRAGMA synchronous = OFF;
         CREATE TABLE packages (package_key_sha256 TEXT PRIMARY KEY) STRICT;
         CREATE TABLE requirements (
             package_key_sha256 TEXT NOT NULL,
             requirement_group_sha256 TEXT NOT NULL,
             PRIMARY KEY (package_key_sha256, requirement_group_sha256)
         ) STRICT;",
    )?;
    let transaction = connection.transaction()?;
    package_oracle.for_each_package(|package| {
        transaction.execute(
            "INSERT INTO packages (package_key_sha256) VALUES (?1)",
            [&package.package_key_sha256],
        )?;
        for group in &package.requirement_groups {
            if group.kind == RepositoryRequirementKind::Depends.as_str()
                || group.kind == RepositoryRequirementKind::PreDepends.as_str()
            {
                transaction.execute(
                    "INSERT OR IGNORE INTO requirements (
                         package_key_sha256, requirement_group_sha256
                     )
                     VALUES (?1, ?2)",
                    params![
                        package.package_key_sha256,
                        native_requirement_group_sha256(group)?
                    ],
                )?;
            }
        }
        Ok(())
    })?;
    transaction.commit()?;

    let mut package_exists = connection
        .prepare("SELECT EXISTS(SELECT 1 FROM packages WHERE package_key_sha256 = ?1)")?;
    let mut requirement_exists = connection.prepare(
        "SELECT EXISTS(
             SELECT 1 FROM requirements
             WHERE package_key_sha256 = ?1 AND requirement_group_sha256 = ?2
         )",
    )?;
    resolution.for_each_root(|root| {
        match &root.outcome {
            NativeResolutionOutcomeV1::Resolved {
                closure_package_keys_sha256,
            } => {
                for package_key in closure_package_keys_sha256 {
                    if !package_exists.query_row([package_key], |row| row.get(0))? {
                        return Err(Error::ConflictError(format!(
                            "native resolution root '{}' references absent closure package '{}'",
                            root.root_package_key_sha256, package_key
                        )));
                    }
                }
            }
            NativeResolutionOutcomeV1::Unresolved { dependencies } => {
                for dependency in dependencies {
                    if !package_exists
                        .query_row([&dependency.requiring_package_key_sha256], |row| row.get(0))?
                    {
                        return Err(Error::ConflictError(format!(
                            "native resolution root '{}' names absent requiring package '{}'",
                            root.root_package_key_sha256, dependency.requiring_package_key_sha256
                        )));
                    }
                    if !requirement_exists.query_row(
                        params![
                            dependency.requiring_package_key_sha256,
                            dependency.requirement_group_sha256
                        ],
                        |row| row.get(0),
                    )? {
                        return Err(Error::ConflictError(format!(
                            "native resolution root '{}' names an absent required group",
                            root.root_package_key_sha256
                        )));
                    }
                }
            }
        }
        Ok(())
    })
}

fn validate_root_coverage(
    resolution: &NativeResolutionOracleReader,
    package_oracle: &NativeParityOracleReader,
) -> Result<()> {
    let mut package_cursor = package_oracle.cursor()?;
    let mut resolution_cursor = resolution.cursor()?;
    loop {
        match (
            package_cursor.next_package()?,
            resolution_cursor.next_root()?,
        ) {
            (Some(package), Some(root))
                if package.package_key_sha256 == root.root_package_key_sha256 => {}
            (Some(package), Some(root)) => {
                return Err(Error::ConflictError(format!(
                    "native resolution root '{}' does not account for package oracle root '{}'",
                    root.root_package_key_sha256, package.package_key_sha256
                )));
            }
            (None, None) => return Ok(()),
            (Some(package), None) => {
                return Err(Error::ConflictError(format!(
                    "native resolution evidence omits package oracle root '{}'",
                    package.package_key_sha256
                )));
            }
            (None, Some(root)) => {
                return Err(Error::ConflictError(format!(
                    "native resolution evidence contains extra root '{}'",
                    root.root_package_key_sha256
                )));
            }
        }
    }
}

fn accumulate_counts(
    counts: &mut NativeResolutionCountsV1,
    root: &NativeResolutionRootV1,
) -> Result<()> {
    counts.roots = checked_add(counts.roots, 1, "roots")?;
    match &root.outcome {
        NativeResolutionOutcomeV1::Resolved {
            closure_package_keys_sha256,
        } => {
            counts.resolved_roots = checked_add(counts.resolved_roots, 1, "resolved roots")?;
            counts.closure_package_references = checked_add(
                counts.closure_package_references,
                u64::try_from(closure_package_keys_sha256.len()).map_err(|_| {
                    Error::InternalError("native resolution closure length exceeds u64".to_string())
                })?,
                "closure package references",
            )?;
        }
        NativeResolutionOutcomeV1::Unresolved { dependencies } => {
            counts.unresolved_roots = checked_add(counts.unresolved_roots, 1, "unresolved roots")?;
            counts.unresolved_dependencies = checked_add(
                counts.unresolved_dependencies,
                u64::try_from(dependencies.len()).map_err(|_| {
                    Error::InternalError(
                        "native unresolved dependency length exceeds u64".to_string(),
                    )
                })?,
                "unresolved dependencies",
            )?;
        }
    }
    Ok(())
}

fn checked_add(current: u64, increment: u64, label: &str) -> Result<u64> {
    current
        .checked_add(increment)
        .ok_or_else(|| Error::InternalError(format!("native resolution {label} exceed u64")))
}

fn require_real_directory(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
        return Err(Error::InvalidPath(format!(
            "native resolution bundle {} must be a real directory",
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
                "native resolution bundle entry {} must be a regular file",
                entry.path().display()
            )));
        }
        names.push(entry.file_name());
    }
    names.sort();
    let mut expected: Vec<OsString> = vec![
        NATIVE_RESOLUTION_MANIFEST_FILE_NAME.into(),
        NATIVE_RESOLUTION_ROOT_FILE_NAME.into(),
    ];
    expected.sort();
    if names != expected {
        return Err(Error::ConflictError(format!(
            "native resolution bundle {} must contain exactly {} and {}",
            path.display(),
            NATIVE_RESOLUTION_MANIFEST_FILE_NAME,
            NATIVE_RESOLUTION_ROOT_FILE_NAME
        )));
    }
    Ok(())
}
