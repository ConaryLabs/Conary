// conary-core/src/recipe/hermetic/plan.rs

use crate::error::{Error, Result};
use crate::hash;
use crate::recipe::BuildSystem;
use crate::recipe::format::{Recipe, SourceSection, is_remote_url};
use crate::recipe::hermetic::command_risk::{classify_build_commands, collect_recipe_command_text};
use crate::recipe::hermetic::ecosystem::evaluate_ecosystem_policy;
use crate::recipe::hermetic::evidence::{
    BuildInputIdentity, BuilderEnvironmentIdentity, BuilderEnvironmentKind, DependencyLock,
    EcosystemPolicyReport, HERMETIC_EVIDENCE_SCHEMA_V1, HermeticBuildEvidence, InputFileIdentity,
    LocalTreeIdentity, LockedRepositoryDependency, PolicyStatus, RecipeIdentity,
    SourceArchiveIdentity, SourceIdentity,
};
use crate::recipe::hermetic::reproducibility::ReproducibilityConfig;
use crate::recipe::hermetic::source_identity::{
    CanonicalLocalFile, CiMode, canonical_local_file_list, local_tree_identity,
};
use crate::recipe::kitchen::{KitchenConfig, SourceChecksumPolicy, SourceDownloadPolicy};
use std::fs;
use std::path::{Component, Path, PathBuf};

const DEFAULT_GENERATOR: &str = "conary-recipe-inference";

#[derive(Debug, Clone)]
pub struct HermeticBuildPlan {
    pub evidence: HermeticBuildEvidence,
    pub local_files: Option<Vec<CanonicalLocalFile>>,
    pub reproducibility: ReproducibilityConfig,
    pub recipe_source_base_dir: PathBuf,
}

#[derive(Debug, Clone)]
pub struct HermeticBuildInput {
    pub recipe_identity: RecipeIdentity,
    pub recipe_source_base_dir: PathBuf,
    pub generated_recipe: Option<Recipe>,
    pub inference_trace_hash: Option<String>,
    pub builder_environment: BuilderEnvironmentIdentity,
    pub locked_repository_dependencies: Vec<LockedRepositoryDependency>,
}

impl HermeticBuildInput {
    pub fn explicit_recipe(
        recipe_source_base_dir: impl Into<PathBuf>,
        recipe_path: impl AsRef<Path>,
        recipe_hash: impl Into<String>,
    ) -> Self {
        Self {
            recipe_identity: RecipeIdentity::ExplicitRecipe {
                path: recipe_path.as_ref().to_string_lossy().to_string(),
                hash: recipe_hash.into(),
            },
            recipe_source_base_dir: recipe_source_base_dir.into(),
            generated_recipe: None,
            inference_trace_hash: None,
            builder_environment: unconfigured_pristine_builder_environment(),
            locked_repository_dependencies: Vec::new(),
        }
    }

    pub fn try_generated_recipe(
        source_base: impl Into<PathBuf>,
        recipe: Recipe,
        inference_trace_hash: impl Into<String>,
    ) -> Result<Self> {
        let inference_trace_hash = inference_trace_hash.into();
        let canonical_hash = canonical_recipe_hash(&recipe)?;
        Ok(Self {
            recipe_identity: RecipeIdentity::GeneratedRecipe {
                generator: DEFAULT_GENERATOR.to_string(),
                canonical_hash,
                inference_trace_hash: inference_trace_hash.clone(),
            },
            recipe_source_base_dir: source_base.into(),
            generated_recipe: Some(recipe),
            inference_trace_hash: Some(inference_trace_hash),
            builder_environment: unconfigured_pristine_builder_environment(),
            locked_repository_dependencies: Vec::new(),
        })
    }

    pub fn generated_recipe(
        source_base: impl Into<PathBuf>,
        recipe: Recipe,
        inference_trace_hash: impl Into<String>,
    ) -> Self {
        Self::try_generated_recipe(source_base, recipe, inference_trace_hash)
            .expect("recipe serialization to canonical JSON should not fail")
    }

    pub fn with_builder_environment(
        mut self,
        builder_environment: BuilderEnvironmentIdentity,
    ) -> Self {
        self.builder_environment = builder_environment;
        self
    }

