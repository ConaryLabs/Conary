// src/repository/metadata.rs

//! Repository metadata data structures
//!
//! Contains types for representing repository and package metadata
//! from JSON repository indexes.

use serde::{Deserialize, Serialize};

/// Repository metadata format (simple JSON index)
#[derive(Debug, Serialize, Deserialize)]
pub struct RepositoryMetadata {
    pub name: String,
    pub version: String,
    pub packages: Vec<PackageMetadata>,
}

/// Delta update information for a package
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeltaInfo {
    pub from_version: String,
    pub from_hash: String,
    pub delta_url: String,
    pub delta_size: i64,
    pub delta_checksum: String,
    pub compression_ratio: f64,
}

/// Package metadata in repository index
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageMetadata {
    pub name: String,
    pub version: String,
    pub architecture: Option<String>,
    pub description: Option<String>,
    pub checksum: String,
    pub size: i64,
    pub download_url: String,
    pub dependencies: Option<Vec<String>>,
    /// Available delta updates from previous versions
    pub delta_from: Option<Vec<DeltaInfo>>,
}
