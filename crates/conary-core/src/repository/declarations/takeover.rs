// conary-core/src/repository/declarations/takeover.rs

//! Transactional native repository takeover and owned declaration projections.

use super::trust_import::{
    NativeRepositoryReference, NativeRepositoryTrustPlan, NativeTrustImportPlan,
    TrustEvidenceSource, TrustImportDisposition, TrustImportEvidence, plan_selected_root_trust,
};
use super::{DiscoveredRepositoryDeclarations, discover_selected_root};
use crate::db::models::{
    AuthenticatedSnapshotIdentity, NativeSourceEcosystem, NativeSourceStream, Repository,
    RepositoryOwnership, RepositoryPolicyScope, RepositorySourcePolicy, RepositoryUpdateMode,
};
use crate::error::{Error, Result};
use crate::filesystem::path::safe_join;
use crate::repository::{
    OpenPgpTrustRoot, RepositoryFormat, RepositoryParserConfig, RepositoryTrustPolicy,
    RpmMetadataAuthority, TrustRole,
};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

mod projection;
mod trust_policy;

use projection::{
    ProjectionContent, StagedProjection, collect_projection_content,
    ensure_projection_inputs_unchanged, load_current_projections, load_prior_projections,
    persist_staged, projection_drift, restore_projection_bytes, stage_projection_writes,
};
use trust_policy::expected_trust_policy;

pub const TAKEOVER_MANIFEST_SCHEMA: u32 = 1;
pub const TAKEOVER_PREVIEW_SCHEMA: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum TakeoverPolicyScope {
    Repository { identity: String },
    Group { identity: String },
}

