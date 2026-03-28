// conary-core/src/model/mod.rs

//! System Model - Declarative OS State Management
//!
//! The system model provides a declarative way to specify the desired state
//! of a system. Instead of running individual install/remove commands, users
//! define a model file that describes what packages should be installed.
//!
//! Conary then computes the diff between current state and desired state,
//! and applies the necessary changes.
//!
//! # Example system.toml
//!
//! ```toml
//! [model]
//! version = 1
//!
//! # Package search path (checked in order)
//! search = [
//!     "fedora@f41:stable",
//!     "conary@extras:stable",
//! ]
//!
//! # Packages to install (and keep installed)
//! install = [
//!     "nginx",
//!     "postgresql",
//!     "redis",
//! ]
//!
//! # Packages to exclude (never install, even as dependencies)
//! exclude = [
//!     "sendmail",
//!     "postfix",
//! ]
//!
//! # Pinned versions (glob patterns supported)
//! [pin]
//! openssl = "3.0.*"
//! kernel = "6.12.*"
//!
//! # Optional packages (install if available, no error if missing)
//! [optional]
//! packages = ["nginx-module-geoip"]
//! ```

mod diff;
pub mod lockfile;
pub mod parser;
pub mod remote;
mod replatform;
pub mod signing;
mod state;

pub use diff::{
    ApplyOptions, DiffAction, ModelDiff, ModelDiffSummary, ReplatformEstimate, ReplatformStatus,
    compute_diff, compute_diff_from_resolved, compute_diff_with_includes,
    compute_diff_with_includes_offline,
};
pub use parser::{
    AiAssistConfig,
    AiAssistMode,
    AiFeature,
    AutomationCategory,
    // Automation config types
    AutomationConfig,
    AutomationMode,
    ConflictStrategy,
    ConvergenceIntent,
    DerivedPackage as ModelDerivedPackage,
    // Federation config types
    FederationConfig,
    FederationTier,
    IncludeConfig,
    MajorUpgradeAutomation,
    ModelConfig,
    OrphanAutomation,
    RepairAutomation,
    RollbackTrigger,
    SecurityAutomation,
    SystemModel,
    UpdateAutomation,
    parse_model_file,
};
pub use replatform::{
    ReplatformBlockedReason, ReplatformExecutionPlan, ReplatformExecutionTransaction,
    SourcePolicyReplatformSnapshot, VisibleRealignmentCandidates, VisibleRealignmentProposal,
    planned_replatform_actions, replatform_estimate_from_affinities, replatform_execution_plan,
    source_policy_replatform_snapshot, visible_realignment_candidates,
};
pub use state::{InstalledPackage, SystemState, capture_current_state, snapshot_to_model};

use rusqlite::Connection;
use std::collections::{HashMap, HashSet};
use tracing::warn;
use std::path::Path;
use thiserror::Error;

/// Default path for the system model file
pub const DEFAULT_MODEL_PATH: &str = "/etc/conary/system.toml";

