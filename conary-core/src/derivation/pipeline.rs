// conary-core/src/derivation/pipeline.rs

//! Full staged build pipeline orchestrator.
//!
//! [`Pipeline`] drives the complete CAS-layered bootstrap: for each stage
//! (Toolchain -> Foundation -> System -> Customization) it executes every
//! derivation in topological order, composes an EROFS image of the stage
//! outputs, and feeds the resulting `build_env_hash` into the next stage.
//!
//! [`Pipeline::generate_profile`] produces a dry-run [`BuildProfile`] without
//! executing any builds, marking all derivation IDs as "pending".

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::time::Instant;

use rusqlite::Connection;
use tracing::{info, warn};

use crate::recipe::Recipe;

use super::compose::{compose_erofs, erofs_image_hash, ComposeError};
use super::environment::BuildEnvironment;
use super::executor::{DerivationExecutor, ExecutionResult, ExecutorError};
use super::id::{DerivationId, DerivationInputs};
use super::index::DerivationIndex;
use super::output::OutputManifest;
use super::recipe_hash;
use super::profile::{
    BuildProfile, ProfileDerivation, ProfileMetadata, ProfileSeedRef, ProfileStage,
};
use super::seed::Seed;
use super::stages::{Stage, StageAssignment};

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
    /// Stop after completing this stage (inclusive).
    pub up_to_stage: Option<Stage>,
    /// Only build these packages. All other packages use cache lookups.
    pub only_packages: Option<Vec<String>>,
    /// When combined with `only_packages`, also rebuild reverse dependents.
    pub cascade: bool,
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
    #[error("compose error: {0}")]
    Compose(String),

    /// A general I/O error.
    #[error("I/O error: {0}")]
    Io(String),

    /// A dependency required by --only target has no cached derivation.
    #[error("package '{package}' depends on '{dependency}' which has no cached derivation -- run a full build first or add '{dependency}' to --only")]
    UncachedDependency {
        package: String,
        dependency: String,
    },

    /// A --only target is in a stage beyond the --up-to cutoff.
    #[error("package '{package}' is in stage '{stage}' but --up-to stops at '{cutoff}'")]
    PackageBeyondCutoff {
        package: String,
        stage: String,
        cutoff: String,
    },
}

