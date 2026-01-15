// src/ccs/export/mod.rs
//! CCS package export to various image formats
//!
//! Supports exporting CCS packages to container images and other formats.

pub mod oci;

use anyhow::Result;
use std::path::Path;

/// Supported export formats
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    /// OCI container image (compatible with podman, docker, etc.)
    Oci,
    // Future formats:
    // Qcow2,  // VM disk image
    // Vmdk,   // VMware disk image
    // Raw,    // Raw disk image
}

impl ExportFormat {
    /// Parse format from string
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "oci" | "oci-archive" | "docker" => Some(Self::Oci),
            _ => None,
        }
    }

    /// Get file extension for format
    pub fn extension(&self) -> &'static str {
        match self {
            Self::Oci => "tar",
        }
    }
}

/// Export packages to the specified format
pub fn export(
    format: ExportFormat,
    packages: &[String],
    output: &Path,
    db_path: Option<&Path>,
) -> Result<()> {
    match format {
        ExportFormat::Oci => oci::export_oci(packages, output, db_path),
    }
}
