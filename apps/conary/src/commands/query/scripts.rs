// apps/conary/src/commands/query/scripts.rs
//! Scriptlet inspection and passive legacy scriptlet bundle rendering.

use anyhow::{Context, Result, bail};
use conary_core::ccs::CcsPackage;
use conary_core::ccs::legacy_scriptlets::{
    DecisionCounts, LegacyScriptletBundle, LegacyScriptletEntry, ScriptletEffect,
};
use conary_core::db::models::{InstalledLegacyScriptletBundle, ScriptletEntry, Trove};
use conary_core::packages::PackageFormat;
use conary_core::packages::arch::ArchPackage;
use conary_core::packages::deb::DebPackage;
use conary_core::packages::rpm::RpmPackage;
use conary_core::packages::traits::ScriptletPhase;
use serde::Serialize;
use std::path::Path;

use crate::commands::{InstalledPackageSelector, resolve_installed_package};
use crate::commands::{PackageFormatType, detect_package_format};

use super::super::open_db;

#[derive(Debug, Clone, Default)]
pub struct ScriptQueryOptions {
    pub db_path: Option<String>,
    pub version: Option<String>,
    pub architecture: Option<String>,
    pub verbose: bool,
    pub entry: Option<String>,
    pub json: bool,
}

#[derive(Debug, Clone, Serialize)]
struct PackageQueryIdentity {
    name: String,
    version: String,
}

#[derive(Debug, Clone, Serialize)]
struct InstalledPackageQueryIdentity {
    name: String,
    version: String,
    architecture: Option<String>,
}

#[derive(Debug, Serialize)]
struct ScriptQueryReport {
    package: PackageQueryIdentity,
    bundle_present: bool,
    bundle: Option<BundleQuerySummary>,
    entries: Vec<EntryQuerySummary>,
    warnings: Vec<String>,
}

#[derive(Debug, Serialize)]
struct InstalledScriptQueryReport {
    package: InstalledPackageQueryIdentity,
    flattened_scriptlets: Vec<FlattenedScriptletQuerySummary>,
    bundle_present: bool,
    bundle: Option<BundleQuerySummary>,
    entries: Vec<EntryQuerySummary>,
    warnings: Vec<String>,
}

#[derive(Debug, Serialize)]
struct FlattenedScriptletQuerySummary {
    source: String,
    phase: String,
    interpreter: String,
    flags: Option<String>,
    package_format: String,
    content_sha256: String,
}

#[derive(Debug, Serialize)]
struct BundleQuerySummary {
    schema: String,
    schema_revision: u16,
    source_format: String,
    source_family: String,
    source_distro: Option<String>,
    source_release: Option<String>,
    source_arch: Option<String>,
    source_package: String,
    source_version: String,
    target_compatibility: String,
    foreign_replay_policy: String,
    publication_policy: String,
    publication_status: String,
    scriptlet_fidelity: String,
    decision_counts: DecisionCounts,
    unsupported_class_counts: std::collections::BTreeMap<String, u32>,
    adapter_registry_digest: Option<String>,
    target_policy_digest: Option<String>,
    evidence_digest: Option<String>,
}

#[derive(Debug, Serialize)]
struct EntryQuerySummary {
    id: String,
    native_slot: String,
    phase: String,
    lifecycle_paths: Vec<String>,
    interpreter: String,
    interpreter_args: Vec<String>,
    body_sha256: String,
    body_encoding: String,
    timeout_ms: u64,
    decision: String,
    reason_code: String,
    human_reason: Option<String>,
    evidence_digest: Option<String>,
    source_evidence_refs: Vec<String>,
    effects: Vec<EffectQuerySummary>,
    unknown_commands: Vec<String>,
    blocked_classes: Vec<String>,
    reserved_metadata: ReservedMetadataSummary,
}

#[derive(Debug, Serialize)]
struct EffectQuerySummary {
    kind: String,
    source: String,
    confidence: String,
    replacement: String,
    adapter_id: Option<String>,
    adapter_digest: Option<String>,
    command: Option<String>,
    args: Vec<String>,
    path: Option<String>,
    reason_code: Option<String>,
}

#[derive(Debug, Serialize)]
struct ReservedMetadataSummary {
    rpm_trigger: bool,
    deb_maintainer: bool,
    arch_install: bool,
    residual_replay: bool,
}