    pub fn with_pristine_builder_environment<S, T>(
        mut self,
        sysroot_hash: Option<S>,
        toolchain_hash: Option<T>,
    ) -> Self
    where
        S: Into<String>,
        T: Into<String>,
    {
        self.builder_environment = BuilderEnvironmentIdentity {
            kind: BuilderEnvironmentKind::Pristine,
            sysroot_hash: sysroot_hash.map(Into::into),
            toolchain_hash: toolchain_hash.map(Into::into),
            diagnostics: Vec::new(),
        };
        self
    }

    pub fn with_locked_repository_dependencies(
        mut self,
        locked_repository_dependencies: Vec<LockedRepositoryDependency>,
    ) -> Self {
        self.locked_repository_dependencies = locked_repository_dependencies;
        self
    }
}

impl HermeticBuildPlan {
    pub fn from_recipe(
        recipe: &Recipe,
        input: HermeticBuildInput,
        ci_mode: CiMode,
    ) -> Result<Self> {
        validate_builder_environment(&input.builder_environment)?;

        let recipe_source_base_dir = input.recipe_source_base_dir.clone();
        let mut diagnostics = input.builder_environment.diagnostics.clone();
        let (source, local_tree, local_files, source_root, source_diagnostics) =
            source_identity_for_recipe(recipe, &input.recipe_source_base_dir, ci_mode)?;
        diagnostics.extend(source_diagnostics);

        let additional_sources = additional_source_identities(recipe)?;
        let patches = patch_identities(recipe, &input.recipe_source_base_dir)?;
        let commands = collect_recipe_command_text(recipe);
        let command_risk = classify_build_commands(&commands);
        let command_text = commands
            .iter()
            .map(|command| command.content.as_str())
            .collect::<Vec<_>>()
            .join("\n");

        let ecosystem_policy = match source_root
            .as_deref()
            .and_then(infer_build_system_from_markers)
        {
            Some(build_system) => evaluate_ecosystem_policy(
                build_system,
                source_root.as_deref().unwrap(),
                &command_text,
            )?,
            None => EcosystemPolicyReport::clean("unknown"),
        };

        validate_repository_dependency_locks(recipe, &input.locked_repository_dependencies)?;

        let mut blockers = Vec::new();
        if ecosystem_policy.status == PolicyStatus::Blocked {
            let support_context = match ecosystem_policy.ecosystem.as_str() {
                "npm" | "python" | "go" => "M2a hermetic support unavailable: ",
                _ => "",
            };
            blockers.push(format!(
                "ecosystem policy blocked: {support_context}{}",
                report_diagnostics(&ecosystem_policy.diagnostics)
            ));
        }
        if command_risk.status == PolicyStatus::Blocked {
            blockers.push(format!(
                "command risk blocked: {}",
                command_risk
                    .entries
                    .iter()
                    .map(|entry| format!(
                        "{}:{}:{}:{}",
                        entry.phase, entry.command, entry.reason_code, entry.evidence
                    ))
                    .collect::<Vec<_>>()
                    .join("; ")
            ));
        }
        if !blockers.is_empty() {
            return Err(Error::ConfigError(format!(
                "hermetic build blocked: {}",
                blockers.join("; ")
            )));
        }

        let reproducibility = ReproducibilityConfig::default();
        let evidence = HermeticBuildEvidence {
            schema_version: HERMETIC_EVIDENCE_SCHEMA_V1,
            build_input: BuildInputIdentity {
                recipe: input.recipe_identity,
                source,
                additional_sources,
                patches,
                local_tree,
                ecosystem_dependencies: ecosystem_policy.identities.clone(),
                builder_environment: input.builder_environment,
            },
            dependency_lock: DependencyLock {
                repository_dependencies: input.locked_repository_dependencies,
            },
            ecosystem_policy,
            command_risk,
            reproducibility: reproducibility.record(),
            diagnostics,
        };

        Ok(Self {
            evidence,
            local_files,
            reproducibility,
            recipe_source_base_dir,
        })
    }

    pub fn apply_to_kitchen_config(&self, config: &mut KitchenConfig) {
        config.use_isolation = true;
        config.allow_network = false;
        config.pristine_mode = true;
        config.auto_makedepends = false;
        config.cleanup_makedepends = false;
        config.checksum_policy = SourceChecksumPolicy::Supported;
        config.source_download_policy = SourceDownloadPolicy::OfflineCacheOnly;
        config.recipe_source_base_dir = Some(self.recipe_source_base_dir.clone());
        config.hermetic_evidence = Some(self.evidence.clone());
        config.hermetic_local_files = self.local_files.clone();
        config.reproducibility = Some(self.reproducibility.clone());
    }
}

