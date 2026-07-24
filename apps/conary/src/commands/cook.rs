// src/commands/cook.rs

//! Cook command - build packages from recipes

mod foreign_package;

use anyhow::{Context, Result};
use conary_core::ccs::manifest::ManifestProvenance;
use conary_core::diagnostics::{
    PACKAGING_JSON_SCHEMA_VERSION, PackagingArtifact, PackagingCommandOutput, PackagingDiagnostic,
    PackagingDiagnosticCode, PackagingEvent, PackagingEventKind, PackagingPhase,
};
use conary_core::recipe::CookResult;
use conary_core::recipe::hermetic::{DivergenceStatus, HermeticBuildInput, detect_ci_mode};
use conary_core::recipe::inference::{
    CookTarget, ResolvedSourceTree, SourceTargetKind, SourceTargetProvenance,
    infer_recipe_from_path, resolve_cook_target,
};
use conary_core::recipe::{
    InferenceOptions, InferenceTrace, Kitchen, KitchenConfig, Recipe, SourceDownloadPolicy,
    SourceSection, parse_recipe_file, validate_recipe,
};
use std::fs::File;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use tracing::info;

use foreign_package::{cook_foreign_package, foreign_package_format};

use super::hermetic_config::{ensure_no_build_dependencies_for_m2a, load_default_hermetic_builder};
use super::hermetic_state::{
    host_build_record_from_cook_result, load_latest_host_build_record_for_recipe,
    resolve_default_state_dir, write_host_build_record_to_dir,
};

pub(crate) fn recipe_source_base_dir(recipe_path: &Path) -> PathBuf {
    recipe_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

#[derive(Debug)]
struct ResolvedCookInput {
    recipe: Recipe,
    recipe_path: Option<PathBuf>,
    recipe_source_base_dir: PathBuf,
    origin_class_override: Option<String>,
    source_provenance_override: Option<SourceTargetProvenance>,
    inference_trace: Option<InferenceTrace>,
    source_kind: Option<SourceTargetKind>,
    _source_tree: Option<ResolvedSourceTree>,
}

struct CookRunOptions<'a> {
    target: Option<&'a str>,
    recipe: Option<&'a str>,
    output_dir: &'a str,
    source_cache: &'a str,
    jobs: Option<u32>,
    keep_builddir: bool,
    validate_only: bool,
    fetch_only: bool,
    explain: bool,
    isolated: bool,
    no_isolation: bool,
    hermetic: bool,
    json: bool,
    operation_id: String,
    source_download_policy_override: Option<SourceDownloadPolicy>,
    origin_class_override: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WatchCookSourcePolicy {
    Initial,
    Refresh,
}

pub(crate) struct CookForTryWatchOptions<'a> {
    pub(crate) target: Option<&'a str>,
    pub(crate) recipe: Option<&'a str>,
    pub(crate) output_dir: &'a str,
    pub(crate) source_cache: &'a str,
    pub(crate) jobs: Option<u32>,
    pub(crate) keep_builddir: bool,
    pub(crate) isolated: bool,
    pub(crate) no_isolation: bool,
    pub(crate) hermetic: bool,
    pub(crate) source_policy: WatchCookSourcePolicy,
    pub(crate) operation_id: String,
}

pub(crate) fn run_cook_for_try_watch(
    options: CookForTryWatchOptions<'_>,
) -> Result<PackagingCommandOutput> {
    let source_download_policy_override = watch_source_download_policy_override(&options);
    let mut sink = io::sink();
    run_cook_operation(
        CookRunOptions {
            target: options.target,
            recipe: options.recipe,
            output_dir: options.output_dir,
            source_cache: options.source_cache,
            jobs: options.jobs,
            keep_builddir: options.keep_builddir,
            validate_only: false,
            fetch_only: false,
            explain: false,
            isolated: options.isolated,
            no_isolation: options.no_isolation,
            hermetic: options.hermetic,
            json: true,
            operation_id: options.operation_id,
            source_download_policy_override,
            origin_class_override: None,
        },
        &mut sink,
    )
}