pub async fn cmd_scripts(package_path: &str) -> Result<()> {
    cmd_scripts_with_options(package_path, ScriptQueryOptions::default()).await
}

pub async fn cmd_scripts_with_options(
    package_path: &str,
    options: ScriptQueryOptions,
) -> Result<()> {
    if package_path.ends_with(".ccs") {
        let package = CcsPackage::parse(package_path).map_err(|error| {
            anyhow::anyhow!("failed to parse CCS package '{package_path}': {error}")
        })?;
        return print_ccs_scriptlet_bundle(&package, &options);
    }

    if !looks_like_package_file(package_path)
        && let Some(db_path) = options.db_path.as_deref()
    {
        return print_installed_scriptlets(package_path, db_path, &options);
    }

    let native_result = detect_package_format(package_path);
    match native_result {
        Ok(format) => print_native_scriptlets(package_path, format, &options),
        Err(native_error) => match CcsPackage::parse(package_path) {
            Ok(package) => print_ccs_scriptlet_bundle(&package, &options),
            Err(_) => Err(native_error),
        },
    }
}

fn looks_like_package_file(package_path: &str) -> bool {
    let lower = package_path.to_ascii_lowercase();
    Path::new(package_path).exists()
        || lower.ends_with(".ccs")
        || lower.ends_with(".rpm")
        || lower.ends_with(".deb")
        || lower.contains(".pkg.tar")
}

fn print_ccs_scriptlet_bundle(package: &CcsPackage, options: &ScriptQueryOptions) -> Result<()> {
    let identity = PackageQueryIdentity {
        name: package.manifest().package.name.clone(),
        version: package.manifest().package.version.clone(),
    };
    let bundle = package.manifest().legacy_scriptlets.as_ref();

    let output = if options.json {
        render_ccs_bundle_json(&identity, bundle, options)?
    } else {
        render_ccs_bundle_text(&identity, bundle, options)?
    };
    println!("{output}");
    Ok(())
}

fn print_native_scriptlets(
    package_path: &str,
    format: PackageFormatType,
    options: &ScriptQueryOptions,
) -> Result<()> {
    if options.json || options.entry.is_some() {
        bail!("--json and --entry are only available for CCS legacy scriptlet bundles");
    }

    let package: Box<dyn PackageFormat> = match format {
        PackageFormatType::Rpm => Box::new(RpmPackage::parse(package_path)?),
        PackageFormatType::Deb => Box::new(DebPackage::parse(package_path)?),
        PackageFormatType::Arch => Box::new(ArchPackage::parse(package_path)?),
    };

    let scriptlets = package.scriptlets();

    if scriptlets.is_empty() {
        println!(
            "[INFO] {} v{} has no scriptlets",
            package.name(),
            package.version()
        );
        return Ok(());
    }

    println!("Package: {} v{}", package.name(), package.version());
    println!("Scriptlets: {}", scriptlets.len());
    println!();

    for scriptlet in scriptlets {
        let phase_name = match scriptlet.phase {
            ScriptletPhase::PreInstall => "pre-install",
            ScriptletPhase::PostInstall => "post-install",
            ScriptletPhase::PreRemove => "pre-remove",
            ScriptletPhase::PostRemove => "post-remove",
            ScriptletPhase::PreUpgrade => "pre-upgrade",
            ScriptletPhase::PostUpgrade => "post-upgrade",
            ScriptletPhase::PreTransaction => "pre-transaction",
            ScriptletPhase::PostTransaction => "post-transaction",
            ScriptletPhase::Trigger => "trigger",
        };

        println!("=== {} ===", phase_name);
        println!("Interpreter: {}", scriptlet.interpreter);
        if let Some(flags) = &scriptlet.flags {
            println!("Flags: {}", flags);
        }
        println!("---");
        for line in scriptlet.content.lines() {
            println!("{}", line);
        }
        println!("---");
        println!();
    }

    Ok(())
}

