// conary-core/src/bootstrap/repart.rs

//! systemd-repart partition definition generator
//!
//! Generates repart.d/*.conf files that systemd-repart uses to create
//! GPT disk images without requiring root privileges or loop devices.

use super::config::TargetArch;
use std::fmt;
use std::path::Path;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum RepartError {
    #[error("Failed to write partition definition: {0}")]
    WriteFailed(String),

    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// A systemd-repart partition definition.
pub struct RepartDefinition {
    /// Partition type (e.g., "esp", "root-x86-64")
    pub partition_type: String,
    /// Filesystem format (e.g., "vfat", "ext4")
    pub format: String,
    /// Minimum size in bytes (None = fill remaining space)
    pub size_min: Option<u64>,
    /// Maximum size in bytes (None = fill remaining space)
    pub size_max: Option<u64>,
    /// Source directory to copy files from
    pub copy_files: Option<String>,
    /// Label for the partition
    pub label: Option<String>,
    /// Whether to minimize the partition size
    pub minimize: bool,
}

impl RepartDefinition {
    /// Create an EFI System Partition definition.
    pub fn esp(size_mb: u64) -> Self {
        let size_bytes = size_mb * 1024 * 1024;
        Self {
            partition_type: "esp".to_string(),
            format: "vfat".to_string(),
            size_min: Some(size_bytes),
            size_max: Some(size_bytes),
            copy_files: Some("/boot:/".to_string()),
            label: Some("ESP".to_string()),
            minimize: false,
        }
    }

    /// Create a root partition definition for the given architecture.
    pub fn root(arch: TargetArch) -> Self {
        let part_type = match arch {
            TargetArch::X86_64 => "root-x86-64",
            TargetArch::Aarch64 => "root-arm64",
            TargetArch::Riscv64 => "root-riscv64",
        };
        Self {
            partition_type: part_type.to_string(),
            format: "ext4".to_string(),
            size_min: None,
            size_max: None,
            copy_files: Some("/:/".to_string()),
            label: Some("root".to_string()),
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
    arch: TargetArch,
    esp_size_mb: u64,
) -> Result<(), RepartError> {
    std::fs::create_dir_all(output_dir).map_err(|e| RepartError::WriteFailed(e.to_string()))?;

    let esp = RepartDefinition::esp(esp_size_mb);
    std::fs::write(output_dir.join("00-esp.conf"), esp.to_string())?;

    let root = RepartDefinition::root(arch);
    std::fs::write(output_dir.join("10-root.conf"), root.to_string())?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_esp_definition() {
        let def = RepartDefinition::esp(512);
        let content = def.to_string();
        assert!(content.contains("[Partition]"));
        assert!(content.contains("Type=esp"));
        assert!(content.contains("SizeMinBytes=536870912")); // 512 * 1024 * 1024
        assert!(content.contains("Format=vfat"));
        assert!(content.contains("Label=ESP"));
    }

    #[test]
    fn test_root_x86_64_definition() {
        let def = RepartDefinition::root(TargetArch::X86_64);
        let content = def.to_string();
        assert!(content.contains("Type=root-x86-64"));
        assert!(content.contains("Format=ext4"));
        assert!(content.contains("Minimize=guess"));
        assert!(!content.contains("SizeMinBytes")); // fills remaining space
    }

    #[test]
    fn test_root_aarch64_definition() {
        let def = RepartDefinition::root(TargetArch::Aarch64);
        let content = def.to_string();
        assert!(content.contains("Type=root-arm64"));
    }

    #[test]
    fn test_root_riscv64_definition() {
        let def = RepartDefinition::root(TargetArch::Riscv64);
        let content = def.to_string();
        assert!(content.contains("Type=root-riscv64"));
    }

    #[test]
    fn test_generate_repart_definitions() {
        let dir = tempfile::tempdir().unwrap();
        let repart_dir = dir.path().join("repart.d");
        generate_repart_definitions(&repart_dir, TargetArch::X86_64, 512).unwrap();
        assert!(repart_dir.join("00-esp.conf").exists());
        assert!(repart_dir.join("10-root.conf").exists());

        let esp = std::fs::read_to_string(repart_dir.join("00-esp.conf")).unwrap();
        assert!(esp.contains("Type=esp"));

        let root = std::fs::read_to_string(repart_dir.join("10-root.conf")).unwrap();
        assert!(root.contains("Type=root-x86-64"));
    }
}
