// apps/conary/src/commands/bootstrap/run.rs

use std::collections::HashSet;
use std::path::PathBuf;

use anyhow::{Context, Result};
use rusqlite::Connection;
use tracing::info;

use super::run_artifact::write_bootstrap_run_generation_artifact;
use super::run_record::{
    finish_bootstrap_run_failure, finish_bootstrap_run_success, start_bootstrap_run_record,
};
use super::types::BootstrapRunOptions;

/// Run the derivation pipeline from a system manifest.
///
/// Loads the manifest, seed, and recipes, assigns stages, then executes the
/// full derivation pipeline. Writes generation output (EROFS image, metadata,
/// profile) and creates a `current` symlink.
pub async fn cmd_bootstrap_run(opts: BootstrapRunOptions<'_>) -> Result<()> {
    use conary_core::db::schema::migrate;
    use conary_core::derivation::build_order::Stage;
    use conary_core::derivation::build_order::compute_build_order;
    use conary_core::derivation::executor::{DerivationExecutor, ExecutorConfig};
    use conary_core::derivation::manifest::SystemManifest;
    use conary_core::derivation::pipeline::{Pipeline, PipelineConfig, PipelineEvent};
    use conary_core::derivation::seed::Seed;
    use conary_core::filesystem::CasStore;

    info!(
        "bootstrap run: manifest={}, work_dir={}, seed={}",
        opts.manifest, opts.work_dir, opts.seed
    );

    if opts.verbose {
        println!("  manifest: {}", opts.manifest);
        println!("  work_dir: {}", opts.work_dir);
        println!("  seed: {}", opts.seed);
        println!("  recipe_dir: {}", opts.recipe_dir);
        if let Some(s) = opts.up_to {
            println!("  up_to: {s}");
        }
        if opts.no_substituters {
            println!("  substituters: disabled");
        }
        if opts.publish {
            println!("  publish: enabled");
        }
    }

    // 1. Load manifest
    let manifest_path = PathBuf::from(opts.manifest);
    let manifest =
        SystemManifest::load(&manifest_path).context("Failed to load system manifest")?;
    println!(
        "System: {} ({})",
        manifest.system.name, manifest.system.target
    );
    println!("Packages: {} included", manifest.packages.include.len());

    // 2. Load seed
    let seed_path = PathBuf::from(opts.seed);
    let seed =
        Seed::load_local(&seed_path).map_err(|e| anyhow::anyhow!("Failed to load seed: {e}"))?;
    println!(
        "Seed: {} ({})",
        &seed.build_env_hash()[..16],
        seed_path.display()
    );

    // 3. Load recipes and filter to manifest includes + transitive deps
    let recipe_dir = PathBuf::from(opts.recipe_dir);
    let all_recipes = conary_core::derivation::load_recipes(&recipe_dir)?;
    println!("Recipes loaded: {}", all_recipes.len());

    let included: HashSet<String> = manifest.packages.include.iter().cloned().collect();
    let mut needed: HashSet<String> = included.clone();
    let mut frontier: Vec<String> = included.into_iter().collect();
    while let Some(pkg) = frontier.pop() {
        if let Some(recipe) = all_recipes.get(&pkg) {
            for dep in recipe
                .build
                .requires
                .iter()
                .chain(recipe.build.makedepends.iter())
            {
                if needed.insert(dep.clone()) {
                    frontier.push(dep.clone());
                }
            }
        }
    }

    let recipes: std::collections::HashMap<String, conary_core::recipe::Recipe> = all_recipes
        .into_iter()
        .filter(|(name, _)| needed.contains(name))
        .collect();
    println!("Recipes after dep resolution: {}", recipes.len());

    // 4. Compute build order
    let custom_packages: HashSet<String> = HashSet::new();
    let mut build_steps = compute_build_order(&recipes, &custom_packages)
        .map_err(|e| anyhow::anyhow!("Build order computation failed: {e}"))?;
    println!("Build order: {} packages", build_steps.len());

    // Apply --up-to filter: drop packages in stages beyond the cutoff.
    if let Some(ref up_to) = opts.up_to {
        let cutoff = Stage::from_str_name(up_to)
            .ok_or_else(|| anyhow::anyhow!("invalid --up-to stage: {up_to}"))?;
        build_steps.retain(|step| step.stage <= cutoff);
        println!("After --up-to {up_to}: {} packages", build_steps.len());
    }

    let mut record =
        start_bootstrap_run_record(&opts, &manifest_path, &recipe_dir, seed.build_env_hash())?;
    let op_dir = record.operation_dir();
    let output_dir = record.output_dir.clone();

    let run_result: Result<(PathBuf, String)> = async {
        // 5. Open DB
        let conn = Connection::open(&record.derivation_db_path)
            .context("Failed to open derivation database")?;
        migrate(&conn).context("Failed to run database migrations")?;

        // 6. Create CAS and executor
        let cas_dir = output_dir.join("objects");
        std::fs::create_dir_all(&cas_dir)?;
        let cas = CasStore::new(&cas_dir).context("Failed to create CAS store")?;

        let executor_config = ExecutorConfig {
            log_dir: Some(op_dir.join("logs")),
            keep_logs: opts.keep_logs,
            shell_on_failure: opts.shell_on_failure,
        };
        let executor = DerivationExecutor::new(cas, cas_dir.clone(), executor_config);

        // 7. Create pipeline
        let pipeline_config = PipelineConfig {
            cas_dir: cas_dir.clone(),
            work_dir: op_dir.join("pipeline"),
            target_triple: manifest.system.target.clone(),
            jobs: std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(4),
            log_dir: Some(op_dir.join("logs")),
            keep_logs: opts.keep_logs,
            shell_on_failure: opts.shell_on_failure,
            only_packages: opts.only.map(|s| s.to_vec()),
            cascade: opts.cascade,
            substituter_sources: if opts.no_substituters {
                vec![]
            } else {
                manifest
                    .substituters
                    .as_ref()
                    .map(|s| s.sources.clone())
                    .unwrap_or_default()
            },
            publish_endpoint: if opts.publish {
                Some("https://remi.conary.io".to_string())
            } else {
                None
            },
            publish_token: None,
        };

        std::fs::create_dir_all(&pipeline_config.work_dir)?;
        let pipeline = Pipeline::new(pipeline_config, executor);

        // 8. Execute pipeline
        println!("\nStarting derivation pipeline...\n");
        let profile = pipeline
            .execute(&seed, &recipes, &build_steps, &conn, |event| match event {
                PipelineEvent::StageStarted {
                    name,
                    package_count,
                } => {
                    println!("[{name}] Stage started ({package_count} packages)");
                }
                PipelineEvent::PackageBuilding { name, stage } => {
                    println!("[{stage}] Building {name}...");
                }
                PipelineEvent::PackageCached { name } => {
                    println!("  [cached] {name}");
                }
                PipelineEvent::PackageBuilt {
                    name,
                    duration_secs,
                } => {
                    println!("  [built] {name} in {duration_secs}s");
                }
                PipelineEvent::PackageFailed { name, error } => {
                    println!("  [FAILED] {name}: {error}");
                }
                PipelineEvent::SubstituterHit {
                    name,
                    peer,
                    objects_fetched,
                } => {
                    println!("  [substituted] {name} from {peer} ({objects_fetched} objects)");
                }
                PipelineEvent::BuildLogWritten { package, path } => {
                    println!("  [log] {package}: {}", path.display());
                }
                PipelineEvent::StageCompleted { name } => {
                    println!("[{name}] Stage completed\n");
                }
                PipelineEvent::PipelineCompleted {
                    total_packages,
                    cached,
                    built,
                } => {
                    println!(
                        "[COMPLETE] {total_packages} packages processed ({built} built, {cached} cached)"
                    );
                }
            })
            .await?;

        // 9. Write generation output
        let gen_dir = output_dir.join("generations").join("1");
        std::fs::create_dir_all(&gen_dir)?;

        let compose_erofs = op_dir.join("pipeline").join("compose").join("root.erofs");
        let stage_erofs = profile.stages.last().map(|stage| {
            op_dir
                .join("pipeline")
                .join(format!("stage-{}", stage.name))
                .join("root.erofs")
        });
        let erofs_source = if compose_erofs.exists() {
            compose_erofs
        } else {
            stage_erofs
                .filter(|p| p.exists())
                .ok_or_else(|| anyhow::anyhow!(
                    "No EROFS image found in pipeline output; bootstrap-run cannot create an exportable generation"
                ))?
        };
        let dest = gen_dir.join("root.erofs");
        std::fs::copy(&erofs_source, &dest)?;
        println!("Generation 1 EROFS: {}", dest.display());

        write_bootstrap_run_generation_artifact(
            &conn,
            &cas_dir,
            &gen_dir,
            &profile,
            &manifest.system.target,
            &manifest.system.name,
        )?;

        let profile_hash = profile.profile.profile_hash.clone();
        let profile_toml = toml::to_string_pretty(&profile)?;
        std::fs::write(gen_dir.join("profile.toml"), &profile_toml)?;

        Ok((gen_dir, profile_hash))
    }
    .await;

    match run_result {
        Ok((gen_dir, profile_hash)) => {
            finish_bootstrap_run_success(&mut record, &gen_dir, &profile_hash)?;
            println!("\nOutput: {}", output_dir.display());
            println!("Profile hash: {profile_hash}");
            Ok(())
        }
        Err(error) => {
            finish_bootstrap_run_failure(&mut record, &error)?;
            Err(error)
        }
    }
}