fn unconfigured_pristine_builder_environment() -> BuilderEnvironmentIdentity {
    BuilderEnvironmentIdentity {
        kind: BuilderEnvironmentKind::Pristine,
        sysroot_hash: None,
        toolchain_hash: None,
        diagnostics: vec!["builder environment identity not configured".to_string()],
    }
}

fn canonical_recipe_hash(recipe: &Recipe) -> Result<String> {
    let bytes = crate::json::canonical_json(recipe)
        .map_err(|error| Error::ConfigError(format!("failed to canonicalize recipe: {error}")))?;
    Ok(hash::sha256_prefixed(&bytes))
}

fn validate_builder_environment(builder: &BuilderEnvironmentIdentity) -> Result<()> {
    if builder.kind != BuilderEnvironmentKind::Pristine {
        return Err(Error::ConfigError(format!(
            "hermetic builds require pristine builder environment, got {:?}",
            builder.kind
        )));
    }

    let mut invalid_fields = Vec::new();
    let has_sysroot = match builder.sysroot_hash.as_deref() {
        Some(hash) if is_sha256_content_identity(hash) => true,
        Some(_) => {
            invalid_fields.push("sysroot_hash");
            false
        }
        None => false,
    };
    let has_toolchain = match builder.toolchain_hash.as_deref() {
        Some(hash) if is_sha256_content_identity(hash) => true,
        Some(_) => {
            invalid_fields.push("toolchain_hash");
            false
        }
        None => false,
    };
    if !invalid_fields.is_empty() {
        return Err(Error::ConfigError(format!(
            "builder environment identity fields must be sha256:<64 hex>: {}",
            invalid_fields.join(", ")
        )));
    }
    if !has_sysroot && !has_toolchain {
        let diagnostic = if builder.diagnostics.is_empty() {
            "missing sysroot_hash or toolchain_hash".to_string()
        } else {
            builder.diagnostics.join("; ")
        };
        return Err(Error::ConfigError(format!(
            "builder environment identity missing sha256 content identity: {diagnostic}"
        )));
    }

    Ok(())
}

