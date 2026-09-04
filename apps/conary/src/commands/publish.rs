// apps/conary/src/commands/publish.rs

//! Publish command - build a recipe project and publish it to a static repo.

mod artifact;
mod target;

use artifact::publish_artifact_form;
pub(crate) use artifact::publish_static_artifact_form_service;
use std::env;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
pub(crate) use target::{PublishDestination, RemiReleaseDestination};

use anyhow::{Context, Result, bail};
use conary_core::ccs::manifest::ManifestProvenance;
use conary_core::diagnostics::{
    DiagnosticEvidence, PackagingArtifact, PackagingCommandOutput, PackagingCommandStatus,
    PackagingDiagnostic, PackagingDiagnosticCode, PackagingEvent, PackagingEventKind,
    PackagingPhase,
};
use conary_core::recipe::Recipe;
use conary_core::recipe::hermetic::{CiMode, DivergenceStatus, HermeticBuildInput};
use conary_core::recipe::{
    CcsPackageSigningAuthority, Kitchen, KitchenConfig, SourceDownloadPolicy, parse_recipe_file,
    validate_recipe,
};
use conary_core::repository::static_repo::RepoLocation;
use conary_core::repository::static_repo::publish::{
    StaticPublishOptions, prepare_static_key_dir, publish_static_repo,
};
use conary_core::repository::static_repo::publish_context::{
    ProjectFormAttestationInput, attach_project_form_attestation,
    prepare_artifact_form_static_context, prepare_project_form_static_context,
};
use conary_core::repository::static_repo::publish_gate::{
    PublishLintReport, format_publish_gate_failures, verify_static_artifact_publish_eligibility,
};

use super::cook::{recipe_source_base_dir, resolve_recipe_path};
use super::hermetic_config::{ensure_no_build_dependencies_for_m2a, load_default_hermetic_builder};
use super::hermetic_state::{load_latest_host_build_record_for_recipe, resolve_default_state_dir};
use super::remi_publish::{RemiPublishOptions, publish_to_remi, resolve_remi_publish_bearer_token};

pub struct PublishOptions {
    pub what: String,
    pub target: Option<String>,
    pub recipe: Option<String>,
    pub key_dir: Option<String>,
    pub state_file: Option<String>,
    pub refresh: bool,
    pub rotate_publish_key: bool,
    pub rotate_root_key: bool,
    pub yes: bool,
    pub json: bool,
}

pub(crate) struct StaticArtifactPublishServiceInput {
    pub artifact_path: PathBuf,
    pub destination: RepoLocation,
    pub key_dir: Option<PathBuf>,
    pub state_file: Option<PathBuf>,
    pub refresh: bool,
    pub rotate_publish_key: bool,
    pub rotate_root_key: bool,
    pub operation_id: String,
}

pub async fn cmd_publish(options: PublishOptions) -> Result<()> {
    let mut stdout = std::io::stdout();
    cmd_publish_with_output(options, &mut stdout).await
}

pub(crate) async fn cmd_publish_with_output(
    options: PublishOptions,
    writer: &mut impl Write,
) -> Result<()> {
    if let Some(target) = options.target.clone() {
        publish_artifact_form(options, &target, writer).await
    } else {
        publish_project_form(options, writer)
    }
}

fn publish_operation_id() -> String {
    super::operation_records::new_operation_id("publish")
}

fn publish_failure_output(
    operation_id: &str,
    code: PackagingDiagnosticCode,
    message: impl Into<String>,
) -> PackagingCommandOutput {
    PackagingCommandOutput::failed(
        operation_id.to_string(),
        "conary publish",
        vec![PackagingDiagnostic::error(
            PackagingPhase::Publish,
            code,
            message,
        )],
    )
}

fn publish_success_output(
    operation_id: &str,
    summary: impl Into<String>,
) -> PackagingCommandOutput {
    let mut output = PackagingCommandOutput::succeeded(operation_id.to_string(), "conary publish");
    output.summary = Some(summary.into());
    output
}

fn publish_gate_failure_output(
    operation_id: &str,
    report: &PublishLintReport,
) -> PackagingCommandOutput {
    let code = report
        .failures
        .first()
        .map(|failure| super::diagnostics::publish_gate_code_to_diagnostic_code(failure.code))
        .unwrap_or(PackagingDiagnosticCode::PublishGateFailed);
    let mut diagnostic = PackagingDiagnostic::error(
        PackagingPhase::Publish,
        code,
        format_publish_gate_failures(report),
    );
    let report_value = serde_json::to_value(report)
        .unwrap_or_else(|error| serde_json::json!({ "serialization_error": error.to_string() }));
    diagnostic.evidence.push(
        DiagnosticEvidence::log("publish-gate", "Static artifact publish gate failed")
            .with_metadata("publish_lint_report", report_value),
    );
    PackagingCommandOutput::failed(operation_id, "conary publish", vec![diagnostic])
}

