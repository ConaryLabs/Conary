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
    Changeset, ChangesetStatus, DependencyEntry, FileEntry, InstallSource, ProvideEntry, Trove,
    TroveType,
};
use conary_core::packages::{
    DependencyInfo, InstalledSourceIdentity, SystemPackageManager, dpkg_query, pacman_query,
    rpm_query,
};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use tracing::debug;

use super::super::create_state_snapshot;
use super::super::open_db;
use super::super::progress::{AdoptPhase, AdoptProgress};
use super::cas_capture::{prepare_cas_backed_package_files, validate_cas_backed_package_files};
use super::checkpoint::write_db_checkpoint;
use super::outcome::{metadata_insert_succeeded, write_warning_metadata};
use super::system::{FileInfoTuple, compute_file_hash};
use crate::commands::AdoptionWarning;

const LIVE_ROOT_PACKAGE_NAME: &str = "conary-live-root";

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
struct NativePackageIdentity {
    requested: String,
    name: String,
    version: String,
    architecture: String,
    description: Option<String>,
}

#[derive(Clone, Debug)]
enum PackageLookup {
    Found(NativePackageIdentity),
    Missing { reason: String },
    Ambiguous { reason: String },
    Unsupported { reason: String },
}

trait NativePackageSource {
    fn manager(&self) -> SystemPackageManager;
    fn source_identity(&self) -> InstalledSourceIdentity;
    fn lookup(&self, requested: &str) -> PackageLookup;
    fn query_files(&self, query_name: &str) -> Result<Vec<FileInfoTuple>>;
    fn query_dependencies(&self, query_name: &str) -> Result<Vec<DependencyInfo>>;
    fn query_provides(&self, query_name: &str) -> Result<Vec<String>>;
}

struct DetectedNativePackageSource {
    manager: SystemPackageManager,
    source_identity: InstalledSourceIdentity,
}

impl DetectedNativePackageSource {
    fn new(manager: SystemPackageManager) -> Self {
        Self {
            manager,
            source_identity: manager.detect_source_identity(),
        }
    }
}

impl NativePackageSource for DetectedNativePackageSource {
    fn manager(&self) -> SystemPackageManager {
        self.manager
    }

    fn source_identity(&self) -> InstalledSourceIdentity {
        self.source_identity.clone()
    }

    fn lookup(&self, requested: &str) -> PackageLookup {
        let result = match self.manager {
            SystemPackageManager::Rpm => rpm_query::query_package(requested).map(|info| {
                let version = info.full_version();
                NativePackageIdentity {
                    requested: requested.to_string(),
                    name: info.name,
                    version,
                    architecture: info.arch,
                    description: info.description.or(info.summary),
                }
            }),
            SystemPackageManager::Dpkg => dpkg_query::query_package(requested).map(|info| {
                let version = info.version_only();
                NativePackageIdentity {
                    requested: requested.to_string(),
                    name: info.name,
                    version,
                    architecture: info.arch,
                    description: info.description,
                }
            }),
            SystemPackageManager::Pacman => pacman_query::query_package(requested).map(|info| {
                let version = info.version_only();
                NativePackageIdentity {
                    requested: requested.to_string(),
                    name: info.name,
                    version,
                    architecture: info.arch,
                    description: info.description,
                }
            }),
            SystemPackageManager::Unknown => {
                return PackageLookup::Unsupported {
                    reason: "no supported native package manager is available".to_string(),
                };
            }
        };

        match result {
            Ok(identity)
                if identity.name.trim().is_empty()
                    || identity.version.trim().is_empty()
                    || identity.architecture.trim().is_empty() =>
            {
                PackageLookup::Unsupported {
                    reason: format!(
                        "{} returned incomplete identity metadata for '{requested}'",
                        self.manager.display_name()
                    ),
                }
            }
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
                )
            })
            .collect())
    }

    fn query_dependencies(&self, query_name: &str) -> Result<Vec<DependencyInfo>> {
        match self.manager {
            SystemPackageManager::Rpm => rpm_query::query_package_dependencies_full(query_name)
                .with_context(|| format!("RPM dependency query failed for '{query_name}'")),
            SystemPackageManager::Dpkg => dpkg_query::query_package_dependencies_full(query_name)
                .with_context(|| format!("dpkg dependency query failed for '{query_name}'")),
            SystemPackageManager::Pacman => {
                pacman_query::query_package_dependencies_full(query_name)
                    .with_context(|| format!("pacman dependency query failed for '{query_name}'"))
            }
            SystemPackageManager::Unknown => Ok(Vec::new()),
        }
    }

    fn query_provides(&self, query_name: &str) -> Result<Vec<String>> {
        match self.manager {
            SystemPackageManager::Rpm => rpm_query::query_package_provides(query_name)
                .with_context(|| format!("RPM provides query failed for '{query_name}'")),
            SystemPackageManager::Dpkg => dpkg_query::query_package_provides(query_name)
                .with_context(|| format!("dpkg provides query failed for '{query_name}'")),
            SystemPackageManager::Pacman => pacman_query::query_package_provides(query_name)
                .with_context(|| format!("pacman provides query failed for '{query_name}'")),
            SystemPackageManager::Unknown => Ok(Vec::new()),
        }
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
    identity: NativePackageIdentity,
    files: Vec<FileInfoTuple>,
    dependencies: Vec<DependencyInfo>,
    provides: Vec<String>,
    shared_directories_skipped: usize,
    duplicate_file_entries_skipped: usize,
}

