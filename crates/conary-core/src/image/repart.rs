// crates/conary-core/src/image/repart.rs

//! systemd-repart partition definition generator.
//!
//! Generates repart.d/*.conf files that systemd-repart uses to create GPT disk
//! images without requiring root privileges or loop devices.

use crate::bootstrap::TargetArch;
use std::fmt;
use std::path::{Path, PathBuf};
use thiserror::Error;

pub const ROOT_FILESYSTEM: &str = "ext4";
pub const BLS_ROOTFSTYPE: &str = ROOT_FILESYSTEM;
pub const ROOT_PARTITION_LABEL: &str = "CONARY_ROOT";
pub const ESP_PARTITION_LABEL: &str = "CONARY_ESP";

#[derive(Debug, Error)]
pub enum RepartError {
    #[error("Failed to write partition definition: {0}")]
    WriteFailed(String),

    #[error(transparent)]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone)]
pub struct DiskImagePlan {
    pub architecture: TargetArch,
    pub esp_staging_dir: PathBuf,
    pub root_staging_dir: PathBuf,
    pub output_raw: PathBuf,
    pub size_bytes: u64,
}

/// A systemd-repart partition definition.
pub struct RepartDefinition {
    /// Partition type (e.g., "esp", "root-x86-64").
    pub partition_type: String,
    /// Filesystem format (e.g., "vfat", "ext4").
    pub format: String,
    /// Minimum size in bytes (None = fill remaining space).
    pub size_min: Option<u64>,
    /// Maximum size in bytes (None = fill remaining space).
    pub size_max: Option<u64>,
    /// Source directory to copy files from.
    pub copy_files: Option<String>,
    /// Source paths to exclude while copying files into the partition.
    pub exclude_files: Vec<String>,
    /// Directories to create in the target partition before first boot.
    pub make_directories: Vec<String>,
    /// Label for the partition.
    pub label: Option<String>,
    /// Whether to minimize the partition size.
    pub minimize: bool,
}

impl RepartDefinition {
    /// Create an EFI System Partition definition.
    pub fn esp(source: &Path, size_mb: u64) -> Self {
        let size_bytes = size_mb * 1024 * 1024;
        Self {
            partition_type: "esp".to_string(),
            format: "vfat".to_string(),
            size_min: Some(size_bytes),
            size_max: Some(size_bytes),
            copy_files: Some(format!("{}:/", source.display())),
            exclude_files: Vec::new(),
            make_directories: Vec::new(),
            label: Some(ESP_PARTITION_LABEL.to_string()),
            minimize: false,
        }
    }

    /// Create a root partition definition for the given architecture.
    pub fn root(arch: TargetArch, source: &Path) -> Self {
        let part_type = match arch {
            TargetArch::X86_64 => "root-x86-64",
            TargetArch::Aarch64 => "root-arm64",
            TargetArch::Riscv64 => "root-riscv64",
        };
        Self {
            partition_type: part_type.to_string(),
            format: ROOT_FILESYSTEM.to_string(),
            size_min: None,
            size_max: None,
            copy_files: Some(format!("{}:/", source.display())),
            exclude_files: vec!["/boot".to_string()],
            make_directories: vec!["/boot".to_string()],
            label: Some(ROOT_PARTITION_LABEL.to_string()),
            minimize: true,
        }
    }
}

impl fmt::Display for RepartDefinition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "[Partition]")?;
        writeln!(f, "Type={}", self.partition_type)?;
        writeln!(f, "Format={}", self.format)?;
        if let Some(min) = self.size_min {
            writeln!(f, "SizeMinBytes={min}")?;
        }
        if let Some(max) = self.size_max {
            writeln!(f, "SizeMaxBytes={max}")?;
        }
        if let Some(ref copy) = self.copy_files {
            writeln!(f, "CopyFiles={copy}")?;
        }
        for exclude in &self.exclude_files {
            writeln!(f, "ExcludeFiles={exclude}")?;
        }
        for directory in &self.make_directories {
            writeln!(f, "MakeDirectories={directory}")?;
        }
        if let Some(ref label) = self.label {
            writeln!(f, "Label={label}")?;
        }
        if self.minimize {
            writeln!(f, "Minimize=guess")?;
        }
        Ok(())
    }
}

