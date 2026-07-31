// apps/conary/src/commands/install/package_set.rs

//! One atomic repository transaction for a declarative package request set.

use std::collections::BTreeMap;

use anyhow::{Context, Result};
use conary_core::db::models::InstallReason;
use conary_core::repository::resolution_policy::RequestScope;
use conary_core::scriptlet::SandboxMode;
use conary_core::version::VersionConstraint;

use super::InstallIntent;
use super::batch::BatchInstaller;
use super::dependencies::resolved_repository_deps_from_sat_result;
use super::repository_batch::{RepositoryBatchSelection, prepare_repository_batch};
use super::source_policy::resolve_canonical_name;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PackageSetRequest {
    pub(crate) name: String,
    pub(crate) version: Option<String>,
    pub(crate) selection_reason: String,
    pub(crate) allow_downgrade: bool,
}

/// Resolve, authenticate, prepare, and execute one complete package request set.
pub(crate) async fn install_package_set(
    db_path: &str,
    sandbox_mode: SandboxMode,
    requests: Vec<PackageSetRequest>,
) -> Result<usize> {
    if requests.is_empty() {
        return Ok(0);
    }
    crate::commands::hint_unconfigured_source_policy();

    let conn = super::super::open_db(db_path)?;
    let policy =
        conary_core::repository::load_effective_policy(&conn, RequestScope::Any)?.resolution;
    let mut resolved_requests = BTreeMap::new();
    for request in requests {
        let name = resolve_canonical_name(&conn, &request.name, None, &policy)?
            .unwrap_or_else(|| request.name.clone());
        if resolved_requests.insert(name.clone(), request).is_some() {
            anyhow::bail!("model package set resolves more than one root request to {name}");
        }
    }
    let solver_requests = resolved_requests
        .iter()
        .map(|(name, request)| {
            let constraint = request
                .version
                .as_deref()
                .map(VersionConstraint::parse)
                .transpose()
                .with_context(|| format!("invalid model version for package {name}"))?
                .unwrap_or(VersionConstraint::Any);
            Ok((name.clone(), constraint))
        })
        .collect::<Result<Vec<_>>>()?;
    let solved = conary_core::resolver::solve_install_with_policy(&conn, &solver_requests, &policy)
        .context("failed to solve the complete model package set")?;
    if let Some(conflict) = solved.conflict_message.as_deref() {
        anyhow::bail!("cannot apply model package set: {conflict}");
    }
    for name in resolved_requests.keys() {
        if !solved
            .install_order
            .iter()
            .any(|package| package.name == *name)
        {
            anyhow::bail!("model root package {name} is absent from the exact SAT selection");
        }
    }
    let selected = resolved_repository_deps_from_sat_result(&solved, "model package set");
    if selected.is_empty() {
        return Ok(0);
    }
    let selections = selected
        .into_iter()
        .map(|selected| {
            if let Some(root) = resolved_requests.get(&selected.package.name) {
                RepositoryBatchSelection {
                    selected,
                    install_reason: InstallReason::Explicit,
                    selection_reason: root.selection_reason.clone(),
                    allow_downgrade: root.allow_downgrade,
                    intent: InstallIntent::PackageChange,
                }
            } else {
                RepositoryBatchSelection {
                    selected,
                    install_reason: InstallReason::Dependency,
                    selection_reason: "Required by model package set".to_string(),
                    allow_downgrade: false,
                    intent: InstallIntent::PackageChange,
                }
            }
        })
        .collect();
    let prepared = prepare_repository_batch(db_path, selections).await?;
    let root_count = resolved_requests.len();
    println!(
        "Installing model package set ({} explicit roots)...",
        root_count
    );
    prepared.install(BatchInstaller::new(db_path, sandbox_mode))?;
    Ok(root_count)
}