fn watch_source_download_policy_override(
    options: &CookForTryWatchOptions<'_>,
) -> Option<SourceDownloadPolicy> {
    let hermetic_requested = options.hermetic || options.isolated;
    if hermetic_requested && options.source_policy == WatchCookSourcePolicy::Refresh {
        Some(SourceDownloadPolicy::OfflineCacheOnly)
    } else {
        None
    }
}

pub(crate) fn cooked_artifact_path(output: &PackagingCommandOutput) -> Result<PathBuf> {
    let artifacts = output
        .artifacts
        .iter()
        .filter(|artifact| artifact.kind.as_deref() == Some("ccs"))
        .collect::<Vec<_>>();
    match artifacts.as_slice() {
        [artifact] => Ok(PathBuf::from(&artifact.path)),
        [] => anyhow::bail!("watch cook completed without a CCS artifact"),
        _ => anyhow::bail!("watch cook produced multiple CCS artifacts"),
    }
}

pub(crate) struct CookRecordedDraftOptions {
    pub(crate) recipe: PathBuf,
    pub(crate) output_dir: PathBuf,
    pub(crate) source_cache: PathBuf,
    pub(crate) operation_id: String,
}

fn recorded_draft_run_options<'a>(
    options: &'a CookRecordedDraftOptions,
    recipe: &'a str,
    output_dir: &'a str,
    source_cache: &'a str,
) -> CookRunOptions<'a> {
    CookRunOptions {
        target: Some(recipe),
        recipe: None,
        output_dir,
        source_cache,
        jobs: None,
        keep_builddir: false,
        validate_only: false,
        fetch_only: false,
        explain: false,
        isolated: true,
        no_isolation: false,
        hermetic: false,
        json: true,
        operation_id: options.operation_id.clone(),
        source_download_policy_override: None,
        origin_class_override: Some("recorded-draft".to_string()),
    }
}

pub(crate) fn run_cook_for_recorded_draft(
    options: CookRecordedDraftOptions,
) -> Result<PackagingCommandOutput> {
    let mut sink = io::sink();
    let recipe = options.recipe.to_string_lossy().to_string();
    let output_dir = options.output_dir.to_string_lossy().to_string();
    let source_cache = options.source_cache.to_string_lossy().to_string();
    run_cook_operation(
        recorded_draft_run_options(&options, &recipe, &output_dir, &source_cache),
        &mut sink,
    )
}

pub(crate) fn resolve_recipe_path(target: Option<&str>, recipe: Option<&str>) -> Result<PathBuf> {
    match resolve_cook_target(target, recipe)? {
        CookTarget::RecipeFile(recipe_path) => Ok(recipe_path),
        CookTarget::SourceTree(_) => {
            anyhow::bail!(
                "Expected a recipe file path, but the cook target resolved to a source tree"
            )
        }
    }
}

fn resolve_cook_input(target: Option<&str>, recipe: Option<&str>) -> Result<ResolvedCookInput> {
    match resolve_cook_target(target, recipe)? {
        CookTarget::RecipeFile(recipe_path) => {
            let parsed = parse_recipe_file(&recipe_path)
                .with_context(|| format!("Failed to parse recipe: {}", recipe_path.display()))?;
            Ok(ResolvedCookInput {
                recipe: parsed,
                recipe_source_base_dir: recipe_source_base_dir(&recipe_path),
                recipe_path: Some(recipe_path),
                origin_class_override: None,
                source_provenance_override: None,
                inference_trace: None,
                source_kind: None,
                _source_tree: None,
            })
        }
        CookTarget::SourceTree(source_tree) => {
            let inference = infer_recipe_from_path(
                &source_tree.root,
                InferenceOptions::for_source_root(source_tree.root.clone()),
            )
            .with_context(|| {
                format!(
                    "Failed to infer recipe from source tree: {}",
                    source_tree.root.display()
                )
            })?;
            Ok(ResolvedCookInput {
                recipe: inference.recipe,
                recipe_path: None,
                recipe_source_base_dir: source_tree.root.clone(),
                origin_class_override: Some("inferred-source".to_string()),
                source_provenance_override: Some(source_tree.provenance.clone()),
                inference_trace: Some(inference.trace),
                source_kind: Some(source_tree.kind),
                _source_tree: Some(source_tree),
            })
        }
    }
}