fn print_installed_scriptlets(
    package_name: &str,
    db_path: &str,
    options: &ScriptQueryOptions,
) -> Result<()> {
    let conn = open_db(db_path)?;
    let selector = InstalledPackageSelector::new(
        package_name.to_string(),
        options.version.clone(),
        options.architecture.clone(),
    );
    let resolved = resolve_installed_package(&conn, &selector)?;
    let flattened = ScriptletEntry::find_by_trove(&conn, resolved.trove_id)?;
    let installed_bundle = InstalledLegacyScriptletBundle::find_by_trove(&conn, resolved.trove_id)?
        .map(|installed| {
            installed
                .bundle()
                .context("installed legacy scriptlet bundle cannot be decoded")
        })
        .transpose()?;

    let output = if options.json {
        render_installed_scripts_json(
            &resolved.trove,
            &flattened,
            installed_bundle.as_ref(),
            options,
        )?
    } else {
        render_installed_scripts_text(
            &resolved.trove,
            &flattened,
            installed_bundle.as_ref(),
            options,
        )?
    };
    println!("{output}");
    Ok(())
}

fn render_installed_scripts_text(
    trove: &Trove,
    flattened: &[ScriptletEntry],
    bundle: Option<&LegacyScriptletBundle>,
    options: &ScriptQueryOptions,
) -> Result<String> {
    let mut output = String::new();
    output.push_str(&format!(
        "Installed package: {} {} [{}]\n",
        trove.name,
        trove.version,
        trove.architecture.as_deref().unwrap_or("none")
    ));
    output.push_str(&format!(
        "Flattened native scriptlets (scriptlets table): {}\n",
        flattened.len()
    ));
    for scriptlet in flattened {
        output.push_str(&format!(
            "  {:<16} source=scriptlets package_format={} interpreter={}\n",
            scriptlet.phase, scriptlet.package_format, scriptlet.interpreter
        ));
        if options.verbose {
            if let Some(flags) = &scriptlet.flags {
                output.push_str(&format!("    flags={flags}\n"));
            }
            output.push_str(&format!(
                "    content_sha256={}\n",
                conary_core::hash::sha256_prefixed(scriptlet.content.as_bytes())
            ));
        }
    }

    let Some(bundle) = bundle else {
        if let Some(entry_id) = &options.entry {
            bail!("legacy scriptlet bundle entry '{entry_id}' not found: package has no bundle");
        }
        output
            .push_str("Installed legacy bundle entries (installed_legacy_scriptlet_bundles): 0\n");
        return Ok(output);
    };

    let entries = filtered_entries(bundle, options.entry.as_deref())?;
    output.push_str(&format!(
        "Installed legacy bundle entries (installed_legacy_scriptlet_bundles): {}\n",
        bundle.entries.len()
    ));
    output.push_str(&format!("Legacy scriptlet bundle: {}\n", bundle.schema));
    output.push_str(&format!(
        "Fidelity: {}\n",
        bundle.scriptlet_fidelity.as_str()
    ));

    for entry in entries {
        output.push_str(&format!(
            "  {:<18} source=installed_legacy_scriptlet_bundles decision={} lifecycle={} reason={}\n",
            entry.id,
            entry.decision.as_str(),
            entry.phase.as_str(),
            entry.reason_code
        ));
        if options.verbose {
            output.push_str(&format!("    Interpreter: {}\n", entry.interpreter));
            if !entry.interpreter_args.is_empty() {
                output.push_str(&format!(
                    "    Interpreter args: {}\n",
                    entry.interpreter_args.join(" ")
                ));
            }
            output.push_str(&format!("    Timeout: {}ms\n", entry.timeout_ms));
            output.push_str(&format!("    body_sha256={}\n", entry.body_sha256));
            if !entry.lifecycle_paths.is_empty() {
                output.push_str(&format!(
                    "    Lifecycle paths: {}\n",
                    entry.lifecycle_paths.join(", ")
                ));
            }
            if let Some(evidence_digest) = &entry.evidence_digest {
                output.push_str(&format!("    evidence_digest={evidence_digest}\n"));
            }
        }
    }

    Ok(output)
}

