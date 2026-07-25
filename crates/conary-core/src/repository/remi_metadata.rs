// conary-core/src/repository/remi_metadata.rs

//! Exact normalized package metadata exchanged by Remi and Conary.

use crate::repository::versioning::VersionScheme;
use serde::{Deserialize, Serialize};

/// A normalized capability provided by one repository package.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemiProvide {
    pub capability: String,
    pub version: Option<String>,
    pub kind: String,
    pub raw: Option<String>,
    pub version_scheme: VersionScheme,
}

/// A normalized requirement clause.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemiRequirement {
    pub capability: String,
    pub version_constraint: Option<String>,
    pub kind: String,
    pub dependency_type: String,
    pub raw: Option<String>,
}

/// A normalized native requirement group and its exact clauses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemiRequirementGroup {
    pub kind: String,
    pub behavior: String,
    pub description: Option<String>,
    pub native_text: Option<String>,
    pub expression_json: String,
    pub clauses: Vec<RemiRequirement>,
}
