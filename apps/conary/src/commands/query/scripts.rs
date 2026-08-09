// apps/conary/src/commands/query/scripts.rs
//! Scriptlet inspection and passive native lifecycle bundle rendering.

use anyhow::{Context, Result, bail};
use conary_core::ccs::CcsPackage;
use conary_core::ccs::archive_reader::inspect_untrusted_ccs_archive;
use conary_core::ccs::native_lifecycle::{NativeLifecycleBundle, NativeLifecycleEntry};
use conary_core::ccs::verify::{TrustPolicy, verify_package};
use conary_core::db::models::{InstalledCcsRemoveHook, InstalledNativeLifecycleBundle, Trove};
use conary_core::packages::PackageFormat;
use conary_core::packages::arch::ArchPackage;
use conary_core::packages::deb::DebPackage;
use conary_core::packages::native_abi::RpmScriptletSlot;
use conary_core::packages::rpm::RpmPackage;
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
    pub policy_path: Option<String>,
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
}

#[derive(Debug, Serialize)]
struct InstalledScriptQueryReport {
    package: InstalledPackageQueryIdentity,
    ccs_remove_hook: Option<CcsRemoveHookQuerySummary>,
    bundle_present: bool,
    bundle: Option<BundleQuerySummary>,
    entries: Vec<EntryQuerySummary>,
}

#[derive(Debug, Serialize)]
struct CcsRemoveHookQuerySummary {
    source: String,
    interpreter: String,
    reversible: Option<bool>,
    script_sha256: String,
}

#[derive(Debug, Serialize)]
struct BundleQuerySummary {
    schema: String,
    schema_revision: u16,
    source_format: String,
    source_family: String,
    source_profile: Option<String>,
    source_release: Option<String>,
    source_arch: Option<String>,
    source_package: String,
    source_version: String,
    scriptlet_fidelity: String,
    evidence_digest: Option<String>,
}

#[derive(Debug, Serialize)]
struct EntryQuerySummary {
    id: String,
    /// The exact persisted typed slot class string, when the entry declares
    /// one.
    native_slot: Option<String>,
    phase: String,
    lifecycle_paths: Vec<String>,
    interpreter: String,
    interpreter_args: Vec<String>,
    body_sha256: String,
    body_encoding: String,
    timeout_ms: u64,
    evidence_digest: Option<String>,
    source_evidence_refs: Vec<String>,
    reserved_metadata: ReservedMetadataSummary,
}

#[derive(Debug, Serialize)]
struct ReservedMetadataSummary {
    rpm_trigger: bool,
    deb_maintainer: bool,
    arch_install: bool,
    arch_hook: bool,
    residual_lifecycle: bool,
}

pub async fn cmd_scripts(package_path: &str) -> Result<()> {
    cmd_scripts_with_options(package_path, ScriptQueryOptions::default()).await
}

pub async fn cmd_scripts_with_options(
    package_path: &str,
    options: ScriptQueryOptions,
) -> Result<()> {
    if !looks_like_package_file(package_path)
        && let Some(db_path) = options.db_path.as_deref()
    {
        return print_installed_scriptlets(package_path, db_path, &options);
    }

    let native_result = detect_package_format(package_path);
    match native_result {
        Ok(format) => print_native_scriptlets(package_path, format, &options),
        Err(native_error) => {
            let package = load_verified_ccs_package(package_path, &options)
                .with_context(|| format!("native package detection failed: {native_error}"))?;
            print_ccs_scriptlet_bundle(&package, &options)
        }
    }
}