/// Generate repart.d definition files in the given directory.
pub fn generate_repart_definitions(
    output_dir: &Path,
    plan: &DiskImagePlan,
    esp_size_mb: u64,
) -> Result<(), RepartError> {
    std::fs::create_dir_all(output_dir).map_err(|e| RepartError::WriteFailed(e.to_string()))?;

    let esp = RepartDefinition::esp(&plan.esp_staging_dir, esp_size_mb);
    std::fs::write(output_dir.join("00-esp.conf"), esp.to_string())?;

    let root = RepartDefinition::root(plan.architecture, &plan.root_staging_dir);
    std::fs::write(output_dir.join("10-root.conf"), root.to_string())?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn esp_definition_copies_staged_esp_to_partition_root() {
        let def = RepartDefinition::esp(Path::new("/staged-esp"), 512);
        let content = def.to_string();
        assert!(content.contains("[Partition]"));
        assert!(content.contains("Type=esp"));
        assert!(content.contains("SizeMinBytes=536870912"));
        assert!(content.contains("Format=vfat"));
        assert!(content.contains("Label=CONARY_ESP"));
        assert!(content.contains("CopyFiles=/staged-esp:/"));
    }

    #[test]
    fn root_definition_copies_staged_root_to_partition_root() {
        let def = RepartDefinition::root(TargetArch::X86_64, Path::new("/staged-root"));
        let content = def.to_string();
        assert!(content.contains("Type=root-x86-64"));
        assert!(content.contains("Format=ext4"));
        assert!(content.contains("CopyFiles=/staged-root:/"));
        assert!(content.contains("ExcludeFiles=/boot"));
        assert!(content.contains("MakeDirectories=/boot"));
        assert!(content.contains("Label=CONARY_ROOT"));
        assert!(content.contains("Minimize=guess"));
        assert!(!content.contains("SizeMinBytes"));
    }

    #[test]
    fn root_architecture_partition_types_are_discoverable() {
        assert!(
            RepartDefinition::root(TargetArch::X86_64, Path::new("/"))
                .to_string()
                .contains("Type=root-x86-64")
        );
        assert!(
            RepartDefinition::root(TargetArch::Aarch64, Path::new("/"))
                .to_string()
                .contains("Type=root-arm64")
        );
        assert!(
            RepartDefinition::root(TargetArch::Riscv64, Path::new("/"))
                .to_string()
                .contains("Type=root-riscv64")
        );
    }

    #[test]
    fn root_filesystem_and_bls_rootfstype_share_one_constant() {
        assert_eq!(ROOT_FILESYSTEM, "ext4");
        assert_eq!(BLS_ROOTFSTYPE, ROOT_FILESYSTEM);
        let root = RepartDefinition::root(TargetArch::X86_64, Path::new("/"));
        assert_eq!(root.format, ROOT_FILESYSTEM);
    }

    #[test]
    fn generate_repart_definitions_from_disk_image_plan() {
        let dir = tempfile::tempdir().unwrap();
        let repart_dir = dir.path().join("repart.d");
        let plan = DiskImagePlan {
            architecture: TargetArch::X86_64,
            esp_staging_dir: PathBuf::from("/esp-stage"),
            root_staging_dir: PathBuf::from("/root-stage"),
            output_raw: dir.path().join("image.raw"),
            size_bytes: 4 * 1024 * 1024 * 1024,
        };

        generate_repart_definitions(&repart_dir, &plan, 512).unwrap();

        let esp = std::fs::read_to_string(repart_dir.join("00-esp.conf")).unwrap();
        assert!(esp.contains("Type=esp"));
        assert!(esp.contains("CopyFiles=/esp-stage:/"));

        let root = std::fs::read_to_string(repart_dir.join("10-root.conf")).unwrap();
        assert!(root.contains("Type=root-x86-64"));
        assert!(root.contains("CopyFiles=/root-stage:/"));
    }
}
