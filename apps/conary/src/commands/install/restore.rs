// src/commands/install/restore.rs
//! Shared install preparation/execution helpers for state restore.

use super::dependencies::extract_runtime_deps;
use super::inner::install_inner;
use super::prepare::{UpgradeCheck, check_upgrade_status, parse_package};
use super::resolve::{
    PolicyOptions, ResolutionOutcome, ResolvedSourceType, resolve_package_path_with_policy,
};
use super::{
    ExtractionResult, InstallOptions, InstallPhase, InstallProgress, InstallSemantics,
    PreScriptletState, ScriptletContext, TransactionContext, build_resolution_policy,
    extract_and_classify_files, resolve_canonical_name, run_pre_install_phase,
};
use anyhow::{Context, Result};
use conary_core::ccs::CcsPackage;
use conary_core::db::models::{
    ProvideEntry, StateMember, Trove, TroveType, generate_capability_variations,
};
use conary_core::packages::PackageFormat;
use conary_core::scriptlet::SandboxMode;
use conary_core::transaction::TransactionEngine;
use rusqlite::{Connection, Transaction};
use std::collections::HashSet;
use tempfile::TempDir;

pub(crate) struct PreparedInstall {
    pkg: Box<dyn PackageFormat>,
    extraction: ExtractionResult,
    selection_reason: Option<String>,
    old_trove_to_upgrade: Option<Trove>,
    semantics: InstallSemantics,
    _temp_dir: Option<TempDir>,
}

pub(crate) struct PreparedInstallExecution {
    prepared: PreparedInstall,
    pre_state: PreScriptletState,
    progress: InstallProgress,
    root: String,
    no_scripts: bool,
    sandbox_mode: SandboxMode,
}

#[derive(Debug, Default)]
pub(crate) struct TargetStateView {
    members: HashSet<(String, Option<String>)>,
    provides: TargetProvidesView,
}

#[derive(Debug, Default)]
struct TargetProvidesView {
    raw: HashSet<String>,
    lower: HashSet<String>,
}

impl TargetProvidesView {
    fn insert(&mut self, capability: impl Into<String>) {
        let capability = capability.into();
        self.lower.insert(capability.to_lowercase());
        self.raw.insert(capability);
    }

    fn contains_like(&self, capability: &str) -> bool {
        if self.raw.contains(capability) {
            return true;
        }

        let prefix_pattern = format!("{capability} ");
        let paren_pattern = format!("{capability}(");
        if self.raw.iter().any(|candidate| {
            candidate.starts_with(&prefix_pattern) || candidate.starts_with(&paren_pattern)
        }) {
            return true;
        }

        if let Some(base) = capability
            .split('(')
            .next()
            .filter(|base| *base != capability)
            && capability.contains(".so")
        {
            let base_paren = format!("{base}(");
            if self.raw.contains(base)
                || self
                    .raw
                    .iter()
                    .any(|candidate| candidate.starts_with(&base_paren))
            {
                return true;
            }
        }

        let lower = capability.to_lowercase();
        self.lower.contains(&lower)
            || self.lower.iter().any(|candidate| {
                candidate.starts_with(&(lower.clone() + " "))
                    || candidate.starts_with(&(lower.clone() + "("))
            })
    }

    fn satisfies(&self, capability: &str) -> bool {
        self.contains_like(capability)
            || generate_capability_variations(capability)
                .into_iter()
                .any(|variation| self.contains_like(&variation))
    }
}

impl TargetStateView {
    fn contains_member(&self, name: &str, architecture: Option<&str>) -> bool {
        self.members
            .contains(&(name.to_string(), architecture.map(str::to_string)))
    }

    fn add_member(&mut self, name: impl Into<String>, architecture: Option<&str>) {
        self.members
            .insert((name.into(), architecture.map(str::to_string)));
    }

    fn add_installed_trove(&mut self, conn: &Connection, trove: &Trove) -> Result<()> {
        self.add_member(&trove.name, trove.architecture.as_deref());
        self.provides.insert(trove.name.clone());
        if let Some(trove_id) = trove.id {
            for provide in ProvideEntry::find_by_trove(conn, trove_id)? {
                self.provides.insert(provide.capability.clone());
                self.provides.insert(provide.to_typed_string());
            }
        }
        Ok(())
    }

