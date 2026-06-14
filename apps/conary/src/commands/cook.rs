// src/commands/cook.rs

//! Cook command - build packages from recipes

use anyhow::{Context, Result};
use conary_core::ccs::manifest::ManifestProvenance;
use conary_core::recipe::CookResult;
use conary_core::recipe::hermetic::{DivergenceStatus, HermeticBuildInput, detect_ci_mode};
use conary_core::recipe::inference::{
    CookTarget, ResolvedSourceTree, SourceTargetKind, SourceTargetProvenance,
    infer_recipe_from_path, resolve_cook_target,
};
use conary_core::recipe::{
    InferenceOptions, InferenceTrace, Kitchen, KitchenConfig, Recipe, SourceSection,
    parse_recipe_file, validate_recipe,
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
    output: &mut impl Write,
) -> Result<()> {
    let hermetic_requested = hermetic || isolated;
    if hermetic_requested && no_isolation {
        anyhow::bail!("--no-isolation conflicts with --isolated/--hermetic");
    }

    let resolved = resolve_cook_input(target, recipe)?;
    let output_dir = Path::new(output_dir);
    let recipe = resolved.recipe.clone();

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

    if explain && let Some(trace) = &resolved.inference_trace {
        write_inference_trace(output, trace)?;
    }

    // Validate the recipe
    let warnings = validate_recipe(&recipe).with_context(|| "Recipe validation failed")?;

    for warning in &warnings {
        writeln!(output, "Warning: {}", warning)?;
    }

    if validate_only {
        writeln!(output, "Recipe validation passed")?;
        if warnings.is_empty() {
            writeln!(output, "[OK] No issues found")?;
        } else {
            writeln!(output, "[OK] {} warning(s)", warnings.len())?;
        }
        return Ok(());
    }

    // Configure the kitchen. Host builds remain the compatibility default;
    // --isolated and --hermetic route through the M2a hermetic planner.
    let mut config = KitchenConfig {
        source_cache: PathBuf::from(source_cache),
        recipe_source_base_dir: Some(resolved.recipe_source_base_dir.clone()),
        origin_class_override: resolved.origin_class_override.clone(),
        source_provenance_override: resolved.source_provenance_override.clone(),
        keep_builddir,
        use_isolation: false,
        pristine_mode: false,
        ..Default::default()
    };

    if let Some(j) = jobs {
        config.jobs = j;
    }
    if !hermetic_requested {
        for key in ["PATH", "HOME", "CARGO_HOME", "RUSTUP_HOME"] {
            if let Ok(value) = std::env::var(key) {
                config.extra_env.push((key.to_string(), value));
            }
        }
    }

    // Fetch-only mode: just download sources and exit
    if fetch_only {
        let kitchen = Kitchen::new(config.clone());
        if matches!(resolved.source_kind, Some(SourceTargetKind::Directory))
            && matches!(recipe.source, SourceSection::Local(_))
        {
            writeln!(
                output,
                "No remote source fetch is required for inferred local source tree."
            )?;
            return Ok(());
        }

        writeln!(output, "Fetching sources (fetch-only mode)...")?;
        let sources = kitchen
            .fetch(&recipe)
            .with_context(|| format!("Failed to fetch sources for {}", recipe.package.name))?;

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

        return Ok(());
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

    // Create kitchen and cook
    let result = if let Some(builder) = hermetic_builder {
        let input =
            hermetic_build_input(&resolved, &recipe)?.with_builder_environment(builder.identity);
        kitchen.cook_hermetic(&recipe, input, output_dir, detect_ci_mode())
    } else {
        kitchen.cook(&recipe, output_dir)
    }
    .with_context(|| format!("Failed to cook {}", recipe.package.name))?;

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

    info!(
        "Successfully cooked {} to {}",
        recipe.package.name,
        result.package_path.display()
    );

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
        )
        .await
        .unwrap_err();

        assert!(
            error.to_string().contains("conflict"),
            "--isolated and --no-isolation conflict should be explicit: {error:#}"
        );
    }
}
