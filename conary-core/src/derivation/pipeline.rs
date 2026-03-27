// conary-core/src/derivation/pipeline.rs

//! Build pipeline orchestrator.
//!
//! [`Pipeline`] drives the complete CAS-layered bootstrap: packages are built
//! sequentially in topological order inside a mutable overlayfs chroot, with
//! each package installed into the live sysroot before the next build starts.
//! A single EROFS image is composed from all outputs at the end.
//!
//! [`Pipeline::generate_profile`] produces a dry-run [`BuildProfile`] without
//! executing any builds, marking all derivation IDs as "pending".

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::time::Instant;

use rusqlite::Connection;
use tracing::{info, warn};

use crate::recipe::Recipe;

use super::compose::compose_erofs;
use super::environment::MutableEnvironment;
use super::executor::{DerivationExecutor, ExecutionResult, ExecutorError};
use super::id::{DerivationId, DerivationInputs};
use super::index::DerivationIndex;
use super::install::{install_to_sysroot, run_ldconfig_if_needed};
use super::output::OutputManifest;
use super::profile::{
    BuildProfile, ProfileDerivation, ProfileMetadata, ProfileSeedRef, ProfileStage,
};
use super::recipe_hash;
use super::seed::Seed;
use crate::derivation::build_order::Stage;

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for the build pipeline.
#[derive(Debug, Clone)]
pub struct PipelineConfig {
    /// Root directory for CAS objects.
    pub cas_dir: PathBuf,
    /// Working directory for intermediate build artifacts.
    pub work_dir: PathBuf,
    /// Target triple (e.g. `x86_64-unknown-linux-gnu`).
    pub target_triple: String,
    /// Maximum parallel jobs (informational; actual parallelism is per-recipe).
    pub jobs: usize,
    /// Directory for build logs. None disables logging.
    pub log_dir: Option<PathBuf>,
    /// Preserve logs even for successful builds.
    pub keep_logs: bool,
    /// Spawn interactive shell on build failure.
    pub shell_on_failure: bool,
    /// Only build these packages. All other packages use cache lookups.
    pub only_packages: Option<Vec<String>>,
    /// When combined with `only_packages`, also rebuild reverse dependents.
    pub cascade: bool,
    /// Substituter endpoints to query for pre-built outputs.
    pub substituter_sources: Vec<String>,
    /// Endpoint to auto-publish successful builds to. None disables.
    pub publish_endpoint: Option<String>,
    /// Bearer token for publish endpoint.
    pub publish_token: Option<String>,
}

// ---------------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------------

/// Progress events emitted during pipeline execution.
#[derive(Debug, Clone)]
pub enum PipelineEvent {
    /// A stage has started building.
    StageStarted {
        /// Stage name (e.g. "toolchain").
        name: String,
        /// Number of packages in this stage.
        package_count: usize,
    },
    /// A package build has started.
    PackageBuilding {
        /// Package name.
        name: String,
        /// Stage the package belongs to.
        stage: String,
    },
    /// A package was found in the derivation cache.
    PackageCached {
        /// Package name.
        name: String,
    },
    /// A package was built successfully.
    PackageBuilt {
        /// Package name.
        name: String,
        /// Wall-clock build duration in seconds.
        duration_secs: u64,
    },
    /// A package build failed.
    PackageFailed {
        /// Package name.
        name: String,
        /// Error description.
        error: String,
    },
    /// A package was fetched from a remote substituter.
    SubstituterHit {
        /// Package name.
        name: String,
        /// Peer that had the cache hit.
        peer: String,
        /// Number of CAS objects fetched.
        objects_fetched: u64,
    },
    /// A build log file was written (preserved on failure or keep_logs).
    BuildLogWritten {
        /// Package name.
        package: String,
        /// Path to the log file.
        path: PathBuf,
    },
    /// A stage has completed successfully.
    StageCompleted {
        /// Stage name.
        name: String,
    },
    /// The entire pipeline has completed.
    PipelineCompleted {
        /// Total packages processed.
        total_packages: usize,
        /// Packages served from cache.
        cached: usize,
        /// Packages built fresh.
        built: usize,
    },
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Errors that can occur during pipeline execution.
#[derive(Debug, thiserror::Error)]
pub enum PipelineError {
    /// A recipe referenced by an assignment was not found.
    #[error("missing recipe: {0}")]
    MissingRecipe(String),