fn render_installed_scripts_json(
    trove: &Trove,
    flattened: &[ScriptletEntry],
    bundle: Option<&LegacyScriptletBundle>,
    options: &ScriptQueryOptions,
) -> Result<String> {
    let (bundle_summary, entries, warnings) = if let Some(bundle) = bundle {
        (
            Some(bundle_summary(bundle)),
            filtered_entries(bundle, options.entry.as_deref())?
                .into_iter()
                .map(entry_summary)
                .collect(),
            collect_warnings(bundle),
        )
    } else {
        if let Some(entry_id) = &options.entry {
            bail!("legacy scriptlet bundle entry '{entry_id}' not found: package has no bundle");
        }
        (None, Vec::new(), Vec::new())
    };

    let report = InstalledScriptQueryReport {
        package: InstalledPackageQueryIdentity {
            name: trove.name.clone(),
            version: trove.version.clone(),
            architecture: trove.architecture.clone(),
        },
        flattened_scriptlets: flattened.iter().map(flattened_summary).collect(),
        bundle_present: bundle.is_some(),
        bundle: bundle_summary,
        entries,
        warnings,
    };

    serde_json::to_string_pretty(&report).context("failed to serialize installed script query JSON")
}

fn render_ccs_bundle_text(
    package: &PackageQueryIdentity,
    bundle: Option<&LegacyScriptletBundle>,
    options: &ScriptQueryOptions,
) -> Result<String> {
    let Some(bundle) = bundle else {
        if let Some(entry_id) = &options.entry {
            bail!("legacy scriptlet bundle entry '{entry_id}' not found: package has no bundle");
        }
        return Ok(format!(
            "Package: {} {}\nNo legacy scriptlet bundle found.\n",
            package.name, package.version
        ));
    };

    let entries = filtered_entries(bundle, options.entry.as_deref())?;
    let mut output = String::new();
    output.push_str(&format!("Package: {} {}\n", package.name, package.version));
    output.push_str(&format!("Legacy scriptlet bundle: {}\n", bundle.schema));
    output.push_str(&format!(
        "Source: {}{}{}{}\n",
        bundle.source_format.as_str(),
        optional_prefixed(" ", bundle.source_distro.as_deref()),
        optional_prefixed(" ", bundle.source_release.as_deref()),
        optional_prefixed(" ", bundle.source_arch.as_deref())
    ));
    output.push_str(&format!(
        "Compatibility: {}\nForeign replay: {}\nFidelity: {}\n",
        bundle.target_compatibility.as_str(),
        bundle.foreign_replay_policy.as_str(),
        bundle.scriptlet_fidelity.as_str()
    ));
    output.push_str(&format!(
        "Entries: {} replaced, {} legacy, {} blocked, {} review\n",
        bundle.decision_counts.replaced,
        bundle.decision_counts.legacy,
        bundle.decision_counts.blocked,
        bundle.decision_counts.review
    ));
    output.push('\n');

    if bundle.entries.is_empty() {
        output.push_str(
            "No legacy scriptlet entries. This package does not require native scriptlet replay.\n",
        );
        return Ok(output);
    }

    for entry in entries {
        output.push_str(&format!(
            "{:<18} {:<9} {:<16} reason={}\n",
            entry.id,
            entry.decision.as_str(),
            entry.phase.as_str(),
            entry.reason_code
        ));

        if options.verbose {
            output.push_str(&format!("  Interpreter: {}\n", entry.interpreter));
            if !entry.interpreter_args.is_empty() {
                output.push_str(&format!(
                    "  Interpreter args: {}\n",
                    entry.interpreter_args.join(" ")
                ));
            }
            output.push_str(&format!("  Timeout: {}ms\n", entry.timeout_ms));
            output.push_str(&format!(
                "  Lifecycle paths: {}\n",
                entry.lifecycle_paths.join(", ")
            ));
            output.push_str(&format!("  body_sha256={}\n", entry.body_sha256));
            if let Some(evidence_digest) = &entry.evidence_digest {
                output.push_str(&format!("  evidence_digest={evidence_digest}\n"));
            }
            if !entry.unknown_commands.is_empty() {
                output.push_str(&format!(
                    "  Unknown commands: {}\n",
                    entry.unknown_commands.join(", ")
                ));
            }
            if !entry.blocked_classes.is_empty() {
                output.push_str(&format!(
                    "  Blocked classes: {}\n",
                    entry.blocked_classes.join(", ")
                ));
            }
            if !entry.effects.is_empty() {
                output.push_str("  Effects:\n");
                for effect in &entry.effects {
                    output.push_str(&format!(
                        "    - {} replacement={} source={} confidence={}",
                        effect.kind,
                        effect.replacement.as_str(),
                        effect.source.as_str(),
                        effect.confidence.as_str()
                    ));
                    if let Some(adapter_id) = &effect.adapter_id {
                        output.push_str(&format!(" adapter={adapter_id}"));
                    }
                    output.push('\n');
                }
            }
        }
    }

    Ok(output)
}

