// conary-core/src/recipe/hermetic/plan.rs

use crate::error::{Error, Result};
use crate::hash;
use crate::recipe::format::{Recipe, SourceSection, is_remote_url};
use crate::recipe::hermetic::command_risk::{classify_build_commands, collect_recipe_command_text};
use crate::recipe::hermetic::evidence::{
    BuildInputIdentity, BuilderEnvironmentIdentity, BuilderEnvironmentKind, DependencyLock,
    HERMETIC_EVIDENCE_SCHEMA, HermeticBuildEvidence, InputFileIdentity, LocalTreeIdentity,
    LockedRepositoryDependency, RecipeIdentity, SourceArchiveIdentity, SourceIdentity,
};
use crate::recipe::hermetic::reproducibility::ReproducibilityConfig;
use crate::recipe::hermetic::source_identity::{
    CanonicalLocalFile, CiMode, canonical_local_file_list, local_tree_identity,
};
use crate::recipe::kitchen::{KitchenConfig, SourceDownloadPolicy};
use std::fs;
use std::path::{Component, Path, PathBuf};

type RecipeSourceIdentity = (
    SourceIdentity,
    Option<LocalTreeIdentity>,
    Option<Vec<CanonicalLocalFile>>,
    Option<PathBuf>,
    Vec<String>,
);

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
            builder_environment: unconfigured_pristine_builder_environment(),
            locked_repository_dependencies: Vec::new(),
        }
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
        let (source, local_tree, local_files, _source_root, source_diagnostics) =
            source_identity_for_recipe(recipe, &input.recipe_source_base_dir, ci_mode)?;
        diagnostics.extend(source_diagnostics);

        let additional_sources = additional_source_identities(recipe)?;
        let patches = patch_identities(recipe, &input.recipe_source_base_dir)?;
        let commands = collect_recipe_command_text(recipe);
        let command_risk = classify_build_commands(&commands);

        validate_repository_dependency_locks(recipe, &input.locked_repository_dependencies)?;

        let reproducibility = ReproducibilityConfig::default();
        let evidence = HermeticBuildEvidence {
            schema_version: HERMETIC_EVIDENCE_SCHEMA,
            build_input: BuildInputIdentity {
                recipe: input.recipe_identity,
                source,
                additional_sources,
                patches,
                local_tree,
                builder_environment: input.builder_environment,
            },
            dependency_lock: DependencyLock {
                repository_dependencies: input.locked_repository_dependencies,
            },
            command_risk,
            reproducibility: reproducibility.record(),
            divergence: Default::default(),
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
) -> Result<RecipeSourceIdentity> {
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
    if lock
        .architecture
        .as_deref()
        .is_none_or(|architecture| architecture.trim().is_empty())
    {
        missing.push("architecture");
    }
    if lock.content_identity.trim().is_empty() {
        missing.push("content_identity");
    }
    missing
}

#[cfg(test)]
mod tests;