fn write_inference_trace(output: &mut impl Write, trace: &InferenceTrace) -> Result<()> {
    writeln!(output, "Inference trace:")?;
    let rendered = trace.render_human();
    if rendered.is_empty() {
        writeln!(output, "  (empty)")?;
    } else {
        for line in rendered.lines() {
            writeln!(output, "  {line}")?;
        }
    }
    Ok(())
}

fn cook_operation_id() -> String {
    super::operation_records::new_operation_id("cook")
}

fn cook_failure_output(operation_id: &str, error: &anyhow::Error) -> PackagingCommandOutput {
    let code = cook_error_code(error);
    let diagnostic = PackagingDiagnostic::error(PackagingPhase::Build, code, error.to_string());
    let mut output = PackagingCommandOutput::failed(
        operation_id.to_string(),
        "conary cook",
        vec![diagnostic.clone()],
    );
    let mut sequence = 0;
    push_cook_event(
        &mut output,
        &mut sequence,
        PackagingPhase::Build,
        PackagingEventKind::OperationStarted,
        "Cook operation started",
    );
    sequence += 1;
    output.events.push(PackagingEvent::diagnostic(
        operation_id,
        sequence,
        diagnostic,
    ));
    push_cook_event(
        &mut output,
        &mut sequence,
        PackagingPhase::Build,
        PackagingEventKind::OperationFinished,
        "Cook operation failed",
    );
    output
}

fn cook_success_output(operation_id: &str, summary: impl Into<String>) -> PackagingCommandOutput {
    let mut output = PackagingCommandOutput::succeeded(operation_id.to_string(), "conary cook");
    output.summary = Some(summary.into());
    output
}

fn push_cook_event(
    report: &mut PackagingCommandOutput,
    sequence: &mut u64,
    phase: PackagingPhase,
    kind: PackagingEventKind,
    message: impl Into<String>,
) {
    *sequence += 1;
    report.events.push(PackagingEvent {
        schema_version: PACKAGING_JSON_SCHEMA_VERSION,
        operation_id: report.operation_id.clone(),
        sequence: *sequence,
        phase,
        kind,
        message: Some(message.into()),
        diagnostic: None,
        artifact: None,
        progress: None,
    });
}

fn cook_error_code(error: &anyhow::Error) -> PackagingDiagnosticCode {
    let message = format!("{error:#}").to_ascii_lowercase();
    if message.contains("source cache") || message.contains("cache miss") {
        PackagingDiagnosticCode::SourceCacheMiss
    } else if message.contains("network") || message.contains("offline") {
        PackagingDiagnosticCode::BuildNetworkAccess
    } else if message.contains("unpinned") || message.contains("content lock") {
        PackagingDiagnosticCode::UnpinnedDependency
    } else if message.contains("command risk") || message.contains("risk report") {
        PackagingDiagnosticCode::CommandRiskEvidence
    } else {
        PackagingDiagnosticCode::CookFailed
    }
}

fn sha256_prefixed_file(path: &Path) -> Result<String> {
    let mut file = File::open(path)
        .with_context(|| format!("Failed to open recipe for hashing: {}", path.display()))?;
    let hash = conary_core::hash::sha256_reader_hex(&mut file)
        .with_context(|| format!("Failed to hash recipe: {}", path.display()))?;
    Ok(format!("sha256:{hash}"))
}

fn hermetic_build_input(
    resolved: &ResolvedCookInput,
    recipe: &Recipe,
) -> Result<HermeticBuildInput> {
    if let Some(recipe_path) = &resolved.recipe_path {
        return Ok(HermeticBuildInput::explicit_recipe(
            &resolved.recipe_source_base_dir,
            recipe_path,
            sha256_prefixed_file(recipe_path)?,
        ));
    }

    let trace = resolved.inference_trace.as_ref().with_context(
        || "Hermetic cook requires an explicit recipe or an inference trace for generated recipes",
    )?;
    let inference_trace_hash = conary_core::hash::sha256_prefixed(trace.render_human().as_bytes());
    Ok(HermeticBuildInput::generated_recipe(
        &resolved.recipe_source_base_dir,
        recipe.clone(),
        inference_trace_hash,
    ))
}