fn render_ccs_bundle_json(
    package: &PackageQueryIdentity,
    bundle: Option<&LegacyScriptletBundle>,
    options: &ScriptQueryOptions,
) -> Result<String> {
    let (bundle_summary, entries, warnings) = if let Some(bundle) = bundle {
        (
            Some(bundle_summary(bundle)),
            filtered_entries(bundle, options.entry.as_deref())?
                .into_iter()
                .map(entry_summary)
                .collect(),
            collect_warnings(bundle),
        )
    } else {
        if let Some(entry_id) = &options.entry {
            bail!("legacy scriptlet bundle entry '{entry_id}' not found: package has no bundle");
        }
        (None, Vec::new(), Vec::new())
    };

    let report = ScriptQueryReport {
        package: package.clone(),
        bundle_present: bundle.is_some(),
        bundle: bundle_summary,
        entries,
        warnings,
    };

    serde_json::to_string_pretty(&report).context("failed to serialize script query JSON")
}

fn filtered_entries<'a>(
    bundle: &'a LegacyScriptletBundle,
    entry_id: Option<&str>,
) -> Result<Vec<&'a LegacyScriptletEntry>> {
    if let Some(entry_id) = entry_id {
        let entry = bundle
            .entries
            .iter()
            .find(|entry| entry.id == entry_id)
            .ok_or_else(|| {
                anyhow::anyhow!("legacy scriptlet bundle entry '{entry_id}' not found")
            })?;
        Ok(vec![entry])
    } else {
        Ok(bundle.entries.iter().collect())
    }
}

fn bundle_summary(bundle: &LegacyScriptletBundle) -> BundleQuerySummary {
    BundleQuerySummary {
        schema: bundle.schema.clone(),
        schema_revision: bundle.schema_revision,
        source_format: bundle.source_format.as_str().to_string(),
        source_family: bundle.source_family.clone(),
        source_distro: bundle.source_distro.clone(),
        source_release: bundle.source_release.clone(),
        source_arch: bundle.source_arch.clone(),
        source_package: bundle.source_package.clone(),
        source_version: bundle.source_version.clone(),
        target_compatibility: bundle.target_compatibility.as_str().to_string(),
        foreign_replay_policy: bundle.foreign_replay_policy.as_str().to_string(),
        publication_policy: bundle.publication_policy.as_str().to_string(),
        publication_status: bundle.publication_status.as_str().to_string(),
        scriptlet_fidelity: bundle.scriptlet_fidelity.as_str().to_string(),
        decision_counts: bundle.decision_counts.clone(),
        unsupported_class_counts: bundle.unsupported_class_counts.clone(),
        adapter_registry_digest: bundle.adapter_registry_digest.clone(),
        target_policy_digest: bundle.target_policy_digest.clone(),
        evidence_digest: bundle.evidence_digest.clone(),
    }
}

fn entry_summary(entry: &LegacyScriptletEntry) -> EntryQuerySummary {
    EntryQuerySummary {
        id: entry.id.clone(),
        native_slot: entry.native_slot.clone(),
        phase: entry.phase.as_str().to_string(),
        lifecycle_paths: entry.lifecycle_paths.clone(),
        interpreter: entry.interpreter.clone(),
        interpreter_args: entry.interpreter_args.clone(),
        body_sha256: entry.body_sha256.clone(),
        body_encoding: entry
            .body_encoding
            .clone()
            .unwrap_or_else(|| "utf-8".to_string()),
        timeout_ms: entry.timeout_ms,
        decision: entry.decision.as_str().to_string(),
        reason_code: entry.reason_code.clone(),
        human_reason: entry.human_reason.clone(),
        evidence_digest: entry.evidence_digest.clone(),
        source_evidence_refs: entry.source_evidence_refs.clone(),
        effects: entry.effects.iter().map(effect_summary).collect(),
        unknown_commands: entry.unknown_commands.clone(),
        blocked_classes: entry.blocked_classes.clone(),
        reserved_metadata: ReservedMetadataSummary {
            rpm_trigger: entry.rpm_trigger.is_some(),
            deb_maintainer: entry.deb_maintainer.is_some(),
            arch_install: entry.arch_install.is_some(),
            residual_replay: entry.residual_replay.is_some(),
        },
    }
}

