// src/commands/install/batch/execution.rs

//! Graph-driven batch payload execution.

use super::super::native_events::PreparedNativeTransaction;
use super::{BatchDbRows, BatchInstaller, PreparedPackage, inner};
use anyhow::{Context, Result};
use conary_core::ccs::native_lifecycle::SourceFormat;
use conary_core::ccs::native_transaction::NativePackageIdentity;
use conary_core::config_transaction::GenerationConfigTransaction;
use conary_core::db::models::{
    ActivationRequest, Changeset, ChangesetStatus, InstalledNativeLifecycleBundle, Trove,
};
use conary_core::filesystem::CasStore;
use conary_core::scriptlet::ExecutionMode;
use std::collections::BTreeSet;
use std::path::Path;

/// Prove every promised path the transaction *relied on* was actually created.
///
/// RPM's `%ghost` marks a path the package owns if present -- `rpm-spec.5`
/// describes it as a file "not to be included in the package", used "when the
/// attributes of the file are important while the contents is not (e.g. a log
/// file)". It is not a claim that the package creates the path, and many
/// promises (`/etc/fstab`, log files, generated certificate links) are
/// legitimately absent after a fresh transaction. Verifying every declared
/// promise therefore asserts something the source never said.
///
/// So this verifies exactly the promises a dependency edge was certified
/// against, as computed by the ordering validator: the transaction proves its
/// own reasoning and nothing beyond it. It still runs regardless of the
/// declaring lifecycle entry's criticality, because RPM's warning-only slots
/// let a scriptlet fail, or silently do nothing, while reporting success -- and
/// it runs after the whole native graph, including post-transaction slots.
///
/// A path is materialized when it exists, symlinks included: the promise is
/// that the path exists, never what its content resolves to.
pub(super) fn verify_promised_paths_materialized(
    witnessed: &[super::ordering::WitnessedPromise],
    selected_root: &Path,
) -> Result<()> {
    for promise in witnessed {
        let target = crate::commands::live_root::target_path(selected_root, &promise.path)
            .with_context(|| {
                format!(
                    "promised path {} declared by {} is not addressable in the target root",
                    promise.path, promise.provider
                )
            })?;
        if std::fs::symlink_metadata(&target).is_err() {
            anyhow::bail!(
                "package {}-{} did not materialize promised path {} during its native lifecycle \
                 (checked after the post-transaction stage); {} depends on it through {}",
                promise.provider,
                promise.provider_version,
                promise.path,
                promise.dependent,
                promise.requirement
            );
        }
    }
    Ok(())
}

struct GraphExecutionInputs {
    final_incoming_paths: BTreeSet<String>,
    finalization_troves: Vec<Option<i64>>,
    installing_deb_identities: Vec<Option<NativePackageIdentity>>,
    removing_deb_identities: Vec<Option<NativePackageIdentity>>,
}

