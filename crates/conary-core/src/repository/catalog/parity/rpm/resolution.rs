// crates/conary-core/src/repository/catalog/parity/rpm/resolution.rs

//! Independent libsolv-backed RPM native resolution evidence production.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use rusqlite::{Connection, OptionalExtension, params};

use super::ffi::{RequiredKind, SolvProblem, SolvProblemRule, SolvResolution};
use super::{
    PINNED_LIBSOLV_VERSION, RPM_PARITY_PROJECTION_SCHEMA_V1, RpmParityMemberInput, SolvPool,
    produce_rpm_parity_oracle, project_package, project_requirement, stage_verified_metadata,
    validate_inputs,
};
use crate::error::{Error, Result};
use crate::repository::catalog::ProfileRevisionV2;
use crate::repository::catalog::parity::{
    NATIVE_RESOLUTION_ROOT_FILE_NAME, NativeParityEcosystemV1, NativeParityImplementationV1,
    NativeParityOracleReader, NativeParityOracleV1, NativeResolutionInstalledStateV1,
    NativeResolutionOracleV1, NativeResolutionOracleWriter, NativeResolutionOutcomeV1,
    NativeResolutionPolicyV1, NativeResolutionProviderPolicyV1,
    NativeResolutionRequirementPolicyV1, NativeResolutionRootPolicyV1, NativeResolutionRootV1,
    NativeUnresolvedDependencyV1, native_requirement_group_sha256,
    verify_native_parity_oracle_bundle, verify_native_resolution_oracle_bundle,
    write_native_resolution_oracle_manifest,
};
use crate::repository::dependency_model::RepositoryRequirementKind;

/// Projection contract for libsolv transaction results and typed problem rules.
pub const RPM_RESOLUTION_PROJECTION_SCHEMA_V3: u32 = 3;

const SOLVER_RULE_PKG_NOT_INSTALLABLE: i32 = 0x101;
const SOLVER_RULE_PKG_NOTHING_PROVIDES_DEP: i32 = 0x102;
const SOLVER_RULE_PKG_REQUIRES: i32 = 0x103;
const SOLVER_RULE_PKG_CONFLICTS: i32 = 0x105;
const SOLVER_RULE_JOB: i32 = 0x400;
const SOLVER_RULE_JOB_UNSUPPORTED: i32 = 0x404;
const SOLVER_RULE_INFARCH: i32 = 0x600;
const SOLVER_RULE_STRICT_REPO_PRIORITY: i32 = 0xd00;

const CREATE_INDEX: &str = "
CREATE TABLE oracle_packages (
    package_key_sha256 TEXT PRIMARY KEY,
    row_json BLOB NOT NULL,
    selected_native_index INTEGER
) STRICT;
CREATE TABLE native_packages (
    native_index INTEGER PRIMARY KEY,
    package_key_sha256 TEXT NOT NULL REFERENCES oracle_packages(package_key_sha256)
) STRICT;
";

