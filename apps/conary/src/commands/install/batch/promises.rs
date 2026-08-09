// apps/conary/src/commands/install/batch/promises.rs

//! Promised-path witness authority for one batch transaction.
//!
//! A promised path (RPM `%ghost`) is a path a package owns without shipping
//! content for it. `rpm-spec.5` describes it as a file "not to be included in
//! the package", used "when the attributes of the file are important while the
//! contents is not (e.g. a log file)" -- ownership if present, never a claim
//! that the package creates it. Many promises (`/etc/fstab`, log files,
//! generated certificate links) are legitimately absent, so verifying every
//! declared promise would assert something the source never said.
//!
//! What the transaction may assert is its own reasoning. Dependency resolution
//! admits a promise as a provider, so an edge can be certified against a path
//! no payload record creates; this module decides which requirements actually
//! leaned on that admission and proves them against the assembled root.
//!
//! The decision is per requirement, never per promise. Two packages promising
//! the same path, or one requirement naming two promised paths, would alibi
//! each other under a promise-at-a-time counterfactual: dropping either leaves
//! the other standing, so neither looks load-bearing and an unusable dependency
//! commits. So planning strips *every* promise the requirement could lean on at
//! once to decide reliance, and the post-condition re-evaluates the requirement
//! with promises admitted only for paths that exist. That is the property worth
//! proving -- the requirement holds against the disk -- and it neither misses a
//! shared path nor fails a requirement whose alternative promise did
//! materialize.

use super::*;
use conary_core::db::models::ProvideEntry;
use conary_core::repository::dependency_model::{
    CapabilityProvenance, ProvidedCapability, RepositoryRequirementGroup, RepositoryRequirementKind,
};
use conary_core::repository::versioning::VersionScheme;
use conary_core::resolver::identity::PackageIdentity;
use std::collections::BTreeSet;
use std::path::Path;

/// Which side of the transaction promised the path.
///
/// The distinction is not cosmetic: it decides the operator's remedy. A batch
/// member that never materialized its promise fails the incoming package, while
/// an already installed holder whose path went missing has to be repaired or
/// reinstalled -- the incoming package is correct in that case. A path both
/// sides promise names both remedies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PromiseHolder {
    /// A package this transaction installs.
    BatchMember,
    /// A package that was already installed before this transaction.
    Installed,
}

/// One package that promised a path, and how it came to be installed.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PromiseHolderIdentity {
    holder: PromiseHolder,
    name: String,
    version: String,
}

impl PromiseHolderIdentity {
    fn unmaterialized(&self, path: &str) -> String {
        match self.holder {
            PromiseHolder::BatchMember => format!(
                "package {}-{} did not materialize promised path {} during its native lifecycle",
                self.name, self.version, path
            ),
            PromiseHolder::Installed => format!(
                "installed package {}-{} no longer holds promised path {}; repair or reinstall {} \
                 to restore the path",
                self.name, self.version, path, self.name
            ),
        }
    }
}

/// A promised path a requirement could lean on, with every package promising it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PromisedPath {
    path: String,
    holders: Vec<PromiseHolderIdentity>,
}

/// One requirement that cannot be satisfied without at least one promise.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PromiseObligation {
    dependent: String,
    requirement: RepositoryRequirementGroup,
    version_scheme: VersionScheme,
    depending_architecture: String,
    paths: Vec<PromisedPath>,
}

/// Everything the transaction must prove about promised paths, plus the exact
/// identities its reasoning was carried out against.
///
/// The universe is carried rather than recomputed so the post-condition asks
/// the resolver the same question the planner asked, against the same facts.
#[derive(Debug, Default)]
pub(super) struct PromiseWitnessPlan {
    native_architecture: String,
    universe: Vec<PackageIdentity>,
    obligations: Vec<PromiseObligation>,
}

impl PromiseWitnessPlan {
    pub(super) fn empty() -> Self {
        Self::default()
    }
}

fn dependency_requirements(
    package: &PreparedPackage,
) -> impl Iterator<Item = &RepositoryRequirementGroup> {
    package.requirements.iter().filter(|requirement| {
        matches!(
            requirement.kind,
            RepositoryRequirementKind::Depends | RepositoryRequirementKind::PreDepends
        )
    })
}

fn requirement_text(requirement: &RepositoryRequirementGroup) -> &str {
    requirement
        .native_text
        .as_deref()
        .unwrap_or("<typed expression>")
}

