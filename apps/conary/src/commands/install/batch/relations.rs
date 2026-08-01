// apps/conary/src/commands/install/batch/relations.rs

//! Exact negative-relation planning for atomic package batches.

use super::*;
use std::collections::HashSet;

impl BatchInstaller<'_> {
    pub(super) fn plan_package_relations_for_batch(
        &self,
        conn: &Connection,
        packages: &mut [PreparedPackage],
    ) -> Result<()> {
        let selected_installed_ids = packages
            .iter()
            .map(PreparedPackage::old_trove_id)
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .flatten()
            .collect::<HashSet<_>>();
        let incoming = packages
            .iter()
            .map(
                |package| conary_core::transaction::IncomingPackageRelations {
                    name: &package.name,
                    version: &package.version,
                    architecture: package.architecture.as_deref(),
                    version_scheme: package.semantics.version_scheme,
                    provides: &package.provides,
                    relations: &package.relations,
                },
            )
            .collect::<Vec<_>>();
        let plan = conary_core::transaction::plan_package_relation_batch_facts(conn, &incoming)?;

        if let Some(removal) = plan
            .removals
            .iter()
            .find(|removal| selected_installed_ids.contains(&removal.trove_id))
        {
            anyhow::bail!(
                "batch co-selects installed package {} {} that incoming package relations remove",
                removal.package_name,
                removal.package_version
            );
        }
        if let Some(deconfiguration) = plan.deconfigurations.iter().find(|deconfiguration| {
            selected_installed_ids.contains(&deconfiguration.package.trove_id)
        }) {
            anyhow::bail!(
                "batch co-selects installed package {} {} that incoming package relations deconfigure",
                deconfiguration.package.package_name,
                deconfiguration.package.package_version
            );
        }
        conary_core::transaction::validate_package_relation_plan(conn, &plan)
            .context("atomic package relations cannot be applied")?;

        for package in packages.iter_mut() {
            package.relation_removals.clear();
            package.relation_deconfigurations.clear();
        }
        for removal in plan.removals {
            let index = removal.triggering_incoming.transaction_index;
            let package = packages.get_mut(index).with_context(|| {
                format!(
                    "relation removal for {} {} refers to missing selected incoming transaction element {index}",
                    removal.package_name, removal.package_version
                )
            })?;
            if package.name != removal.triggering_incoming.package_name
                || package.version != removal.triggering_incoming.package_version
                || package.architecture != removal.triggering_incoming.package_architecture
            {
                anyhow::bail!(
                    "relation removal for {} {} refers to a changed selected incoming identity",
                    removal.package_name,
                    removal.package_version
                );
            }
            package.relation_removals.push(removal);
        }
        for deconfiguration in plan.deconfigurations {
            let index = deconfiguration.triggering_incoming.transaction_index;
            let package = packages.get_mut(index).with_context(|| {
                format!(
                    "relation deconfiguration for {} {} refers to missing selected incoming transaction element {index}",
                    deconfiguration.package.package_name,
                    deconfiguration.package.package_version
                )
            })?;
            if package.name != deconfiguration.triggering_incoming.package_name
                || package.version != deconfiguration.triggering_incoming.package_version
                || package.architecture != deconfiguration.triggering_incoming.package_architecture
            {
                anyhow::bail!(
                    "relation deconfiguration for {} {} refers to a changed selected incoming identity",
                    deconfiguration.package.package_name,
                    deconfiguration.package.package_version
                );
            }
            package.relation_deconfigurations.push(deconfiguration);
        }
        Ok(())
    }
}