/// Produce and independently reopen one strict RPM resolution parity bundle.
pub fn produce_rpm_resolution_oracle(
    profile: &ProfileRevisionV2,
    inputs: &[RpmParityMemberInput<'_>],
    package_oracle_directory: &Path,
    architecture: &str,
    output: &Path,
) -> Result<NativeResolutionOracleV1> {
    let policy = NativeResolutionPolicyV1 {
        architecture: architecture.to_string(),
        installed_state: NativeResolutionInstalledStateV1::Empty,
        roots: NativeResolutionRootPolicyV1::EveryExactPackage,
        positive_requirements: NativeResolutionRequirementPolicyV1::RequiredOnly,
        provider_selection: NativeResolutionProviderPolicyV1::NativePrecedence,
    };
    policy.validate()?;

    let package_oracle = verify_native_parity_oracle_bundle(package_oracle_directory, profile)?;
    require_rpm_package_oracle(package_oracle.manifest())?;
    verify_package_oracle_reprojection(profile, inputs, package_oracle.manifest())?;

    validate_inputs(profile, inputs)?;
    let staging = tempfile::Builder::new()
        .prefix("conary-rpm-resolution-")
        .tempdir()?;
    let staged = stage_verified_metadata(inputs, staging.path())?;
    let mut pool = SolvPool::create()?;
    if SolvPool::version()? != PINNED_LIBSOLV_VERSION {
        return Err(Error::ConfigError(format!(
            "RPM resolution requires libsolv {PINNED_LIBSOLV_VERSION}"
        )));
    }
    for (ordinal, (member, metadata)) in profile.members.iter().zip(&staged).enumerate() {
        let ordinal = u32::try_from(ordinal)
            .map_err(|_| Error::ConfigError("RPM member ordinal exceeds u32".to_string()))?;
        pool.load(
            &format!("conary-member-{ordinal}"),
            &metadata.primary,
            &metadata.filelists,
            ordinal,
            member.precedence,
        )?;
    }
    pool.set_architecture(architecture)?;
    let package_index = PackageResolutionIndex::create(profile, inputs, &pool, &package_oracle)?;

    let implementation = NativeParityImplementationV1 {
        ecosystem: NativeParityEcosystemV1::Rpm,
        name: "libsolv".to_string(),
        version: PINNED_LIBSOLV_VERSION.to_string(),
        projection_schema: RPM_RESOLUTION_PROJECTION_SCHEMA_V3,
    };
    fs::create_dir(output)?;
    let mut writer = NativeResolutionOracleWriter::create(
        output.join(NATIVE_RESOLUTION_ROOT_FILE_NAME),
        profile,
        package_oracle.manifest(),
        implementation,
        policy,
    )?;
    package_oracle.for_each_package(|root| {
        let root_index = package_index.selected_native_index(&root.package_key_sha256)?;
        let outcome =
            resolve_exact_root(&mut pool, &package_index, root_index, &root, architecture)?;
        writer.root(&NativeResolutionRootV1 {
            root_package_key_sha256: root.package_key_sha256,
            outcome,
        })
    })?;
    let manifest = writer.finish()?;
    write_native_resolution_oracle_manifest(output, &manifest)?;
    let reopened = verify_native_resolution_oracle_bundle(output, profile, &package_oracle)?;
    if reopened.manifest() != &manifest {
        return Err(Error::InternalError(
            "reopened RPM resolution manifest differs from produced manifest".to_string(),
        ));
    }
    Ok(manifest)
}

fn require_rpm_package_oracle(manifest: &NativeParityOracleV1) -> Result<()> {
    if manifest.implementation.ecosystem != NativeParityEcosystemV1::Rpm
        || manifest.implementation.name != "libsolv"
        || manifest.implementation.version != PINNED_LIBSOLV_VERSION
        || manifest.implementation.projection_schema != RPM_PARITY_PROJECTION_SCHEMA_V1
    {
        return Err(Error::ConfigError(format!(
            "RPM resolution requires the pinned libsolv {PINNED_LIBSOLV_VERSION} package oracle"
        )));
    }
    Ok(())
}

fn verify_package_oracle_reprojection(
    profile: &ProfileRevisionV2,
    inputs: &[RpmParityMemberInput<'_>],
    expected: &NativeParityOracleV1,
) -> Result<()> {
    let scratch = tempfile::Builder::new()
        .prefix("conary-rpm-resolution-package-proof-")
        .tempdir()?;
    let produced =
        produce_rpm_parity_oracle(profile, inputs, &scratch.path().join("package-oracle"))?;
    if &produced != expected {
        return Err(Error::ConflictError(
            "RPM package oracle does not match a fresh libsolv projection of its authenticated metadata"
                .to_string(),
        ));
    }
    Ok(())
}

struct PackageResolutionIndex {
    _scratch: tempfile::TempDir,
    connection: Connection,
}

impl PackageResolutionIndex {
    fn create(
        profile: &ProfileRevisionV2,
        inputs: &[RpmParityMemberInput<'_>],
        pool: &SolvPool,
        package_oracle: &NativeParityOracleReader,
    ) -> Result<Self> {
        let scratch = tempfile::Builder::new()
            .prefix("conary-rpm-resolution-index-")
            .tempdir()?;
        let mut connection = Connection::open(scratch.path().join("packages.sqlite3"))?;
        connection.execute_batch(CREATE_INDEX)?;
        let transaction = connection.transaction()?;
        package_oracle.for_each_package(|package| {
            transaction.execute(
                "INSERT INTO oracle_packages (package_key_sha256, row_json)
                 VALUES (?1, ?2)",
                params![
                    package.package_key_sha256,
                    crate::json::canonical_json(&package).map_err(|error| {
                        Error::ParseError(format!(
                            "serialize indexed RPM package-oracle row: {error}"
                        ))
                    })?,
                ],
            )?;
            Ok(())
        })?;
        for native_index in 0..pool.package_count() {
            let native = pool.package(native_index)?;
            let package = project_package(profile, inputs, &native)?;
            let expected: Option<Vec<u8>> = transaction
                .query_row(
                    "SELECT row_json FROM oracle_packages WHERE package_key_sha256 = ?1",
                    [&package.package_key_sha256],
                    |row| row.get(0),
                )
                .optional()?;
            let Some(expected) = expected else {
                return Err(Error::ConflictError(format!(
                    "libsolv native package '{}' is absent from the bound RPM package oracle",
                    package.name
                )));
            };
            let expected: crate::repository::catalog::parity::NativeParityPackageV1 =
                serde_json::from_slice(&expected).map_err(|error| {
                    Error::InternalError(format!(
                        "reopen indexed RPM package-oracle row '{}': {error}",
                        package.package_key_sha256
                    ))
                })?;
            if !expected.has_same_profile_facts(&package)? {
                return Err(Error::ConflictError(format!(
                    "libsolv native package '{}' disagrees with the bound RPM package oracle",
                    package.name
                )));
            }
            let native_index = i64::try_from(native_index).map_err(|_| {
                Error::ConfigError("libsolv package index exceeds SQLite i64".to_string())
            })?;
            transaction.execute(
                "INSERT INTO native_packages (native_index, package_key_sha256)
                 VALUES (?1, ?2)",
                params![native_index, package.package_key_sha256],
            )?;
            transaction.execute(
                "UPDATE oracle_packages SET selected_native_index = ?1
                 WHERE package_key_sha256 = ?2 AND selected_native_index IS NULL",
                params![native_index, package.package_key_sha256],
            )?;
        }
        let missing: i64 = transaction.query_row(
            "SELECT COUNT(*) FROM oracle_packages WHERE selected_native_index IS NULL",
            [],
            |row| row.get(0),
        )?;
        if missing != 0 {
            return Err(Error::ConflictError(format!(
                "RPM package oracle contains {missing} packages absent from the native solver pool"
            )));
        }
        transaction.commit()?;
        Ok(Self {
            _scratch: scratch,
            connection,
        })
    }

    fn selected_native_index(&self, package_key: &str) -> Result<usize> {
        let index: i64 = self.connection.query_row(
            "SELECT selected_native_index FROM oracle_packages WHERE package_key_sha256 = ?1",
            [package_key],
            |row| row.get(0),
        )?;
        usize::try_from(index).map_err(|_| {
            Error::InternalError("negative or oversized native RPM package index".to_string())
        })
    }

    fn package_key(&self, native_index: usize) -> Result<String> {
        let native_index = i64::try_from(native_index).map_err(|_| {
            Error::InternalError("native RPM package index exceeds SQLite i64".to_string())
        })?;
        self.connection
            .query_row(
                "SELECT package_key_sha256 FROM native_packages WHERE native_index = ?1",
                [native_index],
                |row| row.get(0),
            )
            .map_err(Into::into)
    }
}

fn resolve_exact_root(
    pool: &mut SolvPool,
    package_index: &PackageResolutionIndex,
    root_index: usize,
    root: &crate::repository::catalog::parity::NativeParityPackageV1,
    architecture: &str,
) -> Result<NativeResolutionOutcomeV1> {
    match pool.solve(root_index)? {
        SolvResolution::Resolved(packages) => {
            let closure = packages
                .into_iter()
                .map(|index| package_index.package_key(index))
                .collect::<Result<BTreeSet<_>>>()?;
            if !closure.contains(&root.package_key_sha256) {
                return Err(Error::ConflictError(format!(
                    "libsolv closure for '{}' omits its exact root",
                    root.name
                )));
            }
            Ok(NativeResolutionOutcomeV1::Resolved {
                closure_package_keys_sha256: closure.into_iter().collect(),
            })
        }
        SolvResolution::Unresolved(rules) => {
            unresolved_outcome(pool, package_index, root_index, root, architecture, rules)
        }
    }
}

fn unresolved_outcome(
    pool: &mut SolvPool,
    package_index: &PackageResolutionIndex,
    root_index: usize,
    root: &crate::repository::catalog::parity::NativeParityPackageV1,
    architecture: &str,
    problems: Vec<SolvProblem>,
) -> Result<NativeResolutionOutcomeV1> {
    let mut dependencies = BTreeSet::new();
    let mut requires_residual_probe = false;
    let shadowed = pool
        .strict_shadowed_packages()?
        .into_iter()
        .collect::<BTreeSet<_>>();
    let visibility = StrictVisibility::derive(pool, root_index, &shadowed, &problems)?;
    for problem in &problems {
        let (problem_dependencies, problem_requires_residual_probe) = project_unresolved_problem(
            pool,
            package_index,
            root,
            architecture,
            &problem.rules,
            &visibility,
            ProjectionScope {
                root_index,
                context: ProjectionContext::Strict,
            },
        )?;
        dependencies.extend(problem_dependencies);
        requires_residual_probe |= problem_requires_residual_probe;
    }
    if requires_residual_probe {
        match pool.solve_without_strict_repo_priority(root_index)? {
            SolvResolution::Resolved(packages) => {
                if !packages.contains(&root_index) {
                    return Err(Error::ConflictError(format!(
                        "libsolv residual probe for '{}' omitted its exact root",
                        root.name
                    )));
                }
            }
            SolvResolution::Unresolved(residual_problems) => {
                let visibility = StrictVisibility::derive(
                    pool,
                    root_index,
                    &shadowed,
                    problems.iter().chain(&residual_problems),
                )?;
                let mut final_dependencies = BTreeSet::new();
                for problem in &problems {
                    let (strict_dependencies, _) = project_unresolved_problem(
                        pool,
                        package_index,
                        root,
                        architecture,
                        &problem.rules,
                        &visibility,
                        ProjectionScope {
                            root_index,
                            context: ProjectionContext::Strict,
                        },
                    )?;
                    final_dependencies.extend(strict_dependencies);
                }
                for problem in &residual_problems {
                    let (residual_dependencies, nested_probe) = project_unresolved_problem(
                        pool,
                        package_index,
                        root,
                        architecture,
                        &problem.rules,
                        &visibility,
                        ProjectionScope {
                            root_index,
                            context: ProjectionContext::ResidualOfStrictProbe,
                        },
                    )?;
                    if nested_probe {
                        return Err(Error::InternalError(format!(
                            "libsolv residual probe for '{}' retained strict-priority authority",
                            root.name
                        )));
                    }
                    final_dependencies.extend(residual_dependencies);
                }
                dependencies = final_dependencies;
            }
        }
    }
    if dependencies.is_empty() {
        return Err(Error::ConflictError(format!(
            "libsolv reported exact root '{}' unresolved without a typed missing requirement",
            root.name
        )));
    }
    Ok(NativeResolutionOutcomeV1::Unresolved {
        dependencies: dependencies.into_iter().collect(),
    })
}

/// Requiring packages that Conary's candidate resolver can also reach under
/// the strict solve's repository-priority authority.
struct StrictVisibility {
    /// The exact root plus every provider reachable from it through hard
    /// required edges that strict repository priority does not shadow.
    reachable: BTreeSet<usize>,
    /// Packages owning a missing-dependency or required edge of their own.
    edge_owners: BTreeSet<usize>,
}

impl StrictVisibility {
    /// `shadowed` comes from the strict solve's own priority rules because a
    /// problem explanation may cite a shadowed provider's other defects
    /// without citing the strict-priority rule itself.
    fn derive<'a>(
        pool: &SolvPool,
        root_index: usize,
        shadowed: &BTreeSet<usize>,
        problems: impl IntoIterator<Item = &'a SolvProblem>,
    ) -> Result<Self> {
        let mut edge_owners = BTreeSet::new();
        let mut required: BTreeMap<usize, Vec<i32>> = BTreeMap::new();
        for rule in problems.into_iter().flat_map(|problem| &problem.rules) {
            let Some(from_index) = rule.from_index else {
                continue;
            };
            match rule.rule_type {
                SOLVER_RULE_PKG_NOTHING_PROVIDES_DEP => {
                    edge_owners.insert(from_index);
                }
                SOLVER_RULE_PKG_REQUIRES if rule.dependency != 0 => {
                    edge_owners.insert(from_index);
                    required
                        .entry(from_index)
                        .or_default()
                        .push(rule.dependency);
                }
                _ => {}
            }
        }
        let mut reachable = BTreeSet::from([root_index]);
        let mut frontier = vec![root_index];
        while let Some(index) = frontier.pop() {
            for dependency in required.get(&index).into_iter().flatten() {
                for provider in pool.providers(*dependency)? {
                    if !shadowed.contains(&provider) && reachable.insert(provider) {
                        frontier.push(provider);
                    }
                }
            }
        }
        Ok(Self {
            reachable,
            edge_owners,
        })
    }

    fn requiring_package_is_visible(&self, requiring_index: Option<usize>) -> bool {
        requiring_index.is_some_and(|index| self.reachable.contains(&index))
    }

    /// A required edge is terminal for Conary only when none of its visible
    /// providers explains the failure with an edge of its own; otherwise the
    /// deeper edge is the one Conary reports.
    fn required_edge_is_terminal(&self, pool: &SolvPool, dependency: i32) -> Result<bool> {
        Ok(!pool.providers(dependency)?.iter().any(|provider| {
            self.reachable.contains(provider) && self.edge_owners.contains(provider)
        }))
    }
}

#[derive(Clone, Copy)]
enum ProjectionContext {
    /// Problem returned by the original strict-priority solve. Provider-policy
    /// rules need authority from a strict-priority rule in this same problem.
    Strict,
    /// Problem returned by the diagnostic probe that one strict-priority plus
    /// provider-policy problem explicitly triggered.
    ResidualOfStrictProbe,
}

#[derive(Clone, Copy)]
struct ProjectionScope {
    root_index: usize,
    context: ProjectionContext,
}

fn project_unresolved_problem(
    pool: &SolvPool,
    package_index: &PackageResolutionIndex,
    root: &crate::repository::catalog::parity::NativeParityPackageV1,
    architecture: &str,
    rules: &[SolvProblemRule],
    visibility: &StrictVisibility,
    scope: ProjectionScope,
) -> Result<(BTreeSet<NativeUnresolvedDependencyV1>, bool)> {
    let mut dependencies = BTreeSet::new();
    let has_required_edge = rules
        .iter()
        .any(|rule| rule.rule_type == SOLVER_RULE_PKG_REQUIRES);
    let has_strict_repo_priority = rules
        .iter()
        .any(|rule| rule.rule_type == SOLVER_RULE_STRICT_REPO_PRIORITY);
    let has_provider_policy_rule = rules.iter().any(|rule| {
        rule.rule_type == SOLVER_RULE_PKG_CONFLICTS || rule.rule_type == SOLVER_RULE_INFARCH
    });
    let tolerates_provider_policy_rules = has_required_edge
        && (has_strict_repo_priority
            || matches!(scope.context, ProjectionContext::ResidualOfStrictProbe));
    for rule in rules {
        match rule.rule_type {
            SOLVER_RULE_PKG_NOTHING_PROVIDES_DEP => {
                let dependency = project_unresolved_dependency(
                    pool,
                    package_index,
                    root,
                    rule.from_index,
                    rule.dependency,
                    "missing-dependency",
                )?;
                if visibility.requiring_package_is_visible(rule.from_index) {
                    dependencies.insert(dependency);
                }
            }
            SOLVER_RULE_PKG_REQUIRES => {
                let dependency = project_unresolved_dependency(
                    pool,
                    package_index,
                    root,
                    rule.from_index,
                    rule.dependency,
                    "uninstallable requirement",
                )?;
                if visibility.requiring_package_is_visible(rule.from_index)
                    && visibility.required_edge_is_terminal(pool, rule.dependency)?
                {
                    dependencies.insert(dependency);
                }
            }
            SOLVER_RULE_JOB => {}
            SOLVER_RULE_PKG_CONFLICTS
                if tolerates_provider_policy_rules
                    && rule.from_index != Some(scope.root_index)
                    && rule.to_index != Some(scope.root_index) =>
            {
                validate_required_provider_conflict_rule(pool, package_index, root, rule)?;
            }
            SOLVER_RULE_STRICT_REPO_PRIORITY => {
                let excluded_index = rule.from_index.ok_or_else(|| {
                    Error::ConflictError(format!(
                        "libsolv strict-priority rule for '{}' has no excluded package",
                        root.name
                    ))
                })?;
                if rule.to_index.is_some() || rule.dependency != 0 {
                    return Err(Error::ConflictError(format!(
                        "libsolv strict-priority rule for '{}' has unexpected target or dependency",
                        root.name
                    )));
                }
                package_index.package_key(excluded_index)?;
            }
            SOLVER_RULE_PKG_NOT_INSTALLABLE => {
                return Err(Error::ConfigError(format!(
                    "libsolv found exact root '{}' not installable under target architecture '{}'",
                    root.name, architecture
                )));
            }
            SOLVER_RULE_INFARCH
                if tolerates_provider_policy_rules
                    && rule.from_index != Some(scope.root_index)
                    && rule.to_index != Some(scope.root_index) =>
            {
                validate_required_provider_inferior_arch_rule(pool, package_index, root, rule)?;
            }
            SOLVER_RULE_JOB_UNSUPPORTED | SOLVER_RULE_INFARCH => {
                return Err(Error::ConfigError(format!(
                    "libsolv rejected exact root '{}' for target architecture '{}' with rule {:#x} (from={:?}, to={:?}, dependency={})",
                    root.name,
                    architecture,
                    rule.rule_type,
                    rule.from_index,
                    rule.to_index,
                    rule.dependency
                )));
            }
            rule_type => {
                return Err(Error::ConflictError(format!(
                    "libsolv found unexpected problem rule {rule_type:#x} while resolving exact root '{}' (from={:?}, to={:?}, dependency={})",
                    root.name, rule.from_index, rule.to_index, rule.dependency
                )));
            }
        }
    }
    Ok((
        dependencies,
        has_strict_repo_priority && has_provider_policy_rule,
    ))
}

fn validate_required_provider_inferior_arch_rule(
    pool: &SolvPool,
    package_index: &PackageResolutionIndex,
    root: &crate::repository::catalog::parity::NativeParityPackageV1,
    rule: &SolvProblemRule,
) -> Result<()> {
    let inferior_index = rule.from_index.ok_or_else(|| {
        Error::ConflictError(format!(
            "libsolv required-provider inferior-architecture rule for '{}' has no package",
            root.name
        ))
    })?;
    if rule.to_index.is_some() || rule.dependency == 0 {
        return Err(Error::ConflictError(format!(
            "libsolv required-provider inferior-architecture rule for '{}' has unexpected target or package-name dependency",
            root.name
        )));
    }
    package_index.package_key(inferior_index)?;
    let package_name = pool.package(inferior_index)?.name()?;
    if pool.dependency(rule.dependency)?.atom()? != package_name {
        return Err(Error::ConflictError(format!(
            "libsolv required-provider inferior-architecture rule for '{}' names a different package",
            root.name
        )));
    }
    Ok(())
}

fn validate_required_provider_conflict_rule(
    pool: &SolvPool,
    package_index: &PackageResolutionIndex,
    root: &crate::repository::catalog::parity::NativeParityPackageV1,
    rule: &SolvProblemRule,
) -> Result<()> {
    let conflicting_index = rule.from_index.ok_or_else(|| {
        Error::ConflictError(format!(
            "libsolv required-provider package-conflict rule for '{}' has no conflicting package",
            root.name
        ))
    })?;
    let provided_index = rule.to_index.ok_or_else(|| {
        Error::ConflictError(format!(
            "libsolv required-provider package-conflict rule for '{}' has no provider package",
            root.name
        ))
    })?;
    if rule.dependency == 0 {
        return Err(Error::ConflictError(format!(
            "libsolv required-provider package-conflict rule for '{}' has no conflicting dependency",
            root.name
        )));
    }
    package_index.package_key(conflicting_index)?;
    package_index.package_key(provided_index)?;
    pool.dependency(rule.dependency)?.text()?;
    Ok(())
}

fn project_unresolved_dependency(
    pool: &SolvPool,
    package_index: &PackageResolutionIndex,
    root: &crate::repository::catalog::parity::NativeParityPackageV1,
    requiring_index: Option<usize>,
    dependency: i32,
    rule_description: &str,
) -> Result<NativeUnresolvedDependencyV1> {
    let requiring_index = requiring_index.ok_or_else(|| {
        Error::ConflictError(format!(
            "libsolv {rule_description} rule for '{}' has no requiring package",
            root.name
        ))
    })?;
    if dependency == 0 {
        return Err(Error::ConflictError(format!(
            "libsolv {rule_description} rule for '{}' has no dependency ID",
            root.name
        )));
    }
    let kind = match pool.required_kind(requiring_index, dependency)? {
        RequiredKind::Depends => RepositoryRequirementKind::Depends,
        RequiredKind::PreDepends => RepositoryRequirementKind::PreDepends,
    };
    let group = project_requirement(pool.dependency(dependency)?, kind)?;
    Ok(NativeUnresolvedDependencyV1 {
        requiring_package_key_sha256: package_index.package_key(requiring_index)?,
        requirement_group_sha256: native_requirement_group_sha256(&group)?,
    })
}