impl From<ComposeError> for PipelineError {
    fn from(e: ComposeError) -> Self {
        PipelineError::Compose(e.to_string())
    }
}

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
        assignments: &[StageAssignment],
        manifest_path: &str,
    ) -> BuildProfile {
        let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();

        let stages_ordered = ordered_stages(assignments);
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
                source: format!("{:?}", seed.metadata.source).to_lowercase(),
            },
            stages: profile_stages,
        };

        profile.profile.profile_hash = profile.compute_hash();
        profile
    }

    /// Execute the full staged pipeline.
    ///
    /// For each stage (Toolchain, Foundation, System, Customization):
    /// 1. Emit `StageStarted`.
    /// 2. For each package in build order:
    ///    - Collect dependency derivation IDs from previously completed packages.
    ///    - Call `executor.execute()`.
    ///    - On `CacheHit`: load manifest from CAS, record in completed outputs.
    ///    - On `Built`: record in completed outputs.
    ///    - Emit the appropriate event.
    /// 3. Compose an EROFS image from stage outputs; compute new `build_env_hash`.
    /// 4. Emit `StageCompleted`.
    ///
    /// Returns a [`BuildProfile`] with all derivation IDs filled in.
    ///
    /// # Errors
    ///
    /// Returns [`PipelineError`] on missing recipes, executor failures,
    /// composition errors, or I/O errors.
    pub fn execute<F>(
        &self,
        seed: &Seed,
        recipes: &HashMap<String, Recipe>,
        assignments: &[StageAssignment],
        conn: &Connection,
        mut on_event: F,
    ) -> Result<BuildProfile, PipelineError>
    where
        F: FnMut(&PipelineEvent),
    {
        let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();

        let stages_ordered = ordered_stages(assignments);

        // Validate --only targets against --up-to cutoff
        if let (Some(only_pkgs), Some(cutoff)) = (&self.config.only_packages, &self.config.up_to_stage) {
            for pkg in only_pkgs {
                if let Some((stage, _)) = stages_ordered.iter().find(|(_, pkgs)| pkgs.contains(pkg))
                    && stage > cutoff
                {
                    return Err(PipelineError::PackageBeyondCutoff {
                        package: pkg.clone(),
                        stage: stage.to_string(),
                        cutoff: cutoff.to_string(),
                    });
                }
            }
        }

        let build_set = compute_build_set(
            self.config.only_packages.as_deref(),
            self.config.cascade,
            self.config.up_to_stage,
            recipes,
            &stages_ordered,
        );

        // Current build environment hash; starts from the seed.
        let mut build_env_hash = seed.build_env_hash().to_owned();

        // Track the current EROFS image path for mounting as build sysroot.
        // Starts as the seed image; updated after each stage's compose_erofs().
        let mut current_image_path = seed.image_path.clone();

        // Map package name -> (DerivationId, OutputManifest) for dependency resolution.
        let mut completed: BTreeMap<String, (DerivationId, OutputManifest)> = BTreeMap::new();

        let mut profile_stages = Vec::new();
        let mut total_cached: usize = 0;
        let mut total_built: usize = 0;

        for (stage, pkgs) in &stages_ordered {
            let stage_name = stage.to_string();

            on_event(&PipelineEvent::StageStarted {
                name: stage_name.clone(),
                package_count: pkgs.len(),
            });

            // Check --up-to: skip stages beyond the cutoff
            if let Some(cutoff) = self.config.up_to_stage
                && *stage > cutoff
            {
                break;
            }

            info!("stage {stage_name}: {} packages", pkgs.len());

            let mut stage_derivations = Vec::new();
            let mut stage_manifests: Vec<OutputManifest> = Vec::new();

            // Mount the current stage's EROFS image as the build sysroot.
            let sysroot = self.config.work_dir.join("sysroot");
            std::fs::create_dir_all(&sysroot)
                .map_err(|e| PipelineError::Io(e.to_string()))?;

            let mut build_env = BuildEnvironment::new(
                current_image_path.clone(),
                self.config.cas_dir.clone(),
                sysroot.clone(),
                build_env_hash.clone(),
            );
            // Mount will fail without root -- that's expected. Log and continue
            // with unmounted sysroot for non-root builds.
            if let Err(e) = build_env.mount() {
                warn!("Could not mount build environment (requires root): {e}");
            }

            for pkg_name in pkgs {
                let recipe = recipes
                    .get(pkg_name.as_str())
                    .ok_or_else(|| PipelineError::MissingRecipe(pkg_name.clone()))?;

                // --only filter: non-targeted packages require a cached derivation.
                if let Some(ref set) = build_set
                    && !set.contains(pkg_name.as_str())
                {
                    // Compute derivation ID and do cache-only lookup.
                    let dep_ids = collect_dep_ids(recipe, &completed);
                    let src_hash = recipe_hash::source_hash(recipe);
                    let script_hash = recipe_hash::build_script_hash(recipe);
                    let inputs = DerivationInputs {
                        source_hash: src_hash,
                        build_script_hash: script_hash,
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
                            dependency: pkg_name.clone(),
                        })?;

                    let manifest =
                        load_manifest_from_cas(&self.executor, &record.manifest_cas_hash)?;

                    on_event(&PipelineEvent::PackageCached {
                        name: pkg_name.clone(),
                    });

                    stage_derivations.push(ProfileDerivation {
                        package: recipe.package.name.clone(),
                        version: recipe.package.version.clone(),
                        derivation_id: derivation_id.as_str().to_owned(),
                    });

                    stage_manifests.push(manifest.clone());
                    completed.insert(pkg_name.clone(), (derivation_id, manifest));
                    total_cached += 1;
                    continue;
                }

                on_event(&PipelineEvent::PackageBuilding {
                    name: pkg_name.clone(),
                    stage: stage_name.clone(),
                });

                // Collect dependency derivation IDs from previously completed packages.
                let dep_ids = collect_dep_ids(recipe, &completed);

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
                        // Load manifest from CAS via the stored manifest_cas_hash.
                        let manifest =
                            load_manifest_from_cas(&self.executor, &record.manifest_cas_hash)?;

                        on_event(&PipelineEvent::PackageCached {
                            name: pkg_name.clone(),
                        });

                        stage_derivations.push(ProfileDerivation {
                            package: recipe.package.name.clone(),
                            version: recipe.package.version.clone(),
                            derivation_id: derivation_id.as_str().to_owned(),
                        });

                        stage_manifests.push(manifest.clone());
                        completed.insert(pkg_name.clone(), (derivation_id, manifest));
                        total_cached += 1;
                    }
                    ExecutionResult::Built {
                        derivation_id,
                        output,
                    } => {
                        let duration = start.elapsed().as_secs();

                        on_event(&PipelineEvent::PackageBuilt {
                            name: pkg_name.clone(),
                            duration_secs: duration,
                        });

                        stage_derivations.push(ProfileDerivation {
                            package: recipe.package.name.clone(),
                            version: recipe.package.version.clone(),
                            derivation_id: derivation_id.as_str().to_owned(),
                        });

                        let manifest = output.manifest;
                        stage_manifests.push(manifest.clone());
                        completed.insert(pkg_name.clone(), (derivation_id, manifest));
                        total_built += 1;
                    }
                }
            }

            // Unmount the build environment after all packages in this stage.
            if build_env.is_mounted() && let Err(e) = build_env.unmount() {
                warn!("Failed to unmount build environment: {e}");
            }

            // Compose EROFS from stage outputs and compute new build_env_hash.
            if !stage_manifests.is_empty() {
                let manifest_refs: Vec<&OutputManifest> = stage_manifests.iter().collect();
                let stage_dir = self.config.work_dir.join(format!("stage-{stage_name}"));
                std::fs::create_dir_all(&stage_dir)
                    .map_err(|e| PipelineError::Io(e.to_string()))?;

                let build_result = compose_erofs(&manifest_refs, &stage_dir)?;

                build_env_hash = erofs_image_hash(&build_result.image_path)?
                    .to_string();

                // Update the image path so the next stage mounts this
                // stage's composed EROFS as its build sysroot.
                current_image_path = build_result.image_path;

                info!(
                    "stage {stage_name} composed: {} objects, env_hash={}",
                    build_result.cas_objects_referenced,
                    &build_env_hash[..16.min(build_env_hash.len())],
                );
            }

            profile_stages.push(ProfileStage {
                name: stage_name.clone(),
                build_env: build_env_hash.clone(),
                derivations: stage_derivations,
            });

            on_event(&PipelineEvent::StageCompleted {
                name: stage_name,
            });
        }

        let total = total_cached + total_built;
        on_event(&PipelineEvent::PipelineCompleted {
            total_packages: total,
            cached: total_cached,
            built: total_built,
        });

        let mut profile = BuildProfile {
            profile: ProfileMetadata {
                manifest: "pipeline".to_owned(),
                profile_hash: String::new(),
                generated_at: now,
                target: self.config.target_triple.clone(),
            },
            seed: ProfileSeedRef {
                id: seed.metadata.seed_id.clone(),
                source: format!("{:?}", seed.metadata.source).to_lowercase(),
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

/// Group assignments by stage in stage order, preserving build_order within
/// each group.
fn ordered_stages(assignments: &[StageAssignment]) -> Vec<(Stage, Vec<String>)> {
    let mut by_stage: BTreeMap<Stage, Vec<(usize, String)>> = BTreeMap::new();

    for a in assignments {
        by_stage
            .entry(a.stage)
            .or_default()
            .push((a.build_order, a.package.clone()));
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
/// skipped — the executor's derivation ID computation handles them being absent.
fn collect_dep_ids(
    recipe: &Recipe,
    completed: &BTreeMap<String, (DerivationId, OutputManifest)>,
) -> BTreeMap<String, DerivationId> {
    let mut dep_ids = BTreeMap::new();

    for dep_name in &recipe.build.requires {
        if let Some((id, _)) = completed.get(dep_name.as_str()) {
            dep_ids.insert(dep_name.clone(), id.clone());
        }
    }

    for dep_name in &recipe.build.makedepends {
        if let Some((id, _)) = completed.get(dep_name.as_str()) {
            dep_ids.insert(dep_name.clone(), id.clone());
        }
    }

    dep_ids
}

/// Compute the set of packages to build based on --only and --cascade flags.
///
/// Returns `None` if `only_packages` is `None` (build everything).
/// When `cascade` is true, expands the set with reverse dependents via `RecipeGraph`.
/// When `up_to_stage` is set, filters out packages beyond the cutoff.
fn compute_build_set(
    only_packages: Option<&[String]>,
    cascade: bool,
    up_to_stage: Option<Stage>,
    recipes: &HashMap<String, Recipe>,
    assignments: &[(Stage, Vec<String>)],
) -> Option<HashSet<String>> {
    let targets = only_packages?;
    let mut build_set: HashSet<String> = targets.iter().cloned().collect();

    if cascade {
        use crate::recipe::RecipeGraph;
        let mut graph = RecipeGraph::new();
        for recipe in recipes.values() {
            graph.add_from_recipe(recipe);
        }

        let mut expanded = HashSet::new();
        for target in &build_set {
            for dep in graph.transitive_dependents(target) {
                expanded.insert(dep);
            }
        }
        build_set.extend(expanded);
    }

    // Filter by up_to_stage if set
    if let Some(cutoff) = up_to_stage {
        let allowed_packages: HashSet<String> = assignments
            .iter()
            .filter(|(stage, _)| *stage <= cutoff)
            .flat_map(|(_, pkgs)| pkgs.iter().cloned())
            .collect();

        let excluded: Vec<String> = build_set.difference(&allowed_packages).cloned().collect();
        for pkg in &excluded {
            warn!("skipping reverse dependent '{pkg}' due to --up-to {cutoff}");
        }
        build_set.retain(|p| allowed_packages.contains(p));
    }

    Some(build_set)
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
    use crate::derivation::compose::erofs_image_hash;
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
        let assignments =
            crate::derivation::stages::assign_stages(&recipes, &custom).unwrap();

        let profile =
            Pipeline::generate_profile(&seed, &recipes, &assignments, "test-manifest");

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
        let assignments =
            crate::derivation::stages::assign_stages(&recipes, &custom).unwrap();

        let p1 = Pipeline::generate_profile(&seed, &recipes, &assignments, "m");
        let p2 = Pipeline::generate_profile(&seed, &recipes, &assignments, "m");

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
            },
            image_path: dir2.path().join("seed.erofs"),
            cas_dir: dir2.path().join("cas"),
        };

        let mut recipes = HashMap::new();
        recipes.insert("a".to_owned(), make_recipe("a", &[], &[]));

        let custom = HashSet::new();
        let assignments =
            crate::derivation::stages::assign_stages(&recipes, &custom).unwrap();

        let p1 = Pipeline::generate_profile(&seed1, &recipes, &assignments, "m");
        let p2 = Pipeline::generate_profile(&seed2, &recipes, &assignments, "m");

        assert_ne!(p1.profile.profile_hash, p2.profile.profile_hash);
    }

    #[test]
    fn ordered_stages_groups_and_sorts_correctly() {
        let assignments = vec![
            StageAssignment {
                package: "nginx".to_owned(),
                stage: Stage::System,
                build_order: 3,
            },
            StageAssignment {
                package: "gcc-pass1".to_owned(),
                stage: Stage::Toolchain,
                build_order: 0,
            },
            StageAssignment {
                package: "make".to_owned(),
                stage: Stage::Foundation,
                build_order: 2,
            },
            StageAssignment {
                package: "gcc-pass2".to_owned(),
                stage: Stage::Toolchain,
                build_order: 1,
            },
        ];

        let stages = ordered_stages(&assignments);

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
        }).unwrap();

        let glibc_manifest = OutputManifest {
            derivation_id: glibc_id.as_str().to_owned(),
            output_hash: "out1".to_owned(),
            files: vec![],
            symlinks: vec![],
            build_duration_secs: 1,
            built_at: "2026-03-19T00:00:00Z".to_owned(),
        };

        let mut completed = BTreeMap::new();
        completed.insert("glibc".to_owned(), (glibc_id.clone(), glibc_manifest));
        // "make" is NOT in completed -- should be skipped.

        let dep_ids = collect_dep_ids(&recipe, &completed);

        assert_eq!(dep_ids.len(), 1);
        assert_eq!(dep_ids.get("glibc").unwrap(), &glibc_id);
        assert!(!dep_ids.contains_key("make"));
    }

    #[test]
    fn empty_assignments_produce_empty_profile() {
        let dir = tempfile::tempdir().unwrap();
        let seed = test_seed(dir.path());
        let recipes = HashMap::new();
        let assignments: Vec<StageAssignment> = vec![];

        let profile = Pipeline::generate_profile(&seed, &recipes, &assignments, "empty");

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
}
