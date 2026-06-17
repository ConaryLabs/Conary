// src/commands/cook.rs

//! Cook command - build packages from recipes

use anyhow::{Context, Result};
use conary_core::ccs::convert::{ConversionOptions, FidelityLevel, LegacyConverter};
use conary_core::ccs::manifest::ManifestProvenance;
use conary_core::diagnostics::{
    PACKAGING_JSON_SCHEMA_VERSION, PackagingArtifact, PackagingCommandOutput, PackagingDiagnostic,
    PackagingDiagnosticCode, PackagingEvent, PackagingEventKind, PackagingPhase,
};
use conary_core::packages::common::PackageMetadata;
use conary_core::packages::registry::{detect_format, parse_package};
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
            writeln!(output, "Warning: {}", warning)?;
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
                writeln!(output, "[OK] No issues found")?;
            } else {
                writeln!(output, "[OK] {} warning(s)", warnings.len())?;
            }
        }
        return Ok(report);
    }

    // Configure the kitchen. Host builds remain the compatibility default;
    // --isolated and --hermetic route through the M2a hermetic planner.
    let mut config = KitchenConfig {
        source_cache: PathBuf::from(options.source_cache),
        recipe_source_base_dir: Some(resolved.recipe_source_base_dir.clone()),
        origin_class_override: resolved.origin_class_override.clone(),
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
                "\n[COMPLETE] Fetched {} source file(s):",
                sources.len()
            )?;
            for source in &sources {
                writeln!(output, "  - {}", source.display())?;
            }

            if kitchen.sources_cached(&recipe) {
                writeln!(
                    output,
                    "\n[OK] All sources are cached. Ready for offline build."
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
            "\n[COMPLETE] Cooked: {}",
            result.package_path.display()
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

fn foreign_package_format(path: &Path) -> Option<&'static str> {
    let name = path.file_name()?.to_str()?;
    if name.ends_with(".rpm") {
        Some("rpm")
    } else if name.ends_with(".deb") {
        Some("deb")
    } else if name.ends_with(".pkg.tar.zst") {
        Some("arch")
    } else {
        None
    }
}

fn cook_foreign_package(
    package_path: &Path,
    output_dir: &Path,
    output: &mut impl Write,
) -> Result<()> {
    let format = detect_format(package_path).with_context(|| {
        format!(
            "Failed to parse foreign package: {}",
            package_path.display()
        )
    })?;
    let package = parse_package(package_path).with_context(|| {
        format!(
            "Failed to parse foreign package: {}",
            package_path.display()
        )
    })?;
    let package_bytes = std::fs::read(package_path)
        .with_context(|| format!("Failed to read foreign package: {}", package_path.display()))?;
    let checksum = conary_core::hash::sha256_prefixed(&package_bytes);
    let extracted = package.extract_file_contents().with_context(|| {
        format!(
            "Failed to extract files for foreign package: {}",
            package_path.display()
        )
    })?;
    let metadata = PackageMetadata {
        package_path: package_path.to_path_buf(),
        name: package.name().to_string(),
        version: package.version().to_string(),
        architecture: package.architecture().map(str::to_string),
        description: package.description().map(str::to_string),
        files: package.files().to_vec(),
        dependencies: package.dependencies().to_vec(),
        provides: package.provides().to_vec(),
        scriptlets: package.scriptlets().to_vec(),
        native_scriptlet_abi: package.native_scriptlet_abi().to_vec(),
        config_files: Vec::new(),
    };
    let converter = LegacyConverter::new(ConversionOptions {
        enable_chunking: true,
        output_dir: output_dir.to_path_buf(),
        auto_classify: true,
        min_fidelity: FidelityLevel::Partial,
        capture_scriptlets: false,
        enable_inference: true,
        inference_options: conary_core::capability::inference::InferenceOptions::fast(),
    });
    let result = converter
        .convert(&metadata, &extracted, format.name(), &checksum)
        .with_context(|| {
            format!(
                "Failed to convert foreign package {}",
                package_path.display()
            )
        })?;
    let converted = result
        .package_path
        .as_ref()
        .context("foreign conversion succeeded without a CCS output path")?;

    writeln!(output, "Converted foreign package: {}", converted.display())?;
    Ok(())
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
                "Warning: could not write hermetic host build record: {error}"
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
            "Warning: hermetic output differs from the latest host build record; this is diagnostic-only in M2a."
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use conary_core::ccs::CcsPackage;
    use conary_core::packages::PackageFormat;
    use conary_core::recipe::SourceDownloadPolicy;
    use std::fs::File;
    use std::process::Command;
    use tar::Builder;

    fn write_local_recipe(recipe_path: &Path) {
        std::fs::write(
            recipe_path,
            r#"
[package]
name = "local"
version = "1.0"

[source]
path = "."

[build]
install = "true"
"#,
        )
        .unwrap();
    }

    fn write_installing_local_recipe(recipe_path: &Path) {
        std::fs::write(
            recipe_path,
            r#"
[package]
name = "local"
version = "1.0"

[source]
path = "."

[build]
install = "mkdir -p %(destdir)s/usr/share/local && printf cooked > %(destdir)s/usr/share/local/output.txt"
"#,
        )
        .unwrap();
    }

    fn write_cargo_source_tree(root: &Path, package_name: &str) {
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join(".cargo")).unwrap();
        std::fs::write(
            root.join("Cargo.toml"),
            format!(
                r#"[package]
name = "{package_name}"
version = "0.1.0"
edition = "2021"
"#
            ),
        )
        .unwrap();
        std::fs::write(
            root.join(".cargo/config.toml"),
            "[build]\ntarget-dir = \"target\"\n",
        )
        .unwrap();
        std::fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
    }

    fn write_tar_archive(source_root: &Path, archive_path: &Path, top_level: &str) {
        let file = File::create(archive_path).unwrap();
        let mut builder = Builder::new(file);
        builder.append_dir_all(top_level, source_root).unwrap();
        builder.finish().unwrap();
    }

    fn git(cwd: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {:?} failed\nstdout:\n{}\nstderr:\n{}",
            args,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    fn initialize_git_remote(source_root: &Path, remote: &Path, package_name: &str) -> String {
        write_cargo_source_tree(source_root, package_name);
        git(source_root, &["init"]);
        git(
            source_root,
            &["config", "user.email", "conary@example.invalid"],
        );
        git(source_root, &["config", "user.name", "Conary Test"]);
        git(source_root, &["add", "."]);
        git(source_root, &["commit", "-m", "initial"]);
        let commit = git(source_root, &["rev-parse", "HEAD"]);
        git(
            source_root.parent().unwrap(),
            &[
                "clone",
                "--bare",
                source_root.to_str().unwrap(),
                remote.to_str().unwrap(),
            ],
        );
        commit
    }

    fn cooked_manifest_provenance(
        output_dir: &Path,
        package_name: &str,
        version: &str,
    ) -> conary_core::ccs::manifest::ManifestProvenance {
        let package_path = output_dir.join(format!("{package_name}-{version}-1.ccs"));
        let package = CcsPackage::parse(&package_path.to_string_lossy()).unwrap();
        package.manifest().provenance.clone().unwrap()
    }

    #[test]
    fn test_recipe_source_base_dir_uses_recipe_parent() {
        assert_eq!(
            recipe_source_base_dir(Path::new("/work/recipes/pkg/recipe.toml")),
            PathBuf::from("/work/recipes/pkg")
        );
    }

    #[test]
    fn cooked_artifact_path_extracts_single_ccs_artifact() {
        let mut output = PackagingCommandOutput::succeeded("watch-1", "conary cook");
        output.artifacts.push(PackagingArtifact {
            path: "/tmp/demo.ccs".to_string(),
            kind: Some("ccs".to_string()),
        });

        assert_eq!(
            cooked_artifact_path(&output).unwrap(),
            PathBuf::from("/tmp/demo.ccs")
        );
    }

    #[test]
    fn watch_refresh_cook_options_force_offline_policy_for_hermetic_refresh() {
        let options = CookForTryWatchOptions {
            target: Some("."),
            recipe: None,
            output_dir: "dist",
            source_cache: "sources",
            jobs: None,
            keep_builddir: false,
            isolated: true,
            no_isolation: false,
            hermetic: false,
            operation_id: "watch-1".to_string(),
            source_policy: WatchCookSourcePolicy::Refresh,
        };

        assert_eq!(
            watch_source_download_policy_override(&options),
            Some(SourceDownloadPolicy::OfflineCacheOnly)
        );
    }

    #[test]
    fn watch_refresh_preserves_source_policy_for_non_hermetic_refresh() {
        let options = CookForTryWatchOptions {
            target: Some("."),
            recipe: None,
            output_dir: "dist",
            source_cache: "sources",
            jobs: None,
            keep_builddir: false,
            isolated: false,
            no_isolation: false,
            hermetic: false,
            source_policy: WatchCookSourcePolicy::Refresh,
            operation_id: "watch-1".to_string(),
        };

        assert_eq!(watch_source_download_policy_override(&options), None);
    }

    #[test]
    fn watch_initial_cook_does_not_force_offline_policy() {
        let options = CookForTryWatchOptions {
            target: Some("."),
            recipe: None,
            output_dir: "dist",
            source_cache: "sources",
            jobs: None,
            keep_builddir: false,
            isolated: true,
            no_isolation: false,
            hermetic: false,
            source_policy: WatchCookSourcePolicy::Initial,
            operation_id: "watch-1".to_string(),
        };

        let _adapter: for<'a> fn(CookForTryWatchOptions<'a>) -> Result<PackagingCommandOutput> =
            run_cook_for_try_watch;
        assert_eq!(watch_source_download_policy_override(&options), None);
    }

    #[test]
    fn foreign_package_format_detects_release_artifacts() {
        assert_eq!(foreign_package_format(Path::new("pkg.rpm")), Some("rpm"));
        assert_eq!(foreign_package_format(Path::new("pkg.deb")), Some("deb"));
        assert_eq!(
            foreign_package_format(Path::new("pkg.pkg.tar.zst")),
            Some("arch")
        );
        assert_eq!(foreign_package_format(Path::new("recipe.toml")), None);
    }

    #[tokio::test]
    async fn cook_foreign_package_routes_before_recipe_resolution() {
        let temp = tempfile::tempdir().unwrap();
        let foreign = temp.path().join("demo.rpm");
        let output_dir = temp.path().join("out");
        std::fs::write(&foreign, b"not really rpm").unwrap();
        let mut output = Vec::new();

        let error = cmd_cook_with_output(
            Some(foreign.to_str().unwrap()),
            None,
            output_dir.to_str().unwrap(),
            temp.path().join("sources").to_str().unwrap(),
            None,
            false,
            false,
            false,
            false,
            false,
            false,
            false,
            false,
            &mut output,
        )
        .await
        .unwrap_err();
        let error = format!("{error:#}");

        assert!(error.contains("Failed to parse foreign package"), "{error}");
    }

    #[test]
    fn host_iteration_env_pins_cargo_target_dir_to_recipe_local_target() {
        let mut config = KitchenConfig::default();

        add_host_iteration_env(&mut config);

        assert!(
            config
                .extra_env
                .iter()
                .any(|(key, value)| key == "CARGO_TARGET_DIR" && value == "target"),
            "host iteration cook should override host Cargo target-dir config: {:?}",
            config.extra_env
        );
    }

    #[test]
    fn resolve_cook_input_prefers_recipe_flag_over_target() {
        let temp = tempfile::tempdir().unwrap();
        let source_tree = temp.path().join("source-tree");
        let recipe_path = temp.path().join("explicit.toml");
        write_cargo_source_tree(&source_tree, "target-marker");
        write_local_recipe(&recipe_path);
        let expected = recipe_path.canonicalize().unwrap();
        let resolved = resolve_cook_input(
            Some(source_tree.to_str().unwrap()),
            Some(recipe_path.to_str().unwrap()),
        )
        .unwrap();

        assert_eq!(resolved.recipe_path.as_deref(), Some(expected.as_path()));
        assert_eq!(resolved.recipe.package.name, "local");
        assert!(resolved.origin_class_override.is_none());
    }

    #[test]
    fn resolve_cook_input_accepts_directory_with_recipe_toml() {
        let temp = tempfile::tempdir().unwrap();
        let recipe_path = temp.path().join("recipe.toml");
        write_local_recipe(&recipe_path);
        let expected = recipe_path.canonicalize().unwrap();
        let resolved = resolve_cook_input(Some(temp.path().to_str().unwrap()), None).unwrap();

        assert_eq!(resolved.recipe_path.as_deref(), Some(expected.as_path()));
        assert!(resolved.origin_class_override.is_none());
    }

    #[test]
    fn resolve_cook_input_infers_existing_bare_source_directory_for_m1b() {
        let temp = tempfile::tempdir().unwrap();
        let bare_target = temp.path().join("source-tree");
        write_cargo_source_tree(&bare_target, "bare-source");

        let resolved = resolve_cook_input(Some(bare_target.to_str().unwrap()), None).unwrap();

        assert!(resolved.recipe_path.is_none());
        assert_eq!(resolved.recipe.package.name, "bare-source");
        assert_eq!(
            resolved.origin_class_override.as_deref(),
            Some("inferred-source")
        );
    }

    #[test]
    fn resolve_cook_input_unsupported_target_mentions_supported_forms() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("missing-source-tree");

        let error = resolve_cook_input(Some(target.to_str().unwrap()), None).unwrap_err();

        assert!(
            error.to_string().contains("Unsupported source target"),
            "unsupported target error should name supported forms: {error:#}"
        );
    }

    #[tokio::test]
    async fn cook_directory_with_recipe_toml_uses_explicit_recipe_provenance() {
        let temp = tempfile::tempdir().unwrap();
        let recipe_dir = temp.path().join("recipe-dir");
        let output_dir = temp.path().join("out");
        let source_cache = temp.path().join("sources");
        std::fs::create_dir_all(&recipe_dir).unwrap();
        write_installing_local_recipe(&recipe_dir.join("recipe.toml"));

        cmd_cook(
            Some(recipe_dir.to_str().unwrap()),
            None,
            output_dir.to_str().unwrap(),
            source_cache.to_str().unwrap(),
            None,
            false,
            false,
            false,
            false,
            false,
            false,
            false,
            false,
        )
        .await
        .unwrap();

        let provenance = cooked_manifest_provenance(&output_dir, "local", "1.0");
        assert_eq!(provenance.origin_class.as_deref(), Some("native-built"));
        assert_eq!(provenance.hardening_level.as_deref(), Some("host"));
    }

    #[tokio::test]
    async fn cook_cargo_directory_infers_recipe_and_stamps_inferred_source() {
        let temp = tempfile::tempdir().unwrap();
        let source_tree = temp.path().join("cargo-source");
        let output_dir = temp.path().join("out");
        let source_cache = temp.path().join("sources");
        write_cargo_source_tree(&source_tree, "inferred-local");

        cmd_cook(
            Some(source_tree.to_str().unwrap()),
            None,
            output_dir.to_str().unwrap(),
            source_cache.to_str().unwrap(),
            None,
            false,
            false,
            false,
            false,
            false,
            false,
            false,
            false,
        )
        .await
        .unwrap();

        let provenance = cooked_manifest_provenance(&output_dir, "inferred-local", "0.1.0");
        assert_eq!(provenance.origin_class.as_deref(), Some("inferred-source"));
        assert_eq!(provenance.hardening_level.as_deref(), Some("host"));
        assert!(
            provenance
                .upstream_url
                .as_deref()
                .is_some_and(|url| url.starts_with("local:")),
            "local source inference should stamp a local source marker: {provenance:?}"
        );
    }

    #[tokio::test]
    async fn cook_archive_target_stamps_archive_source_identity() {
        let temp = tempfile::tempdir().unwrap();
        let source_tree = temp.path().join("archive-source");
        let archive_path = temp.path().join("archive-demo.tar");
        let output_dir = temp.path().join("out");
        let source_cache = temp.path().join("sources");
        write_cargo_source_tree(&source_tree, "archive-demo");
        write_tar_archive(&source_tree, &archive_path, "archive-demo-0.1.0");

        cmd_cook(
            Some(archive_path.to_str().unwrap()),
            None,
            output_dir.to_str().unwrap(),
            source_cache.to_str().unwrap(),
            None,
            false,
            false,
            false,
            false,
            false,
            false,
            false,
            false,
        )
        .await
        .unwrap();

        let provenance = cooked_manifest_provenance(&output_dir, "archive-demo", "0.1.0");
        assert_eq!(provenance.origin_class.as_deref(), Some("inferred-source"));
        assert_eq!(provenance.upstream_url.as_deref(), archive_path.to_str());
        assert!(
            provenance
                .upstream_hash
                .as_deref()
                .is_some_and(|hash| hash.starts_with("sha256:")),
            "archive inference should stamp archive checksum: {provenance:?}"
        );
        assert!(provenance.git_commit.is_none());
    }

    #[tokio::test]
    async fn cook_git_target_stamps_git_source_identity() {
        let temp = tempfile::tempdir().unwrap();
        let source_tree = temp.path().join("git-source");
        let remote = temp.path().join("git-demo.git");
        let output_dir = temp.path().join("out");
        let source_cache = temp.path().join("sources");
        let commit = initialize_git_remote(&source_tree, &remote, "git-demo");

        cmd_cook(
            Some(remote.to_str().unwrap()),
            None,
            output_dir.to_str().unwrap(),
            source_cache.to_str().unwrap(),
            None,
            false,
            false,
            false,
            false,
            false,
            false,
            false,
            false,
        )
        .await
        .unwrap();

        let provenance = cooked_manifest_provenance(&output_dir, "git-demo", "0.1.0");
        assert_eq!(provenance.origin_class.as_deref(), Some("inferred-source"));
        assert_eq!(provenance.upstream_url.as_deref(), remote.to_str());
        assert_eq!(provenance.git_commit.as_deref(), Some(commit.as_str()));
        assert!(provenance.upstream_hash.is_none());
    }

    #[tokio::test]
    async fn cook_recipe_flag_wins_over_source_target_markers() {
        let temp = tempfile::tempdir().unwrap();
        let source_tree = temp.path().join("cargo-source");
        let recipe_path = temp.path().join("explicit.toml");
        let output_dir = temp.path().join("out");
        let source_cache = temp.path().join("sources");
        write_cargo_source_tree(&source_tree, "target-marker");
        write_local_recipe(&recipe_path);

        let mut output = Vec::new();
        cmd_cook_with_output(
            Some(source_tree.to_str().unwrap()),
            Some(recipe_path.to_str().unwrap()),
            output_dir.to_str().unwrap(),
            source_cache.to_str().unwrap(),
            None,
            false,
            true,
            false,
            false,
            false,
            false,
            false,
            false,
            &mut output,
        )
        .await
        .unwrap();

        let output = String::from_utf8(output).unwrap();
        assert!(output.contains("Recipe: local version 1.0"), "{output}");
        assert!(
            !output.contains("target-marker"),
            "--recipe should bypass target inference: {output}"
        );
    }

    #[tokio::test]
    async fn cook_positional_custom_toml_recipe_validates() {
        let temp = tempfile::tempdir().unwrap();
        let recipe_path = temp.path().join("custom.toml");
        let output_dir = temp.path().join("out");
        let source_cache = temp.path().join("sources");
        write_local_recipe(&recipe_path);

        let mut output = Vec::new();
        cmd_cook_with_output(
            Some(recipe_path.to_str().unwrap()),
            None,
            output_dir.to_str().unwrap(),
            source_cache.to_str().unwrap(),
            None,
            false,
            true,
            false,
            false,
            false,
            false,
            false,
            false,
            &mut output,
        )
        .await
        .unwrap();

        let output = String::from_utf8(output).unwrap();
        assert!(output.contains("Reading recipe:"), "{output}");
        assert!(output.contains("Recipe: local version 1.0"), "{output}");
        assert!(output.contains("Recipe validation passed"), "{output}");
        assert!(
            !output_dir.exists(),
            "validate-only custom recipe should not create build output"
        );
    }

    #[tokio::test]
    async fn cook_validate_only_json_has_schema_version_and_summary() {
        let temp = tempfile::tempdir().unwrap();
        let recipe_path = temp.path().join("recipe.toml");
        let output_dir = temp.path().join("out");
        let source_cache = temp.path().join("sources");
        write_local_recipe(&recipe_path);

        let mut output = Vec::new();
        cmd_cook_with_output(
            Some(recipe_path.to_str().unwrap()),
            None,
            output_dir.to_str().unwrap(),
            source_cache.to_str().unwrap(),
            None,
            false,
            true,
            false,
            false,
            false,
            false,
            false,
            true,
            &mut output,
        )
        .await
        .unwrap();

        let value: serde_json::Value = serde_json::from_slice(&output).expect("valid cook json");
        assert_eq!(value["schema_version"], 1);
        assert_eq!(value["command"], "conary cook");
        assert_eq!(value["status"], "succeeded");
        assert_eq!(value["summary"], "Recipe validation passed");
        assert!(value["operation_id"].as_str().unwrap().starts_with("cook-"));
    }

    #[tokio::test]
    async fn cook_json_conflict_error_is_single_structured_json() {
        let temp = tempfile::tempdir().unwrap();
        let recipe_path = temp.path().join("recipe.toml");
        let output_dir = temp.path().join("out");
        let source_cache = temp.path().join("sources");
        write_local_recipe(&recipe_path);

        let mut output = Vec::new();
        let error = cmd_cook_with_output(
            Some(recipe_path.to_str().unwrap()),
            None,
            output_dir.to_str().unwrap(),
            source_cache.to_str().unwrap(),
            None,
            false,
            false,
            false,
            false,
            true,
            true,
            false,
            true,
            &mut output,
        )
        .await
        .unwrap_err();

        let rendered = String::from_utf8(output).unwrap();
        assert!(format!("{error:#}").contains("--no-isolation conflicts"));
        assert!(rendered.trim_start().starts_with('{'), "{rendered}");
        assert!(!rendered.contains("Reading recipe:"));
        let value: serde_json::Value = serde_json::from_str(&rendered).expect("valid error json");
        assert_eq!(value["status"], "failed");
        assert_eq!(value["diagnostics"][0]["code"], "cook-failed");
    }

    #[tokio::test]
    async fn cook_validate_only_explain_prints_trace_for_inferred_recipe_without_building() {
        let temp = tempfile::tempdir().unwrap();
        let source_tree = temp.path().join("cargo-source");
        let output_dir = temp.path().join("out");
        let source_cache = temp.path().join("sources");
        write_cargo_source_tree(&source_tree, "validate-demo");

        let mut output = Vec::new();
        cmd_cook_with_output(
            Some(source_tree.to_str().unwrap()),
            None,
            output_dir.to_str().unwrap(),
            source_cache.to_str().unwrap(),
            None,
            false,
            true,
            false,
            true,
            false,
            false,
            false,
            false,
            &mut output,
        )
        .await
        .unwrap();

        let output = String::from_utf8(output).unwrap();
        assert!(output.contains("Inference trace:"), "{output}");
        assert!(output.contains("Recipe validation passed"), "{output}");
        assert!(
            !output_dir.exists(),
            "validate-only inference should not create build output"
        );
    }

    #[tokio::test]
    async fn cook_fetch_only_inferred_local_source_reports_no_remote_fetch() {
        let temp = tempfile::tempdir().unwrap();
        let source_tree = temp.path().join("cargo-source");
        let output_dir = temp.path().join("out");
        let source_cache = temp.path().join("sources");
        write_cargo_source_tree(&source_tree, "fetch-demo");

        let mut output = Vec::new();
        cmd_cook_with_output(
            Some(source_tree.to_str().unwrap()),
            None,
            output_dir.to_str().unwrap(),
            source_cache.to_str().unwrap(),
            None,
            false,
            false,
            true,
            false,
            false,
            false,
            false,
            false,
            &mut output,
        )
        .await
        .unwrap();

        let output = String::from_utf8(output).unwrap();
        assert!(
            output.contains("No remote source fetch is required"),
            "{output}"
        );
        assert!(
            !output_dir.exists(),
            "fetch-only local inference should not build package output"
        );
    }

    #[tokio::test]
    async fn cook_hermetic_requires_hermetic_config_before_planning() {
        let temp = tempfile::tempdir().unwrap();
        let recipe_path = temp.path().join("recipe.toml");
        let output_dir = temp.path().join("out");
        let source_cache = temp.path().join("sources");
        write_local_recipe(&recipe_path);

        let error = cmd_cook(
            Some(recipe_path.to_str().unwrap()),
            None,
            output_dir.to_str().unwrap(),
            source_cache.to_str().unwrap(),
            None,
            false,
            false,
            false,
            false,
            false,
            false,
            true,
            false,
        )
        .await
        .unwrap_err();
        let error = format!("{error:#}");

        assert!(error.contains("hermetic config"), "{error}");
        assert!(
            !error.contains("Hermetic cook/publish is an M2 feature"),
            "hermetic cook should no longer use the old reserved-feature rejection: {error}"
        );
        assert!(
            !output_dir.join("local-1.0-1.ccs").exists(),
            "hermetic planning failure should not write a package"
        );
    }

    #[tokio::test]
    async fn cook_isolated_fails_closed_without_hermetic_config() {
        let temp = tempfile::tempdir().unwrap();
        let recipe_path = temp.path().join("recipe.toml");
        let output_dir = temp.path().join("out");
        let source_cache = temp.path().join("sources");
        write_local_recipe(&recipe_path);

        let mut output = Vec::new();
        let error = cmd_cook_with_output(
            Some(recipe_path.to_str().unwrap()),
            None,
            output_dir.to_str().unwrap(),
            source_cache.to_str().unwrap(),
            None,
            false,
            false,
            false,
            false,
            true,
            false,
            false,
            false,
            &mut output,
        )
        .await
        .unwrap_err();
        let error = format!("{error:#}");
        let output = String::from_utf8(output).unwrap();

        assert!(error.contains("hermetic config"), "{error}");
        assert!(
            !output.contains("attested"),
            "M2a cook output must not claim attestation before M2b: {output}"
        );
        assert!(
            !output.contains("Cooking with"),
            "missing hermetic config should fail before cooking starts: {output}"
        );
        assert!(
            !output_dir.join("local-1.0-1.ccs").exists(),
            "hermetic config failure should not write a package"
        );
    }

    #[tokio::test]
    async fn cook_no_isolation_is_hidden_host_default_compatibility_noop_with_provenance() {
        let temp = tempfile::tempdir().unwrap();
        let default_root = temp.path().join("default");
        let compat_root = temp.path().join("compat");
        std::fs::create_dir_all(&default_root).unwrap();
        std::fs::create_dir_all(&compat_root).unwrap();
        let default_recipe = default_root.join("recipe.toml");
        let compat_recipe = compat_root.join("recipe.toml");
        let default_output = temp.path().join("default-out");
        let compat_output = temp.path().join("compat-out");
        let source_cache = temp.path().join("sources");
        write_installing_local_recipe(&default_recipe);
        write_installing_local_recipe(&compat_recipe);

        cmd_cook(
            Some(default_recipe.to_str().unwrap()),
            None,
            default_output.to_str().unwrap(),
            source_cache.to_str().unwrap(),
            None,
            false,
            false,
            false,
            false,
            false,
            false,
            false,
            false,
        )
        .await
        .unwrap();
        cmd_cook(
            Some(compat_recipe.to_str().unwrap()),
            None,
            compat_output.to_str().unwrap(),
            source_cache.to_str().unwrap(),
            None,
            false,
            false,
            false,
            false,
            false,
            true,
            false,
            false,
        )
        .await
        .unwrap();

        for provenance in [
            cooked_manifest_provenance(&default_output, "local", "1.0"),
            cooked_manifest_provenance(&compat_output, "local", "1.0"),
        ] {
            assert_eq!(provenance.origin_class.as_deref(), Some("native-built"));
            assert_eq!(provenance.hardening_level.as_deref(), Some("host"));
        }
    }

    #[tokio::test]
    async fn cook_no_isolation_conflicts_with_isolated() {
        let temp = tempfile::tempdir().unwrap();
        let recipe_path = temp.path().join("recipe.toml");
        let output_dir = temp.path().join("out");
        let source_cache = temp.path().join("sources");
        write_local_recipe(&recipe_path);

        let error = cmd_cook(
            Some(recipe_path.to_str().unwrap()),
            None,
            output_dir.to_str().unwrap(),
            source_cache.to_str().unwrap(),
            None,
            false,
            false,
            false,
            false,
            true,
            true,
            false,
            false,
        )
        .await
        .unwrap_err();

        assert!(
            error.to_string().contains("conflict"),
            "--isolated and --no-isolation conflict should be explicit: {error:#}"
        );
    }
}