    fn add_prepared_install(&mut self, prepared: &PreparedInstall) {
        self.add_member(prepared.pkg.name(), prepared.pkg.architecture());
        self.provides.insert(prepared.pkg.name().to_string());
        for provide in &prepared.extraction.language_provides {
            self.provides.insert(provide.to_dep_string());
            self.provides.insert(provide.name.clone());
        }
    }

    fn dependency_satisfied(&self, dependency: &str) -> bool {
        self.provides.satisfies(dependency)
    }
}

pub(crate) fn build_target_state_view(
    conn: &Connection,
    members: &[StateMember],
) -> Result<TargetStateView> {
    let mut target_state = TargetStateView::default();

    for member in members {
        if let Some(trove) = Trove::find_by_name(conn, &member.trove_name)?
            .into_iter()
            .find(|trove| {
                trove.version == member.trove_version
                    && (member.architecture == trove.architecture
                        || member.architecture.is_none()
                        || trove.architecture.is_none())
                    && trove.trove_type == TroveType::Package
            })
        {
            target_state.add_installed_trove(conn, &trove)?;
        }
    }

    Ok(target_state)
}

pub(crate) fn add_prepared_install_to_target_state(
    target_state: &mut TargetStateView,
    prepared: &PreparedInstall,
) {
    target_state.add_prepared_install(prepared);
}

pub(crate) fn validate_prepared_install_dependencies(
    prepared: &PreparedInstall,
    target_state: &TargetStateView,
) -> Result<()> {
    let unsatisfied: Vec<_> = extract_runtime_deps(prepared.pkg.as_ref())
        .into_iter()
        .filter(|dep| !should_skip_restore_dependency(dep.name.as_str()))
        .filter(|dep| {
            !target_state.contains_member(&dep.name, None)
                && !target_state.dependency_satisfied(dep.name.as_str())
        })
        .collect();

    if unsatisfied.is_empty() {
        return Ok(());
    }

    let summary = unsatisfied
        .iter()
        .map(|dep| format!("{} {}", dep.name, dep.constraint))
        .collect::<Vec<_>>()
        .join(", ");

    anyhow::bail!(
        "Restore target '{}' has unsatisfied dependencies in the destination state: {}",
        prepared.pkg.name(),
        summary
    );
}

pub(crate) async fn prepare_install_for_restore(
    conn: &Connection,
    package: &str,
    opts: InstallOptions<'_>,
) -> Result<PreparedInstall> {
    let InstallOptions {
        db_path,
        root: _,
        version,
        repo,
        architecture,
        selection_reason,
        allow_downgrade,
        from_distro,
        ..
    } = opts;

    let effective_source_policy = conary_core::repository::load_effective_policy(
        conn,
        conary_core::repository::resolution_policy::RequestScope::Any,
    )?;
    let policy = build_resolution_policy(
        effective_source_policy.resolution,
        from_distro.as_deref(),
        repo.as_deref(),
    );
    let primary_flavor = effective_source_policy.primary_flavor;
    let resolved_name = resolve_canonical_name(conn, package, from_distro.as_deref(), &policy)?;
    let package_name = resolved_name.unwrap_or_else(|| package.to_string());

    let progress = InstallProgress::single("Restoring");
    progress.set_phase(&package_name, InstallPhase::Downloading);
    let policy_opts = PolicyOptions {
        policy: Some(policy),
        is_root: true,
        primary_flavor,
    };

    let resolved = match resolve_package_path_with_policy(
        &package_name,
        db_path,
        version.as_deref(),
        repo.as_deref(),
        architecture.as_deref(),
        &progress,
        &policy_opts,
    )
    .await?
    {
        ResolutionOutcome::AlreadyInstalled { name, version } => {
            anyhow::bail!(
                "Restore preflight expected '{}' to be absent/pending, but resolver reported {} {} already installed",
                package,
                name,
                version
            );
        }
        ResolutionOutcome::Resolved(pkg) => pkg,
    };

    let path_str = resolved
        .path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("Invalid package path (non-UTF8)"))?;

    let (pkg, semantics) =
        if resolved.source_type == ResolvedSourceType::Remi || path_str.ends_with(".ccs") {
            (
                Box::new(CcsPackage::parse(path_str).context("Failed to parse CCS package")?)
                    as Box<dyn PackageFormat>,
                InstallSemantics::ccs(),
            )
        } else {
            let format = super::detect_package_format(path_str)
                .with_context(|| format!("Failed to detect package format for '{}'", path_str))?;
            (
                parse_package(&resolved.path, format)?,
                InstallSemantics::legacy(format),
            )
        };

    progress.set_phase(package, InstallPhase::Parsing);
    let extraction = extract_and_classify_files(
        pkg.as_ref(),
        &super::ComponentSelection::Defaults,
        &progress,
    )?;

    let old_trove_to_upgrade =
        match check_upgrade_status(conn, pkg.as_ref(), &semantics, allow_downgrade)? {
            UpgradeCheck::FreshInstall => None,
            UpgradeCheck::Upgrade(trove) | UpgradeCheck::Downgrade(trove) => Some(*trove),
        };

    Ok(PreparedInstall {
        pkg,
        extraction,
        selection_reason: selection_reason.map(str::to_string),
        old_trove_to_upgrade,
        semantics,
        _temp_dir: resolved._temp_dir,
    })
}