fn is_sha256_content_identity(value: &str) -> bool {
    let Some(hex) = value.strip_prefix("sha256:") else {
        return false;
    };
    hex.len() == 64 && hex.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn source_identity_for_recipe(
    recipe: &Recipe,
    recipe_source_base_dir: &Path,
    ci_mode: CiMode,
) -> Result<(
    SourceIdentity,
    Option<LocalTreeIdentity>,
    Option<Vec<CanonicalLocalFile>>,
    Option<PathBuf>,
    Vec<String>,
)> {
    match &recipe.source {
        SourceSection::Local(source) => {
            let resolved = source
                .resolve_against(recipe_source_base_dir)
                .map_err(Error::ConfigError)?;
            let canonical_root = fs::canonicalize(&resolved).map_err(|error| {
                Error::NotFound(format!(
                    "Local source path not found: {} ({error})",
                    resolved.display()
                ))
            })?;
            let local_tree = local_tree_identity(&canonical_root, ci_mode)?;
            let local_files = canonical_local_file_list(&canonical_root, ci_mode)?;
            let source = SourceIdentity::LocalTree {
                root_display: canonical_root.to_string_lossy().to_string(),
                tree_hash: local_tree.tree_hash.clone(),
            };
            let diagnostics = local_tree.warnings.clone();
            Ok((
                source,
                Some(local_tree),
                Some(local_files),
                Some(canonical_root),
                diagnostics,
            ))
        }
        SourceSection::Remote(source) => {
            if source.checksum.trim().is_empty() {
                return Err(Error::ConfigError(
                    "remote source archive is missing checksum content identity".to_string(),
                ));
            }
            Ok((
                SourceIdentity::Archive {
                    url: recipe.archive_url(),
                    checksum: source.checksum.clone(),
                },
                None,
                None,
                None,
                Vec::new(),
            ))
        }
    }
}

fn additional_source_identities(recipe: &Recipe) -> Result<Vec<SourceArchiveIdentity>> {
    let Some(source) = recipe.remote_source() else {
        return Ok(Vec::new());
    };

    source
        .additional
        .iter()
        .map(|additional| {
            if additional.checksum.trim().is_empty() {
                return Err(Error::ConfigError(format!(
                    "additional source '{}' is missing checksum content identity",
                    additional.url
                )));
            }
            Ok(SourceArchiveIdentity {
                url: recipe.substitute(&additional.url, ""),
                checksum: additional.checksum.clone(),
                extracted: additional.extract,
                target: additional.extract_to.clone(),
            })
        })
        .collect()
}

fn patch_identities(
    recipe: &Recipe,
    recipe_source_base_dir: &Path,
) -> Result<Vec<InputFileIdentity>> {
    let Some(patches) = &recipe.patches else {
        return Ok(Vec::new());
    };

    patches
        .files
        .iter()
        .map(|patch| {
            let patch_file = recipe.substitute(&patch.file, "");
            if is_remote_url(&patch_file) {
                let checksum = patch.checksum.as_ref().ok_or_else(|| {
                    Error::ConfigError(format!(
                        "remote patch '{}' is missing checksum content identity",
                        patch.file
                    ))
                })?;
                if checksum.trim().is_empty() {
                    return Err(Error::ConfigError(format!(
                        "remote patch '{}' is missing checksum content identity",
                        patch.file
                    )));
                }
                return Ok(InputFileIdentity {
                    path: patch_file,
                    hash: checksum.clone(),
                });
            }

            let patch_path = resolve_local_patch_path(recipe_source_base_dir, &patch_file)?;
            Ok(InputFileIdentity {
                path: patch_path.to_string_lossy().to_string(),
                hash: sha256_file(&patch_path)?,
            })
        })
        .collect()
}

fn resolve_local_patch_path(recipe_source_base_dir: &Path, patch_file: &str) -> Result<PathBuf> {
    let relative_patch = clean_relative_local_patch_path(patch_file)?;
    let canonical_recipe_dir = fs::canonicalize(recipe_source_base_dir).map_err(|error| {
        Error::ConfigError(format!(
            "recipe source base dir not found for local patch resolution: {} ({error})",
            recipe_source_base_dir.display()
        ))
    })?;
    let patch_path = canonical_recipe_dir.join(relative_patch);
    let canonical_patch = fs::canonicalize(&patch_path).map_err(|error| {
        Error::NotFound(format!(
            "local patch file not found: {} ({error})",
            patch_path.display()
        ))
    })?;

    if !canonical_patch.starts_with(&canonical_recipe_dir) {
        return Err(Error::ConfigError(format!(
            "local patch path must stay within the recipe directory: {}",
            patch_file
        )));
    }

    Ok(canonical_patch)
}

fn clean_relative_local_patch_path(patch_file: &str) -> Result<PathBuf> {
    let path = Path::new(patch_file);
    if path.as_os_str().is_empty() {
        return Err(Error::ConfigError(
            "local patch path cannot be empty".to_string(),
        ));
    }
    if path.is_absolute() {
        return Err(Error::ConfigError(format!(
            "local patch path must be relative to the recipe directory: {patch_file}"
        )));
    }

    let mut clean = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => clean.push(part),
            Component::CurDir => {}
            Component::ParentDir => {
                return Err(Error::ConfigError(format!(
                    "local patch path must stay within the recipe directory: {patch_file}"
                )));
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(Error::ConfigError(format!(
                    "local patch path must be relative to the recipe directory: {patch_file}"
                )));
            }
        }
    }

    if clean.as_os_str().is_empty() {
        return Err(Error::ConfigError(
            "local patch path cannot be empty".to_string(),
        ));
    }

    Ok(clean)
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path).map_err(|error| {
        Error::NotFound(format!(
            "local input file not found: {} ({error})",
            path.display()
        ))
    })?;
    let hex = hash::sha256_reader_hex(&mut file)?;
    Ok(format!("sha256:{hex}"))
}

