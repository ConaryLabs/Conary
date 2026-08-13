// conary-core/src/repository/declarations/takeover/projection.rs

//! Projection discovery, drift inspection, staging, and byte restoration.

use super::*;
use crate::filesystem::durable::sync_parent_directory;
use std::ffi::{CString, OsStr};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ProjectionContent {
    pub(super) path: PathBuf,
    pub(super) content: Vec<u8>,
    pub(super) mode: u32,
}

pub(super) fn ensure_projection_inputs_unchanged(
    selected_root: &Path,
    projections: &[ProjectionContent],
) -> Result<()> {
    for projection in projections {
        let path = safe_join(selected_root, &projection.path)?;
        let observed = fs::read(&path)?;
        let expected_sha256 = crate::hash::sha256(&projection.content);
        let observed_sha256 = crate::hash::sha256(&observed);
        if observed_sha256 != expected_sha256 {
            return Err(Error::ConflictError(format!(
                "native repository projection {} changed after preview: expected {}, observed {}",
                projection.path.display(),
                expected_sha256,
                observed_sha256
            )));
        }
    }
    Ok(())
}

pub(super) fn collect_projection_content(
    selected_root: &Path,
    declarations: &DiscoveredRepositoryDeclarations,
    trust: &NativeTrustImportPlan,
    repositories: &[TakeoverRepositoryInput],
) -> Result<(Vec<ProjectionContent>, Vec<TakeoverBlocker>)> {
    let mut sources: Vec<(PathBuf, Vec<u8>)> = Vec::new();
    sources.extend(
        declarations
            .apt
            .iter()
            .map(|document| (document.path.clone(), document.source.as_bytes().to_vec())),
    );
    sources.extend(
        declarations
            .dnf
            .iter()
            .map(|document| (document.path.clone(), document.source.as_bytes().to_vec())),
    );
    sources.extend(
        declarations
            .zypper_repositories
            .iter()
            .map(|document| (document.path.clone(), document.source.as_bytes().to_vec())),
    );
    sources.extend(
        declarations
            .zypper_services
            .iter()
            .map(|document| (document.path.clone(), document.source.as_bytes().to_vec())),
    );
    if let Some(alpm) = &declarations.alpm {
        sources.extend(
            alpm.files
                .iter()
                .map(|document| (document.path.clone(), document.source.as_bytes().to_vec())),
        );
    }
    if let Some(eopkg) = &declarations.eopkg {
        sources.push((eopkg.path.clone(), eopkg.source.as_bytes().to_vec()));
    }
    for plan in &trust.repositories {
        for evidence in &plan.evidence {
            if let TrustEvidenceSource::SelectedRootFile { path } = &evidence.source {
                sources.push((path.clone(), fs::read(safe_join(selected_root, path)?)?));
            }
        }
        for input in repositories
            .iter()
            .filter(|input| input.declaration == plan.repository)
        {
            if let Some(paths) = explicit_apt_global_trust_paths(selected_root, plan, &input.trust)
            {
                for path in paths {
                    sources.push((path.clone(), fs::read(safe_join(selected_root, &path)?)?));
                }
            }
            if let Some(paths) =
                explicit_zypper_global_trust_paths(selected_root, plan, &input.trust)
            {
                for path in paths {
                    sources.push((path.clone(), fs::read(safe_join(selected_root, &path)?)?));
                }
            }
        }
    }
    sources.sort_by(|left, right| left.0.cmp(&right.0));
    sources.dedup_by(|left, right| left.0 == right.0 && left.1 == right.1);
    let mut projections = Vec::new();
    let mut blockers = Vec::new();
    for (path, source) in sources {
        let host_path = safe_join(selected_root, &path)?;
        let metadata = fs::symlink_metadata(&host_path)?;
        if !metadata.file_type().is_file() {
            blockers.push(TakeoverBlocker::ProjectionNotRegular { path });
            continue;
        }
        projections.push(ProjectionContent {
            path,
            content: source,
            mode: metadata.permissions().mode() & 0o7777,
        });
    }
    Ok((projections, blockers))
}