pub(crate) fn run_pre_install_for_prepared(
    conn: &Connection,
    root: &str,
    no_scripts: bool,
    sandbox_mode: SandboxMode,
    prepared: PreparedInstall,
) -> Result<PreparedInstallExecution> {
    let progress = InstallProgress::single("Restoring");
    let scriptlet_ctx = ScriptletContext {
        root,
        no_scripts,
        sandbox_mode,
        semantics: prepared.semantics,
        old_trove: prepared.old_trove_to_upgrade.as_ref(),
    };
    let pre_state = run_pre_install_phase(
        conn,
        prepared.pkg.as_ref(),
        &prepared.extraction.installed_component_types,
        &scriptlet_ctx,
        &progress,
    )?;

    Ok(PreparedInstallExecution {
        prepared,
        pre_state,
        progress,
        root: root.to_string(),
        no_scripts,
        sandbox_mode,
    })
}

pub(crate) fn install_prepared_inner(
    tx: &Transaction<'_>,
    engine: &mut TransactionEngine,
    changeset_id: i64,
    db_path: &str,
    execution: &PreparedInstallExecution,
) -> Result<()> {
    let tx_ctx = TransactionContext {
        db_path,
        root: &execution.root,
        semantics: execution.prepared.semantics,
        selection_reason: execution.prepared.selection_reason.as_deref(),
        old_trove_to_upgrade: execution.prepared.old_trove_to_upgrade.as_ref(),
    };
    install_inner(
        tx,
        engine,
        changeset_id,
        execution.prepared.pkg.as_ref(),
        &execution.prepared.extraction,
        &tx_ctx,
        &execution.progress,
    )?;
    Ok(())
}

pub(crate) fn finalize_prepared_install_without_snapshot(
    conn: &Connection,
    changeset_id: i64,
    execution: &PreparedInstallExecution,
) -> Result<()> {
    let scriptlet_ctx = ScriptletContext {
        root: &execution.root,
        no_scripts: execution.no_scripts,
        sandbox_mode: execution.sandbox_mode,
        semantics: execution.prepared.semantics,
        old_trove: execution.prepared.old_trove_to_upgrade.as_ref(),
    };
    let tx_result = super::InstallTransactionResult { changeset_id };
    super::finalize_install_without_snapshot(
        conn,
        execution.prepared.pkg.as_ref(),
        &execution.prepared.extraction,
        &scriptlet_ctx,
        &execution.pre_state,
        &tx_result,
        &execution.progress,
    )
}

fn should_skip_restore_dependency(name: &str) -> bool {
    name.starts_with("rpmlib(")
        || name.starts_with('/')
        || name.contains(" if ")
        || name.contains(" unless ")
        || name.starts_with("((")
}