impl BatchInstaller<'_> {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn execute_selected_root_native_graph(
        &self,
        conn: &mut rusqlite::Connection,
        cas: &CasStore,
        packages: &[PreparedPackage],
        stored_files_by_pkg: &[Vec<inner::StoredInstallFile>],
        tx_description: &str,
        summary: &str,
        native_transaction: &PreparedNativeTransaction,
        native_execution_mode: &ExecutionMode,
        selected: &mut crate::commands::generation::selected_root::SelectedRootSession,
        rollback_root: conary_core::generation::root_manifest::CapturedSelectedRoot,
        ccs_hook_executors: &mut [Option<conary_core::ccs::HookExecutor>],
        witnessed_promises: &[super::ordering::WitnessedPromise],
    ) -> Result<(i64, Vec<i64>, Vec<i64>)> {
        let graph_inputs = self.prepare_graph_execution(conn, packages)?;
        let runtime_root = conary_core::runtime_root::ConaryRuntimeRoot::from_db_path(
            std::path::PathBuf::from(self.db_path),
        );
        let selected_root = selected.selected_root().to_path_buf();
        let tx = conn.unchecked_transaction()?;
        let execution = (|| -> Result<_> {
            let BatchDbRows {
                changeset_id,
                trove_ids,
                retained_upgrade_trove_ids,
                retain_for_lifecycle_by_pkg,
            } = Self::insert_batch_db_rows(
                &tx,
                &selected_root,
                packages,
                tx_description,
                None,
                rollback_root,
            )?;
            let mut config_transaction = GenerationConfigTransaction::default();
            self.drive_graph(
                &tx,
                selected,
                &selected_root,
                cas,
                packages,
                stored_files_by_pkg,
                changeset_id,
                &trove_ids,
                &retain_for_lifecycle_by_pkg,
                &mut config_transaction,
                native_transaction,
                native_execution_mode,
                &graph_inputs,
                &retained_upgrade_trove_ids,
            )?;
            verify_promised_paths_materialized(witnessed_promises, &selected_root)?;
            let mut activation_requests = native_transaction.take_activation_requests();
            for (package, executor) in packages.iter().zip(ccs_hook_executors.iter()) {
                let (Some(ccs), Some(executor)) = (package.ccs.as_ref(), executor.as_ref()) else {
                    continue;
                };
                if super::super::ccs_transaction::ccs_has_post_hooks(&ccs.hooks) {
                    super::super::ccs_transaction::execute_ccs_post_hooks(
                        &package.name,
                        &package.version,
                        &ccs.hooks,
                        executor,
                    )?;
                    activation_requests.extend(
                        super::super::ccs_transaction::ccs_activation_requests(
                            &package.name,
                            &package.version,
                            executor,
                        ),
                    );
                }
            }
            let all_file_paths = packages
                .iter()
                .flat_map(|pkg| pkg.extracted_files.iter().map(|file| file.path.clone()))
                .collect::<Vec<_>>();
            super::super::run_triggers(&tx, &selected_root, changeset_id, &all_file_paths)?;
            ActivationRequest::append_batch(&tx, changeset_id, &activation_requests)
                .context("failed to persist exact batch generation activation requests")?;
            config_transaction.validate()?;
            Changeset::find_by_id(&tx, changeset_id)?
                .context("batch changeset disappeared before commit")?
                .update_status(&tx, ChangesetStatus::Applied)?;
            let publication_debt =
                crate::commands::generation::publication::record_selected_root_state(
                    &tx,
                    &crate::commands::generation::publication::PublicationRequest {
                        db_path: self.db_path,
                        summary,
                        trigger_changeset_id: Some(changeset_id),
                        tx_uuid: None,
                        config_transaction,
                    },
                )?;
            Ok((changeset_id, trove_ids, publication_debt))
        })();
        let (changeset_id, trove_ids, publication_debt) = match execution {
            Ok(result) => result,
            Err(error) => {
                drop(tx);
                selected
                    .rollback()
                    .context("failed to discard selected generation root")?;
                return Err(error);
            }
        };
        if let Err(error) = selected.persist_for_publication(&runtime_root, &publication_debt) {
            drop(tx);
            let _ = crate::commands::generation::selected_root::remove_publication_candidate(
                &runtime_root,
                &publication_debt,
            );
            return Err(error.context("failed to persist exact selected-root publication input"));
        }
        if let Err(error) = tx.commit() {
            crate::commands::generation::selected_root::remove_publication_candidate(
                &runtime_root,
                &publication_debt,
            )
            .context("failed to discard selected-root candidate after database commit error")?;
            return Err(error.into());
        }

        let outcome = crate::commands::generation::publication::publish_recorded_selected_root(
            conn,
            self.db_path,
            summary,
            publication_debt,
        )?;
        if outcome.needs_publication {
            crate::commands::append_deferred_follow_up_metadata(
                conn,
                changeset_id,
                crate::commands::publication_deferred_follow_up(
                    "generation publication is pending".to_string(),
                ),
            )?;
            crate::commands::generation::publication::warn_if_publication_pending(
                changeset_id,
                &outcome,
            );
        }
        Ok((changeset_id, trove_ids, Vec::new()))
    }

    fn prepare_graph_execution(
        &self,
        conn: &rusqlite::Connection,
        packages: &[PreparedPackage],
    ) -> Result<GraphExecutionInputs> {
        let final_incoming_paths = packages
            .iter()
            .flat_map(|package| package.extracted_files.iter())
            .map(|file| super::super::native_graph::normalize_archive_path(&file.path))
            .collect::<BTreeSet<_>>();
        let finalization_troves = packages
            .iter()
            .map(PreparedPackage::old_trove_id)
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .chain(
                packages
                    .iter()
                    .flat_map(|package| package.relation_removals.iter())
                    .map(|removal| Some(removal.trove_id)),
            )
            .collect::<Vec<_>>();
        let installing_deb_identities = packages
            .iter()
            .map(|package| {
                package
                    .native_lifecycle_state
                    .bundle_to_persist
                    .as_ref()
                    .filter(|bundle| bundle.source_format == SourceFormat::Deb)
                    .map(|bundle| {
                        NativePackageIdentity::new(
                            &package.name,
                            &package.version,
                            bundle.source_arch.as_deref(),
                        )
                    })
            })
            .collect::<Vec<_>>();
        let removing_deb_identities = finalization_troves
            .iter()
            .map(|trove_id| {
                trove_id
                    .map(|trove_id| deb_identity_for_trove(conn, trove_id))
                    .transpose()
                    .map(Option::flatten)
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(GraphExecutionInputs {
            final_incoming_paths,
            finalization_troves,
            installing_deb_identities,
            removing_deb_identities,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn drive_graph(
        &self,
        tx: &rusqlite::Transaction<'_>,
        root_mutation: &mut impl super::super::native_graph::TransactionRootMutation,
        selected_root: &Path,
        cas: &CasStore,
        packages: &[PreparedPackage],
        stored_files_by_pkg: &[Vec<inner::StoredInstallFile>],
        changeset_id: i64,
        trove_ids: &[i64],
        retain_for_lifecycle_by_pkg: &[bool],
        config_transaction: &mut GenerationConfigTransaction,
        native_transaction: &PreparedNativeTransaction,
        native_execution_mode: &ExecutionMode,
        inputs: &GraphExecutionInputs,
        retained_trove_ids: &[i64],
    ) -> Result<()> {
        let retained_trove_ids = retained_trove_ids.iter().copied().collect::<BTreeSet<_>>();
        let mut payload = BatchGraphPayload {
            tx,
            root_mutation,
            selected_root,
            cas,
            packages,
            stored_files_by_pkg,
            changeset_id,
            trove_ids,
            retain_for_lifecycle_by_pkg,
            config_transaction,
            inputs,
            retained_trove_ids: &retained_trove_ids,
            native_transaction,
        };
        super::super::native_graph::drive_native_graph(
            tx,
            native_transaction,
            selected_root,
            native_execution_mode,
            &mut payload,
        )
    }
}

struct BatchGraphPayload<'a, R> {
    tx: &'a rusqlite::Transaction<'a>,
    root_mutation: &'a mut R,
    selected_root: &'a Path,
    cas: &'a CasStore,
    packages: &'a [PreparedPackage],
    stored_files_by_pkg: &'a [Vec<inner::StoredInstallFile>],
    changeset_id: i64,
    trove_ids: &'a [i64],
    retain_for_lifecycle_by_pkg: &'a [bool],
    config_transaction: &'a mut GenerationConfigTransaction,
    inputs: &'a GraphExecutionInputs,
    retained_trove_ids: &'a BTreeSet<i64>,
    native_transaction: &'a PreparedNativeTransaction,
}

impl<R> super::super::native_graph::NativeGraphPayloadMutation for BatchGraphPayload<'_, R>
where
    R: super::super::native_graph::TransactionRootMutation,
{
    fn apply_payload(&mut self, _conn: &rusqlite::Connection, change_index: usize) -> Result<()> {
        if let Some(package) = self.packages.get(change_index) {
            let old_trove_id = package.old_trove_id()?;
            let stored_files = self
                .stored_files_by_pkg
                .get(change_index)
                .context("batch payload has no stored-file input")?;
            let semantics = package.semantics;
            let resolved_files =
                inner::resolve_stored_install_files(self.selected_root, stored_files, semantics)?;
            let directory_plan = inner::preflight_resolved_file_ownership(
                self.tx,
                self.selected_root,
                &resolved_files,
                &package.name,
                &package.relation_removals,
                semantics,
            )?;
            let all_package_files =
                super::super::live_root_files_from_stored_files(self.cas, &resolved_files)?;
            let package_files = all_package_files
                .iter()
                .filter(|file| !directory_plan.preserves_leaf(&file.path))
                .cloned()
                .collect::<Vec<_>>();
            let through_symlink_files = directory_plan.through_symlink_root_files(&resolved_files);
            let retain_for_lifecycle = self
                .retain_for_lifecycle_by_pkg
                .get(change_index)
                .copied()
                .context("batch payload has no lifecycle-retention authority")?;
            let immediately_replaced = if retain_for_lifecycle {
                Vec::new()
            } else {
                old_trove_id.into_iter().collect::<Vec<_>>()
            };
            let mut captured = crate::commands::generation::config_transaction::capture_install(
                self.tx,
                self.selected_root,
                self.cas,
                crate::commands::generation::config_transaction::ConfigInstallCapture {
                    source: super::super::config_files::source_for_semantics(package.semantics),
                    declared: &package.config_declarations,
                    incoming: &package_files,
                    replacing_trove_id: old_trove_id,
                    replaced_trove_ids: &immediately_replaced,
                },
            )?;
            self.config_transaction
                .entries
                .append(&mut captured.entries);
            let mut plan = super::super::config_files::prepare_config_install(
                self.tx,
                self.selected_root,
                super::super::config_files::source_for_semantics(package.semantics),
                &package.config_declarations,
                old_trove_id,
                package_files,
            )?;
            let hardlink_references = super::super::execute::prepare_preserved_hardlink_references(
                self.tx,
                &directory_plan,
                &all_package_files,
                &mut plan.files,
            )?;
            self.root_mutation
                .apply_install_files_with_references(&plan.files, &hardlink_references)?;
            self.root_mutation
                .apply_install_files(&through_symlink_files)?;
            self.root_mutation.apply_remove_paths(&plan.remove_paths)?;
            if let Some(ccs) = package.ccs.as_ref() {
                super::super::file_capabilities::apply_selected_file_capabilities(
                    self.selected_root,
                    &ccs.file_capabilities,
                    plan.files.iter(),
                )?;
            }
            let retain_for_lifecycle = self
                .retain_for_lifecycle_by_pkg
                .get(change_index)
                .copied()
                .context("batch payload has no retention decision")?;
            if !retain_for_lifecycle && let Some(old_id) = old_trove_id {
                Trove::delete(self.tx, old_id)?;
            }
            BatchInstaller::insert_batch_payload_rows(
                self.tx,
                self.changeset_id,
                package,
                *self
                    .trove_ids
                    .get(change_index)
                    .context("batch payload has no installed trove identity")?,
                &resolved_files,
                &directory_plan,
            )?;
            if let Some(Some(package)) = self.inputs.installing_deb_identities.get(change_index) {
                self.native_transaction
                    .mark_install_payload_applied_for(self.tx, package)?;
            }
        }
        Ok(())
    }

    fn finalize_old_payload(
        &mut self,
        _conn: &rusqlite::Connection,
        change_index: usize,
    ) -> Result<()> {
        let Some(Some(trove_id)) = self.inputs.finalization_troves.get(change_index) else {
            return Ok(());
        };
        if !self.retained_trove_ids.contains(trove_id) {
            return Ok(());
        }
        if let Some(Some(package)) = self.inputs.removing_deb_identities.get(change_index) {
            self.native_transaction
                .mark_remove_payload_started_for(self.tx, package)?;
        }
        super::super::native_graph::finalize_owned_trove(
            self.tx,
            self.root_mutation,
            self.selected_root,
            *trove_id,
            &self.inputs.final_incoming_paths,
        )?;
        if let Some(Some(package)) = self.inputs.removing_deb_identities.get(change_index) {
            self.native_transaction.mark_remove_payload_completed_for(
                self.tx,
                package,
                change_index,
            )?;
        }
        Ok(())
    }

    fn purge_config_files(
        &mut self,
        _conn: &rusqlite::Connection,
        change_index: usize,
    ) -> Result<()> {
        anyhow::bail!(
            "batch install transaction graph unexpectedly purges config for change {change_index}"
        )
    }
}

fn deb_identity_for_trove(
    conn: &rusqlite::Connection,
    trove_id: i64,
) -> Result<Option<NativePackageIdentity>> {
    let Some(installed) = InstalledNativeLifecycleBundle::find_by_trove(conn, trove_id)? else {
        return Ok(None);
    };
    let bundle = installed.bundle()?;
    Ok((bundle.source_format == SourceFormat::Deb).then(|| {
        NativePackageIdentity::new(
            &installed.source_package,
            &installed.source_version,
            installed.source_arch.as_deref(),
        )
    }))
}