fn flattened_summary(scriptlet: &ScriptletEntry) -> FlattenedScriptletQuerySummary {
    FlattenedScriptletQuerySummary {
        source: "scriptlets".to_string(),
        phase: scriptlet.phase.clone(),
        interpreter: scriptlet.interpreter.clone(),
        flags: scriptlet.flags.clone(),
        package_format: scriptlet.package_format.clone(),
        content_sha256: conary_core::hash::sha256_prefixed(scriptlet.content.as_bytes()),
    }
}

fn effect_summary(effect: &ScriptletEffect) -> EffectQuerySummary {
    EffectQuerySummary {
        kind: effect.kind.clone(),
        source: effect.source.as_str().to_string(),
        confidence: effect.confidence.as_str().to_string(),
        replacement: effect.replacement.as_str().to_string(),
        adapter_id: effect.adapter_id.clone(),
        adapter_digest: effect.adapter_digest.clone(),
        command: effect.command.clone(),
        args: effect.args.clone(),
        path: effect.path.clone(),
        reason_code: effect.reason_code.clone(),
    }
}

fn collect_warnings(bundle: &LegacyScriptletBundle) -> Vec<String> {
    let mut warnings = Vec::new();
    push_unknown_warning(
        &mut warnings,
        "source_format",
        bundle.source_format.as_str(),
        bundle.source_format.is_known(),
    );
    push_unknown_warning(
        &mut warnings,
        "target_compatibility",
        bundle.target_compatibility.as_str(),
        bundle.target_compatibility.is_known(),
    );
    push_unknown_warning(
        &mut warnings,
        "foreign_replay_policy",
        bundle.foreign_replay_policy.as_str(),
        bundle.foreign_replay_policy.is_known(),
    );
    push_unknown_warning(
        &mut warnings,
        "publication_policy",
        bundle.publication_policy.as_str(),
        bundle.publication_policy.is_known(),
    );
    push_unknown_warning(
        &mut warnings,
        "publication_status",
        bundle.publication_status.as_str(),
        bundle.publication_status.is_known(),
    );
    push_unknown_warning(
        &mut warnings,
        "scriptlet_fidelity",
        bundle.scriptlet_fidelity.as_str(),
        bundle.scriptlet_fidelity.is_known(),
    );

    for entry in &bundle.entries {
        push_unknown_warning(
            &mut warnings,
            &format!("entry {} decision", entry.id),
            entry.decision.as_str(),
            entry.decision.is_known(),
        );
        push_unknown_warning(
            &mut warnings,
            &format!("entry {} phase", entry.id),
            entry.phase.as_str(),
            entry.phase.is_known(),
        );
        for effect in &entry.effects {
            push_unknown_warning(
                &mut warnings,
                &format!("entry {} effect {} source", entry.id, effect.kind),
                effect.source.as_str(),
                effect.source.is_known(),
            );
            push_unknown_warning(
                &mut warnings,
                &format!("entry {} effect {} confidence", entry.id, effect.kind),
                effect.confidence.as_str(),
                effect.confidence.is_known(),
            );
            push_unknown_warning(
                &mut warnings,
                &format!("entry {} effect {} replacement", entry.id, effect.kind),
                effect.replacement.as_str(),
                effect.replacement.is_known(),
            );
        }
    }

    warnings
}

fn push_unknown_warning(warnings: &mut Vec<String>, field: &str, value: &str, known: bool) {
    if !known {
        warnings.push(format!("{field} has unknown passive value '{value}'"));
    }
}

fn optional_prefixed(prefix: &str, value: Option<&str>) -> String {
    value
        .filter(|value| !value.is_empty())
        .map(|value| format!("{prefix}{value}"))
        .unwrap_or_default()
}

#[cfg(test)]
mod query_scripts {
    use super::*;
    use conary_core::ccs::legacy_scriptlets::{
        DecisionCounts, EffectConfidence, EffectReplacement, EffectSource, ForeignReplayPolicy,
        LEGACY_SCRIPTLET_SCHEMA_V1, LegacyScriptletBundle, LegacyScriptletEntry, LifecyclePath,
        NativeInvocation, PublicationPolicy, PublicationStatus, ScriptletDecision, ScriptletEffect,
        ScriptletFidelity, SourceFormat, TargetCompatibility, TransactionOrder, VersionScheme,
    };
    use std::collections::BTreeMap;