pub(super) fn projection_drift(
    conn: &Connection,
    selected_root: &Path,
    root_identity: &str,
) -> Result<Vec<TakeoverBlocker>> {
    let mut statement = conn.prepare(
        "SELECT projection.path, projection.sha256, projection.mode
         FROM native_repository_projections projection
         JOIN native_repository_takeovers takeover ON takeover.id = projection.takeover_id
         WHERE takeover.selected_root = ?1 ORDER BY projection.path",
    )?;
    let rows = statement
        .query_map([root_identity], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, u32>(2)?,
            ))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let mut blockers = Vec::new();
    for (path, expected_sha256, expected_mode) in rows {
        let path = PathBuf::from(path);
        let host_path = safe_join(selected_root, &path)?;
        let observed = match fs::read(&host_path) {
            Ok(content) => Some((
                crate::hash::sha256(&content),
                fs::metadata(&host_path)?.permissions().mode() & 0o7777,
            )),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        let observed_sha256 = observed.as_ref().map(|(sha256, _)| sha256.clone());
        if observed_sha256.as_deref() != Some(expected_sha256.as_str()) {
            blockers.push(TakeoverBlocker::ProjectionDrift {
                path,
                expected_sha256,
                observed_sha256,
            });
        } else if observed
            .as_ref()
            .is_some_and(|(_, mode)| *mode != expected_mode)
        {
            blockers.push(TakeoverBlocker::ProjectionModeDrift {
                path,
                expected_mode,
                observed_mode: observed.map(|(_, mode)| mode),
            });
        }
    }
    Ok(blockers)
}

pub(super) fn load_prior_projections(
    conn: &Connection,
    takeover_id: i64,
) -> Result<Vec<ProjectionContent>> {
    load_projections(conn, takeover_id, "prior_content", "prior_mode")
}

pub(super) fn load_current_projections(
    conn: &Connection,
    takeover_id: i64,
) -> Result<Vec<ProjectionContent>> {
    load_projections(conn, takeover_id, "content", "mode")
}

