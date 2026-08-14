// apps/conary/src/commands/adopt/packages.rs

//! Specific package adoption.
//!
//! Package preview and apply share one discovery and policy plan. Preview opens
//! SQLite read-only and stops before CAS, checkpoint, snapshot, hook, generation,
//! native-package-manager, or live-root mutation.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use anyhow::{Context, Result, anyhow, bail};
use conary_core::db::backup::CheckpointReason;
use conary_core::db::models::{
    Changeset, ChangesetStatus, ExistingDirectoryMaterialization, FileEntry, InstallSource, Trove,
    TroveType,
};
use conary_core::packages::{
    InstalledPackageIdentity, SystemPackageManager, dpkg_query, pacman_query, rpm_query,
};
use conary_core::repository::dependency_model::{ProvidedCapability, RepositoryRequirementGroup};
use conary_core::repository::versioning::VersionScheme;
use rusqlite::{Connection, OpenFlags, OptionalExtension};

use super::super::create_state_snapshot;
use super::super::open_db;
use super::super::progress::{AdoptPhase, AdoptProgress};
use super::cas_capture::{capture_package_files, validate_package_files};
use super::checkpoint::write_db_checkpoint;
use super::system::FileInfoTuple;

mod file_validation;
use file_validation::validate_planned_file_claims;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AdoptionMode {
    Track,
    Full,
}

impl AdoptionMode {
    fn from_full(full: bool) -> Self {
        if full { Self::Full } else { Self::Track }
    }