    fn bundle_fixture() -> LegacyScriptletBundle {
        let legacy_body = "systemctl daemon-reload\n";
        let replaced_body = "ldconfig\n";
        LegacyScriptletBundle {
            schema: LEGACY_SCRIPTLET_SCHEMA_V1.to_string(),
            schema_revision: 1,
            source_format: SourceFormat::Rpm,
            source_family: "fedora-rhel".to_string(),
            source_distro: Some("fedora".to_string()),
            source_release: Some("44".to_string()),
            source_arch: Some("x86_64".to_string()),
            source_package: "nginx".to_string(),
            source_version: "1.28.0-1.fc44".to_string(),
            source_checksum: Some(
                "sha256:3333333333333333333333333333333333333333333333333333333333333333"
                    .to_string(),
            ),
            version_scheme: VersionScheme::Rpm,
            conversion_tool: "remi".to_string(),
            conversion_tool_version: "0.8.0".to_string(),
            conversion_policy: "safe-or-legacy".to_string(),
            adapter_registry_digest: Some(
                "sha256:4444444444444444444444444444444444444444444444444444444444444444"
                    .to_string(),
            ),
            target_policy_digest: None,
            evidence_digest: Some(
                "sha256:5555555555555555555555555555555555555555555555555555555555555555"
                    .to_string(),
            ),
            target_compatibility: TargetCompatibility::SourceNative,
            allowed_targets: vec!["rpm/fedora/44/x86_64".to_string()],
            foreign_replay_policy: ForeignReplayPolicy::Deny,
            publication_policy: PublicationPolicy::PublicIfNoBlocked,
            publication_status: PublicationStatus::PrivateReview,
            scriptlet_fidelity: ScriptletFidelity::Mixed,
            decision_counts: DecisionCounts {
                replaced: 1,
                legacy: 1,
                blocked: 0,
                review: 0,
                extra: BTreeMap::new(),
            },
            unsupported_class_counts: BTreeMap::new(),
            entries: vec![
                entry_fixture("rpm:%preun", ScriptletDecision::Replaced, replaced_body),
                entry_fixture("rpm:%post", ScriptletDecision::Legacy, legacy_body),
            ],
            extra: BTreeMap::new(),
        }
    }

    fn entry_fixture(id: &str, decision: ScriptletDecision, body: &str) -> LegacyScriptletEntry {
        LegacyScriptletEntry {
            id: id.to_string(),
            native_slot: id.split(':').nth(1).unwrap_or("%post").to_string(),
            phase: if id.ends_with("%preun") {
                LifecyclePath::PreRemove
            } else {
                LifecyclePath::PostInstall
            },
            lifecycle_paths: vec!["install:first".to_string()],
            interpreter: "/bin/sh".to_string(),
            interpreter_args: vec!["-e".to_string()],
            body_sha256: conary_core::hash::sha256_prefixed(body.as_bytes()),
            body: body.to_string(),
            body_encoding: None,
            native_invocation: NativeInvocation {
                args: vec!["1".to_string()],
                environment: vec!["RPM_INSTALL_PREFIX=/".to_string()],
                stdin: Some("none".to_string()),
                chroot: Some("install-root".to_string()),
                extra: BTreeMap::new(),
            },
            transaction_order: TransactionOrder {
                position: "after-payload".to_string(),
                before: vec![],
                after: vec!["payload".to_string()],
                extra: BTreeMap::new(),
            },
            timeout_ms: 30_000,
            sandbox: None,
            capabilities: vec!["ldconfig".to_string()],
            decision,
            reason_code: "protected-replay-required".to_string(),
            human_reason: Some("fixture reason".to_string()),
            evidence_digest: Some(
                "sha256:6666666666666666666666666666666666666666666666666666666666666666"
                    .to_string(),
            ),
            source_evidence_refs: vec!["capture:rpm:%post".to_string()],
            effects: vec![ScriptletEffect {
                kind: "ldconfig".to_string(),
                source: EffectSource::StaticSignal,
                confidence: EffectConfidence::Declared,
                replacement: EffectReplacement::Complete,
                adapter_id: Some("ldconfig/v1".to_string()),
                adapter_digest: Some(
                    "sha256:7777777777777777777777777777777777777777777777777777777777777777"
                        .to_string(),
                ),
                command: Some("ldconfig".to_string()),
                args: vec!["-X".to_string()],
                path: Some("/usr/lib64".to_string()),
                reason_code: Some("ldconfig-cache-refresh".to_string()),
                extra: BTreeMap::new(),
            }],
            unknown_commands: vec!["systemctl".to_string()],
            blocked_classes: vec![],
            rpm_trigger: None,
            deb_maintainer: None,
            arch_install: None,
            residual_replay: None,
            extra: BTreeMap::new(),
        }
    }