/// Errors that can occur when working with system models
#[derive(Debug, Error)]
pub enum ModelError {
    #[error("Failed to read model file: {0}")]
    ReadError(#[from] std::io::Error),

    #[error("Failed to parse model file: {0}")]
    ParseError(#[from] toml::de::Error),

    #[error("Invalid model version: expected {expected}, found {found}")]
    VersionMismatch { expected: u32, found: u32 },

    #[error("Database error: {0}")]
    DatabaseError(String),

    #[error("Invalid search path: {0}")]
    InvalidSearchPath(String),

    #[error("Conflicting package specifications: {0}")]
    ConflictingSpecs(String),

    #[error("Pin pattern invalid: {0}")]
    InvalidPinPattern(String),

    #[error("Remote fetch failed: {0}")]
    RemoteFetchError(String),

    #[error("Remote collection not found: {0}")]
    RemoteNotFound(String),
}

/// Result type for model operations
pub type ModelResult<T> = Result<T, ModelError>;

/// Load a system model from the default or specified path
pub fn load_model(path: Option<&Path>) -> ModelResult<SystemModel> {
    let path = path.unwrap_or_else(|| Path::new(DEFAULT_MODEL_PATH));
    parse_model_file(path)
}

/// Check if a system model file exists
pub fn model_exists(path: Option<&Path>) -> bool {
    let path = path.unwrap_or_else(|| Path::new(DEFAULT_MODEL_PATH));
    path.exists()
}

/// A composition layer in the resolved model
#[derive(Debug, Clone)]
pub struct ModelLayer {
    /// Layer name (e.g. "local", "group-base@repo:stable")
    pub name: String,
    /// Packages contributed by this layer
    pub packages: Vec<String>,
    /// Whether this is the local model layer
    pub is_local: bool,
}

/// A resolved model with all includes expanded
#[derive(Debug, Clone)]
pub struct ResolvedModel {
    /// All packages to install (from local model + includes)
    pub install: Vec<String>,

    /// Merged pin constraints (package -> version pattern)
    pub pins: HashMap<String, String>,

    /// All optional packages
    pub optionals: Vec<String>,

    /// All excluded packages
    pub exclude: Vec<String>,

    /// Search path (from local model)
    pub search: Vec<String>,

    /// Source of each package (for debugging/display)
    pub sources: HashMap<String, String>,

    /// Ordered layers showing composition precedence (first = lowest priority)
    pub layers: Vec<ModelLayer>,

    /// Fast lookup set for install + optional membership checks during resolution.
    /// Mirrors `install` and `optionals` for O(1) `contains` checks.
    #[doc(hidden)]
    pub known_packages: HashSet<String>,
}

impl ResolvedModel {
    /// Create a resolved model from a base system model (no includes resolved yet)
    pub fn from_model(model: &SystemModel) -> Self {
        let mut sources = HashMap::new();
        let mut known_packages = HashSet::new();
        for pkg in &model.config.install {
            sources.insert(pkg.clone(), "local".to_string());
            known_packages.insert(pkg.clone());
        }
        for pkg in &model.optional.packages {
            sources.insert(pkg.clone(), "local (optional)".to_string());
            known_packages.insert(pkg.clone());
        }

        Self {
            install: model.config.install.clone(),
            pins: model.pin.clone(),
            optionals: model.optional.packages.clone(),
            exclude: model.config.exclude.clone(),
            search: model.config.search.clone(),
            sources,
            layers: vec![ModelLayer {
                name: "local".to_string(),
                packages: model.config.install.clone(),
                is_local: true,
            }],
            known_packages,
        }
    }
}

/// A member spec from an included collection
#[derive(Debug, Clone)]
pub struct IncludedMember {
    pub name: String,
    pub version_constraint: Option<String>,
    pub is_optional: bool,
}

/// Fetched collection data from a remote include
#[derive(Debug, Clone)]
pub struct FetchedCollection {
    pub name: String,
    pub members: Vec<IncludedMember>,
    /// Nested includes (collections can include other collections)
    pub includes: Vec<String>,
    /// Version pins from the remote collection
    pub pins: std::collections::HashMap<String, String>,
    /// Package exclusions from the remote collection
    pub exclude: Vec<String>,
}

/// Parse a trove spec like "group-base@repo:branch" or just "group-base"
///
/// Returns (name, optional label spec)
pub fn parse_trove_spec(spec: &str) -> ModelResult<(String, Option<String>)> {
    if let Some((name, label)) = spec.split_once('@') {
        Ok((name.to_string(), Some(label.to_string())))
    } else {
        Ok((spec.to_string(), None))
    }
}

/// Fetch a collection from local database or repository
///
/// First checks if the collection exists locally (as a trove with type=collection).
/// If not found locally, attempts to fetch from repositories matching the label spec.
async fn fetch_collection(
    conn: &Connection,
    name: &str,
    label: Option<&str>,
    offline: bool,
    require_signatures: bool,
    trusted_keys: &[String],
) -> ModelResult<FetchedCollection> {
    use crate::db::models::{CollectionMember, Trove, TroveType};

    // First, try to find locally
    let troves =
        Trove::find_by_name(conn, name).map_err(|e| ModelError::DatabaseError(e.to_string()))?;

    let collection = troves
        .into_iter()
        .find(|t| t.trove_type == TroveType::Collection);

    if let Some(coll) = collection {
        let coll_id = coll
            .id
            .ok_or_else(|| ModelError::DatabaseError("Collection has no ID".to_string()))?;

        let members = CollectionMember::find_by_collection(conn, coll_id)
            .map_err(|e| ModelError::DatabaseError(e.to_string()))?;

        let fetched_members: Vec<IncludedMember> = members
            .into_iter()
            .map(|m| IncludedMember {
                name: m.member_name,
                version_constraint: m.member_version,
                is_optional: m.is_optional,
            })
            .collect();

        return Ok(FetchedCollection {
            name: name.to_string(),
            members: fetched_members,
            includes: Vec::new(), // Local collections don't have nested includes (yet)
            pins: std::collections::HashMap::new(),
            exclude: Vec::new(),
        });
    }

    // Remote fetch via label
    if let Some(label_str) = label {
        return remote::fetch_and_verify_remote_collection(
            conn,
            name,
            label_str,
            offline,
            require_signatures,
            trusted_keys,
        )
        .await;
    }

    // No label, no local match
    Err(ModelError::InvalidSearchPath(format!(
        "Collection '{}' not found locally and no label specified for remote fetch",
        name
    )))
}

/// Maximum depth for nested include resolution (prevents unbounded recursion)
const MAX_INCLUDE_DEPTH: usize = 10;

/// Resolve all includes in a system model
///
/// This performs a two-pass resolution:
/// 1. Collect all include specs
/// 2. Fetch each collection, detect cycles, and merge members
///
/// Returns a fully resolved model with all includes expanded.
pub async fn resolve_includes(
    model: &SystemModel,
    conn: &Connection,
) -> ModelResult<ResolvedModel> {
    resolve_includes_with_options(model, conn, false).await
}

/// Resolve includes with offline mode support
///
/// When `offline` is true, only cached remote collections are used (no HTTP).
pub async fn resolve_includes_with_options(
    model: &SystemModel,
    conn: &Connection,
    offline: bool,
) -> ModelResult<ResolvedModel> {
    let mut resolved = ResolvedModel::from_model(model);

    if model.include.models.is_empty() {
        return Ok(resolved);
    }

    if !model.include.require_signatures {
        warn!(
            "Remote includes are configured with signature verification disabled; unsigned collections may be accepted"
        );
    }

    let mut visited: HashSet<String> = HashSet::new();

    resolve_includes_recursive(
        &model.include.models,
        &model.include.on_conflict,
        conn,
        &mut resolved,
        &mut visited,
        offline,
        model.include.require_signatures,
        &model.include.trusted_keys,
        0,
    )
    .await?;

    Ok(resolved)
}

#[allow(clippy::too_many_arguments)]
async fn resolve_includes_recursive(
    includes: &[String],
    on_conflict: &parser::ConflictStrategy,
    conn: &Connection,
    resolved: &mut ResolvedModel,
    visited: &mut HashSet<String>,
    offline: bool,
    require_signatures: bool,
    trusted_keys: &[String],
    depth: usize,
) -> ModelResult<()> {
    if depth > MAX_INCLUDE_DEPTH {
        return Err(ModelError::ConflictingSpecs(format!(
            "Include depth limit exceeded (max {}): possible circular or deeply nested includes",
            MAX_INCLUDE_DEPTH
        )));
    }

    for include_spec in includes {
        // Cycle detection: only flag if spec is on the current recursion stack.
        // Using a stack-based approach so diamond includes (A->B->D, A->C->D)
        // don't falsely trigger cycle detection when D is seen via two paths.
        if visited.contains(include_spec) {
            return Err(ModelError::ConflictingSpecs(format!(
                "Circular include detected: {}",
                include_spec
            )));
        }
        visited.insert(include_spec.clone());

        // Parse "group-name@repo:branch" or "group-name"
        let (name, label) = parse_trove_spec(include_spec)?;

        // Fetch collection from local DB or repository
        let collection = fetch_collection(
            conn,
            &name,
            label.as_deref(),
            offline,
            require_signatures,
            trusted_keys,
        )
        .await?;

        // Recursively resolve nested includes if the collection has them
        if !collection.includes.is_empty() {
            Box::pin(resolve_includes_recursive(
                &collection.includes,
                on_conflict,
                conn,
                resolved,
                visited,
                offline,
                require_signatures,
                trusted_keys,
                depth + 1,
            ))
            .await?;
        }

        // Remove from stack on backtrack so diamond includes work correctly
        visited.remove(include_spec);

        // Track packages contributed by this include for layer info
        let mut layer_packages = Vec::new();

        // Merge members according to conflict strategy
        for member in &collection.members {
            let already_defined = resolved.known_packages.contains(&member.name);

            if already_defined {
                match on_conflict {
                    parser::ConflictStrategy::Local => {
                        // Local wins, skip this member
                        continue;
                    }
                    parser::ConflictStrategy::Remote => {
                        // Remote wins, update pin if provided
                        if let Some(constraint) = &member.version_constraint {
                            resolved
                                .pins
                                .insert(member.name.clone(), constraint.clone());
                        }
                        resolved.sources.insert(
                            member.name.clone(),
                            format!("included from {}", include_spec),
                        );
                        layer_packages.push(member.name.clone());
                    }
                    parser::ConflictStrategy::Error => {
                        return Err(ModelError::ConflictingSpecs(format!(
                            "Package '{}' defined in both local model and included '{}'",
                            member.name, include_spec
                        )));
                    }
                }
            } else {
                // New package from include
                if member.is_optional {
                    resolved.optionals.push(member.name.clone());
                } else {
                    resolved.install.push(member.name.clone());
                }
                resolved.known_packages.insert(member.name.clone());

                if let Some(constraint) = &member.version_constraint {
                    resolved
                        .pins
                        .insert(member.name.clone(), constraint.clone());
                }

                resolved.sources.insert(
                    member.name.clone(),
                    format!("included from {}", include_spec),
                );
                layer_packages.push(member.name.clone());
            }
        }

        // Merge remote collection pins (local pins take precedence)
        for (pkg, constraint) in &collection.pins {
            resolved
                .pins
                .entry(pkg.clone())
                .or_insert_with(|| constraint.clone());
        }

        // Merge remote collection excludes
        for excluded in &collection.exclude {
            if !resolved.exclude.contains(excluded) {
                resolved.exclude.push(excluded.clone());
            }
        }

        // Record this include as a composition layer
        resolved.layers.push(ModelLayer {
            name: include_spec.clone(),
            packages: layer_packages,
            is_local: false,
        });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_parse_minimal_model() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(
            file,
            r#"
[model]
version = 1
install = ["nginx"]
"#
        )
        .unwrap();

        let model = parse_model_file(file.path()).unwrap();
        assert_eq!(model.config.version, 1);
        assert_eq!(model.config.install, vec!["nginx"]);
    }

    #[test]
    fn test_parse_full_model() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(
            file,
            r#"
[model]
version = 1
search = ["fedora@f41:stable", "extras@local:dev"]
install = ["nginx", "postgresql", "redis"]
exclude = ["sendmail"]

[pin]
openssl = "3.0.*"
kernel = "6.12.*"

[optional]
packages = ["nginx-module-geoip"]
"#
        )
        .unwrap();

        let model = parse_model_file(file.path()).unwrap();
        assert_eq!(model.config.version, 1);
        assert_eq!(model.config.search.len(), 2);
        assert_eq!(model.config.install.len(), 3);
        assert_eq!(model.config.exclude.len(), 1);
        assert_eq!(model.pin.get("openssl"), Some(&"3.0.*".to_string()));
        assert_eq!(model.optional.packages.len(), 1);
    }
}