/// Cook a package from a recipe
///
/// # Arguments
/// * `target` - Optional recipe path or directory containing recipe.toml
/// * `recipe` - Optional explicit recipe path. Wins over target when present.
/// * `output_dir` - Output directory for the built package
/// * `source_cache` - Directory for caching downloaded sources
/// * `jobs` - Number of parallel build jobs (None = auto)
/// * `keep_builddir` - Keep build directory after completion
/// * `validate_only` - Only validate the recipe, don't cook
/// * `fetch_only` - Only fetch sources, don't build
/// * `explain` - Print inference trace for inferred source trees
/// * `isolated` - Use the hermetic sandboxed isolation path
/// * `no_isolation` - Hidden compatibility no-op for the M1a host default
/// * `hermetic` - Hidden compatibility flag for the M2a hermetic build path
/// * `json` - Emit structured packaging JSON output
#[allow(clippy::too_many_arguments)]
pub async fn cmd_cook(
    target: Option<&str>,
    recipe: Option<&str>,
    output_dir: &str,
    source_cache: &str,
    jobs: Option<u32>,
    keep_builddir: bool,
    validate_only: bool,
    fetch_only: bool,
    explain: bool,
    isolated: bool,
    no_isolation: bool,
    hermetic: bool,
    json: bool,
) -> Result<()> {
    let mut output = io::stdout();
    cmd_cook_with_output(
        target,
        recipe,
        output_dir,
        source_cache,
        jobs,
        keep_builddir,
        validate_only,
        fetch_only,
        explain,
        isolated,
        no_isolation,
        hermetic,
        json,
        &mut output,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn cmd_cook_with_output(
    target: Option<&str>,
    recipe: Option<&str>,
    output_dir: &str,
    source_cache: &str,
    jobs: Option<u32>,
    keep_builddir: bool,
    validate_only: bool,
    fetch_only: bool,
    explain: bool,
    isolated: bool,
    no_isolation: bool,
    hermetic: bool,
    json: bool,
    output: &mut impl Write,
) -> Result<()> {
    let operation_id = cook_operation_id();
    let options = CookRunOptions {
        target,
        recipe,
        output_dir,
        source_cache,
        jobs,
        keep_builddir,
        validate_only,
        fetch_only,
        explain,
        isolated,
        no_isolation,
        hermetic,
        json,
        operation_id: operation_id.clone(),
        source_download_policy_override: None,
        origin_class_override: None,
    };
    match run_cook_operation(options, output) {
        Ok(mut report) => {
            report.operation_id = operation_id.clone();
            if json {
                super::diagnostics::write_packaging_output(&report, true, output)?;
            }
            super::diagnostics::write_packaging_record_if_possible(&report);
            Ok(())
        }
        Err(error) => {
            let report = cook_failure_output(&operation_id, &error);
            if json {
                super::diagnostics::write_packaging_output(&report, true, output)?;
            }
            super::diagnostics::write_packaging_record_if_possible(&report);
            Err(error)
        }
    }
}

fn run_cook_operation(
    options: CookRunOptions<'_>,
    output: &mut impl Write,
) -> Result<PackagingCommandOutput> {
    let hermetic_requested = options.hermetic || options.isolated;
    if hermetic_requested && options.no_isolation {
        anyhow::bail!("--no-isolation conflicts with --isolated/--hermetic");
    }

    if options.recipe.is_none()
        && let Some(target) = options.target
    {
        let target_path = Path::new(target);
        if foreign_package_format(target_path).is_some() {
            if options.json {
                let mut sink = io::sink();
                cook_foreign_package(target_path, Path::new(options.output_dir), &mut sink)?;
            } else {
                cook_foreign_package(target_path, Path::new(options.output_dir), output)?;
            }
            return Ok(cook_success_output(
                &options.operation_id,
                "Foreign package converted",
            ));
        }
    }

    let resolved = resolve_cook_input(options.target, options.recipe)?;
    let output_dir = Path::new(options.output_dir);
    let recipe = resolved.recipe.clone();

    if !options.json {
        if let Some(recipe_path) = &resolved.recipe_path {
            writeln!(output, "Reading recipe: {}", recipe_path.display())?;
        } else {
            writeln!(
                output,
                "Inferring recipe from: {}",
                resolved.recipe_source_base_dir.display()
            )?;
        }

        writeln!(
            output,
            "Recipe: {} version {}",
            recipe.package.name, recipe.package.version
        )?;

        if options.explain
            && let Some(trace) = &resolved.inference_trace
        {
            write_inference_trace(output, trace)?;
        }
    }

    // Validate the recipe
    let warnings = validate_recipe(&recipe).with_context(|| "Recipe validation failed")?;

    if !options.json {
        for warning in &warnings {
            writeln!(output, "{}", crate::ui::warn_line(warning))?;
        }
    }

    if options.validate_only {
        let mut report = cook_success_output(&options.operation_id, "Recipe validation passed");
        let mut sequence = 0;
        push_cook_event(
            &mut report,
            &mut sequence,
            PackagingPhase::RecipeValidation,
            PackagingEventKind::OperationStarted,
            "Cook operation started",
        );
        push_cook_event(
            &mut report,
            &mut sequence,
            PackagingPhase::RecipeValidation,
            PackagingEventKind::PhaseStarted,
            "Recipe validation started",
        );
        for warning in &warnings {
            report.diagnostics.push(PackagingDiagnostic::warning(
                PackagingPhase::RecipeValidation,
                PackagingDiagnosticCode::RecipeValidationWarning,
                warning.to_string(),
            ));
        }
        for diagnostic in &report.diagnostics {
            sequence += 1;
            report.events.push(PackagingEvent::diagnostic(
                options.operation_id.as_str(),
                sequence,
                diagnostic.clone(),
            ));
        }
        push_cook_event(
            &mut report,
            &mut sequence,
            PackagingPhase::RecipeValidation,
            PackagingEventKind::PhaseFinished,
            "Recipe validation finished",
        );
        push_cook_event(
            &mut report,
            &mut sequence,
            PackagingPhase::RecipeValidation,
            PackagingEventKind::OperationFinished,
            "Cook operation finished",
        );
        if !options.json {
            writeln!(output, "Recipe validation passed")?;
            if warnings.is_empty() {
                writeln!(
                    output,
                    "{}",
                    crate::ui::status_line("OK", "No issues found")
                )?;
            } else {
                writeln!(
                    output,
                    "{}",
                    crate::ui::status_line("OK", &format!("{} warning(s)", warnings.len()))
                )?;
            }
        }
        return Ok(report);
    }

    // Configure the kitchen. Host builds remain the compatibility default;
    // --isolated and --hermetic route through the M2a hermetic planner.
    let mut config = KitchenConfig {
        source_cache: PathBuf::from(options.source_cache),
        recipe_source_base_dir: Some(resolved.recipe_source_base_dir.clone()),
        origin_class_override: options
            .origin_class_override
            .clone()
            .or_else(|| resolved.origin_class_override.clone()),
        source_provenance_override: resolved.source_provenance_override.clone(),
        keep_builddir: options.keep_builddir,
        use_isolation: false,
        pristine_mode: false,
        ..Default::default()
    };

    if let Some(j) = options.jobs {
        config.jobs = j;
    }
    if !hermetic_requested {
        add_host_iteration_env(&mut config);
    }
    if let Some(policy) = options.source_download_policy_override {
        config.source_download_policy = policy;
    }

    // Fetch-only mode: just download sources and exit
    if options.fetch_only {
        let kitchen = Kitchen::new(config.clone());
        if matches!(resolved.source_kind, Some(SourceTargetKind::Directory))
            && matches!(recipe.source, SourceSection::Local(_))
        {
            if !options.json {
                writeln!(
                    output,
                    "No remote source fetch is required for inferred local source tree."
                )?;
            }
            let mut report =
                cook_success_output(&options.operation_id, "No remote source fetch is required");
            let mut sequence = 0;
            push_cook_event(
                &mut report,
                &mut sequence,
                PackagingPhase::SourceFetch,
                PackagingEventKind::OperationStarted,
                "Cook operation started",
            );
            push_cook_event(
                &mut report,
                &mut sequence,
                PackagingPhase::SourceFetch,
                PackagingEventKind::PhaseStarted,
                "Source fetch started",
            );
            push_cook_event(
                &mut report,
                &mut sequence,
                PackagingPhase::SourceFetch,
                PackagingEventKind::PhaseFinished,
                "Source fetch finished",
            );
            push_cook_event(
                &mut report,
                &mut sequence,
                PackagingPhase::SourceFetch,
                PackagingEventKind::OperationFinished,
                "Cook operation finished",
            );
            return Ok(report);
        }

        if !options.json {
            writeln!(output, "Fetching sources (fetch-only mode)...")?;
        }
        let sources = kitchen
            .fetch(&recipe)
            .with_context(|| format!("Failed to fetch sources for {}", recipe.package.name))?;

        if !options.json {
            writeln!(
                output,
                "\n{}",
                crate::ui::status_line("Fetched", &format!("{} source file(s):", sources.len()))
            )?;
            for source in &sources {
                writeln!(output, "  - {}", source.display())?;
            }

            if kitchen.sources_cached(&recipe) {
                writeln!(
                    output,
                    "\n{}",
                    crate::ui::status_line("Cached", "all sources. Ready for offline build.")
                )?;
            }
        }

        let mut report = cook_success_output(
            &options.operation_id,
            format!("Fetched {} source file(s)", sources.len()),
        );
        let mut sequence = 0;
        push_cook_event(
            &mut report,
            &mut sequence,
            PackagingPhase::SourceFetch,
            PackagingEventKind::OperationStarted,
            "Cook operation started",
        );
        push_cook_event(
            &mut report,
            &mut sequence,
            PackagingPhase::SourceFetch,
            PackagingEventKind::PhaseStarted,
            "Source fetch started",
        );
        push_cook_event(
            &mut report,
            &mut sequence,
            PackagingPhase::SourceFetch,
            PackagingEventKind::PhaseFinished,
            "Source fetch finished",
        );
        push_cook_event(
            &mut report,
            &mut sequence,
            PackagingPhase::SourceFetch,
            PackagingEventKind::OperationFinished,
            "Cook operation finished",
        );
        return Ok(report);
    }

    let hermetic_builder = if hermetic_requested {
        let builder = load_default_hermetic_builder()?;
        ensure_no_build_dependencies_for_m2a(&recipe)?;
        config.use_isolation = true;
        config.pristine_mode = true;
        config.sysroot = Some(builder.sysroot_path.clone());
        config.auto_makedepends = false;
        config.cleanup_makedepends = false;
        configure_host_record_for_hermetic(&mut config, &recipe);
        Some(builder)
    } else {
        None
    };

    let kitchen = Kitchen::new(config.clone());

    // Create output directory if needed
    std::fs::create_dir_all(output_dir).with_context(|| {
        format!(
            "Failed to create output directory: {}",
            output_dir.display()
        )
    })?;

    if !options.json {
        // Print mode information
        if hermetic_requested {
            writeln!(
                output,
                "Cooking with {} parallel jobs (hermetic)...",
                config.jobs
            )?;
            writeln!(output, "  - Sources prefetched before build")?;
            writeln!(output, "  - Network disabled during build")?;
            writeln!(
                output,
                "  - Build evidence recorded without M2b attestation"
            )?;
        } else {
            writeln!(
                output,
                "Cooking with {} parallel jobs (host)...",
                config.jobs
            )?;
        }

        // Check if sources are cached
        if kitchen.sources_cached(&recipe) {
            writeln!(
                output,
                "  - Sources already cached (offline build possible)"
            )?;
        } else {
            writeln!(output, "Fetching source...")?;
        }

        writeln!(output, "Configuring...")?;
        writeln!(output, "Building ({} parallel jobs)...", config.jobs)?;
    }

    // Create kitchen and cook
    let result = if let Some(builder) = hermetic_builder {
        let input =
            hermetic_build_input(&resolved, &recipe)?.with_builder_environment(builder.identity);
        kitchen.cook_hermetic(&recipe, input, output_dir, detect_ci_mode())
    } else {
        kitchen.cook(&recipe, output_dir)
    }
    .with_context(|| format!("Failed to cook {}", recipe.package.name))?;

    if !options.json {
        writeln!(output, "Installing to staging...")?;

        writeln!(
            output,
            "\n{}",
            crate::ui::status_line("Cooked", &result.package_path.display().to_string())
        )?;

        if !result.warnings.is_empty() {
            writeln!(output, "\nBuild warnings:")?;
            for warning in &result.warnings {
                writeln!(output, "  - {}", warning)?;
            }
        }
        if hermetic_requested {
            print_divergence_summary(output, result.provenance.as_ref())?;
        } else {
            write_host_record_after_host_cook(output, &recipe, &result)?;
        }
    }

    info!(
        "Successfully cooked {} to {}",
        recipe.package.name,
        result.package_path.display()
    );

    let mut report = cook_success_output(&options.operation_id, "Cooked package");
    let mut sequence = 0;
    push_cook_event(
        &mut report,
        &mut sequence,
        PackagingPhase::Build,
        PackagingEventKind::OperationStarted,
        "Cook operation started",
    );
    push_cook_event(
        &mut report,
        &mut sequence,
        PackagingPhase::Build,
        PackagingEventKind::PhaseStarted,
        "Build started",
    );
    report.artifacts.push(PackagingArtifact {
        path: result.package_path.display().to_string(),
        kind: Some("ccs".to_string()),
    });
    push_cook_event(
        &mut report,
        &mut sequence,
        PackagingPhase::Build,
        PackagingEventKind::ArtifactCreated,
        "Cooked artifact created",
    );
    push_cook_event(
        &mut report,
        &mut sequence,
        PackagingPhase::Build,
        PackagingEventKind::PhaseFinished,
        "Build finished",
    );
    push_cook_event(
        &mut report,
        &mut sequence,
        PackagingPhase::Build,
        PackagingEventKind::OperationFinished,
        "Cook operation finished",
    );
    Ok(report)
}

fn configure_host_record_for_hermetic(config: &mut KitchenConfig, recipe: &Recipe) {
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

fn add_host_iteration_env(config: &mut KitchenConfig) {
    for key in ["PATH", "HOME", "CARGO_HOME", "RUSTUP_HOME"] {
        if let Ok(value) = std::env::var(key) {
            config.extra_env.push((key.to_string(), value));
        }
    }

    config
        .extra_env
        .push(("CARGO_TARGET_DIR".to_string(), "target".to_string()));
}

fn write_host_record_after_host_cook(
    output: &mut impl Write,
    recipe: &Recipe,
    result: &CookResult,
) -> Result<()> {
    if skip_default_host_record_write_in_unit_tests() {
        return Ok(());
    }
    let Some(record) = host_build_record_from_cook_result(recipe, result) else {
        return Ok(());
    };
    match resolve_default_state_dir()
        .and_then(|state_dir| write_host_build_record_to_dir(&state_dir, &record))
    {
        Ok(_) => {}
        Err(error) => {
            writeln!(
                output,
                "{}",
                crate::ui::warn_line(&format!(
                    "could not write hermetic host build record: {error}"
                ))
            )?;
        }
    }
    Ok(())
}

fn skip_default_host_record_write_in_unit_tests() -> bool {
    cfg!(test) && std::env::var_os("CONARY_HERMETIC_STATE_DIR").is_none()
}

fn print_divergence_summary(
    output: &mut impl Write,
    provenance: Option<&ManifestProvenance>,
) -> Result<()> {
    let Some(evidence) = provenance.and_then(|provenance| provenance.hermetic_evidence.as_ref())
    else {
        return Ok(());
    };
    if evidence.divergence.status == DivergenceStatus::DiffersFromHost {
        writeln!(
            output,
            "{}",
            crate::ui::warn_line(
                "hermetic output differs from the latest host build record; this is diagnostic-only in M2a."
            )
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