fn publish_project_form(options: PublishOptions, writer: &mut impl Write) -> Result<()> {
    let operation_id = publish_operation_id();
    match run_project_form_publish(&options, &operation_id, writer) {
        Ok(report) => {
            if options.json {
                super::diagnostics::write_packaging_output(&report, true, writer)?;
            }
            super::diagnostics::write_packaging_record_if_possible(&report);
            Ok(())
        }
        Err(error) => {
            let output = publish_failure_output(
                &operation_id,
                PackagingDiagnosticCode::ProjectPublishPreflightFailed,
                error.to_string(),
            );
            if options.json {
                super::diagnostics::write_packaging_output(&output, true, writer)?;
            }
            super::diagnostics::write_packaging_record_if_possible(&output);
            Err(error)
        }
    }
}

fn run_project_form_publish(
    options: &PublishOptions,
    operation_id: &str,
    writer: &mut impl Write,
) -> Result<PackagingCommandOutput> {
    let destination = RepoLocation::parse(&options.what)
        .with_context(|| format!("parse static repo destination {}", options.what))?;
    ensure_static_local_publish_destination(&destination)?;
    let repo_name = derive_repo_name(&destination, &options.what)?;
    let key_dir = resolve_key_dir(options.key_dir.as_deref(), &repo_name)?;
    let state_file = options
        .state_file
        .as_deref()
        .map(PathBuf::from)
        .unwrap_or_else(|| key_dir.join("last-published.toml"));
    let recipe_path = resolve_recipe_path(None, options.recipe.as_deref())?;
    // Parsed for future interactive confirmation; M1a publish is non-interactive.
    let _ = options.yes;

    if !options.json {
        writeln!(writer, "Reading recipe: {}", recipe_path.display())?;
    }
    let recipe = parse_recipe_file(&recipe_path)
        .with_context(|| format!("Failed to parse recipe: {}", recipe_path.display()))?;
    let warnings = validate_recipe(&recipe).with_context(|| "Recipe validation failed")?;
    if !options.json {
        for warning in &warnings {
            writeln!(writer, "{}", crate::ui::warn_line(warning))?;
        }
    }
    let prepared = prepare_project_form_static_context(&destination, &key_dir)
        .with_context(|| format!("prepare static publish context for {}", repo_name))?;

    let builder = load_default_hermetic_builder()?;
    ensure_no_build_dependencies_for_m2a(&recipe)?;

    let output_dir = tempfile::tempdir().context("create temporary publish output directory")?;
    let builder_identity = builder.identity;
    let builder_sysroot = builder.sysroot_path;
    let hermetic_input = HermeticBuildInput::explicit_recipe(
        recipe_source_base_dir(&recipe_path),
        recipe_path.clone(),
        sha256_prefixed_file(&recipe_path)?,
    )
    .with_builder_environment(builder_identity);
    let mut config = publish_kitchen_config(&recipe_path, output_dir.path(), builder_sysroot);
    config.ccs_signing_authority = Some(CcsPackageSigningAuthority::from_key_pair(
        &prepared.active_publish_key,
    ));
    configure_host_record_for_publish(&mut config, &recipe);
    let kitchen = Kitchen::new(config);

    if !options.json {
        writeln!(
            writer,
            "Cooking and attesting {} {} for static release publish...",
            recipe.package.name, recipe.package.version
        )?;
    }

    let result = kitchen
        .cook_hermetic(
            &recipe,
            hermetic_input,
            output_dir.path(),
            release_publish_ci_mode(),
        )
        .with_context(|| format!("Failed to hermetically cook {}", recipe.package.name))?;
    if !options.json {
        print_divergence_summary(writer, result.provenance.as_ref())?;
    }
    let attested_package_path = attach_project_form_attestation(ProjectFormAttestationInput {
        package_path: &result.package_path,
        provenance: result
            .provenance
            .as_ref()
            .context("project-form publish requires hermetic provenance")?,
        context: &prepared,
        conary_version: env!("CARGO_PKG_VERSION"),
    })?;

    let outcome = publish_static_repo(StaticPublishOptions {
        repo_name: repo_name.clone(),
        repo_description: None,
        destination,
        key_dir,
        state_file,
        package_paths: vec![attested_package_path.clone()],
        refresh: options.refresh,
        rotate_publish_key: options.rotate_publish_key,
        rotate_root_key: options.rotate_root_key,
        artifact_gate_context: None,
    })
    .with_context(|| format!("publish static repo {}", repo_name))?;

    if !options.json {
        writeln!(writer, "Published static repo: {repo_name}")?;
        writeln!(
            writer,
            "Root fingerprint(s): {}",
            outcome.root_key_ids.join(", ")
        )?;
        writeln!(writer, "Publish key ID: {}", outcome.publish_key_id)?;
        writeln!(
            writer,
            "Versions: root={} targets={} snapshot={} timestamp={}",
            outcome.root_version,
            outcome.targets_version,
            outcome.snapshot_version,
            outcome.timestamp_version
        )?;
        writeln!(writer, "Packages: {}", outcome.package_count)?;
        if !outcome.preview_warning.is_empty() {
            writeln!(writer, "{}", outcome.preview_warning)?;
        }
    }

    let mut output =
        publish_success_output(operation_id, format!("Published static repo: {repo_name}"));
    output.artifacts.push(PackagingArtifact {
        path: attested_package_path.display().to_string(),
        kind: Some("ccs".to_string()),
    });
    Ok(output)
}