impl PlannedPackage {
    fn record_summary(&self) -> String {
        format!(
            "records: 1 package, {} files, {} dependencies, {} provides, 1 changeset, 1 state snapshot",
            self.files.len(),
            self.dependencies
                .iter()
                .filter(|dependency| !dependency.name.is_empty())
                .count(),
            self.provides
                .iter()
                .filter(|provide| !provide.is_empty())
                .count()
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
    source_identity: InstalledSourceIdentity,
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
) -> Result<()> {
    if packages.is_empty() {
        bail!("No packages specified");
    }

    super::super::hint_unconfigured_source_policy();

    let manager = SystemPackageManager::detect();
    if !manager.is_available() {
        bail!("No supported package manager found. Conary supports RPM, dpkg, and pacman.");
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
            "Package database schema is {version}, but this Conary binary requires {}; preview refuses to migrate state",
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

        let requested_variants = tracked_variants(conn, requested)?;
        if !requested_variants.is_empty() {
            outcomes.push(PackagePlanOutcome::AlreadyTracked {
                requested: requested.clone(),
                variants: requested_variants,
            });
            continue;
        }

        match source.lookup(requested) {
            PackageLookup::Found(identity) => {
                let canonical_variants = tracked_variants(conn, &identity.name)?;
                if !canonical_variants.is_empty() {
                    outcomes.push(PackagePlanOutcome::AlreadyTracked {
                        requested: requested.clone(),
                        variants: canonical_variants,
                    });
                    continue;
                }

                match collect_planned_package(conn, identity, mode, source) {
                    Ok((package, conflicts)) if conflicts.is_empty() => {
                        outcomes.push(PackagePlanOutcome::Ready(package));
                    }
                    Ok((package, conflicts)) => {
                        outcomes.push(PackagePlanOutcome::Conflict { package, conflicts });
                    }
                    Err(error) => outcomes.push(PackagePlanOutcome::Unsupported {
                        requested: requested.clone(),
                        reason: error.to_string(),
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
    resolve_planned_file_claims(&mut outcomes);

    Ok(PackageAdoptionPlan {
        manager: source.manager(),
        mode,
        source_identity: source.source_identity(),
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

fn tracked_variants(conn: &Connection, name: &str) -> Result<Vec<TrackedVariant>> {
    Trove::find_by_name(conn, name)?
        .into_iter()
        .filter(|trove| trove.trove_type == TroveType::Package)
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
    identity: NativePackageIdentity,
    mode: AdoptionMode,
    source: &dyn NativePackageSource,
) -> Result<(PlannedPackage, Vec<FileConflict>)> {
    let query_name = identity.requested.as_str();
    let raw_files = source
        .query_files(query_name)
        .with_context(|| format!("could not inspect files for '{query_name}'"))?;
    let dependencies = source
        .query_dependencies(query_name)
        .with_context(|| format!("could not inspect dependencies for '{query_name}'"))?;
    let provides = source
        .query_provides(query_name)
        .with_context(|| format!("could not inspect provides for '{query_name}'"))?;

    let mut files = Vec::with_capacity(raw_files.len());
    let mut conflicts = Vec::new();
    let mut seen_paths = HashSet::new();
    let mut shared_directories_skipped = 0usize;
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

        if owner_name == LIVE_ROOT_PACKAGE_NAME || owner_name == identity.name {
            files.push(file);
        } else if is_directory_mode(file.2) && is_directory_mode(existing.permissions) {
            shared_directories_skipped += 1;
        } else {
            conflicts.push(FileConflict {
                path: file.0,
                owner: owner_name.to_string(),
            });
        }
    }

    if mode == AdoptionMode::Full && conflicts.is_empty() {
        validate_cas_backed_package_files(&identity.name, &files)?;
    }

    Ok((
        PlannedPackage {
            identity,
            files,
            dependencies,
            provides,
            shared_directories_skipped,
            duplicate_file_entries_skipped,
        },
        conflicts,
    ))
}

fn is_directory_mode(mode: i32) -> bool {
    mode & 0o170000 == 0o040000
}

fn mark_duplicate_resolutions_ambiguous(outcomes: &mut [PackagePlanOutcome]) {
    let mut resolved = HashMap::<String, Vec<usize>>::new();
    for (index, outcome) in outcomes.iter().enumerate() {
        if let PackagePlanOutcome::Ready(package) = outcome {
            resolved
                .entry(package.identity.name.clone())
                .or_default()
                .push(index);
        }
    }

    for (canonical_name, indices) in resolved {
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
            "multiple requested names resolve to native package '{canonical_name}': {requested_names}; request it once using its canonical name"
        );
        for index in indices {
            let requested = outcomes[index]
                .ready_package()
                .map(|package| package.identity.requested.clone())
                .unwrap_or_else(|| canonical_name.clone());
            outcomes[index] = PackagePlanOutcome::Ambiguous {
                requested,
                reason: reason.clone(),
            };
        }
    }
}

fn resolve_planned_file_claims(outcomes: &mut [PackagePlanOutcome]) {
    let mut claims = HashMap::<String, (String, i32)>::new();

    for outcome in outcomes.iter_mut() {
        let PackagePlanOutcome::Ready(mut package) = outcome.clone() else {
            continue;
        };
        let mut conflicts = Vec::new();

        package.files.retain(|file| {
            let Some((owner, owner_mode)) = claims.get(&file.0) else {
                return true;
            };
            if is_directory_mode(file.2) && is_directory_mode(*owner_mode) {
                package.shared_directories_skipped += 1;
                false
            } else {
                conflicts.push(FileConflict {
                    path: file.0.clone(),
                    owner: owner.clone(),
                });
                true
            }
        });

        if conflicts.is_empty() {
            for file in &package.files {
                claims.insert(file.0.clone(), (package.identity.name.clone(), file.2));
            }
            *outcome = PackagePlanOutcome::Ready(package);
        } else {
            *outcome = PackagePlanOutcome::Conflict { package, conflicts };
        }
    }
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
    crate::ui::field(
        "Native source",
        plan.source_identity
            .source_distro
            .as_deref()
            .unwrap_or("unknown"),
    );

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
                package.identity.name, package.identity.version, package.identity.architecture
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
    if package.shared_directories_skipped > 0 {
        let warning = format!(
            "{} shared director{} already tracked; existing recorded ownership will be preserved",
            package.shared_directories_skipped,
            if package.shared_directories_skipped == 1 {
                "y is"
            } else {
                "ies are"
            }
        );
        crate::ui::row(
            crate::ui::Status::Warn,
            &[&package.identity.requested, &warning],
        );
    }
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
        AdoptProgress::single(&format!("Adopting {}", ready[0].identity.name))
    };

    for package in ready {
        let identity = &package.identity;
        progress.set_phase(&identity.name, AdoptPhase::Querying);

        let files_with_hashes = if plan.mode == AdoptionMode::Full {
            progress.set_phase(&identity.name, AdoptPhase::CasStorage);
            match prepare_cas_backed_package_files(
                &identity.name,
                &package.files,
                cas.as_ref()
                    .expect("CAS store must be available for full adoption"),
            ) {
                Ok(files) => files,
                Err(error) => {
                    let detail = format!("{}: {error}", identity.name);
                    crate::ui::row(crate::ui::Status::Fail, &[&detail]);
                    progress.fail_package(&identity.name, &error.to_string());
                    continue;
                }
            }
        } else {
            package
                .files
                .iter()
                .cloned()
                .map(|file| {
                    let hash = compute_file_hash(
                        &file.0,
                        file.2,
                        file.3.as_deref(),
                        file.6.as_deref(),
                        false,
                        None,
                    );
                    (file, hash)
                })
                .collect()
        };

        let mut changeset = Changeset::new(format!(
            "Adopt {} {} ({})",
            identity.name,
            identity.version,
            match plan.mode {
                AdoptionMode::Track => "track",
                AdoptionMode::Full => "full",
            }
        ));

        write_db_checkpoint(db_path, CheckpointReason::PreMutation)?;
        let (changeset_id, adopted, has_warnings) = conary_core::db::transaction(conn, |tx| {
            validate_planned_file_claims(tx, package)?;
            let changeset_id = changeset.insert(tx)?;

            let mut trove = Trove::new_with_source(
                identity.name.clone(),
                identity.version.clone(),
                TroveType::Package,
                install_source.clone(),
            );
            trove.architecture = Some(identity.architecture.clone());
            trove.description = identity.description.clone();
            trove.installed_by_changeset_id = Some(changeset_id);
            trove.selection_reason = Some("Adopted from the native package manager".to_string());
            trove.source_distro = plan.source_identity.source_distro.clone();
            trove.version_scheme = plan.source_identity.version_scheme.clone();
            let trove_id = trove.insert(tx)?;

            progress.set_phase(&identity.name, AdoptPhase::Inserting);
            let total_inserts = files_with_hashes.len()
                + package
                    .dependencies
                    .iter()
                    .filter(|dependency| !dependency.name.is_empty())
                    .count()
                + package
                    .provides
                    .iter()
                    .filter(|provide| !provide.is_empty())
                    .count();
            let mut insert_failures = 0usize;

            for (
                (file_path, file_size, file_mode, _digest, file_user, file_group, file_link_target),
                hash,
            ) in &files_with_hashes
            {
                let mut file_entry = FileEntry::new(
                    file_path.clone(),
                    hash.clone(),
                    *file_size,
                    *file_mode,
                    trove_id,
                );
                file_entry.owner = file_user.clone();
                file_entry.group_name = file_group.clone();
                file_entry.symlink_target = file_link_target.clone();
                if let Err(error) = file_entry.insert_or_replace(tx) {
                    debug!("Failed to insert file {file_path}: {error}");
                    insert_failures += 1;
                }
            }

            for dependency in &package.dependencies {
                if dependency.name.is_empty() {
                    continue;
                }
                let mut entry = DependencyEntry::new(
                    trove_id,
                    dependency.name.clone(),
                    None,
                    "runtime".to_string(),
                    dependency.constraint.clone(),
                );
                if let Err(error) = entry.insert(tx) {
                    debug!("Failed to insert dependency {}: {error}", dependency.name);
                    insert_failures += 1;
                }
            }

            for provide in &package.provides {
                if provide.is_empty() {
                    continue;
                }
                let mut entry = ProvideEntry::new(trove_id, provide.clone(), None);
                if let Err(error) = entry.insert_or_ignore(tx) {
                    debug!("Failed to insert provide {provide}: {error}");
                    insert_failures += 1;
                }
            }

            if !metadata_insert_succeeded(total_inserts, insert_failures) {
                Trove::delete(tx, trove_id)?;
                changeset.update_status(tx, ChangesetStatus::RolledBack)?;
                return Ok((changeset_id, false, false));
            }

            let warnings = if total_inserts > 0 && insert_failures > 0 {
                vec![AdoptionWarning::partial_insert_failure(
                    identity.name.clone(),
                    total_inserts,
                    insert_failures,
                )]
            } else {
                Vec::new()
            };
            let has_warnings = !warnings.is_empty();
            write_warning_metadata(tx, changeset_id, warnings).map_err(|error| {
                conary_core::Error::Io(std::io::Error::other(error.to_string()))
            })?;
            changeset.update_status(tx, ChangesetStatus::Applied)?;
            Ok((changeset_id, true, has_warnings))
        })?;

        if adopted {
            create_state_snapshot(conn, changeset_id, &format!("Adopt {}", identity.name))?;
        }
        write_db_checkpoint(db_path, CheckpointReason::PostSuccess)?;

        if !adopted {
            continue;
        }
        if has_warnings {
            crate::ui::warn(
                "Adopted with warnings. Run `conary system history` to inspect adoption warning metadata.",
            );
        }
        progress.complete_package(&identity.name);
    }

    progress.finish("Adoption complete");
    Ok(())
}

fn validate_planned_file_claims(
    conn: &Connection,
    package: &PlannedPackage,
) -> conary_core::Result<()> {
    for file in &package.files {
        let Some(existing) = FileEntry::find_by_path(conn, &file.0)? else {
            continue;
        };
        let owner = Trove::find_by_id(conn, existing.trove_id)?;
        let owner_name = owner
            .as_ref()
            .map(|trove| trove.name.as_str())
            .unwrap_or("<missing tracked owner>");
        if owner_name == LIVE_ROOT_PACKAGE_NAME || owner_name == package.identity.name {
            continue;
        }
        return Err(conary_core::Error::ConflictError(format!(
            "Path {} became tracked by package {} after adoption planning",
            file.0, owner_name
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use conary_core::db;
    use conary_core::db::models::{FileEntry, InstallSource, Trove, TroveType};
    use walkdir::WalkDir;

    use super::*;

    #[derive(Default)]
    struct FixtureSource {
        lookups: HashMap<String, PackageLookup>,
        files: HashMap<String, Vec<FileInfoTuple>>,
        dependencies: HashMap<String, Vec<DependencyInfo>>,
        provides: HashMap<String, Vec<String>>,
    }

    impl FixtureSource {
        fn with_ready(mut self, requested: &str, name: &str, file: FileInfoTuple) -> Self {
            self.lookups.insert(
                requested.to_string(),
                PackageLookup::Found(NativePackageIdentity {
                    requested: requested.to_string(),
                    name: name.to_string(),
                    version: "1.2.3-4".to_string(),
                    architecture: "x86_64".to_string(),
                    description: Some("Fixture package".to_string()),
                }),
            );
            self.files.insert(requested.to_string(), vec![file]);
            self.dependencies.insert(
                requested.to_string(),
                vec![DependencyInfo {
                    name: "libc".to_string(),
                    constraint: Some(">= 2".to_string()),
                }],
            );
            self.provides
                .insert(requested.to_string(), vec![name.to_string()]);
            self
        }

        fn with_lookup(mut self, requested: &str, lookup: PackageLookup) -> Self {
            self.lookups.insert(requested.to_string(), lookup);
            self
        }
    }

    impl NativePackageSource for FixtureSource {
        fn manager(&self) -> SystemPackageManager {
            SystemPackageManager::Rpm
        }

        fn source_identity(&self) -> InstalledSourceIdentity {
            InstalledSourceIdentity {
                source_distro: Some("fedora-44".to_string()),
                version_scheme: Some("rpm".to_string()),
            }
        }

        fn lookup(&self, requested: &str) -> PackageLookup {
            self.lookups
                .get(requested)
                .cloned()
                .unwrap_or_else(|| PackageLookup::Missing {
                    reason: format!("{requested} is absent"),
                })
        }

        fn query_files(&self, query_name: &str) -> Result<Vec<FileInfoTuple>> {
            Ok(self.files.get(query_name).cloned().unwrap_or_default())
        }

        fn query_dependencies(&self, query_name: &str) -> Result<Vec<DependencyInfo>> {
            Ok(self
                .dependencies
                .get(query_name)
                .cloned()
                .unwrap_or_default())
        }

        fn query_provides(&self, query_name: &str) -> Result<Vec<String>> {
            Ok(self.provides.get(query_name).cloned().unwrap_or_default())
        }
    }

    fn file_tuple(path: &Path, mode: i32) -> FileInfoTuple {
        (
            path.to_string_lossy().into_owned(),
            fs::symlink_metadata(path)
                .map(|metadata| i64::try_from(metadata.len()).unwrap())
                .unwrap_or(0),
            mode,
            None,
            Some("root".to_string()),
            Some("root".to_string()),
            None,
        )
    }

    fn temp_db() -> (tempfile::TempDir, String) {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("conary.db");
        db::init(&db_path).unwrap();
        (temp, db_path.to_string_lossy().into_owned())
    }

    fn seed_tracked_file(db_path: &str, package: &str, path: &Path, mode: i32) {
        let conn = db::open(db_path).unwrap();
        let mut trove = Trove::new(package.to_string(), "9.9.9".to_string(), TroveType::Package);
        let trove_id = trove.insert(&conn).unwrap();
        let mut file = FileEntry::new(
            path.to_string_lossy().into_owned(),
            "existing-hash".to_string(),
            0,
            mode,
            trove_id,
        );
        file.insert(&conn).unwrap();
    }

    fn tree_snapshot(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
        let mut snapshot = WalkDir::new(root)
            .follow_links(false)
            .into_iter()
            .filter_map(std::result::Result::ok)
            .filter(|entry| entry.file_type().is_file())
            .filter(|entry| {
                !entry
                    .path()
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().starts_with("conary.db"))
            })
            .map(|entry| {
                (
                    entry.path().strip_prefix(root).unwrap().to_path_buf(),
                    fs::read(entry.path()).unwrap(),
                )
            })
            .collect::<Vec<_>>();
        snapshot.sort_by(|left, right| left.0.cmp(&right.0));
        snapshot
    }

    #[derive(Debug, PartialEq, Eq)]
    struct DatabaseSnapshot {
        schema: Vec<(String, String, Option<String>)>,
        rows: Vec<(String, Vec<Vec<Vec<u8>>>)>,
    }

    fn database_snapshot(db_path: &str) -> DatabaseSnapshot {
        use rusqlite::types::ValueRef;

        let conn = Connection::open_with_flags(
            db_path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .unwrap();
        let mut schema = conn
            .prepare(
                "SELECT type, name, sql FROM sqlite_schema
                 WHERE name NOT LIKE 'sqlite_autoindex_%'
                 ORDER BY type, name",
            )
            .unwrap()
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        schema.sort();

        let table_names = schema
            .iter()
            .filter(|(kind, _, _)| kind == "table")
            .map(|(_, name, _)| name.clone())
            .collect::<Vec<_>>();
        let mut rows = Vec::with_capacity(table_names.len());
        for table in table_names {
            let quoted = table.replace('"', "\"\"");
            let mut statement = conn
                .prepare(&format!("SELECT * FROM \"{quoted}\""))
                .unwrap();
            let column_count = statement.column_count();
            let mut table_rows = statement
                .query_map([], |row| {
                    (0..column_count)
                        .map(|column| {
                            Ok(match row.get_ref(column)? {
                                ValueRef::Null => vec![0],
                                ValueRef::Integer(value) => {
                                    let mut encoded = vec![1];
                                    encoded.extend_from_slice(&value.to_be_bytes());
                                    encoded
                                }
                                ValueRef::Real(value) => {
                                    let mut encoded = vec![2];
                                    encoded.extend_from_slice(&value.to_bits().to_be_bytes());
                                    encoded
                                }
                                ValueRef::Text(value) => {
                                    let mut encoded = vec![3];
                                    encoded.extend_from_slice(value);
                                    encoded
                                }
                                ValueRef::Blob(value) => {
                                    let mut encoded = vec![4];
                                    encoded.extend_from_slice(value);
                                    encoded
                                }
                            })
                        })
                        .collect::<rusqlite::Result<Vec<_>>>()
                })
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap();
            table_rows.sort();
            rows.push((table, table_rows));
        }

        DatabaseSnapshot { schema, rows }
    }

    fn ready_package(plan: &PackageAdoptionPlan) -> &PlannedPackage {
        plan.outcomes
            .iter()
            .find_map(PackagePlanOutcome::ready_package)
            .expect("expected a ready package")
    }

    #[test]
    fn plan_classifies_ready_tracked_missing_ambiguous_unsupported_and_conflict() {
        let (temp, db_path) = temp_db();
        let ready_path = temp.path().join("root/usr/bin/ready");
        let tracked_path = temp.path().join("root/usr/bin/tracked");
        let conflict_path = temp.path().join("root/usr/bin/conflict");
        fs::create_dir_all(ready_path.parent().unwrap()).unwrap();
        fs::write(&ready_path, b"ready").unwrap();
        fs::write(&tracked_path, b"tracked").unwrap();
        fs::write(&conflict_path, b"conflict").unwrap();
        seed_tracked_file(&db_path, "tracked", &tracked_path, 0o100755);
        seed_tracked_file(&db_path, "other-owner", &conflict_path, 0o100755);

        let source = FixtureSource::default()
            .with_ready("ready", "ready", file_tuple(&ready_path, 0o100755))
            .with_ready(
                "conflicting",
                "conflicting",
                file_tuple(&conflict_path, 0o100755),
            )
            .with_lookup(
                "missing",
                PackageLookup::Missing {
                    reason: "not installed".to_string(),
                },
            )
            .with_lookup(
                "ambiguous",
                PackageLookup::Ambiguous {
                    reason: "two native variants match".to_string(),
                },
            )
            .with_lookup(
                "unsupported",
                PackageLookup::Unsupported {
                    reason: "metadata cannot be represented".to_string(),
                },
            );
        let conn = db::open(&db_path).unwrap();
        let packages = [
            "ready",
            "tracked",
            "missing",
            "ambiguous",
            "unsupported",
            "conflicting",
        ]
        .map(str::to_string);

        let plan = build_adoption_plan(&conn, &packages, AdoptionMode::Track, &source).unwrap();

        assert!(matches!(plan.outcomes[0], PackagePlanOutcome::Ready(_)));
        assert!(matches!(
            plan.outcomes[1],
            PackagePlanOutcome::AlreadyTracked { .. }
        ));
        assert!(matches!(
            plan.outcomes[2],
            PackagePlanOutcome::Missing { .. }
        ));
        assert!(matches!(
            plan.outcomes[3],
            PackagePlanOutcome::Ambiguous { .. }
        ));
        assert!(matches!(
            plan.outcomes[4],
            PackagePlanOutcome::Unsupported { .. }
        ));
        assert!(matches!(
            plan.outcomes[5],
            PackagePlanOutcome::Conflict { .. }
        ));
    }

    #[test]
    fn preview_and_apply_share_identity_mode_and_record_plan() {
        let (temp, db_path) = temp_db();
        let live_file = temp.path().join("native-root/usr/bin/fixture");
        fs::create_dir_all(live_file.parent().unwrap()).unwrap();
        fs::write(&live_file, b"fixture").unwrap();
        let source = FixtureSource::default().with_ready(
            "fixture",
            "fixture",
            file_tuple(&live_file, 0o100755),
        );
        let packages = vec!["fixture".to_string()];

        cmd_adopt_with_source(&packages, &db_path, false, true, &source).unwrap();
        let conn = db::open(&db_path).unwrap();
        assert!(Trove::find_by_name(&conn, "fixture").unwrap().is_empty());
        drop(conn);

        cmd_adopt_with_source(&packages, &db_path, false, false, &source).unwrap();

        let conn = db::open(&db_path).unwrap();
        let troves = Trove::find_by_name(&conn, "fixture").unwrap();
        assert_eq!(troves.len(), 1);
        assert_eq!(troves[0].version, "1.2.3-4");
        assert_eq!(troves[0].architecture.as_deref(), Some("x86_64"));
        assert_eq!(troves[0].install_source, InstallSource::AdoptedTrack);
        let trove_id = troves[0].id.unwrap();
        assert_eq!(FileEntry::find_by_trove(&conn, trove_id).unwrap().len(), 1);
        assert_eq!(
            DependencyEntry::find_by_trove(&conn, trove_id)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            ProvideEntry::find_by_trove(&conn, trove_id).unwrap().len(),
            1
        );
    }

    #[test]
    fn preview_reads_committed_wal_rows_without_mutating_logical_state_or_other_paths() {
        let (temp, db_path) = temp_db();
        let mut writer = db::open(&db_path).unwrap();
        let mut trove = Trove::new(
            "wal-fixture".to_string(),
            "1.0.0".to_string(),
            TroveType::Package,
        );
        db::transaction(&mut writer, |tx| trove.insert(tx).map(|_| ())).unwrap();
        let database_before = database_snapshot(&db_path);
        let before = tree_snapshot(temp.path());

        let preview = open_preview_db(&db_path).unwrap();

        assert_eq!(
            Trove::find_by_name(&preview, "wal-fixture").unwrap().len(),
            1
        );
        assert_eq!(database_snapshot(&db_path), database_before);
        assert_eq!(tree_snapshot(temp.path()), before);
    }

    #[test]
    fn full_preview_leaves_database_cas_backups_hooks_generations_and_live_file_unchanged() {
        let (temp, db_path) = temp_db();
        let live_file = temp.path().join("native-root/usr/bin/fixture");
        fs::create_dir_all(live_file.parent().unwrap()).unwrap();
        fs::write(&live_file, b"fixture bytes").unwrap();
        for marker in [
            temp.path().join("objects/existing"),
            temp.path().join("backups/existing"),
            temp.path().join("hooks/existing"),
            temp.path().join("generations/1/existing"),
        ] {
            fs::create_dir_all(marker.parent().unwrap()).unwrap();
            fs::write(marker, b"keep").unwrap();
        }
        let source = FixtureSource::default().with_ready(
            "fixture",
            "fixture",
            file_tuple(&live_file, 0o100755),
        );
        let database_before = database_snapshot(&db_path);
        let before = tree_snapshot(temp.path());

        cmd_adopt_with_source(&["fixture".to_string()], &db_path, true, true, &source).unwrap();

        assert_eq!(database_snapshot(&db_path), database_before);
        assert_eq!(tree_snapshot(temp.path()), before);
        assert_eq!(fs::read(&live_file).unwrap(), b"fixture bytes");
    }

    #[test]
    fn refused_conflict_preview_is_tree_read_only() {
        let (temp, db_path) = temp_db();
        let live_file = temp.path().join("native-root/usr/bin/conflict");
        fs::create_dir_all(live_file.parent().unwrap()).unwrap();
        fs::write(&live_file, b"fixture bytes").unwrap();
        seed_tracked_file(&db_path, "other-owner", &live_file, 0o100755);
        let source = FixtureSource::default().with_ready(
            "fixture",
            "fixture",
            file_tuple(&live_file, 0o100755),
        );
        let database_before = database_snapshot(&db_path);
        let before = tree_snapshot(temp.path());

        let error = cmd_adopt_with_source(&["fixture".to_string()], &db_path, false, true, &source)
            .unwrap_err();

        assert!(error.to_string().contains("No packages are eligible"));
        assert_eq!(database_snapshot(&db_path), database_before);
        assert_eq!(tree_snapshot(temp.path()), before);
    }

    #[test]
    fn duplicate_aliases_resolving_to_one_native_package_are_ambiguous() {
        let (temp, db_path) = temp_db();
        let live_file = temp.path().join("native-root/usr/bin/fixture");
        fs::create_dir_all(live_file.parent().unwrap()).unwrap();
        fs::write(&live_file, b"fixture").unwrap();
        let source = FixtureSource::default()
            .with_ready("fixture", "fixture", file_tuple(&live_file, 0o100755))
            .with_ready(
                "fixture.x86_64",
                "fixture",
                file_tuple(&live_file, 0o100755),
            );
        let conn = db::open(&db_path).unwrap();

        let plan = build_adoption_plan(
            &conn,
            &["fixture".to_string(), "fixture.x86_64".to_string()],
            AdoptionMode::Track,
            &source,
        )
        .unwrap();

        assert_eq!(plan.ready_count(), 0);
        assert!(
            plan.outcomes
                .iter()
                .all(|outcome| matches!(outcome, PackagePlanOutcome::Ambiguous { .. }))
        );
    }

    #[test]
    fn shared_directories_preserve_existing_recorded_owner() {
        let (temp, db_path) = temp_db();
        let shared_dir = temp.path().join("native-root/usr/share");
        let package_file = shared_dir.join("fixture");
        fs::create_dir_all(&shared_dir).unwrap();
        fs::write(&package_file, b"fixture").unwrap();
        seed_tracked_file(&db_path, "directory-owner", &shared_dir, 0o040755);
        let source = FixtureSource::default().with_ready(
            "fixture",
            "fixture",
            file_tuple(&shared_dir, 0o040755),
        );
        let conn = db::open(&db_path).unwrap();

        let plan = build_adoption_plan(
            &conn,
            &["fixture".to_string()],
            AdoptionMode::Track,
            &source,
        )
        .unwrap();
        let package = ready_package(&plan);

        assert!(package.files.is_empty());
        assert_eq!(package.shared_directories_skipped, 1);
    }

    #[test]
    fn planned_packages_resolve_shared_directories_and_file_conflicts_before_apply() {
        let (temp, db_path) = temp_db();
        let shared_dir = temp.path().join("native-root/usr/share");
        let shared_file = shared_dir.join("fixture");
        fs::create_dir_all(&shared_dir).unwrap();
        fs::write(&shared_file, b"fixture").unwrap();
        let source = FixtureSource::default()
            .with_ready("first", "first", file_tuple(&shared_dir, 0o040755))
            .with_ready("second", "second", file_tuple(&shared_dir, 0o040755))
            .with_ready(
                "conflicting",
                "conflicting",
                file_tuple(&shared_dir, 0o100755),
            );
        let conn = db::open(&db_path).unwrap();

        let plan = build_adoption_plan(
            &conn,
            &[
                "first".to_string(),
                "second".to_string(),
                "conflicting".to_string(),
            ],
            AdoptionMode::Track,
            &source,
        )
        .unwrap();

        assert!(matches!(plan.outcomes[0], PackagePlanOutcome::Ready(_)));
        let PackagePlanOutcome::Ready(second) = &plan.outcomes[1] else {
            panic!("second package should remain ready");
        };
        assert!(second.files.is_empty());
        assert_eq!(second.shared_directories_skipped, 1);
        assert!(matches!(
            plan.outcomes[2],
            PackagePlanOutcome::Conflict { .. }
        ));
    }
}
