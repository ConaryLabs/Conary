// crates/conary-core/src/ccs/inspector.rs
//! Explicitly non-authoritative CCS package inspection.
//!
//! Tools for reading and examining .ccs packages.

use crate::ccs::archive_reader::inspect_untrusted_ccs_archive;
use crate::ccs::builder::{ComponentData, FileEntry};
use crate::ccs::manifest::CcsManifest;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::File;
use std::path::Path;

/// Structurally decoded package data that grants no trust or mutation authority.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UntrustedPackageInspection {
    /// Package manifest
    pub manifest: CcsManifest,
    /// All files in the package
    pub files: Vec<FileEntry>,
    /// Components
    pub components: HashMap<String, ComponentData>,
}

impl UntrustedPackageInspection {
    /// Decode a package for diagnostics without authenticating it.
    pub fn inspect_untrusted_file(path: &Path) -> Result<Self> {
        let file = File::open(path)
            .with_context(|| format!("Failed to open package: {}", path.display()))?;

        let contents = inspect_untrusted_ccs_archive(file)?;

        // Files come from the signed authority. The archive no longer carries a
        // duplicated `components/*.json` copy of the same records.
        let files: Vec<FileEntry> =
            crate::ccs::v3::component_view::file_entries(&contents.v3_authority);

        Ok(Self {
            manifest: contents.manifest,
            files,
            components: contents.components,
        })
    }

    pub fn name(&self) -> &str {
        &self.manifest.package.name
    }

    pub fn version(&self) -> &str {
        &self.manifest.package.version
    }

    pub fn file_count(&self) -> usize {
        self.files.len()
    }

    pub fn total_size(&self) -> u64 {
        self.files
            .iter()
            .filter_map(|file| file.content.as_ref().map(|content| content.size))
            .sum()
    }

    pub fn component_names(&self) -> Vec<&str> {
        self.components.keys().map(|s| s.as_str()).collect()
    }
}