fn infer_build_system_from_markers(source_root: &Path) -> Option<BuildSystem> {
    if source_root.join("Cargo.toml").is_file() {
        return Some(BuildSystem::Cargo);
    }
    if source_root.join("package.json").is_file() {
        return Some(BuildSystem::Npm);
    }
    if source_root.join("pyproject.toml").is_file() || source_root.join("setup.py").is_file() {
        return Some(BuildSystem::Python);
    }
    if source_root.join("go.mod").is_file() {
        return Some(BuildSystem::Go);
    }
    if source_root.join("CMakeLists.txt").is_file() {
        return Some(BuildSystem::CMake);
    }
    if source_root.join("meson.build").is_file() {
        return Some(BuildSystem::Meson);
    }
    if source_root.join("configure.ac").is_file()
        || source_root.join("configure.in").is_file()
        || source_root.join("configure").is_file()
    {
        return Some(BuildSystem::Autotools);
    }
    None
}

fn validate_repository_dependency_locks(
    recipe: &Recipe,
    locked_repository_dependencies: &[LockedRepositoryDependency],
) -> Result<()> {
    for dependency in recipe.all_build_deps() {
        let Some(lock) = locked_repository_dependencies
            .iter()
            .find(|lock| lock.package == dependency)
        else {
            return Err(Error::ConfigError(format!(
                "build dependency '{dependency}' requires a locked repository dependency with content identity"
            )));
        };

        let missing = missing_lock_fields(lock);
        if !missing.is_empty() {
            return Err(Error::ConfigError(format!(
                "build dependency '{dependency}' has incomplete lock; missing {} including content identity",
                missing.join(", ")
            )));
        }
    }

    Ok(())
}

fn missing_lock_fields(lock: &LockedRepositoryDependency) -> Vec<&'static str> {
    let mut missing = Vec::new();
    if lock.repository_url.trim().is_empty() {
        missing.push("repository_url");
    }
    if lock.snapshot_version.trim().is_empty() {
        missing.push("snapshot_version");
    }
    if lock.package.trim().is_empty() {
        missing.push("package");
    }
    if lock.version.trim().is_empty() {
        missing.push("version");
    }
    if lock.release.trim().is_empty() {
        missing.push("release");
    }
    if !lock
        .architecture
        .as_deref()
        .is_some_and(|architecture| !architecture.trim().is_empty())
    {
        missing.push("architecture");
    }
    if lock.content_identity.trim().is_empty() {
        missing.push("content_identity");
    }
    missing
}