    fn package_identity() -> PackageQueryIdentity {
        PackageQueryIdentity {
            name: "nginx".to_string(),
            version: "1.28.0".to_string(),
        }
    }

    #[test]
    fn script_query_summary_renders_bundle_counts() {
        let output = render_ccs_bundle_text(
            &package_identity(),
            Some(&bundle_fixture()),
            &ScriptQueryOptions::default(),
        )
        .expect("render summary");

        assert!(output.contains("Package: nginx 1.28.0"));
        assert!(output.contains("Legacy scriptlet bundle: conary.legacy-scriptlets.v1"));
        assert!(output.contains("Entries: 1 replaced, 1 legacy, 0 blocked, 0 review"));
        assert!(output.contains("rpm:%post"));
        assert!(!output.contains("systemctl daemon-reload"));
    }

    #[test]
    fn script_query_verbose_renders_entry_details() {
        let output = render_ccs_bundle_text(
            &package_identity(),
            Some(&bundle_fixture()),
            &ScriptQueryOptions {
                verbose: true,
                ..ScriptQueryOptions::default()
            },
        )
        .expect("render verbose");

        assert!(output.contains("Interpreter: /bin/sh"));
        assert!(output.contains("Timeout: 30000ms"));
        assert!(output.contains("Effects:"));
        assert!(output.contains("body_sha256="));
        assert!(!output.contains("systemctl daemon-reload"));
    }

    #[test]
    fn script_query_entry_filter_renders_one_entry() {
        let output = render_ccs_bundle_text(
            &package_identity(),
            Some(&bundle_fixture()),
            &ScriptQueryOptions {
                entry: Some("rpm:%post".to_string()),
                ..ScriptQueryOptions::default()
            },
        )
        .expect("render entry");

        assert!(output.contains("rpm:%post"));
        assert!(!output.contains("rpm:%preun"));
    }

    #[test]
    fn script_query_json_omits_raw_bodies_by_default() {
        let output = render_ccs_bundle_json(
            &package_identity(),
            Some(&bundle_fixture()),
            &ScriptQueryOptions::default(),
        )
        .expect("render json");
        let json: serde_json::Value = serde_json::from_str(&output).expect("valid json");

        assert_eq!(json["bundle_present"], true);
        assert!(output.contains("body_sha256"));
        assert!(!output.contains("systemctl daemon-reload"));
        assert!(json["entries"][0].get("body").is_none());
    }

    #[test]
    fn script_query_json_reports_no_bundle_without_entries() {
        let output =
            render_ccs_bundle_json(&package_identity(), None, &ScriptQueryOptions::default())
                .expect("render json");
        let json: serde_json::Value = serde_json::from_str(&output).expect("valid json");

        assert_eq!(json["bundle_present"], false);
        assert!(json["bundle"].is_null());
        assert!(
            json["entries"]
                .as_array()
                .expect("entries array")
                .is_empty()
        );
    }

    #[test]
    fn script_query_json_reports_zero_entry_bundle() {
        let mut bundle = bundle_fixture();
        bundle.entries.clear();
        bundle.decision_counts = DecisionCounts::default();
        bundle.scriptlet_fidelity = ScriptletFidelity::NativeFree;

        let output = render_ccs_bundle_json(
            &package_identity(),
            Some(&bundle),
            &ScriptQueryOptions::default(),
        )
        .expect("render json");
        let json: serde_json::Value = serde_json::from_str(&output).expect("valid json");

        assert_eq!(json["bundle_present"], true);
        assert!(
            json["entries"]
                .as_array()
                .expect("entries array")
                .is_empty()
        );
    }
}