impl TakeoverPolicyScope {
    fn materialize(&self) -> Result<RepositoryPolicyScope> {
        match self {
            Self::Repository { identity } => RepositoryPolicyScope::repository(identity.clone()),
            Self::Group { identity } => RepositoryPolicyScope::group(identity.clone()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum TakeoverSourceStream {
    Release { identity: String },
    Channel { identity: String },
    Rolling { identity: String },
}

impl TakeoverSourceStream {
    fn materialize(&self) -> Result<NativeSourceStream> {
        match self {
            Self::Release { identity } => NativeSourceStream::release(identity.clone()),
            Self::Channel { identity } => NativeSourceStream::channel(identity.clone()),
            Self::Rolling { identity } => NativeSourceStream::rolling(identity.clone()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "kebab-case", deny_unknown_fields)]
pub enum TakeoverUpdatePolicy {
    Follow,
    Pin { snapshot_sha256: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TakeoverRepositoryInput {
    pub declaration: NativeRepositoryReference,
    pub name: String,
    pub source_identity: String,
    pub repository_identity: String,
    pub scope: TakeoverPolicyScope,
    pub stream: TakeoverSourceStream,
    pub update: TakeoverUpdatePolicy,
    pub metadata_url: String,
    pub content_url: Option<String>,
    pub parser: RepositoryParserConfig,
    pub trust: RepositoryTrustPolicy,
    pub enabled: bool,
    pub priority: i32,
    pub metadata_expire: i32,
    pub source_profile: Option<String>,
}

impl TakeoverRepositoryInput {
    fn materialize(&self) -> Result<Repository> {
        let ecosystem = NativeSourceEcosystem::from_repository_format(self.parser.format())?;
        let (mode, pin) = match &self.update {
            TakeoverUpdatePolicy::Follow => (RepositoryUpdateMode::Follow, None),
            TakeoverUpdatePolicy::Pin { snapshot_sha256 } => (
                RepositoryUpdateMode::Pin,
                Some(AuthenticatedSnapshotIdentity::from_sha256(
                    snapshot_sha256.clone(),
                )?),
            ),
        };
        let policy = RepositorySourcePolicy::new(
            self.source_identity.clone(),
            self.scope.materialize()?,
            ecosystem,
            self.stream.materialize()?,
            mode,
        )?;
        let mut repository = Repository::new(self.name.clone(), self.metadata_url.clone());
        repository.content_url = self.content_url.clone();
        repository.enabled = self.enabled;
        repository.priority = self.priority;
        repository.metadata_expire = self.metadata_expire;
        repository.source_profile = self.source_profile.clone();
        repository.managed_by = RepositoryOwnership::NativeProjection;
        repository.set_parser_config(self.parser.clone())?;
        repository.set_trust_policy(self.trust.clone())?;
        repository.set_native_source_policy(policy, self.repository_identity.clone(), pin)?;
        Ok(repository)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeRepositoryTakeoverManifest {
    pub schema: u32,
    pub repositories: Vec<TakeoverRepositoryInput>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProjectionOwnershipChange {
    NativeToConary,
    ConaryVerified,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeProjectionPreview {
    pub path: PathBuf,
    pub sha256: String,
    pub mode: u32,
    pub ownership: ProjectionOwnershipChange,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum TakeoverBlocker {
    EnabledTrust {
        repository: NativeRepositoryReference,
        disposition: TrustImportDisposition,
    },
    MissingEnrollment {
        repository: NativeRepositoryReference,
    },
    UnknownEnrollment {
        repository: NativeRepositoryReference,
        name: String,
    },
    DuplicateEnrollmentProduct {
        repository: NativeRepositoryReference,
        names: Vec<String>,
    },
    ParserFormatMismatch {
        repository: NativeRepositoryReference,
        name: String,
        expected: RepositoryFormat,
        requested: RepositoryFormat,
    },
    EnabledStateMismatch {
        repository: NativeRepositoryReference,
        name: String,
        declared: bool,
        requested: bool,
    },
    DuplicateRepositoryName {
        name: String,
    },
    DuplicateRepositoryIdentity {
        identity: String,
    },
    ExistingRepository {
        name: String,
        ownership: String,
    },
    TrustPolicyMismatch {
        repository: NativeRepositoryReference,
        name: String,
    },
    DynamicZypperServices {
        paths: Vec<PathBuf>,
    },
    ProjectionNotRegular {
        path: PathBuf,
    },
    ProjectionDrift {
        path: PathBuf,
        expected_sha256: String,
        observed_sha256: Option<String>,
    },
    ProjectionModeDrift {
        path: PathBuf,
        expected_mode: u32,
        observed_mode: Option<u32>,
    },
    ExistingTakeoverInputChanged,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeRepositoryTakeoverPreview {
    pub schema: u32,
    pub selected_root: PathBuf,
    pub declarations: DiscoveredRepositoryDeclarations,
    pub trust: NativeTrustImportPlan,
    pub repositories: Vec<TakeoverRepositoryInput>,
    pub projections: Vec<NativeProjectionPreview>,
    pub blockers: Vec<TakeoverBlocker>,
}

impl NativeRepositoryTakeoverPreview {
    pub fn sha256(&self) -> Result<String> {
        let bytes = serde_json::to_vec(self).map_err(|error| {
            Error::ParseError(format!(
                "failed to serialize repository takeover preview: {error}"
            ))
        })?;
        Ok(crate::hash::sha256(&bytes))
    }

    pub fn require_applicable(&self) -> Result<()> {
        if self.blockers.is_empty() {
            Ok(())
        } else {
            Err(Error::ConfigError(format!(
                "native repository takeover has {} blocker(s)",
                self.blockers.len()
            )))
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TakeoverApplyStatus {
    Applied,
    AlreadyApplied,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TakeoverApplyOutcome {
    pub status: TakeoverApplyStatus,
    pub preview_sha256: String,
    pub repository_ids: Vec<i64>,
}

/// Build a deterministic selected-root preview without mutation.
pub fn preview_native_repository_takeover(
    conn: &Connection,
    selected_root: impl AsRef<Path>,
    manifest: &NativeRepositoryTakeoverManifest,
) -> Result<NativeRepositoryTakeoverPreview> {
    validate_manifest(manifest)?;
    let selected_root = selected_root.as_ref();
    let root_identity = selected_root_identity(selected_root)?;
    if let Some((preview_json, expected_sha256)) = conn
        .query_row(
            "SELECT preview_json, preview_sha256 FROM native_repository_takeovers WHERE selected_root = ?1",
            [&root_identity],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?
    {
        let mut preview: NativeRepositoryTakeoverPreview = serde_json::from_str(&preview_json)
            .map_err(|error| Error::ParseError(format!("invalid persisted takeover preview: {error}")))?;
        if preview.sha256()? != expected_sha256 {
            return Err(Error::ParseError(
                "persisted native repository takeover preview digest mismatch".to_string(),
            ));
        }
        let mut requested = manifest.repositories.clone();
        requested.sort_by(|left, right| left.name.cmp(&right.name));
        if requested != preview.repositories {
            preview.blockers.push(TakeoverBlocker::ExistingTakeoverInputChanged);
        }
        preview.blockers.extend(projection_drift(conn, selected_root, &root_identity)?);
        return Ok(preview);
    }

    let declarations = discover_selected_root(selected_root).map_err(|error| {
        Error::ConfigError(format!(
            "native repository declaration discovery failed: {error}"
        ))
    })?;
    let trust = plan_selected_root_trust(&declarations);
    let (projections, projection_blockers) =
        collect_projection_content(selected_root, &declarations, &trust)?;
    let mut repositories = manifest.repositories.clone();
    repositories.sort_by(|left, right| left.name.cmp(&right.name));
    let mut blockers = validate_enrollments(conn, &declarations, &trust, &repositories)?;
    blockers.extend(projection_blockers);
    let projection_previews = projections
        .iter()
        .map(|projection| NativeProjectionPreview {
            path: projection.path.clone(),
            sha256: crate::hash::sha256(&projection.content),
            mode: projection.mode,
            ownership: ProjectionOwnershipChange::NativeToConary,
        })
        .collect();
    if !declarations.zypper_services.is_empty() {
        blockers.push(TakeoverBlocker::DynamicZypperServices {
            paths: declarations
                .zypper_services
                .iter()
                .map(|document| document.path.clone())
                .collect(),
        });
    }
    Ok(NativeRepositoryTakeoverPreview {
        schema: TAKEOVER_PREVIEW_SCHEMA,
        selected_root: selected_root.to_path_buf(),
        declarations,
        trust,
        repositories,
        projections: projection_previews,
        blockers,
    })
}

/// Apply the exact preview using one SQLite transaction and atomic file exchanges.
pub fn apply_native_repository_takeover(
    conn: &Connection,
    selected_root: impl AsRef<Path>,
    manifest: &NativeRepositoryTakeoverManifest,
    expected_preview_sha256: &str,
) -> Result<TakeoverApplyOutcome> {
    apply_native_repository_takeover_with_persist(
        conn,
        selected_root.as_ref(),
        manifest,
        expected_preview_sha256,
        persist_staged,
    )
}

fn apply_native_repository_takeover_with_persist(
    conn: &Connection,
    selected_root: &Path,
    manifest: &NativeRepositoryTakeoverManifest,
    expected_preview_sha256: &str,
    persist: impl FnOnce(Vec<StagedProjection>) -> Result<Vec<ProjectionContent>>,
) -> Result<TakeoverApplyOutcome> {
    let preview = preview_native_repository_takeover(conn, selected_root, manifest)?;
    preview.require_applicable()?;
    let preview_sha256 = preview.sha256()?;
    if preview_sha256 != expected_preview_sha256 {
        return Err(Error::ConflictError(format!(
            "native repository takeover preview changed: expected {expected_preview_sha256}, observed {preview_sha256}"
        )));
    }
    let root_identity = selected_root_identity(selected_root)?;
    if conn
        .query_row(
            "SELECT 1 FROM native_repository_takeovers WHERE selected_root = ?1",
            [&root_identity],
            |row| row.get::<_, i64>(0),
        )
        .optional()?
        .is_some()
    {
        let repository_ids = takeover_repository_ids(conn, &root_identity)?;
        return Ok(TakeoverApplyOutcome {
            status: TakeoverApplyStatus::AlreadyApplied,
            preview_sha256,
            repository_ids,
        });
    }

    let (projections, projection_blockers) =
        collect_projection_content(selected_root, &preview.declarations, &preview.trust)?;
    if !projection_blockers.is_empty() {
        return Err(Error::ConflictError(
            "native repository projection type changed after preview".to_string(),
        ));
    }
    ensure_projection_inputs_unchanged(selected_root, &projections)?;
    let staged = stage_projection_writes(selected_root, &projections, &projections)?;
    let preview_json = serde_json::to_string(&preview).map_err(|error| {
        Error::ParseError(format!(
            "failed to serialize repository takeover preview: {error}"
        ))
    })?;
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "INSERT INTO native_repository_takeovers (selected_root, preview_sha256, preview_json) VALUES (?1, ?2, ?3)",
        params![root_identity, preview_sha256, preview_json],
    )?;
    let takeover_id = tx.last_insert_rowid();
    let mut repository_ids = Vec::new();
    for input in &preview.repositories {
        let mut repository = input.materialize()?;
        let id = repository.insert(&tx)?;
        tx.execute(
            "INSERT INTO native_repository_takeover_members (takeover_id, repository_id) VALUES (?1, ?2)",
            params![takeover_id, id],
        )?;
        repository_ids.push(id);
    }
    let prior_projections = persist(staged)?;
    let projection_result: Result<()> = (|| {
        for (projection, prior) in projections.iter().zip(&prior_projections) {
            tx.execute(
                "INSERT INTO native_repository_projections
                 (takeover_id, path, content, sha256, mode, prior_existed, prior_content, prior_mode)
                 VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6, ?7)",
                params![
                    takeover_id,
                    projection.path.to_string_lossy(),
                    projection.content,
                    crate::hash::sha256(&projection.content),
                    projection.mode,
                    &prior.content,
                    prior.mode,
                ],
            )?;
        }
        Ok(())
    })();
    if let Err(error) = projection_result {
        restore_projection_bytes(selected_root, &projections, &prior_projections)?;
        return Err(error);
    }
    if let Err(error) = tx.commit() {
        restore_projection_bytes(selected_root, &projections, &prior_projections)?;
        return Err(error.into());
    }
    Ok(TakeoverApplyOutcome {
        status: TakeoverApplyStatus::Applied,
        preview_sha256,
        repository_ids,
    })
}

/// Restore pre-takeover projection bytes and delete only takeover-owned rows.
pub fn rollback_native_repository_takeover(
    conn: &Connection,
    selected_root: impl AsRef<Path>,
) -> Result<()> {
    let selected_root = selected_root.as_ref();
    let root_identity = selected_root_identity(selected_root)?;
    let takeover_id = conn
        .query_row(
            "SELECT id FROM native_repository_takeovers WHERE selected_root = ?1",
            [&root_identity],
            |row| row.get::<_, i64>(0),
        )
        .optional()?
        .ok_or_else(|| {
            Error::NotFound(format!("no native repository takeover for {root_identity}"))
        })?;
    let drift = projection_drift(conn, selected_root, &root_identity)?;
    if !drift.is_empty() {
        return Err(Error::ConflictError(format!(
            "native repository rollback blocked by {} projection drift finding(s)",
            drift.len()
        )));
    }
    let current = load_current_projections(conn, takeover_id)?;
    let prior = load_prior_projections(conn, takeover_id)?;
    let staged = stage_projection_writes(selected_root, &prior, &current)?;
    let repository_ids = takeover_repository_ids(conn, &root_identity)?;
    let tx = conn.unchecked_transaction()?;
    for repository_id in &repository_ids {
        let ownership = Repository::find_by_id(&tx, *repository_id)?
            .ok_or_else(|| Error::NotFound(format!("takeover repository {repository_id}")))?
            .managed_by;
        if ownership != RepositoryOwnership::NativeProjection {
            return Err(Error::ConflictError(format!(
                "takeover repository {repository_id} is no longer owned by native projections"
            )));
        }
        tx.execute(
            "DELETE FROM native_repository_takeover_members WHERE takeover_id = ?1 AND repository_id = ?2",
            params![takeover_id, repository_id],
        )?;
        Repository::delete(&tx, *repository_id)?;
    }
    tx.execute(
        "DELETE FROM native_repository_takeovers WHERE id = ?1",
        [takeover_id],
    )?;
    let observed_current = persist_staged(staged)?;
    if let Err(error) = tx.commit() {
        restore_projection_bytes(selected_root, &prior, &observed_current)?;
        return Err(error.into());
    }
    Ok(())
}

fn validate_manifest(manifest: &NativeRepositoryTakeoverManifest) -> Result<()> {
    if manifest.schema != TAKEOVER_MANIFEST_SCHEMA {
        return Err(Error::ConfigError(format!(
            "native repository takeover manifest schema {} is unsupported; expected {}",
            manifest.schema, TAKEOVER_MANIFEST_SCHEMA
        )));
    }
    for repository in &manifest.repositories {
        repository.materialize()?;
    }
    Ok(())
}

fn validate_enrollments(
    conn: &Connection,
    declarations: &DiscoveredRepositoryDeclarations,
    trust: &NativeTrustImportPlan,
    repositories: &[TakeoverRepositoryInput],
) -> Result<Vec<TakeoverBlocker>> {
    let mut blockers = Vec::new();
    let mut names = BTreeSet::new();
    let mut identities = BTreeSet::new();
    let mut products: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for input in repositories {
        if !names.insert(input.name.clone()) {
            blockers.push(TakeoverBlocker::DuplicateRepositoryName {
                name: input.name.clone(),
            });
        }
        let identity_key = (
            input.source_identity.clone(),
            match &input.scope {
                TakeoverPolicyScope::Repository { identity } => {
                    format!("repository:{identity}")
                }
                TakeoverPolicyScope::Group { identity } => format!("group:{identity}"),
            },
            input.repository_identity.clone(),
        );
        if !identities.insert(identity_key) {
            blockers.push(TakeoverBlocker::DuplicateRepositoryIdentity {
                identity: input.repository_identity.clone(),
            });
        }
        let Some(plan) = trust
            .repositories
            .iter()
            .find(|plan| plan.repository == input.declaration)
        else {
            blockers.push(TakeoverBlocker::UnknownEnrollment {
                repository: input.declaration.clone(),
                name: input.name.clone(),
            });
            continue;
        };
        let expected_format = match &input.declaration {
            NativeRepositoryReference::Apt { .. } => RepositoryFormat::Debian,
            NativeRepositoryReference::Dnf { .. } | NativeRepositoryReference::Zypper { .. } => {
                RepositoryFormat::Fedora
            }
            NativeRepositoryReference::Alpm { .. } => RepositoryFormat::Arch,
        };
        let requested_format = input.parser.format();
        if requested_format != expected_format {
            blockers.push(TakeoverBlocker::ParserFormatMismatch {
                repository: input.declaration.clone(),
                name: input.name.clone(),
                expected: expected_format,
                requested: requested_format,
            });
        }
        let product_key = serde_json::to_string(&(
            &input.declaration,
            &input.metadata_url,
            &input.content_url,
            &input.parser,
        ))
        .map_err(|error| {
            Error::ParseError(format!(
                "failed to serialize native enrollment product: {error}"
            ))
        })?;
        products
            .entry(product_key)
            .or_default()
            .push(input.name.clone());
        if requested_format == expected_format
            && matches!(plan.disposition, TrustImportDisposition::Importable)
            && expected_trust_policy(
                &declarations.root,
                declarations,
                plan,
                input.parser.format(),
            )?
            .as_ref()
                != Some(&input.trust)
        {
            blockers.push(TakeoverBlocker::TrustPolicyMismatch {
                repository: input.declaration.clone(),
                name: input.name.clone(),
            });
        }
        let declared = plan.enabled.unwrap_or(true);
        if declared != input.enabled {
            blockers.push(TakeoverBlocker::EnabledStateMismatch {
                repository: input.declaration.clone(),
                name: input.name.clone(),
                declared,
                requested: input.enabled,
            });
        }
        if let Some(existing) = Repository::find_by_name(conn, &input.name)? {
            blockers.push(TakeoverBlocker::ExistingRepository {
                name: input.name.clone(),
                ownership: existing.managed_by.as_str().to_string(),
            });
        }
    }
    for names in products.values().filter(|names| names.len() > 1) {
        let first = repositories
            .iter()
            .find(|input| input.name == names[0])
            .expect("product names came from repository inputs");
        blockers.push(TakeoverBlocker::DuplicateEnrollmentProduct {
            repository: first.declaration.clone(),
            names: names.clone(),
        });
    }
    for plan in &trust.repositories {
        let enabled = plan.enabled.unwrap_or(true);
        let matching = repositories
            .iter()
            .any(|input| input.declaration == plan.repository);
        if !matching {
            blockers.push(TakeoverBlocker::MissingEnrollment {
                repository: plan.repository.clone(),
            });
        }
        if enabled && !matches!(plan.disposition, TrustImportDisposition::Importable) {
            blockers.push(TakeoverBlocker::EnabledTrust {
                repository: plan.repository.clone(),
                disposition: plan.disposition.clone(),
            });
        }
    }
    Ok(blockers)
}

fn takeover_repository_ids(conn: &Connection, root_identity: &str) -> Result<Vec<i64>> {
    let mut statement = conn.prepare(
        "SELECT member.repository_id
         FROM native_repository_takeover_members member
         JOIN native_repository_takeovers takeover ON takeover.id = member.takeover_id
         WHERE takeover.selected_root = ?1 ORDER BY member.repository_id",
    )?;
    statement
        .query_map([root_identity], |row| row.get(0))?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Into::into)
}

fn selected_root_identity(selected_root: &Path) -> Result<String> {
    selected_root
        .canonicalize()?
        .to_str()
        .map(ToOwned::to_owned)
        .ok_or_else(|| Error::ConfigError("selected root path is not valid UTF-8".to_string()))
}

#[cfg(test)]
mod tests;