fn report_diagnostics(diagnostics: &[String]) -> String {
    if diagnostics.is_empty() {
        "no diagnostic details recorded".to_string()
    } else {
        diagnostics.join("; ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recipe::format::{
        BuildSection, LocalSourceSection, PackageSection, Recipe, SourceSection,
    };
    use crate::recipe::hermetic::{
        BuilderEnvironmentKind, CiMode, HERMETIC_EVIDENCE_SCHEMA_V1, PolicyStatus, SourceIdentity,
    };
    use crate::recipe::kitchen::{KitchenConfig, SourceChecksumPolicy, SourceDownloadPolicy};
    use crate::recipe::{PatchInfo, PatchSection};
    use std::collections::HashMap;
    use std::fs;
    use std::path::{Path, PathBuf};
    use tempfile::TempDir;

    const TEST_SYSROOT_IDENTITY: &str =
        "sha256:1111111111111111111111111111111111111111111111111111111111111111";
    const TEST_TOOLCHAIN_IDENTITY: &str =
        "sha256:2222222222222222222222222222222222222222222222222222222222222222";

    struct RecipeFixture {
        dir: TempDir,
        recipe: Recipe,
        recipe_path: PathBuf,
    }

    impl RecipeFixture {
        fn path(&self) -> &Path {
            self.dir.path()
        }

        fn recipe_path(&self) -> &Path {
            &self.recipe_path
        }
    }

    #[test]
    fn hermetic_plan_for_local_cargo_project_is_clean() {
        let fixture = cargo_project_with_lock(".");
        let recipe = fixture.recipe.clone();
        let input = HermeticBuildInput::generated_recipe(
            fixture.path(),
            recipe.clone(),
            "sha256:inference-trace",
        )
        .with_pristine_builder_environment(
            Some(TEST_SYSROOT_IDENTITY),
            Some(TEST_TOOLCHAIN_IDENTITY),
        );

        let plan = HermeticBuildPlan::from_recipe(&recipe, input, CiMode::Off).unwrap();

        assert_eq!(plan.evidence.schema_version, HERMETIC_EVIDENCE_SCHEMA_V1);
        assert_eq!(plan.evidence.ecosystem_policy.status, PolicyStatus::Clean);
        assert_eq!(plan.evidence.command_risk.status, PolicyStatus::Clean);
        assert_eq!(
            plan.evidence.build_input.builder_environment.kind,
            BuilderEnvironmentKind::Pristine
        );
        assert!(plan.local_files.is_some());
    }

    #[test]
    fn hermetic_plan_apply_sets_kitchen_hermetic_controls() {
        let fixture = cargo_project_with_lock(".");
        let recipe = fixture.recipe.clone();
        let input = HermeticBuildInput::generated_recipe(
            fixture.path(),
            recipe.clone(),
            "sha256:inference-trace",
        )
        .with_pristine_builder_environment(
            Some(TEST_SYSROOT_IDENTITY),
            Some(TEST_TOOLCHAIN_IDENTITY),
        );
        let plan = HermeticBuildPlan::from_recipe(&recipe, input, CiMode::Off).unwrap();
        let mut config = KitchenConfig {
            checksum_policy: SourceChecksumPolicy::BootstrapLegacy,
            ..KitchenConfig::with_auto_makedepends(true)
        };

        plan.apply_to_kitchen_config(&mut config);

        assert!(config.use_isolation);
        assert!(!config.allow_network);
        assert!(config.pristine_mode);
        assert!(!config.auto_makedepends);
        assert!(!config.cleanup_makedepends);
        assert_eq!(config.checksum_policy, SourceChecksumPolicy::Supported);
        assert_eq!(
            config.source_download_policy,
            SourceDownloadPolicy::OfflineCacheOnly
        );
        assert_eq!(config.hermetic_evidence, Some(plan.evidence.clone()));
        assert_eq!(config.hermetic_local_files, plan.local_files);
        assert_eq!(config.reproducibility, Some(plan.reproducibility));
        assert_eq!(
            config.recipe_source_base_dir,
            Some(fixture.path().to_path_buf())
        );
    }

    #[test]
    fn hermetic_plan_blocks_npm_fetch_command() {
        let fixture = npm_project();
        let recipe = fixture.recipe.clone();
        let input = HermeticBuildInput::generated_recipe(
            fixture.path(),
            recipe.clone(),
            "sha256:inference-trace",
        )
        .with_pristine_builder_environment(
            Some(TEST_SYSROOT_IDENTITY),
            Some(TEST_TOOLCHAIN_IDENTITY),
        );

        let error = HermeticBuildPlan::from_recipe(&recipe, input, CiMode::Off).unwrap_err();

        assert!(error.to_string().contains("npm"));
        assert!(error.to_string().contains("M2a hermetic support"));
    }

    #[test]
    fn hermetic_plan_blocks_unlocked_build_dependencies() {
        let fixture = cargo_project_with_lock(".");
        let mut recipe = fixture.recipe.clone();
        recipe.build.makedepends = vec!["openssl-devel".to_string()];
        let input = HermeticBuildInput::explicit_recipe(
            fixture.path(),
            fixture.recipe_path(),
            "sha256:recipe",
        )
        .with_pristine_builder_environment(
            Some(TEST_SYSROOT_IDENTITY),
            Some(TEST_TOOLCHAIN_IDENTITY),
        );

        let error = HermeticBuildPlan::from_recipe(&recipe, input, CiMode::Off).unwrap_err();

        assert!(error.to_string().contains("build dependency"));
        assert!(error.to_string().contains("content identity"));
    }

    #[test]
    fn hermetic_plan_resolves_source_path_relative_to_recipe_base() {
        let fixture = cargo_project_with_lock("src");
        fs::write(fixture.path().join("README.md"), "outside source root\n").unwrap();
        let input = HermeticBuildInput::explicit_recipe(
            fixture.path(),
            fixture.recipe_path(),
            "sha256:recipe",
        )
        .with_pristine_builder_environment(
            Some(TEST_SYSROOT_IDENTITY),
            Some(TEST_TOOLCHAIN_IDENTITY),
        );

        let plan = HermeticBuildPlan::from_recipe(&fixture.recipe, input, CiMode::Off).unwrap();
        let local_files = plan.local_files.as_ref().unwrap();

        assert!(matches!(
            &plan.evidence.build_input.source,
            SourceIdentity::LocalTree { root_display, .. } if root_display.ends_with("/src")
        ));
        assert!(
            local_files
                .iter()
                .any(|file| file.relative_path == Path::new("Cargo.toml"))
        );
        assert!(
            local_files
                .iter()
                .all(|file| !file.relative_path.starts_with(".."))
        );
        assert!(
            local_files
                .iter()
                .all(|file| file.relative_path != Path::new("README.md"))
        );
    }

    #[test]
    fn hermetic_plan_blocks_default_builder_identity() {
        let fixture = cargo_project_with_lock(".");
        let recipe = fixture.recipe.clone();
        let input = HermeticBuildInput::generated_recipe(
            fixture.path(),
            recipe.clone(),
            "sha256:inference-trace",
        );

        let error = HermeticBuildPlan::from_recipe(&recipe, input, CiMode::Off).unwrap_err();

        assert!(error.to_string().contains("builder environment identity"));
        assert!(error.to_string().contains("sha256"));
    }

    #[test]
    fn hermetic_plan_rejects_placeholder_builder_identity() {
        let fixture = cargo_project_with_lock(".");
        let recipe = fixture.recipe.clone();
        let input = HermeticBuildInput::generated_recipe(
            fixture.path(),
            recipe.clone(),
            "sha256:inference-trace",
        )
        .with_pristine_builder_environment(
            Some("sha256:m2a-pristine-sysroot-test"),
            Some(TEST_TOOLCHAIN_IDENTITY),
        );

        let error = HermeticBuildPlan::from_recipe(&recipe, input, CiMode::Off).unwrap_err();

        assert!(error.to_string().contains("builder environment identity"));
        assert!(error.to_string().contains("sha256"));
    }

    #[test]
    fn hermetic_plan_rejects_parent_traversal_local_patch() {
        let fixture = cargo_project_with_lock(".");
        let mut recipe = fixture.recipe.clone();
        recipe.patches = Some(PatchSection {
            files: vec![PatchInfo {
                file: "../outside.patch".to_string(),
                checksum: None,
                strip: 1,
                condition: None,
            }],
        });
        let input = HermeticBuildInput::explicit_recipe(
            fixture.path(),
            fixture.recipe_path(),
            "sha256:recipe",
        )
        .with_pristine_builder_environment(
            Some(TEST_SYSROOT_IDENTITY),
            Some(TEST_TOOLCHAIN_IDENTITY),
        );

        let error = HermeticBuildPlan::from_recipe(&recipe, input, CiMode::Off).unwrap_err();

        assert!(error.to_string().contains("local patch"));
        assert!(error.to_string().contains("recipe directory"));
    }

    #[test]
    fn hermetic_plan_rejects_absolute_local_patch() {
        let fixture = cargo_project_with_lock(".");
        let mut recipe = fixture.recipe.clone();
        recipe.patches = Some(PatchSection {
            files: vec![PatchInfo {
                file: fixture
                    .path()
                    .join("local.patch")
                    .to_string_lossy()
                    .to_string(),
                checksum: None,
                strip: 1,
                condition: None,
            }],
        });
        let input = HermeticBuildInput::explicit_recipe(
            fixture.path(),
            fixture.recipe_path(),
            "sha256:recipe",
        )
        .with_pristine_builder_environment(
            Some(TEST_SYSROOT_IDENTITY),
            Some(TEST_TOOLCHAIN_IDENTITY),
        );

        let error = HermeticBuildPlan::from_recipe(&recipe, input, CiMode::Off).unwrap_err();

        assert!(error.to_string().contains("local patch"));
        assert!(error.to_string().contains("relative"));
    }

    #[test]
    fn hermetic_plan_records_substituted_local_patch_identity() {
        let fixture = cargo_project_with_lock(".");
        let patch_dir = fixture.path().join("patches");
        fs::create_dir_all(&patch_dir).unwrap();
        let patch_bytes = b"diff --git a/file.txt b/file.txt\n";
        fs::write(patch_dir.join("0.1.0.patch"), patch_bytes).unwrap();
        let mut recipe = fixture.recipe.clone();
        recipe.patches = Some(PatchSection {
            files: vec![PatchInfo {
                file: "patches/%(version)s.patch".to_string(),
                checksum: None,
                strip: 1,
                condition: None,
            }],
        });
        let input = HermeticBuildInput::explicit_recipe(
            fixture.path(),
            fixture.recipe_path(),
            "sha256:recipe",
        )
        .with_pristine_builder_environment(
            Some(TEST_SYSROOT_IDENTITY),
            Some(TEST_TOOLCHAIN_IDENTITY),
        );

        let plan = HermeticBuildPlan::from_recipe(&recipe, input, CiMode::Off).unwrap();
        let patch = &plan.evidence.build_input.patches[0];

        assert!(patch.path.ends_with("patches/0.1.0.patch"));
        assert_eq!(patch.hash, crate::hash::sha256_prefixed(patch_bytes));
    }

    #[test]
    fn generated_recipe_hash_is_stable_across_variables_insertion_order() {
        let dir = tempfile::tempdir().unwrap();
        let mut first = recipe_with_local_source(".", Some("true"), Some("true"));
        first.variables.insert("zebra".to_string(), "1".to_string());
        first.variables.insert("apple".to_string(), "2".to_string());
        let mut second = recipe_with_local_source(".", Some("true"), Some("true"));
        second
            .variables
            .insert("apple".to_string(), "2".to_string());
        second
            .variables
            .insert("zebra".to_string(), "1".to_string());

        let first_input =
            HermeticBuildInput::generated_recipe(dir.path(), first, "sha256:inference-trace");
        let second_input =
            HermeticBuildInput::generated_recipe(dir.path(), second, "sha256:inference-trace");

        assert_eq!(first_input.recipe_identity, second_input.recipe_identity);
    }

    fn cargo_project_with_lock(source_path: &str) -> RecipeFixture {
        let dir = tempfile::tempdir().unwrap();
        let source_root = dir.path().join(source_path);
        fs::create_dir_all(source_root.join("src")).unwrap();
        fs::write(
            source_root.join("Cargo.toml"),
            r#"[package]
name = "hello"
version = "0.1.0"
edition = "2021"
"#,
        )
        .unwrap();
        fs::write(
            source_root.join("Cargo.lock"),
            r#"# This file is automatically @generated by Cargo.
version = 3

[[package]]
name = "hello"
version = "0.1.0"
"#,
        )
        .unwrap();
        fs::write(
            source_root.join("src").join("main.rs"),
            "fn main() { println!(\"hello\"); }\n",
        )
        .unwrap();
        fs::write(dir.path().join("local.patch"), "diff --git a/a b/a\n").unwrap();

        RecipeFixture {
            recipe: recipe_with_local_source(
                source_path,
                Some("cargo build --release --locked --offline"),
                Some("install -Dm755 target/release/hello %(destdir)s/usr/bin/hello"),
            ),
            recipe_path: dir.path().join("recipe.toml"),
            dir,
        }
    }

    fn npm_project() -> RecipeFixture {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("package.json"),
            r#"{"scripts":{"build":"node build.js"}}"#,
        )
        .unwrap();
        fs::write(dir.path().join("package-lock.json"), "{}\n").unwrap();
        fs::write(dir.path().join("local.patch"), "diff --git a/a b/a\n").unwrap();

        RecipeFixture {
            recipe: recipe_with_local_source(
                ".",
                Some("npm ci --omit=dev"),
                Some("mkdir -p %(destdir)s/usr/lib/hello && cp -a . %(destdir)s/usr/lib/hello"),
            ),
            recipe_path: dir.path().join("recipe.toml"),
            dir,
        }
    }

    fn recipe_with_local_source(
        source_path: &str,
        make: Option<&str>,
        install: Option<&str>,
    ) -> Recipe {
        Recipe {
            package: PackageSection {
                name: "hello".to_string(),
                version: "0.1.0".to_string(),
                release: "1".to_string(),
                summary: None,
                description: None,
                license: None,
                homepage: None,
            },
            source: SourceSection::Local(LocalSourceSection {
                path: PathBuf::from(source_path),
            }),
            build: BuildSection {
                requires: Vec::new(),
                makedepends: Vec::new(),
                configure: None,
                make: make.map(str::to_string),
                install: install.map(str::to_string),
                check: None,
                setup: None,
                post_install: None,
                workdir: None,
                environment: HashMap::new(),
                jobs: None,
                script_file: None,
                stage: None,
            },
            patches: Some(PatchSection {
                files: vec![PatchInfo {
                    file: "local.patch".to_string(),
                    checksum: None,
                    strip: 1,
                    condition: None,
                }],
            }),
            cross: None,
            components: None,
            variables: HashMap::new(),
        }
    }
}