/// Every capability name this batch names in a `Depends`/`PreDepends` atom.
///
/// This bounds which promises are even considered. It is exact data off the
/// batch's own requirement expressions and decides nothing on its own: naming a
/// path is not relying on it, which is what [`plan_promise_witnesses`] settles.
pub(super) fn batch_requirement_atom_names(packages: &[PreparedPackage]) -> BTreeSet<String> {
    let mut atoms = BTreeSet::new();
    for package in packages {
        for requirement in dependency_requirements(package) {
            for clause in requirement.expression.atoms() {
                atoms.insert(clause.name.clone());
            }
        }
    }
    atoms
}

/// Whether any package -- incoming or installed -- promises one of `atoms`.
///
/// A batch of two or more packages already loads the installed identity
/// universe for ordering, so this exists for the single-package batch, which
/// otherwise has no reason to load it at all. The incoming side is answered in
/// memory; each installed probe is an indexed point lookup on
/// `provides.capability` and the first hit ends the scan.
pub(super) fn batch_requirements_can_lean_on_a_promise(
    conn: &Connection,
    packages: &[PreparedPackage],
    atoms: &BTreeSet<String>,
) -> Result<bool> {
    let incoming = packages.iter().any(|package| {
        package
            .provides
            .iter()
            .any(|capability| capability.is_promised_path() && atoms.contains(&capability.name))
    });
    if incoming {
        return Ok(true);
    }
    for atom in atoms {
        let promised = ProvideEntry::find_all_by_capability(conn, atom)?
            .iter()
            .any(|entry| {
                matches!(
                    entry.provenance,
                    CapabilityProvenance::SourcePromisedPath { .. }
                )
            });
        if promised {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Decide which requirements this transaction cannot satisfy without a promise.
///
/// `installed` is consumed: the planner keeps the identity universe alive for
/// the post-condition instead of rebuilding or re-cloning it.
///
/// Reliance is decided by stripping every candidate promise for a requirement
/// at once. Removing them one at a time is what lets two holders of one path --
/// or two promised alternatives of one expression -- excuse each other into
/// invisibility.
pub(super) fn plan_promise_witnesses(
    packages: &[PreparedPackage],
    installed: Vec<PackageIdentity>,
    selected: &[PackageIdentity],
    native_architecture: &str,
) -> Result<PromiseWitnessPlan> {
    let installed_count = installed.len();
    let mut universe = installed;
    universe.extend(selected.iter().cloned());

    let mut obligations = Vec::new();
    for package in packages {
        let depending_architecture = package
            .architecture
            .as_deref()
            .filter(|architecture| !architecture.is_empty())
            .unwrap_or(native_architecture)
            .to_string();
        for requirement in dependency_requirements(package) {
            let paths = promised_paths_named_by(requirement, &universe, installed_count);
            if paths.is_empty() {
                continue;
            }
            if !expression_satisfied(
                requirement,
                package.semantics.version_scheme,
                &depending_architecture,
                native_architecture,
                &universe,
            )? {
                // The ordering validator owns an unsatisfiable edge. Inventing a
                // promise failure here would replace its diagnostic with a worse
                // one.
                continue;
            }

            let stripped = strip_promised_paths(&mut universe, &paths);
            let survived = expression_satisfied(
                requirement,
                package.semantics.version_scheme,
                &depending_architecture,
                native_architecture,
                &universe,
            );
            restore_promised_paths(&mut universe, stripped);
            if survived? {
                continue;
            }
            obligations.push(PromiseObligation {
                dependent: package.name.clone(),
                requirement: requirement.clone(),
                version_scheme: package.semantics.version_scheme,
                depending_architecture: depending_architecture.clone(),
                paths,
            });
        }
    }

    Ok(PromiseWitnessPlan {
        native_architecture: native_architecture.to_string(),
        universe,
        obligations,
    })
}

/// The promised paths this requirement names, each with every package that
/// promises it.
///
/// Holder side is decided structurally by position in the universe, not by
/// inspecting identity fields: everything below `installed_count` was installed
/// before this transaction, everything above it is incoming.
fn promised_paths_named_by(
    requirement: &RepositoryRequirementGroup,
    universe: &[PackageIdentity],
    installed_count: usize,
) -> Vec<PromisedPath> {
    let atoms = requirement
        .expression
        .atoms()
        .into_iter()
        .map(|clause| clause.name.clone())
        .collect::<BTreeSet<_>>();
    let mut paths = Vec::new();
    for atom in atoms {
        let mut holders = Vec::new();
        for (index, identity) in universe.iter().enumerate() {
            if !identity
                .provided_capabilities
                .iter()
                .any(|capability| capability.is_promised_path() && capability.name == atom)
            {
                continue;
            }
            holders.push(PromiseHolderIdentity {
                holder: if index < installed_count {
                    PromiseHolder::Installed
                } else {
                    PromiseHolder::BatchMember
                },
                name: identity.name.clone(),
                version: identity.version.clone(),
            });
        }
        if !holders.is_empty() {
            paths.push(PromisedPath {
                path: atom,
                holders,
            });
        }
    }
    paths
}

/// Remove every promise for every named path from the whole universe, returning
/// them so the caller can restore the universe afterwards.
///
/// Only promises are removed: a package that both promises and ships one path
/// is rejected by signed-authority validation, but an unrelated capability
/// sharing the name must keep witnessing the requirement.
fn strip_promised_paths(
    universe: &mut [PackageIdentity],
    paths: &[PromisedPath],
) -> Vec<(usize, ProvidedCapability)> {
    let names = paths
        .iter()
        .map(|promised| promised.path.as_str())
        .collect::<BTreeSet<_>>();
    let mut stripped = Vec::new();
    for (index, identity) in universe.iter_mut().enumerate() {
        identity.provided_capabilities.retain(|capability| {
            if capability.is_promised_path() && names.contains(capability.name.as_str()) {
                stripped.push((index, capability.clone()));
                false
            } else {
                true
            }
        });
    }
    stripped
}

fn restore_promised_paths(
    universe: &mut [PackageIdentity],
    stripped: Vec<(usize, ProvidedCapability)>,
) {
    for (index, capability) in stripped {
        universe[index].provided_capabilities.push(capability);
    }
}

fn expression_satisfied(
    requirement: &RepositoryRequirementGroup,
    version_scheme: VersionScheme,
    depending_architecture: &str,
    native_architecture: &str,
    packages: &[PackageIdentity],
) -> Result<bool> {
    conary_core::resolver::requirement_expression_satisfied(
        &requirement.expression,
        version_scheme,
        depending_architecture,
        native_architecture,
        packages,
    )
    .map_err(anyhow::Error::from)
}

/// Prove every requirement that leaned on a promise still holds against the
/// assembled root.
///
/// A promised path is admitted only if it exists, symlinks included and content
/// unread: the promise is that the path exists, never what it resolves to. The
/// requirement is then re-evaluated. That is deliberately weaker than requiring
/// every promised path to exist -- an expression offering two promised
/// alternatives is satisfied by either -- and deliberately stronger than
/// checking promises one at a time, which two holders of one path would pass by
/// pointing at each other.
///
/// This runs regardless of the declaring lifecycle entry's criticality, because
/// RPM's warning-only slots let a scriptlet fail, or silently do nothing, while
/// reporting success -- and it runs after the whole native graph, including
/// post-transaction slots.
pub(super) fn verify_promised_paths_materialized(
    plan: &mut PromiseWitnessPlan,
    selected_root: &Path,
) -> Result<()> {
    let PromiseWitnessPlan {
        native_architecture,
        universe,
        obligations,
    } = plan;
    for obligation in obligations.iter() {
        let mut absent = Vec::new();
        for promised in &obligation.paths {
            let target = crate::commands::live_root::target_path(selected_root, &promised.path)
                .with_context(|| {
                    format!(
                        "promised path {} is not addressable in the target root",
                        promised.path
                    )
                })?;
            if std::fs::symlink_metadata(&target).is_err() {
                absent.push(promised.clone());
            }
        }
        if absent.is_empty() {
            continue;
        }

        let stripped = strip_promised_paths(universe, &absent);
        let satisfied = expression_satisfied(
            &obligation.requirement,
            obligation.version_scheme,
            &obligation.depending_architecture,
            native_architecture,
            universe,
        );
        restore_promised_paths(universe, stripped);
        if satisfied? {
            continue;
        }

        let clauses = absent
            .iter()
            .flat_map(|promised| {
                promised
                    .holders
                    .iter()
                    .map(|holder| holder.unmaterialized(&promised.path))
            })
            .collect::<Vec<_>>();
        anyhow::bail!(
            "{} (checked after the post-transaction stage); {} depends on it through {}",
            clauses.join("; "),
            obligation.dependent,
            requirement_text(&obligation.requirement)
        );
    }
    Ok(())
}