    /// The underlying executor failed.
    #[error(transparent)]
    Executor(#[from] ExecutorError),

    /// EROFS composition or hashing failed.
    #[error(transparent)]
    Compose(#[from] crate::derivation::compose::ComposeError),

    /// A general I/O error.
    #[error("I/O error: {0}")]
    Io(String),

    /// A non-targeted package has no cached derivation (required by --only).
    #[error(
        "package '{package}' has no cached derivation -- run a full build first or add it to --only"
    )]
    UncachedDependency { package: String },

    /// Installing a derivation into the sysroot failed.
    #[error("install error: {0}")]
    Install(String),
}

// From<ComposeError> is derived via #[from] on the Compose variant above.

// ---------------------------------------------------------------------------
// Pipeline
// ---------------------------------------------------------------------------

/// Full staged build pipeline orchestrator.
///
/// Drives derivation execution across all bootstrap stages, composing EROFS
/// images between stages to propagate the build environment forward.
pub struct Pipeline {
    config: PipelineConfig,
    executor: DerivationExecutor,
}

impl Pipeline {
    /// Create a new pipeline with the given configuration and executor.
    #[must_use]
    pub fn new(config: PipelineConfig, executor: DerivationExecutor) -> Self {
        Self { config, executor }
    }

    /// Generate a [`BuildProfile`] from recipes and stage assignments without
    /// building anything.
    ///
    /// All derivation IDs are marked as `"pending"` since no actual
    /// content-addressing has been computed. This is useful for dry-run
    /// planning and diffing against previous profiles.
    #[must_use]
    pub fn generate_profile(
        seed: &Seed,
        recipes: &HashMap<String, Recipe>,
        build_steps: &[crate::derivation::build_order::BuildStep],
        manifest_path: &str,
    ) -> BuildProfile {
        let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();

        let stages_ordered = ordered_stages(build_steps);
        let mut profile_stages = Vec::new();

        for (stage, pkgs) in &stages_ordered {
            let derivations: Vec<ProfileDerivation> = pkgs
                .iter()
                .filter_map(|name| {
                    recipes.get(name.as_str()).map(|recipe| ProfileDerivation {
                        package: recipe.package.name.clone(),
                        version: recipe.package.version.clone(),
                        derivation_id: "pending".to_owned(),
                    })
                })
                .collect();

            profile_stages.push(ProfileStage {
                name: stage.to_string(),
                build_env: "pending".to_owned(),
                derivations,
            });
        }

        let mut profile = BuildProfile {
            profile: ProfileMetadata {
                manifest: manifest_path.to_owned(),
                profile_hash: String::new(),
                generated_at: now,
                target: seed.metadata.target_triple.clone(),
            },
            seed: ProfileSeedRef {
                id: seed.metadata.seed_id.clone(),
                source: seed.metadata.source.to_string(),
            },
            stages: profile_stages,
        };

        profile.profile.profile_hash = profile.compute_hash();
        profile
    }

