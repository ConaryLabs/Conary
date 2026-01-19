// src/model/mod.rs

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

pub mod parser;
mod diff;
mod state;

pub use parser::{
    SystemModel, ModelConfig, parse_model_file, DerivedPackage as ModelDerivedPackage,
    IncludeConfig, ConflictStrategy,
    // Automation config types
    AutomationConfig, AutomationMode, AutomationCategory, AiFeature,
    AiAssistConfig, AiAssistMode, SecurityAutomation, OrphanAutomation,
    UpdateAutomation, MajorUpgradeAutomation, RepairAutomation, RollbackTrigger,
    // Federation config types
    FederationConfig, FederationTier,
};
pub use diff::{ModelDiff, DiffAction, compute_diff, compute_diff_with_includes, compute_diff_from_resolved, ApplyOptions};
pub use state::{SystemState, InstalledPackage, capture_current_state, snapshot_to_model};

use rusqlite::Connection;
use std::collections::{HashMap, HashSet};
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
}

impl ResolvedModel {
    /// Create a resolved model from a base system model (no includes resolved yet)
    pub fn from_model(model: &SystemModel) -> Self {
        let mut sources = HashMap::new();
        for pkg in &model.config.install {
            sources.insert(pkg.clone(), "local".to_string());
        }
        for pkg in &model.optional.packages {
            sources.insert(pkg.clone(), "local (optional)".to_string());
        }

        Self {
            install: model.config.install.clone(),
            pins: model.pin.clone(),
            optionals: model.optional.packages.clone(),
            exclude: model.config.exclude.clone(),
            search: model.config.search.clone(),
            sources,
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
fn fetch_collection(
    conn: &Connection,
    name: &str,
    _label: Option<&str>,
) -> ModelResult<FetchedCollection> {
    use crate::db::models::{CollectionMember, Trove, TroveType};

    // First, try to find locally
    let troves = Trove::find_by_name(conn, name)
        .map_err(|e| ModelError::DatabaseError(e.to_string()))?;

    let collection = troves
        .into_iter()
        .find(|t| t.trove_type == TroveType::Collection);

    if let Some(coll) = collection {
        let coll_id = coll.id.ok_or_else(|| {
            ModelError::DatabaseError("Collection has no ID".to_string())
        })?;

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
        });
    }

    // TODO: Fetch from remote repository using label spec
    // For now, return an error if not found locally
    Err(ModelError::InvalidSearchPath(format!(
        "Collection '{}' not found locally. Remote collection fetching not yet implemented.",
        name
    )))
}

/// Resolve all includes in a system model
///
/// This performs a two-pass resolution:
/// 1. Collect all include specs
/// 2. Fetch each collection, detect cycles, and merge members
///
/// Returns a fully resolved model with all includes expanded.
pub fn resolve_includes(
    model: &SystemModel,
    conn: &Connection,
) -> ModelResult<ResolvedModel> {
    let mut resolved = ResolvedModel::from_model(model);

    if model.include.models.is_empty() {
        return Ok(resolved);
    }

    let mut visited: HashSet<String> = HashSet::new();

    resolve_includes_recursive(
        &model.include.models,
        &model.include.on_conflict,
        conn,
        &mut resolved,
        &mut visited,
    )?;

    Ok(resolved)
}

fn resolve_includes_recursive(
    includes: &[String],
    on_conflict: &parser::ConflictStrategy,
    conn: &Connection,
    resolved: &mut ResolvedModel,
    visited: &mut HashSet<String>,
) -> ModelResult<()> {
    for include_spec in includes {
        // Cycle detection
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
        let collection = fetch_collection(conn, &name, label.as_deref())?;

        // Recursively resolve nested includes if the collection has them
        if !collection.includes.is_empty() {
            resolve_includes_recursive(
                &collection.includes,
                on_conflict,
                conn,
                resolved,
                visited,
            )?;
        }

        // Merge members according to conflict strategy
        for member in &collection.members {
            let already_defined = resolved.install.contains(&member.name)
                || resolved.optionals.contains(&member.name);

            if already_defined {
                match on_conflict {
                    parser::ConflictStrategy::Local => {
                        // Local wins, skip this member
                        continue;
                    }
                    parser::ConflictStrategy::Remote => {
                        // Remote wins, update pin if provided
                        if let Some(constraint) = &member.version_constraint {
                            resolved.pins.insert(member.name.clone(), constraint.clone());
                        }
                        resolved.sources.insert(
                            member.name.clone(),
                            format!("included from {}", include_spec),
                        );
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

                if let Some(constraint) = &member.version_constraint {
                    resolved.pins.insert(member.name.clone(), constraint.clone());
                }

                resolved.sources.insert(
                    member.name.clone(),
                    format!("included from {}", include_spec),
                );
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;
    use std::io::Write;

    #[test]
    fn test_parse_minimal_model() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, r#"
[model]
version = 1
install = ["nginx"]
"#).unwrap();

        let model = parse_model_file(file.path()).unwrap();
        assert_eq!(model.config.version, 1);
        assert_eq!(model.config.install, vec!["nginx"]);
    }

    #[test]
    fn test_parse_full_model() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, r#"
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
"#).unwrap();

        let model = parse_model_file(file.path()).unwrap();
        assert_eq!(model.config.version, 1);
        assert_eq!(model.config.search.len(), 2);
        assert_eq!(model.config.install.len(), 3);
        assert_eq!(model.config.exclude.len(), 1);
        assert_eq!(model.pin.get("openssl"), Some(&"3.0.*".to_string()));
        assert_eq!(model.optional.packages.len(), 1);
    }
}