    fn install_source(self) -> InstallSource {
        match self {
            Self::Track => InstallSource::AdoptedTrack,
            Self::Full => InstallSource::AdoptedFull,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Track => "track (metadata only)",
            Self::Full => "full (CAS-backed)",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ResolvedNativePackage {
    requested: String,
    native: InstalledPackageIdentity,
    description: Option<String>,
}

#[derive(Clone, Debug)]
enum PackageLookup {
    Found(ResolvedNativePackage),
    Missing { reason: String },
    Ambiguous { reason: String },
    Unsupported { reason: String },
}

trait NativePackageSource {
    fn manager(&self) -> SystemPackageManager;
    fn lookup(&self, requested: &str) -> PackageLookup;
    fn query_files(&self, query_name: &str) -> Result<Vec<FileInfoTuple>>;
    fn query_requirements(&self, query_name: &str) -> Result<Vec<RepositoryRequirementGroup>>;
    fn query_provides(
        &self,
        identity: &InstalledPackageIdentity,
    ) -> Result<Vec<ProvidedCapability>>;
}

struct DetectedNativePackageSource {
    manager: SystemPackageManager,
}

impl DetectedNativePackageSource {
    fn new(manager: SystemPackageManager) -> Self {
        Self { manager }
    }
}

impl NativePackageSource for DetectedNativePackageSource {
    fn manager(&self) -> SystemPackageManager {
        self.manager
    }

    fn lookup(&self, requested: &str) -> PackageLookup {
        let result = match self.manager {
            SystemPackageManager::Rpm => {
                rpm_query::query_package(requested).map(|record| ResolvedNativePackage {
                    requested: requested.to_string(),
                    native: record.identity,
                    description: record.info.description.or(record.info.summary),
                })
            }
            SystemPackageManager::Dpkg => {
                dpkg_query::query_package(requested).map(|record| ResolvedNativePackage {
                    requested: requested.to_string(),
                    native: record.identity,
                    description: record.info.description,
                })
            }
            SystemPackageManager::Pacman => {
                pacman_query::query_package(requested).map(|record| ResolvedNativePackage {
                    requested: requested.to_string(),
                    native: record.identity,
                    description: record.info.description,
                })
            }
            SystemPackageManager::Eopkg => {
                conary_core::packages::eopkg::query::query_package(requested).map(|record| {
                    ResolvedNativePackage {
                        requested: requested.to_string(),
                        native: record.identity,
                        description: record.info.description,
                    }
                })
            }
            SystemPackageManager::Unknown => {
                return PackageLookup::Unsupported {
                    reason: "no supported native package manager is available".to_string(),
                };
            }
        };

        match result {
            Ok(identity) => PackageLookup::Found(identity),
            Err(conary_core::Error::NotFound(_)) => PackageLookup::Missing {
                reason: format!(
                    "'{requested}' is not installed in the {} database",
                    self.manager.display_name()
                ),
            },
            Err(conary_core::Error::ConflictError(reason)) => PackageLookup::Ambiguous { reason },
            Err(error) => PackageLookup::Unsupported {
                reason: format!(
                    "{} could not inspect '{requested}': {error}",
                    self.manager.display_name()
                ),
            },
        }
    }

    fn query_files(&self, query_name: &str) -> Result<Vec<FileInfoTuple>> {
        let files = match self.manager {
            SystemPackageManager::Rpm => rpm_query::query_package_files(query_name)
                .with_context(|| format!("RPM file query failed for '{query_name}'"))?,
            SystemPackageManager::Dpkg => dpkg_query::query_package_files(query_name)
                .with_context(|| format!("dpkg file query failed for '{query_name}'"))?,
            SystemPackageManager::Pacman => pacman_query::query_package_files(query_name)
                .with_context(|| format!("pacman file query failed for '{query_name}'"))?,
            SystemPackageManager::Eopkg => {
                conary_core::packages::eopkg::query::query_package_files(query_name)
                    .with_context(|| format!("eopkg file query failed for '{query_name}'"))?
            }
            SystemPackageManager::Unknown => Vec::new(),
        };
        Ok(files
            .into_iter()
            .map(|file| {
                (
                    file.path,
                    file.size,
                    file.mode,
                    file.digest,
                    file.user,
                    file.group,
                    file.link_target,
                    file.absence_policy,
                )
            })
            .collect())
    }

    fn query_requirements(&self, query_name: &str) -> Result<Vec<RepositoryRequirementGroup>> {
        super::requirements::query_package_requirements(self.manager, query_name)
    }

    fn query_provides(
        &self,
        identity: &InstalledPackageIdentity,
    ) -> Result<Vec<ProvidedCapability>> {
        super::provides::query_package_provides(self.manager, identity)
    }
}

#[derive(Clone, Debug)]
struct TrackedVariant {
    version: String,
    architecture: Option<String>,
    authority: &'static str,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct FileConflict {
    path: String,
    owner: String,
}

#[derive(Clone, Debug)]
struct PlannedPackage {
    identity: ResolvedNativePackage,
    files: Vec<FileInfoTuple>,
    requirements: Vec<RepositoryRequirementGroup>,
    provides: Vec<ProvidedCapability>,
    duplicate_file_entries_skipped: usize,
}

impl PlannedPackage {
    fn record_summary(&self) -> String {
        format!(
            "records: 1 package, {} files, {} dependencies, {} provides, 1 changeset, 1 state snapshot",
            self.files.len(),
            self.requirements.len(),
            self.provides.len()
        )
    }
}

#[derive(Clone, Debug)]
enum PackagePlanOutcome {
    Ready(PlannedPackage),
    AlreadyTracked {
        requested: String,
        variants: Vec<TrackedVariant>,
    },
    Missing {
        requested: String,
        reason: String,
    },
    Ambiguous {
        requested: String,
        reason: String,
    },
    Unsupported {
        requested: String,
        reason: String,
    },
    Conflict {
        package: PlannedPackage,
        conflicts: Vec<FileConflict>,
    },
}

impl PackagePlanOutcome {
    fn ready_package(&self) -> Option<&PlannedPackage> {
        match self {
            Self::Ready(package) => Some(package),
            _ => None,
        }
    }

    fn is_refusal(&self) -> bool {
        matches!(
            self,
            Self::Missing { .. }
                | Self::Ambiguous { .. }
                | Self::Unsupported { .. }
                | Self::Conflict { .. }
        )
    }
}

#[derive(Clone, Debug)]
struct PackageAdoptionPlan {
    manager: SystemPackageManager,
    mode: AdoptionMode,
    version_scheme: VersionScheme,
    outcomes: Vec<PackagePlanOutcome>,
}

impl PackageAdoptionPlan {
    fn ready_count(&self) -> usize {
        self.outcomes
            .iter()
            .filter(|outcome| outcome.ready_package().is_some())
            .count()
    }

    fn refusal_count(&self) -> usize {
        self.outcomes
            .iter()
            .filter(|outcome| outcome.is_refusal())
            .count()
    }
}

/// Adopt or preview specific native packages.
pub async fn cmd_adopt(
    packages: &[String],
    db_path: &str,
    full: bool,
    dry_run: bool,
    requested_manager: Option<SystemPackageManager>,
) -> Result<()> {
    if packages.is_empty() {
        bail!("No packages specified");
    }

    super::super::hint_default_convergence();

    let manager = SystemPackageManager::resolve(requested_manager)?;
    if !manager.is_available() {
        bail!("No supported package manager found. Conary supports RPM, dpkg, pacman, and eopkg.");
    }
    let source = DetectedNativePackageSource::new(manager);
    cmd_adopt_with_source(packages, db_path, full, dry_run, &source)
}

fn cmd_adopt_with_source(
    packages: &[String],
    db_path: &str,
    full: bool,
    dry_run: bool,
    source: &dyn NativePackageSource,
) -> Result<()> {
    let mode = AdoptionMode::from_full(full);

    if dry_run {
        let conn = open_preview_db(db_path)?;
        let plan = build_adoption_plan(&conn, packages, mode, source)?;
        render_preview(&plan);
        refuse_empty_actionable_plan(&plan)?;
        return Ok(());
    }

    let mut conn = open_db(db_path)?;
    let plan = build_adoption_plan(&conn, packages, mode, source)?;
    render_non_ready_outcomes(&plan);
    refuse_empty_actionable_plan(&plan)?;
    execute_adoption_plan(&mut conn, db_path, &plan)
}

fn open_preview_db(path: &str) -> Result<Connection> {
    let path = Path::new(path);
    if !path.exists() {
        bail!(
            "Failed to open package database: database not found at {}",
            path.display()
        );
    }

    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .context("Failed to open package database read-only for adoption preview")?;
    let version = conn
        .query_row(
            "SELECT version FROM schema_version ORDER BY version DESC LIMIT 1",
            [],
            |row| row.get::<_, i32>(0),
        )
        .optional()
        .context("Failed to read package database schema version during adoption preview")?
        .ok_or_else(|| anyhow!("Package database has no schema version"))?;
    if version != conary_core::db::schema::SCHEMA_VERSION {
        bail!(
            "Package database schema is {version}, but this Conary binary requires {}; rebuild the database before preview",
            conary_core::db::schema::SCHEMA_VERSION
        );
    }
    Ok(conn)
}

fn build_adoption_plan(
    conn: &Connection,
    packages: &[String],
    mode: AdoptionMode,
    source: &dyn NativePackageSource,
) -> Result<PackageAdoptionPlan> {
    let mut outcomes = Vec::with_capacity(packages.len());

    for requested in packages {
        if let Some(reason) = invalid_exact_package_request(requested) {
            outcomes.push(PackagePlanOutcome::Unsupported {
                requested: requested.clone(),
                reason,
            });
            continue;
        }

        match source.lookup(requested) {
            PackageLookup::Found(identity) => {
                let exact_variants = tracked_exact_variant(conn, &identity.native)?;
                if !exact_variants.is_empty() {
                    outcomes.push(PackagePlanOutcome::AlreadyTracked {
                        requested: requested.clone(),
                        variants: exact_variants,
                    });
                    continue;
                }

                match collect_planned_package(conn, identity, source) {
                    Ok((package, conflicts)) if conflicts.is_empty() => {
                        outcomes.push(PackagePlanOutcome::Ready(package));
                    }
                    Ok((package, conflicts)) => {
                        outcomes.push(PackagePlanOutcome::Conflict { package, conflicts });
                    }
                    Err(error) => outcomes.push(PackagePlanOutcome::Unsupported {
                        requested: requested.clone(),
                        reason: format!("{error:#}"),
                    }),
                }
            }
            PackageLookup::Missing { reason } => outcomes.push(PackagePlanOutcome::Missing {
                requested: requested.clone(),
                reason,
            }),
            PackageLookup::Ambiguous { reason } => {
                outcomes.push(PackagePlanOutcome::Ambiguous {
                    requested: requested.clone(),
                    reason,
                });
            }
            PackageLookup::Unsupported { reason } => {
                outcomes.push(PackagePlanOutcome::Unsupported {
                    requested: requested.clone(),
                    reason,
                });
            }
        }
    }

    mark_duplicate_resolutions_ambiguous(&mut outcomes);
    resolve_planned_file_claims(&mut outcomes)?;

    let manager = source.manager();
    let version_scheme = manager.version_scheme().ok_or_else(|| {
        anyhow!(
            "Native package source {} has no exact version scheme",
            manager.display_name()
        )
    })?;

    Ok(PackageAdoptionPlan {
        manager,
        mode,
        version_scheme,
        outcomes,
    })
}

fn invalid_exact_package_request(requested: &str) -> Option<String> {
    if requested.is_empty() || requested.trim() != requested {
        return Some("package names must be non-empty and have no surrounding whitespace".into());
    }
    if requested.chars().any(char::is_whitespace) {
        return Some("package names cannot contain whitespace".into());
    }
    if requested.starts_with('-') {
        return Some("package names beginning with '-' are not supported".into());
    }
    if requested
        .chars()
        .any(|character| matches!(character, '*' | '?' | '[' | ']'))
    {
        return Some(
            "package adoption requires an exact native package name; glob patterns are unsupported"
                .into(),
        );
    }
    None
}

fn tracked_exact_variant(
    conn: &Connection,
    identity: &InstalledPackageIdentity,
) -> Result<Vec<TrackedVariant>> {
    Trove::find_by_name(conn, identity.name())?
        .into_iter()
        .filter(|trove| {
            trove.trove_type == TroveType::Package
                && trove.native_package_identity.as_ref() == Some(identity)
        })
        .map(|trove| {
            let authority = if trove.install_source.is_adopted() {
                "native authority"
            } else {
                "Conary-owned"
            };
            Ok(TrackedVariant {
                version: trove.version,
                architecture: trove.architecture,
                authority,
            })
        })
        .collect()
}

fn collect_planned_package(
    conn: &Connection,
    identity: ResolvedNativePackage,
    source: &dyn NativePackageSource,
) -> Result<(PlannedPackage, Vec<FileConflict>)> {
    let query_name = identity.native.selector();
    let raw_files = source
        .query_files(query_name)
        .with_context(|| format!("could not inspect files for '{query_name}'"))?;
    let requirements = source
        .query_requirements(query_name)
        .with_context(|| format!("could not inspect dependencies for '{query_name}'"))?;
    let mut provides = source
        .query_provides(&identity.native)
        .with_context(|| format!("could not inspect provides for '{query_name}'"))?;

    let mut files = Vec::with_capacity(raw_files.len());
    let mut conflicts = Vec::new();
    let mut seen_paths = HashSet::new();
    let mut duplicate_file_entries_skipped = 0usize;

    for file in raw_files {
        if !seen_paths.insert(file.0.clone()) {
            duplicate_file_entries_skipped += 1;
            continue;
        }

        let Some(existing) = FileEntry::find_by_path(conn, &file.0)? else {
            files.push(file);
            continue;
        };
        let owner = Trove::find_by_id(conn, existing.trove_id)?;
        let owner_name = owner
            .as_ref()
            .map(|trove| trove.name.as_str())
            .unwrap_or("<missing tracked owner>");

        let existing_owner_is_reusable = owner
            .as_ref()
            .is_some_and(|trove| trove.install_source == InstallSource::CapturedRoot)
            || owner_name == identity.native.name();
        let existing_path_is_shared_directory = !existing_owner_is_reusable
            && live_path_is_directory(&file.0)?
            && matches!(
                existing.node.source.kind,
                conary_core::payload::PayloadNodeKind::Directory
            );
        if existing_owner_is_reusable || existing_path_is_shared_directory {
            files.push(file);
        } else {
            conflicts.push(FileConflict {
                path: file.0,
                owner: owner_name.to_string(),
            });
        }
    }

    if conflicts.is_empty() {
        validate_package_files(identity.native.name(), &files)?;
        conary_core::repository::dependency_model::extend_materialized_file_provides(
            &mut provides,
            identity.native.source_package_format(),
            files.iter().filter_map(|file| {
                std::fs::symlink_metadata(&file.0)
                    .ok()
                    .map(|_| file.0.as_str())
            }),
        )?;
    }

    Ok((
        PlannedPackage {
            identity,
            files,
            requirements,
            provides,
            duplicate_file_entries_skipped,
        },
        conflicts,
    ))
}

fn live_path_is_directory(path: &str) -> Result<bool> {
    let node =
        conary_core::generation::root_manifest::capture_existing_payload_node(Path::new(path))
            .map_err(anyhow::Error::from)
            .with_context(|| format!("could not capture exact live payload node '{path}'"))?;
    Ok(matches!(
        node.source.kind,
        conary_core::payload::PayloadNodeKind::Directory
    ))
}

fn mark_duplicate_resolutions_ambiguous(outcomes: &mut [PackagePlanOutcome]) {
    let mut resolved = HashMap::<String, Vec<usize>>::new();
    for (index, outcome) in outcomes.iter().enumerate() {
        if let PackagePlanOutcome::Ready(package) = outcome {
            resolved
                .entry(package.identity.native.selector().to_string())
                .or_default()
                .push(index);
        }
    }

    for (canonical_selector, indices) in resolved {
        if indices.len() < 2 {
            continue;
        }
        let requested_names = indices
            .iter()
            .filter_map(|index| outcomes[*index].ready_package())
            .map(|package| package.identity.requested.clone())
            .collect::<Vec<_>>()
            .join(", ");
        let reason = format!(
            "multiple requested names resolve to native package '{canonical_selector}': {requested_names}; request it once using its canonical selector"
        );
        for index in indices {
            let requested = outcomes[index]
                .ready_package()
                .map(|package| package.identity.requested.clone())
                .unwrap_or_else(|| canonical_selector.clone());
            outcomes[index] = PackagePlanOutcome::Ambiguous {
                requested,
                reason: reason.clone(),
            };
        }
    }
}

fn resolve_planned_file_claims(outcomes: &mut [PackagePlanOutcome]) -> Result<()> {
    let mut claims = HashMap::<String, (String, bool)>::new();

    for outcome in outcomes.iter_mut() {
        let PackagePlanOutcome::Ready(mut package) = outcome.clone() else {
            continue;
        };
        let mut conflicts = Vec::new();
        let mut retained = Vec::with_capacity(package.files.len());
        for file in package.files.drain(..) {
            let Some((owner, owner_is_directory)) = claims.get(&file.0) else {
                retained.push(file);
                continue;
            };
            if *owner_is_directory && live_path_is_directory(&file.0)? {
                retained.push(file);
            } else {
                conflicts.push(FileConflict {
                    path: file.0.clone(),
                    owner: owner.clone(),
                });
                retained.push(file);
            }
        }
        package.files = retained;

        if conflicts.is_empty() {
            for file in &package.files {
                claims.insert(
                    file.0.clone(),
                    (
                        package.identity.native.selector().to_string(),
                        live_path_is_directory(&file.0)?,
                    ),
                );
            }
            *outcome = PackagePlanOutcome::Ready(package);
        } else {
            *outcome = PackagePlanOutcome::Conflict { package, conflicts };
        }
    }
    Ok(())
}

fn refuse_empty_actionable_plan(plan: &PackageAdoptionPlan) -> Result<()> {
    if plan.ready_count() == 0 && plan.refusal_count() > 0 {
        bail!("No packages are eligible for adoption; resolve the reported refusal(s) and retry");
    }
    Ok(())
}

fn render_preview(plan: &PackageAdoptionPlan) {
    crate::ui::heading("Package adoption preview");
    crate::ui::field("Package manager", plan.manager.display_name());
    crate::ui::field("Mode", plan.mode.label());
    crate::ui::field("Version scheme", plan.version_scheme.as_str());

    for outcome in &plan.outcomes {
        render_outcome(outcome, true);
    }

    crate::ui::note(
        "Preview only: no SQLite rows or schema, checkpoint backups, CAS objects, native package-manager state, hooks, generations, or live-root paths were changed.",
    );
}

fn render_non_ready_outcomes(plan: &PackageAdoptionPlan) {
    for outcome in &plan.outcomes {
        if !matches!(outcome, PackagePlanOutcome::Ready(_)) {
            render_outcome(outcome, false);
        }
    }
}

fn render_outcome(outcome: &PackagePlanOutcome, preview: bool) {
    match outcome {
        PackagePlanOutcome::Ready(package) => {
            if !preview {
                return;
            }
            let identity = format!(
                "{} {} [{}]",
                package.identity.native.name(),
                package.identity.native.version(),
                package.identity.native.architecture()
            );
            let mode = format!("mode: {}", if preview { "preview" } else { "apply" });
            let records = package.record_summary();
            crate::ui::row(crate::ui::Status::Pending, &[&identity, &mode, &records]);
            render_package_warnings(package);
        }
        PackagePlanOutcome::AlreadyTracked {
            requested,
            variants,
        } => {
            let variants = variants
                .iter()
                .map(|variant| {
                    format!(
                        "{} [{}] ({})",
                        variant.version,
                        variant.architecture.as_deref().unwrap_or("noarch"),
                        variant.authority
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            let detail = format!("already tracked: {variants}");
            crate::ui::row(crate::ui::Status::Skip, &[requested, &detail]);
        }
        PackagePlanOutcome::Missing { requested, reason } => {
            crate::ui::row(crate::ui::Status::Missing, &[requested, reason]);
        }
        PackagePlanOutcome::Ambiguous { requested, reason } => {
            crate::ui::row(crate::ui::Status::Warn, &[requested, reason]);
        }
        PackagePlanOutcome::Unsupported { requested, reason } => {
            crate::ui::row(crate::ui::Status::Fail, &[requested, reason]);
        }
        PackagePlanOutcome::Conflict { package, conflicts } => {
            let shown = conflicts
                .iter()
                .take(3)
                .map(|conflict| format!("{} owned by {}", conflict.path, conflict.owner))
                .collect::<Vec<_>>()
                .join(", ");
            let remaining = conflicts.len().saturating_sub(3);
            let detail = if remaining == 0 {
                format!("tracked file conflict(s): {shown}")
            } else {
                format!("tracked file conflict(s): {shown}, and {remaining} more")
            };
            crate::ui::row(
                crate::ui::Status::Fail,
                &[&package.identity.requested, &detail],
            );
            render_package_warnings(package);
        }
    }
}

fn render_package_warnings(package: &PlannedPackage) {
    if package.duplicate_file_entries_skipped > 0 {
        let warning = format!(
            "{} duplicate native file entr{} ignored",
            package.duplicate_file_entries_skipped,
            if package.duplicate_file_entries_skipped == 1 {
                "y was"
            } else {
                "ies were"
            }
        );
        crate::ui::row(
            crate::ui::Status::Warn,
            &[&package.identity.requested, &warning],
        );
    }
}

fn execute_adoption_plan(
    conn: &mut Connection,
    db_path: &str,
    plan: &PackageAdoptionPlan,
) -> Result<()> {
    let ready = plan
        .outcomes
        .iter()
        .filter_map(PackagePlanOutcome::ready_package)
        .collect::<Vec<_>>();
    if ready.is_empty() {
        return Ok(());
    }

    let objects_dir = conary_core::db::paths::objects_dir(db_path);
    let cas = if plan.mode == AdoptionMode::Full {
        Some(conary_core::filesystem::CasStore::new(&objects_dir)?)
    } else {
        None
    };
    let install_source = plan.mode.install_source();
    let mut progress = if ready.len() > 1 {
        AdoptProgress::new(ready.len() as u64, "Adopting")
    } else {
        AdoptProgress::single(&format!("Adopting {}", ready[0].identity.native.selector()))
    };

    for package in ready {
        let identity = &package.identity;
        let selector = identity.native.selector();
        progress.set_phase(selector, AdoptPhase::Querying);

        if plan.mode == AdoptionMode::Full {
            progress.set_phase(selector, AdoptPhase::CasStorage);
        }
        let captured_files = match capture_package_files(
            selector,
            &package.files,
            if plan.mode == AdoptionMode::Full {
                cas.as_ref()
                    .map(|cas| cas as &dyn conary_core::filesystem::PrivateCasWriter)
            } else {
                None
            },
        ) {
            Ok(files) => files,
            Err(error) => {
                let detail = format!("{selector}: {error}");
                crate::ui::row(crate::ui::Status::Fail, &[&detail]);
                progress.fail_package(selector, &error.to_string());
                continue;
            }
        };

        let mut changeset = Changeset::new(format!(
            "Adopt {} {} ({})",
            identity.native.name(),
            identity.native.version(),
            match plan.mode {
                AdoptionMode::Track => "track",
                AdoptionMode::Full => "full",
            }
        ));

        write_db_checkpoint(db_path, CheckpointReason::PreMutation)?;
        let changeset_id = conary_core::db::transaction(conn, |tx| {
            validate_planned_file_claims(tx, package)?;
            let changeset_id = changeset.insert(tx)?;

            let mut trove = Trove::new_with_source(
                identity.native.name().to_string(),
                identity.native.version(),
                TroveType::Package,
                install_source.clone(),
                plan.version_scheme,
            );
            trove.architecture = Some(identity.native.architecture().to_string());
            trove.debian_multi_arch = identity.native.debian_multi_arch();
            trove.description = identity.description.clone();
            trove.installed_by_changeset_id = Some(changeset_id);
            trove.selection_reason = Some("Adopted from the native package manager".to_string());
            trove.native_package_identity = Some(identity.native.clone());
            let trove_id = trove.insert(tx)?;

            progress.set_phase(selector, AdoptPhase::Inserting);
            for captured in &captured_files {
                let file_path = &captured.source.0;
                let mut file_entry = captured.file_entry(trove_id);
                file_entry
                    .insert_or_replace(
                        tx,
                        ExistingDirectoryMaterialization::ApplyIncoming,
                    )
                    .map_err(|error| {
                        conary_core::Error::ConfigError(format!(
                            "failed to persist exact adopted payload authority for {file_path}: {error}"
                        ))
                    })?;
            }

            super::requirements::insert_package_requirements(
                tx,
                trove_id,
                plan.version_scheme,
                identity.native.name(),
                &package.requirements,
            )
            .map_err(|error| {
                conary_core::Error::ConfigError(format!(
                    "failed to persist exact adopted requirements for {selector}: {error}"
                ))
            })?;

            super::provides::insert_package_provides(
                tx,
                trove_id,
                &identity.native,
                &package.provides,
            )?;

            changeset.update_status(tx, ChangesetStatus::Applied)?;
            Ok(changeset_id)
        })?;

        create_state_snapshot(conn, changeset_id, &format!("Adopt {selector}"))?;
        write_db_checkpoint(db_path, CheckpointReason::PostSuccess)?;

        progress.complete_package(selector);
    }

    progress.finish("Adoption complete");
    Ok(())
}

#[cfg(test)]
#[path = "packages/tests.rs"]
mod tests;