    /// Execute the build pipeline.
    ///
    /// Builds all packages sequentially in topological order inside a mutable
    /// overlayfs chroot. Each package is installed into the live sysroot after
    /// building so subsequent packages can link against it. A single EROFS image
    /// is composed from all outputs at the end.
    ///
    /// The `build_env_hash` stays constant throughout (the seed hash) -- there
    /// are no per-stage EROFS boundaries.
    ///
    /// # Errors
    ///
    /// Returns [`PipelineError`] on missing recipes, executor failures,
    /// install failures, composition errors, or I/O errors.
    pub async fn execute<F>(
        &self,
        seed: &Seed,
        recipes: &HashMap<String, Recipe>,
        build_steps: &[crate::derivation::build_order::BuildStep],
        conn: &Connection,
        mut on_event: F,
    ) -> Result<BuildProfile, PipelineError>
    where
        F: FnMut(&PipelineEvent),
    {
        let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();

        // In chroot mode the build environment hash stays constant -- all
        // packages see the seed's hash, not one that evolves per stage.
        let build_env_hash = seed.build_env_hash().to_owned();

        // Determine which packages to actually build (--only filter).
        let build_set: Option<HashSet<String>> = self
            .config
            .only_packages
            .as_ref()
            .map(|pkgs| pkgs.iter().cloned().collect());

        // Count packages per stage for progress reporting.
        let mut stage_counts: HashMap<Stage, usize> = HashMap::new();
        for step in build_steps {
            *stage_counts.entry(step.stage).or_insert(0) += 1;
        }

        // Mount the mutable overlayfs environment once.
        let chroot_dir = self.config.work_dir.join("chroot");
        std::fs::create_dir_all(&chroot_dir).map_err(|e| PipelineError::Io(e.to_string()))?;

        let mut env = MutableEnvironment::new(
            seed.image_path.clone(),
            self.config.cas_dir.clone(),
            chroot_dir,
            build_env_hash.clone(),
        );
        env.mount().map_err(|e| {
            PipelineError::Io(format!(
                "Mutable environment mount failed (requires root): {e}"
            ))
        })?;

        let sysroot = env.sysroot();

        // When --only is active, pre-populate acc.completed from the index so
        // that collect_dep_ids produces correct derivation IDs for all packages
        // that will be served from cache.  Without this, early non-targeted
        // packages have an empty dep map, producing wrong IDs for later packages
        // that depend on them.
        let mut completed_seed: HashMap<String, DerivationId> = HashMap::new();
        if build_set.is_some() {
            for step in build_steps {
                let pkg_name = &step.package;
                let Some(recipe) = recipes.get(pkg_name.as_str()) else {
                    continue;
                };
                // Compute ID using whatever is already in completed_seed (which
                // grows in topological order, same as the main loop).
                let dep_ids = collect_dep_ids(recipe, &completed_seed);
                let inputs = DerivationInputs {
                    source_hash: recipe_hash::source_hash(recipe),
                    build_script_hash: recipe_hash::build_script_hash(recipe),
                    dependency_ids: dep_ids,
                    build_env_hash: build_env_hash.clone(),
                    target_triple: self.config.target_triple.clone(),
                    build_options: BTreeMap::new(),
                };
                if let Ok(id) = DerivationId::compute(&inputs) {
                    completed_seed.insert(pkg_name.clone(), id);
                }
            }
        }

        let mut acc = BuildAccumulator {
            completed: completed_seed,
            derivations: Vec::new(),
            manifests: Vec::new(),
        };
        let mut total_cached: usize = 0;
        let mut total_built: usize = 0;

        // Track current stage for progress events.
        let mut current_stage: Option<Stage> = None;

        for step in build_steps {
            // Emit StageStarted/StageCompleted on stage transitions.
            if current_stage != Some(step.stage) {
                if let Some(prev) = current_stage {
                    on_event(&PipelineEvent::StageCompleted {
                        name: prev.to_string(),
                    });
                }
                let count = stage_counts.get(&step.stage).copied().unwrap_or(0);
                on_event(&PipelineEvent::StageStarted {
                    name: step.stage.to_string(),
                    package_count: count,
                });
                info!("stage {}: {count} packages", step.stage);
                current_stage = Some(step.stage);
            }

            let pkg_name = &step.package;

            let recipe = recipes
                .get(pkg_name.as_str())
                .ok_or_else(|| PipelineError::MissingRecipe(pkg_name.clone()))?;

            // --only filter: non-targeted packages require a cached derivation.
            if let Some(ref set) = build_set
                && !set.contains(pkg_name.as_str())
            {
                let dep_ids = collect_dep_ids(recipe, &acc.completed);
                let inputs = DerivationInputs {
                    source_hash: recipe_hash::source_hash(recipe),
                    build_script_hash: recipe_hash::build_script_hash(recipe),
                    dependency_ids: dep_ids,
                    build_env_hash: build_env_hash.clone(),
                    target_triple: self.config.target_triple.clone(),
                    build_options: BTreeMap::new(),
                };
                let derivation_id = DerivationId::compute(&inputs)
                    .map_err(|e| PipelineError::Io(format!("derivation ID: {e}")))?;

                let index = DerivationIndex::new(conn);
                let record = index
                    .lookup(derivation_id.as_str())
                    .map_err(|e| PipelineError::Io(format!("index lookup: {e}")))?
                    .ok_or_else(|| PipelineError::UncachedDependency {
                        package: pkg_name.clone(),
                    })?;

                let manifest = load_manifest_from_cas(&self.executor, &record.manifest_cas_hash)?;

                record_package(
                    &manifest,
                    &sysroot,
                    &self.config.cas_dir,
                    recipe,
                    &derivation_id,
                    pkg_name,
                    &mut acc,
                )?;

                on_event(&PipelineEvent::PackageCached {
                    name: pkg_name.clone(),
                });
                total_cached += 1;
                continue;
            }

            on_event(&PipelineEvent::PackageBuilding {
                name: pkg_name.clone(),
                stage: step.stage.to_string(),
            });

            let dep_ids = collect_dep_ids(recipe, &acc.completed);
            let start = Instant::now();

            let result = self.executor.execute(
                recipe,
                &build_env_hash,
                &dep_ids,
                &self.config.target_triple,
                &sysroot,
                conn,
            )?;

            match result {
                ExecutionResult::CacheHit {
                    derivation_id,
                    record,
                } => {
                    let manifest =
                        load_manifest_from_cas(&self.executor, &record.manifest_cas_hash)?;

                    record_package(
                        &manifest,
                        &sysroot,
                        &self.config.cas_dir,
                        recipe,
                        &derivation_id,
                        pkg_name,
                        &mut acc,
                    )?;

                    on_event(&PipelineEvent::PackageCached {
                        name: pkg_name.clone(),
                    });
                    total_cached += 1;
                }
                ExecutionResult::Built {
                    derivation_id,
                    output,
                } => {
                    let duration = start.elapsed().as_secs();
                    let manifest = output.manifest;

                    record_package(
                        &manifest,
                        &sysroot,
                        &self.config.cas_dir,
                        recipe,
                        &derivation_id,
                        pkg_name,
                        &mut acc,
                    )?;

                    on_event(&PipelineEvent::PackageBuilt {
                        name: pkg_name.clone(),
                        duration_secs: duration,
                    });

                    // Set trust level 2 (locally built).
                    let idx = DerivationIndex::new(conn);
                    let _ = idx.set_trust_level(derivation_id.as_str(), 2);

                    total_built += 1;
                }
            }
        }

        // Close the last phase.
        if let Some(last_phase) = current_stage {
            on_event(&PipelineEvent::StageCompleted {
                name: last_phase.to_string(),
            });
        }

        // Unmount the mutable environment before composing.
        if env.is_mounted()
            && let Err(e) = env.unmount()
        {
            warn!("Failed to unmount mutable environment: {e}");
        }

        // Compose a single EROFS image from all outputs at the end.
        if !acc.manifests.is_empty() {
            let manifest_refs: Vec<&OutputManifest> = acc.manifests.iter().collect();
            let compose_dir = self.config.work_dir.join("compose");
            std::fs::create_dir_all(&compose_dir).map_err(|e| PipelineError::Io(e.to_string()))?;

            let build_result = compose_erofs(&manifest_refs, &compose_dir)?;

            info!(
                "composed: {} objects, image={}",
                build_result.cas_objects_referenced,
                build_result.image_path.display(),
            );
        }

        let total = total_cached + total_built;
        on_event(&PipelineEvent::PipelineCompleted {
            total_packages: total,
            cached: total_cached,
            built: total_built,
        });

        let profile_stages = vec![ProfileStage {
            name: "pipeline".to_owned(),
            build_env: build_env_hash,
            derivations: acc.derivations,
        }];

        let mut profile = BuildProfile {
            profile: ProfileMetadata {
                manifest: "pipeline-chroot".to_owned(),
                profile_hash: String::new(),
                generated_at: now,
                target: self.config.target_triple.clone(),
            },
            seed: ProfileSeedRef {
                id: seed.metadata.seed_id.clone(),
                source: seed.metadata.source.to_string(),
            },
            stages: profile_stages,
        };

        profile.profile.profile_hash = profile.compute_hash();
        Ok(profile)
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Group build steps by stage in stage order, preserving build order within
/// each group. Used by [`Pipeline::generate_profile`] for dry-run planning.
fn ordered_stages(
    build_steps: &[crate::derivation::build_order::BuildStep],
) -> Vec<(Stage, Vec<String>)> {
    let mut by_stage: BTreeMap<Stage, Vec<(usize, String)>> = BTreeMap::new();

    for step in build_steps {
        by_stage
            .entry(step.stage)
            .or_default()
            .push((step.order, step.package.clone()));
    }

    let mut result = Vec::new();
    for (stage, mut pkgs) in by_stage {
        pkgs.sort_by_key(|(order, _)| *order);
        let names: Vec<String> = pkgs.into_iter().map(|(_, name)| name).collect();
        result.push((stage, names));
    }
    result
}

/// Collect dependency derivation IDs for a recipe from the completed map.
///
/// Only includes dependencies that have already been built (are in `completed`).
/// Cross-stage or external dependencies that are not in the map are silently
/// skipped -- the executor's derivation ID computation handles them being absent.
fn collect_dep_ids(
    recipe: &Recipe,
    completed: &HashMap<String, DerivationId>,
) -> BTreeMap<String, DerivationId> {
    let mut dep_ids = BTreeMap::new();

    for dep_name in recipe
        .build
        .requires
        .iter()
        .chain(&recipe.build.makedepends)
    {
        if let Some(id) = completed.get(dep_name.as_str()) {
            dep_ids.insert(dep_name.clone(), id.clone());
        }
    }

    dep_ids
}

/// Mutable accumulator state threaded through the build loop.
struct BuildAccumulator {
    /// Package name -> derivation ID for dependency resolution.
    completed: HashMap<String, DerivationId>,
    /// Profile entries for all processed packages.
    derivations: Vec<ProfileDerivation>,
    /// Output manifests for final EROFS composition.
    manifests: Vec<OutputManifest>,
}

/// Install a package into the live sysroot, record its derivation ID, and
/// push its manifest and profile entry into the accumulator.
///
/// This consolidates the repeated install-record-push pattern that appears
/// in the --only cache path, CacheHit arm, and Built arm of `Pipeline::execute`.
fn record_package(
    manifest: &OutputManifest,
    sysroot: &std::path::Path,
    cas_dir: &std::path::Path,
    recipe: &Recipe,
    derivation_id: &DerivationId,
    pkg_name: &str,
    acc: &mut BuildAccumulator,
) -> Result<(), PipelineError> {
    install_to_sysroot(manifest, sysroot, cas_dir)
        .map_err(|e| PipelineError::Install(e.to_string()))?;
    run_ldconfig_if_needed(manifest, sysroot);

    acc.derivations.push(ProfileDerivation {
        package: recipe.package.name.clone(),
        version: recipe.package.version.clone(),
        derivation_id: derivation_id.as_str().to_owned(),
    });
    acc.manifests.push(manifest.clone());
    acc.completed
        .insert(pkg_name.to_owned(), derivation_id.clone());
    Ok(())
}

/// Load an `OutputManifest` from CAS using the stored manifest hash.
fn load_manifest_from_cas(
    executor: &DerivationExecutor,
    manifest_cas_hash: &str,
) -> Result<OutputManifest, PipelineError> {
    let bytes = executor
        .cas()
        .retrieve(manifest_cas_hash)
        .map_err(|e| PipelineError::Io(format!("CAS retrieve manifest: {e}")))?;

    let toml_str = std::str::from_utf8(&bytes)
        .map_err(|e| PipelineError::Io(format!("manifest is not valid UTF-8: {e}")))?;

    let manifest: OutputManifest = toml::from_str(toml_str)
        .map_err(|e| PipelineError::Io(format!("manifest TOML parse: {e}")))?;

    Ok(manifest)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::derivation::compose::{ComposeError, erofs_image_hash};
    use crate::derivation::id::DerivationInputs;
    use crate::derivation::seed::{SeedMetadata, SeedSource};
    use crate::derivation::test_helpers::helpers::make_recipe;
    use std::collections::HashSet;
    use std::path::Path;

    /// Build a minimal test seed without a real EROFS image.
    fn test_seed(dir: &Path) -> Seed {
        let image_content = b"test seed image bytes for pipeline";
        let image_path = dir.join("seed.erofs");
        std::fs::write(&image_path, image_content).unwrap();

        let actual_hash = erofs_image_hash(&image_path).unwrap();

        Seed {
            metadata: SeedMetadata {
                seed_id: actual_hash,
                source: SeedSource::SelfBuilt,
                origin_url: None,
                builder: Some("test".to_owned()),
                packages: vec!["gcc".to_owned()],
                target_triple: "x86_64-unknown-linux-gnu".to_owned(),
                verified_by: vec![],
                origin_distro: None,
                origin_version: None,
            },
            image_path,
            cas_dir: dir.join("cas"),
        }
    }

    #[test]
    fn generate_profile_produces_correct_structure() {
        let dir = tempfile::tempdir().unwrap();
        let seed = test_seed(dir.path());

        let mut recipes = HashMap::new();
        recipes.insert("gcc-pass1".to_owned(), make_recipe("gcc-pass1", &[], &[]));
        recipes.insert(
            "gcc-pass2".to_owned(),
            make_recipe("gcc-pass2", &["gcc-pass1"], &[]),
        );
        recipes.insert("make".to_owned(), make_recipe("make", &[], &[]));
        recipes.insert("nginx".to_owned(), make_recipe("nginx", &[], &[]));

        let custom = HashSet::new();
        let build_steps =
            crate::derivation::build_order::compute_build_order(&recipes, &custom).unwrap();

        let profile = Pipeline::generate_profile(&seed, &recipes, &build_steps, "test-manifest");

        // Verify metadata.
        assert_eq!(profile.profile.manifest, "test-manifest");
        assert_eq!(profile.profile.target, "x86_64-unknown-linux-gnu");
        assert!(!profile.profile.profile_hash.is_empty());
        assert_eq!(profile.seed.id, seed.metadata.seed_id);

        // Should have stages (at least toolchain, foundation, system).
        assert!(
            !profile.stages.is_empty(),
            "profile should have at least one stage"
        );

        // All derivation IDs should be "pending".
        for stage in &profile.stages {
            assert_eq!(stage.build_env, "pending");
            for drv in &stage.derivations {
                assert_eq!(drv.derivation_id, "pending");
            }
        }

        // Verify total package count matches.
        let total_drvs: usize = profile.stages.iter().map(|s| s.derivations.len()).sum();
        assert_eq!(total_drvs, 4, "all 4 recipes should appear in the profile");
    }

    #[test]
    fn generate_profile_hash_is_deterministic() {
        let dir = tempfile::tempdir().unwrap();
        let seed = test_seed(dir.path());

        let mut recipes = HashMap::new();
        recipes.insert("a".to_owned(), make_recipe("a", &[], &[]));

        let custom = HashSet::new();
        let build_steps =
            crate::derivation::build_order::compute_build_order(&recipes, &custom).unwrap();

        let p1 = Pipeline::generate_profile(&seed, &recipes, &build_steps, "m");
        let p2 = Pipeline::generate_profile(&seed, &recipes, &build_steps, "m");

        // The hash should be the same even though generated_at differs.
        assert_eq!(p1.profile.profile_hash, p2.profile.profile_hash);
    }

    #[test]
    fn generate_profile_different_seeds_produce_different_hashes() {
        let dir1 = tempfile::tempdir().unwrap();
        let seed1 = test_seed(dir1.path());

        let dir2 = tempfile::tempdir().unwrap();
        // Write different content so the seed hash differs.
        std::fs::write(dir2.path().join("seed.erofs"), b"different seed content").unwrap();
        let hash2 = erofs_image_hash(&dir2.path().join("seed.erofs")).unwrap();
        let seed2 = Seed {
            metadata: SeedMetadata {
                seed_id: hash2,
                source: SeedSource::SelfBuilt,
                origin_url: None,
                builder: None,
                packages: vec![],
                target_triple: "x86_64-unknown-linux-gnu".to_owned(),
                verified_by: vec![],
                origin_distro: None,
                origin_version: None,
            },
            image_path: dir2.path().join("seed.erofs"),
            cas_dir: dir2.path().join("cas"),
        };

        let mut recipes = HashMap::new();
        recipes.insert("a".to_owned(), make_recipe("a", &[], &[]));

        let custom = HashSet::new();
        let build_steps =
            crate::derivation::build_order::compute_build_order(&recipes, &custom).unwrap();

        let p1 = Pipeline::generate_profile(&seed1, &recipes, &build_steps, "m");
        let p2 = Pipeline::generate_profile(&seed2, &recipes, &build_steps, "m");

        assert_ne!(p1.profile.profile_hash, p2.profile.profile_hash);
    }

    #[test]
    fn ordered_stages_groups_and_sorts_correctly() {
        use crate::derivation::build_order::BuildStep;

        let steps = vec![
            BuildStep {
                package: "nginx".to_owned(),
                stage: Stage::System,
                order: 3,
            },
            BuildStep {
                package: "gcc-pass1".to_owned(),
                stage: Stage::Toolchain,
                order: 0,
            },
            BuildStep {
                package: "make".to_owned(),
                stage: Stage::Foundation,
                order: 2,
            },
            BuildStep {
                package: "gcc-pass2".to_owned(),
                stage: Stage::Toolchain,
                order: 1,
            },
        ];

        let stages = ordered_stages(&steps);

        // Should be ordered: Toolchain, Foundation, System.
        assert_eq!(stages.len(), 3);
        assert_eq!(stages[0].0, Stage::Toolchain);
        assert_eq!(stages[0].1, vec!["gcc-pass1", "gcc-pass2"]);
        assert_eq!(stages[1].0, Stage::Foundation);
        assert_eq!(stages[1].1, vec!["make"]);
        assert_eq!(stages[2].0, Stage::System);
        assert_eq!(stages[2].1, vec!["nginx"]);
    }

    #[test]
    fn collect_dep_ids_picks_up_completed_deps() {
        let recipe = make_recipe("bash", &["glibc"], &["make"]);

        let glibc_id = DerivationId::compute(&DerivationInputs {
            source_hash: "src1".to_owned(),
            build_script_hash: "script1".to_owned(),
            dependency_ids: BTreeMap::new(),
            build_env_hash: "env1".to_owned(),
            target_triple: "x86_64-unknown-linux-gnu".to_owned(),
            build_options: BTreeMap::new(),
        })
        .unwrap();

        let mut completed = HashMap::new();
        completed.insert("glibc".to_owned(), glibc_id.clone());
        // "make" is NOT in completed -- should be skipped.

        let dep_ids = collect_dep_ids(&recipe, &completed);

        assert_eq!(dep_ids.len(), 1);
        assert_eq!(dep_ids.get("glibc").unwrap(), &glibc_id);
        assert!(!dep_ids.contains_key("make"));
    }

    #[test]
    fn empty_build_steps_produce_empty_profile() {
        let dir = tempfile::tempdir().unwrap();
        let seed = test_seed(dir.path());
        let recipes = HashMap::new();
        let build_steps: Vec<crate::derivation::build_order::BuildStep> = vec![];

        let profile = Pipeline::generate_profile(&seed, &recipes, &build_steps, "empty");

        assert!(profile.stages.is_empty());
    }

    #[test]
    fn pipeline_error_from_compose_error() {
        let ce = ComposeError::EmptyComposition;
        let pe: PipelineError = ce.into();
        assert!(matches!(pe, PipelineError::Compose(_)));
    }

    #[test]
    fn pipeline_error_from_executor_error() {
        let ee = ExecutorError::Build("test".to_owned());
        let pe: PipelineError = ee.into();
        assert!(matches!(pe, PipelineError::Executor(_)));
    }

    #[test]
    fn pipeline_config_fields() {
        let config = PipelineConfig {
            cas_dir: PathBuf::from("/tmp/cas"),
            work_dir: PathBuf::from("/tmp/work"),
            target_triple: "x86_64-unknown-linux-gnu".to_owned(),
            jobs: 4,
            log_dir: None,
            keep_logs: false,
            shell_on_failure: false,
            only_packages: None,
            cascade: false,
            substituter_sources: vec![],
            publish_endpoint: None,
            publish_token: None,
        };
        assert_eq!(config.jobs, 4);
        assert!(config.only_packages.is_none());
    }
}