fn publish_kitchen_config(
    recipe_path: &Path,
    output_dir: &Path,
    sysroot: PathBuf,
) -> KitchenConfig {
    KitchenConfig {
        source_cache: output_dir.join("sources"),
        recipe_source_base_dir: Some(recipe_source_base_dir(recipe_path)),
        allow_network: false,
        use_isolation: true,
        pristine_mode: true,
        sysroot: Some(sysroot),
        source_download_policy: SourceDownloadPolicy::AllowDownloads,
        ..Default::default()
    }
}

fn release_publish_ci_mode() -> CiMode {
    CiMode::On
}

fn sha256_prefixed_file(path: &Path) -> Result<String> {
    let mut file = File::open(path)
        .with_context(|| format!("Failed to open recipe for hashing: {}", path.display()))?;
    let hash = conary_core::hash::sha256_reader_hex(&mut file)
        .with_context(|| format!("Failed to hash recipe: {}", path.display()))?;
    Ok(format!("sha256:{hash}"))
}

fn configure_host_record_for_publish(config: &mut KitchenConfig, recipe: &Recipe) {
    let architecture = Some(std::env::consts::ARCH);
    match resolve_default_state_dir() {
        Ok(state_dir) => {
            let lookup = load_latest_host_build_record_for_recipe(&state_dir, recipe, architecture);
            config.expected_host_build_record = lookup.record;
            config.host_build_record_diagnostics = lookup.diagnostics;
        }
        Err(error) => {
            config.host_build_record_diagnostics = vec![format!(
                "failed to resolve hermetic host record state directory: {error}"
            )];
        }
    }
}

fn print_divergence_summary(
    writer: &mut impl Write,
    provenance: Option<&ManifestProvenance>,
) -> Result<()> {
    let Some(evidence) = provenance.and_then(|provenance| provenance.hermetic_evidence.as_ref())
    else {
        return Ok(());
    };
    if evidence.divergence.status == DivergenceStatus::DiffersFromHost {
        writeln!(
            writer,
            "{}",
            crate::ui::warn_line(
                "hermetic output differs from the latest host build record; this is diagnostic-only in M2a."
            )
        )?;
    }
    Ok(())
}

fn ensure_static_local_publish_destination(destination: &RepoLocation) -> Result<()> {
    if matches!(destination, RepoLocation::Http { .. }) {
        bail!(
            "static publisher supports local filesystem destinations; Remi HTTP(S) targets use the Remi release path"
        );
    }

    Ok(())
}

fn derive_repo_name(destination: &RepoLocation, display: &str) -> Result<String> {
    let repo_name = match destination {
        RepoLocation::File { root } => root.file_name().map(|name| name.to_owned()),
        RepoLocation::Http { base } => base
            .rsplit('/')
            .find(|segment| !segment.is_empty())
            .map(std::ffi::OsString::from),
    };

    let repo_name = repo_name
        .and_then(|name| name.into_string().ok())
        .filter(|name| !name.trim().is_empty())
        .with_context(|| format!("derive static repo name from destination {display}"))?;

    Ok(repo_name)
}

fn resolve_key_dir(key_dir: Option<&str>, repo_name: &str) -> Result<PathBuf> {
    if let Some(key_dir) = key_dir {
        return Ok(PathBuf::from(key_dir));
    }

    prepare_static_key_dir(&config_base_dir()?.join("conary").join("keys"), repo_name)
}

fn config_base_dir() -> Result<PathBuf> {
    if let Some(config_home) = env::var_os("XDG_CONFIG_HOME") {
        return Ok(PathBuf::from(config_home));
    }

    if let Some(home) = env::var_os("HOME") {
        return Ok(PathBuf::from(home).join(".config"));
    }

    bail!("cannot determine config directory; set XDG_CONFIG_HOME or HOME")
}

#[cfg(test)]
#[path = "publish/tests.rs"]
mod tests;