fn load_projections(
    conn: &Connection,
    takeover_id: i64,
    content_column: &str,
    mode_column: &str,
) -> Result<Vec<ProjectionContent>> {
    let sql = format!(
        "SELECT path, {content_column}, {mode_column}
         FROM native_repository_projections WHERE takeover_id = ?1 ORDER BY path"
    );
    let mut statement = conn.prepare(&sql)?;
    statement
        .query_map([takeover_id], |row| {
            Ok(ProjectionContent {
                path: PathBuf::from(row.get::<_, String>(0)?),
                content: row.get(1)?,
                mode: row.get(2)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Into::into)
}

pub(super) struct StagedProjection {
    projection_path: PathBuf,
    path: PathBuf,
    temporary: tempfile::NamedTempFile,
    expected_sha256: String,
    expected_mode: u32,
}

pub(super) fn stage_projection_writes(
    selected_root: &Path,
    desired: &[ProjectionContent],
    expected: &[ProjectionContent],
) -> Result<Vec<StagedProjection>> {
    if desired.len() != expected.len()
        || desired
            .iter()
            .zip(expected)
            .any(|(desired, expected)| desired.path != expected.path)
    {
        return Err(Error::ConfigError(
            "native repository projection desired and expected path sets differ".to_string(),
        ));
    }
    let mut staged = Vec::new();
    for (desired, expected) in desired.iter().zip(expected) {
        let path = safe_join(selected_root, &desired.path)?;
        let parent = path.parent().ok_or_else(|| {
            Error::ConfigError(format!(
                "projection {} has no parent",
                desired.path.display()
            ))
        })?;
        fs::create_dir_all(parent)?;
        let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
        temporary.as_file_mut().write_all(&desired.content)?;
        temporary
            .as_file_mut()
            .set_permissions(fs::Permissions::from_mode(desired.mode))?;
        temporary.as_file_mut().sync_all()?;
        staged.push(StagedProjection {
            projection_path: desired.path.clone(),
            path,
            temporary,
            expected_sha256: crate::hash::sha256(&expected.content),
            expected_mode: expected.mode,
        });
    }
    Ok(staged)
}

pub(super) fn persist_staged(staged: Vec<StagedProjection>) -> Result<Vec<ProjectionContent>> {
    persist_staged_with_failure(staged, None)
}

fn persist_staged_with_failure(
    staged: Vec<StagedProjection>,
    fail_after: Option<usize>,
) -> Result<Vec<ProjectionContent>> {
    let mut exchanged = Vec::new();
    for (index, staged) in staged.into_iter().enumerate() {
        if fail_after == Some(index) {
            rollback_exchanged(&mut exchanged)?;
            return Err(Error::IoError(
                "forced projection persist failure".to_string(),
            ));
        }
        match exchange_and_verify(staged) {
            Ok(exchanged_projection) => exchanged.push(exchanged_projection),
            Err(error) => {
                rollback_exchanged(&mut exchanged)?;
                return Err(error);
            }
        }
    }
    let priors = exchanged.iter().map(|(_, prior)| prior.clone()).collect();
    drop(exchanged);
    Ok(priors)
}

pub(super) fn restore_projection_bytes(
    selected_root: &Path,
    expected: &[ProjectionContent],
    desired: &[ProjectionContent],
) -> Result<()> {
    let staged = stage_projection_writes(selected_root, desired, expected)?;
    persist_staged(staged).map(|_| ())
}

fn exchange_and_verify(staged: StagedProjection) -> Result<(StagedProjection, ProjectionContent)> {
    exchange_paths(&staged)?;
    let (content, mode) = match read_displaced_projection(&staged) {
        Ok(observed) => observed,
        Err(error) => {
            exchange_paths(&staged)?;
            return Err(error);
        }
    };
    let observed_sha256 = crate::hash::sha256(&content);
    if observed_sha256 != staged.expected_sha256 || mode != staged.expected_mode {
        exchange_paths(&staged)?;
        return Err(Error::ConflictError(format!(
            "native repository projection {} changed before atomic exchange: expected {} mode {:o}, observed {} mode {:o}",
            staged.projection_path.display(),
            staged.expected_sha256,
            staged.expected_mode,
            observed_sha256,
            mode,
        )));
    }
    let path = staged.projection_path.clone();
    Ok((
        staged,
        ProjectionContent {
            path,
            content,
            mode,
        },
    ))
}

fn read_displaced_projection(staged: &StagedProjection) -> Result<(Vec<u8>, u32)> {
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(staged.temporary.path())?;
    let metadata = file.metadata()?;
    if !metadata.file_type().is_file() {
        return Err(Error::ConflictError(format!(
            "native repository projection {} became a non-regular file before atomic exchange",
            staged.projection_path.display()
        )));
    }
    let mut content = Vec::new();
    file.read_to_end(&mut content)?;
    Ok((content, metadata.permissions().mode() & 0o7777))
}

fn exchange_paths(staged: &StagedProjection) -> Result<()> {
    let parent = staged.path.parent().ok_or_else(|| {
        Error::ConfigError(format!(
            "projection {} has no parent",
            staged.projection_path.display()
        ))
    })?;
    let directory = File::open(parent)?;
    let temporary_name = staged.temporary.path().file_name().ok_or_else(|| {
        Error::ConfigError("staged projection has no temporary basename".to_string())
    })?;
    let target_name = staged.path.file_name().ok_or_else(|| {
        Error::ConfigError(format!(
            "projection {} has no basename",
            staged.projection_path.display()
        ))
    })?;
    let exchange = || rename_exchange_at(&directory, temporary_name, target_name);
    exchange().map_err(|error| {
        Error::IoError(format!(
            "failed to atomically exchange native repository projection {}: {error}",
            staged.projection_path.display()
        ))
    })?;
    if let Err(sync_error) = sync_parent_directory(&staged.path) {
        if let Err(rollback_error) = exchange() {
            return Err(Error::IoError(format!(
                "failed to sync atomic exchange for native repository projection {}: {sync_error}; exchange rollback also failed: {rollback_error}",
                staged.projection_path.display()
            )));
        }
        let _ = sync_parent_directory(&staged.path);
        return Err(Error::IoError(format!(
            "failed to sync atomic exchange for native repository projection {}: {sync_error}",
            staged.projection_path.display()
        )));
    }
    Ok(())
}

fn rename_exchange_at(
    directory: &File,
    source: &OsStr,
    destination: &OsStr,
) -> std::io::Result<()> {
    let source = CString::new(source.as_bytes())
        .map_err(|_| std::io::Error::from_raw_os_error(libc::EINVAL))?;
    let destination = CString::new(destination.as_bytes())
        .map_err(|_| std::io::Error::from_raw_os_error(libc::EINVAL))?;
    // SAFETY: both path arguments are owned NUL-terminated byte strings, the
    // directory descriptor remains open for the call, and renameat2 does not
    // retain any pointer or descriptor after returning.
    let result = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            directory.as_raw_fd(),
            source.as_ptr(),
            directory.as_raw_fd(),
            destination.as_ptr(),
            libc::RENAME_EXCHANGE,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

fn rollback_exchanged(exchanged: &mut Vec<(StagedProjection, ProjectionContent)>) -> Result<()> {
    while let Some((staged, _)) = exchanged.pop() {
        exchange_paths(&staged)?;
    }
    Ok(())
}

#[cfg(test)]
pub(super) fn persist_staged_after_one_for_test(
    staged: Vec<StagedProjection>,
) -> Result<Vec<ProjectionContent>> {
    persist_staged_with_failure(staged, Some(1))
}
