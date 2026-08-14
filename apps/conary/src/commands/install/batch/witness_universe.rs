// apps/conary/src/commands/install/batch/witness_universe.rs

//! Which installed identities may witness this transaction's own reasoning.
//!
//! Ordering and promise planning certify dependency edges against a set of
//! package identities. That set is the transaction's *end state*, not the
//! database as it stands when planning runs: an identity this transaction
//! itself deletes cannot carry an edge the transaction is about to rely on.
//!
//! Two installed identities leave during a batch, and both used to remain
//! admissible witnesses:
//!
//! * the pre-upgrade identity of a package the batch upgrades, whose
//!   capabilities -- ordinary provides and promised paths alike -- the incoming
//!   version is free to drop; and
//! * an installed package an incoming negative relation removes, which is
//!   selected by the same relation authority that plans the removal.
//!
//! Both are named by exact trove identity, never by package name: a name can be
//! shared by parallel-installed identities, and only the trove this transaction
//! deletes is leaving. The surviving set plus the incoming identities is what
//! the transaction actually provides when it commits.

use super::*;
use conary_core::resolver::identity::PackageIdentity;
use std::collections::BTreeSet;

/// The installed identities that survive this transaction, ready to witness.
///
/// Architecture is defaulted to the native one where an installed row left it
/// empty, because requirement evaluation compares architecture qualifiers and
/// an installed identity with no architecture is a native-architecture one.
pub(super) fn surviving_installed_identities(
    conn: &Connection,
    packages: &[PreparedPackage],
    native_architecture: &str,
) -> Result<Vec<PackageIdentity>> {
    let outgoing = outgoing_installed_troves(conn, packages)?;
    Ok(
        conary_core::resolver::load_installed_package_identities(conn)?
            .into_iter()
            .filter(|identity| {
                identity
                    .installed_trove_id
                    .is_none_or(|trove_id| !outgoing.contains(&trove_id))
            })
            .map(|mut identity| {
                if identity.architecture.as_deref().is_none_or(str::is_empty) {
                    identity.architecture = Some(native_architecture.to_string());
                }
                identity
            })
            .collect(),
    )
}

/// Every installed trove this transaction deletes.
///
/// Superseded identities come from the batch itself: preparation already bound
/// each incoming package to the exact installed trove it replaces.
///
/// Relation removals come from the same planner that later attaches them to the
/// incoming packages, asked here for a different question. *Which* installed
/// packages leave is a property of the batch's composition and installed state,
/// so it is answerable before the batch is ordered. *When* each removal runs is
/// not: a removal is sequenced before the earliest incoming element that
/// triggers it, and "earliest" only exists once the transaction has an
/// execution order. [`super::relations`] answers that second question after
/// ordering, against the ordered batch.
fn outgoing_installed_troves(
    conn: &Connection,
    packages: &[PreparedPackage],
) -> Result<BTreeSet<i64>> {
    let mut outgoing = packages
        .iter()
        .map(PreparedPackage::old_trove_id)
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .flatten()
        .collect::<BTreeSet<_>>();
    let incoming = super::relations::incoming_relation_facts(packages);
    let plan = conary_core::transaction::plan_package_relation_batch_facts(conn, &incoming)?;
    outgoing.extend(plan.removals.iter().map(|removal| removal.trove_id));
    Ok(outgoing)
}