fn load_verified_ccs_package(
    package_path: &str,
    options: &ScriptQueryOptions,
) -> Result<CcsPackage> {
    let file = std::fs::File::open(package_path)
        .with_context(|| format!("open package file {package_path}"))?;
    inspect_untrusted_ccs_archive(file)
        .with_context(|| format!("{package_path} is not a current signed CCS v3 archive"))?;
    let policy_path = options
        .policy_path
        .as_deref()
        .with_context(|| format!("CCS package inspection requires --policy for {package_path}"))?;
    let policy = TrustPolicy::from_file(Path::new(policy_path))
        .with_context(|| format!("load CCS trust policy {policy_path}"))?;
    let verified = verify_package(Path::new(package_path), &policy)
        .with_context(|| format!("verify CCS package authority for {package_path}"))?;
    CcsPackage::from_verified_archive(package_path, &verified)
        .with_context(|| format!("open verified CCS package {package_path}"))
}

fn looks_like_package_file(package_path: &str) -> bool {
    Path::new(package_path).is_file()
}

fn print_ccs_scriptlet_bundle(package: &CcsPackage, options: &ScriptQueryOptions) -> Result<()> {
    let identity = PackageQueryIdentity {
        name: package.manifest().package.name.clone(),
        version: package.manifest().package.version.clone(),
    };
    let bundle = package.manifest().native_lifecycle.as_ref();

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
        bail!("--json and --entry are only available for CCS native lifecycle bundles");
    }

    let package: Box<dyn PackageFormat> = match format {
        PackageFormatType::Rpm => Box::new(RpmPackage::parse(package_path)?),
        PackageFormatType::Deb => Box::new(DebPackage::parse(package_path)?),
        PackageFormatType::Arch => Box::new(ArchPackage::parse(package_path)?),
    };

    let scriptlets = package.native_scriptlet_abi();

    if scriptlets.is_empty() {
        crate::ui::note(&format!(
            "{} v{} has no scriptlets",
            package.name(),
            package.version()
        ));
        return Ok(());
    }

    println!("Package: {} v{}", package.name(), package.version());
    println!("Scriptlets: {}", scriptlets.len());
    println!();

    for scriptlet in scriptlets {
        println!("=== {} ===", scriptlet.native_slot);
        println!("Lifecycle: {:?}", scriptlet.primary_lifecycle);
        if let Some(interpreter) = &scriptlet.interpreter {
            println!("Interpreter: {}", interpreter);
        }
        if !scriptlet.interpreter_args.is_empty() {
            println!("Arguments: {}", scriptlet.interpreter_args.join(" "));
        }
        println!("---");
        if let Some(text) = &scriptlet.body.text {
            for line in text.lines() {
                println!("{}", line);
            }
        } else {
            println!(
                "<binary body: {} bytes, {}>",
                scriptlet.body.bytes.len(),
                scriptlet.body.sha256
            );
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
    let ccs_remove_hook = InstalledCcsRemoveHook::find_by_trove(&conn, resolved.trove_id)?;
    let installed_bundle = InstalledNativeLifecycleBundle::find_by_trove(&conn, resolved.trove_id)?
        .map(|installed| {
            installed
                .bundle()
                .context("installed native lifecycle bundle cannot be decoded")
        })
        .transpose()?;

    let output = if options.json {
        render_installed_scripts_json(
            &resolved.trove,
            ccs_remove_hook.as_ref(),
            installed_bundle.as_ref(),
            options,
        )?
    } else {
        render_installed_scripts_text(
            &resolved.trove,
            ccs_remove_hook.as_ref(),
            installed_bundle.as_ref(),
            options,
        )?
    };
    println!("{output}");
    Ok(())
}

fn render_installed_scripts_text(
    trove: &Trove,
    ccs_remove_hook: Option<&InstalledCcsRemoveHook>,
    bundle: Option<&NativeLifecycleBundle>,
    options: &ScriptQueryOptions,
) -> Result<String> {
    let mut output = String::new();
    output.push_str(&format!(
        "Installed package: {} {} [{}]\n",
        trove.name,
        trove.version,
        trove.architecture.as_deref().unwrap_or("none")
    ));
    if let Some(hook) = ccs_remove_hook {
        output.push_str(&format!(
            "Installed CCS remove hook (installed_ccs_remove_hooks): present\n  pre-remove       source=installed_ccs_remove_hooks interpreter=/bin/sh reversible={}\n",
            hook.reversible
                .map(|value| value.to_string())
                .unwrap_or_else(|| "unspecified".to_string())
        ));
        if options.verbose {
            output.push_str(&format!(
                "    script_sha256={}\n",
                conary_core::hash::sha256_prefixed(hook.script.as_bytes())
            ));
        }
    } else {
        output.push_str("Installed CCS remove hook (installed_ccs_remove_hooks): none\n");
    }

    let Some(bundle) = bundle else {
        if let Some(entry_id) = &options.entry {
            bail!("native lifecycle bundle entry '{entry_id}' not found: package has no bundle");
        }
        output.push_str("Installed native lifecycle entries: 0\n");
        return Ok(output);
    };

    let entries = filtered_entries(bundle, options.entry.as_deref())?;
    output.push_str(&format!(
        "Installed native lifecycle entries: {}\n",
        bundle.entries.len()
    ));
    output.push_str(&format!("Native lifecycle bundle: {}\n", bundle.schema));
    output.push_str(&format!(
        "Fidelity: {}\n",
        bundle.scriptlet_fidelity.as_str()
    ));

    for entry in entries {
        output.push_str(&format!(
            "  {:<18} source=installed_native_lifecycle_bundles lifecycle={}\n",
            entry.id,
            entry.phase.as_str()
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
    ccs_remove_hook: Option<&InstalledCcsRemoveHook>,
    bundle: Option<&NativeLifecycleBundle>,
    options: &ScriptQueryOptions,
) -> Result<String> {
    let (bundle_summary, entries) = if let Some(bundle) = bundle {
        (
            Some(bundle_summary(bundle)),
            filtered_entries(bundle, options.entry.as_deref())?
                .into_iter()
                .map(entry_summary)
                .collect(),
        )
    } else {
        if let Some(entry_id) = &options.entry {
            bail!("native lifecycle bundle entry '{entry_id}' not found: package has no bundle");
        }
        (None, Vec::new())
    };

    let report = InstalledScriptQueryReport {
        package: InstalledPackageQueryIdentity {
            name: trove.name.clone(),
            version: trove.version.clone(),
            architecture: trove.architecture.clone(),
        },
        ccs_remove_hook: ccs_remove_hook.map(ccs_remove_hook_summary),
        bundle_present: bundle.is_some(),
        bundle: bundle_summary,
        entries,
    };

    serde_json::to_string_pretty(&report).context("failed to serialize installed script query JSON")
}

fn render_ccs_bundle_text(
    package: &PackageQueryIdentity,
    bundle: Option<&NativeLifecycleBundle>,
    options: &ScriptQueryOptions,
) -> Result<String> {
    let Some(bundle) = bundle else {
        if let Some(entry_id) = &options.entry {
            bail!("native lifecycle bundle entry '{entry_id}' not found: package has no bundle");
        }
        return Ok(format!(
            "Package: {} {}\nNo native lifecycle bundle found.\n",
            package.name, package.version
        ));
    };

    let entries = filtered_entries(bundle, options.entry.as_deref())?;
    let mut output = String::new();
    output.push_str(&format!("Package: {} {}\n", package.name, package.version));
    output.push_str(&format!("Native lifecycle bundle: {}\n", bundle.schema));
    output.push_str(&format!(
        "Source: {}{}{}{}\n",
        bundle.source_format.as_str(),
        optional_prefixed(" ", bundle.source_profile.as_deref()),
        optional_prefixed(" ", bundle.source_release.as_deref()),
        optional_prefixed(" ", bundle.source_arch.as_deref())
    ));
    output.push_str(&format!(
        "Fidelity: {}\n",
        bundle.scriptlet_fidelity.as_str()
    ));
    output.push_str(&format!("Native entries: {}\n", bundle.entries.len()));
    output.push('\n');

    if bundle.entries.is_empty() {
        output.push_str("No native lifecycle entries.\n");
        return Ok(output);
    }

    for entry in entries {
        output.push_str(&format!("{:<18} {:<16}\n", entry.id, entry.phase.as_str()));

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
        }
    }

    Ok(output)
}

fn render_ccs_bundle_json(
    package: &PackageQueryIdentity,
    bundle: Option<&NativeLifecycleBundle>,
    options: &ScriptQueryOptions,
) -> Result<String> {
    let (bundle_summary, entries) = if let Some(bundle) = bundle {
        (
            Some(bundle_summary(bundle)),
            filtered_entries(bundle, options.entry.as_deref())?
                .into_iter()
                .map(entry_summary)
                .collect(),
        )
    } else {
        if let Some(entry_id) = &options.entry {
            bail!("native lifecycle bundle entry '{entry_id}' not found: package has no bundle");
        }
        (None, Vec::new())
    };

    let report = ScriptQueryReport {
        package: package.clone(),
        bundle_present: bundle.is_some(),
        bundle: bundle_summary,
        entries,
    };

    serde_json::to_string_pretty(&report).context("failed to serialize script query JSON")
}

fn filtered_entries<'a>(
    bundle: &'a NativeLifecycleBundle,
    entry_id: Option<&str>,
) -> Result<Vec<&'a NativeLifecycleEntry>> {
    if let Some(entry_id) = entry_id {
        let entry = bundle
            .entries
            .iter()
            .find(|entry| entry.id == entry_id)
            .ok_or_else(|| {
                anyhow::anyhow!("native lifecycle bundle entry '{entry_id}' not found")
            })?;
        Ok(vec![entry])
    } else {
        Ok(bundle.entries.iter().collect())
    }
}

fn bundle_summary(bundle: &NativeLifecycleBundle) -> BundleQuerySummary {
    BundleQuerySummary {
        schema: bundle.schema.clone(),
        schema_revision: bundle.schema_revision,
        source_format: bundle.source_format.as_str().to_string(),
        source_family: bundle.source_family.clone(),
        source_profile: bundle.source_profile.clone(),
        source_release: bundle.source_release.clone(),
        source_arch: bundle.source_arch.clone(),
        source_package: bundle.source_package.clone(),
        source_version: bundle.source_version.clone(),
        scriptlet_fidelity: bundle.scriptlet_fidelity.as_str().to_string(),
        evidence_digest: bundle.evidence_digest.clone(),
    }
}

fn entry_summary(entry: &NativeLifecycleEntry) -> EntryQuerySummary {
    EntryQuerySummary {
        id: entry.id.clone(),
        native_slot: entry
            .native_slot
            .map(RpmScriptletSlot::as_str)
            .map(str::to_string),
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
        evidence_digest: entry.evidence_digest.clone(),
        source_evidence_refs: entry.source_evidence_refs.clone(),
        reserved_metadata: ReservedMetadataSummary {
            rpm_trigger: entry.rpm_trigger.is_some(),
            deb_maintainer: entry.deb_maintainer.is_some(),
            arch_install: entry.arch_install.is_some(),
            arch_hook: entry.arch_hook.is_some(),
            residual_lifecycle: entry.residual_lifecycle.is_some(),
        },
    }
}

fn ccs_remove_hook_summary(hook: &InstalledCcsRemoveHook) -> CcsRemoveHookQuerySummary {
    CcsRemoveHookQuerySummary {
        source: "installed_ccs_remove_hooks".to_string(),
        interpreter: "/bin/sh".to_string(),
        reversible: hook.reversible,
        script_sha256: conary_core::hash::sha256_prefixed(hook.script.as_bytes()),
    }
}

fn optional_prefixed(prefix: &str, value: Option<&str>) -> String {
    value
        .filter(|value| !value.is_empty())
        .map(|value| format!("{prefix}{value}"))
        .unwrap_or_default()
}

#[cfg(test)]
#[path = "scripts/tests.rs"]
mod tests;
