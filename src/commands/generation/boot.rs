// src/commands/generation/boot.rs
//! BLS boot entries and GRUB fallback for generation switching

use super::metadata::{GenerationMetadata, generation_path};
use anyhow::{Context, Result, anyhow};
use std::path::PathBuf;
use tracing::{info, warn};

/// BLS entries directory
const BLS_DIR: &str = "/boot/loader/entries";

/// Detected boot loader type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootLoader {
    /// Boot Loader Specification (systemd-boot, etc.)
    Bls,
    /// GRUB (legacy config generation)
    Grub,
    /// No recognized boot loader
    None,
}

/// Detect the system boot loader by checking for BLS directory or GRUB config tools.
#[must_use]
pub fn detect_bootloader() -> BootLoader {
    if std::path::Path::new(BLS_DIR).exists() {
        return BootLoader::Bls;
    }

    if which_grub_mkconfig().is_some() {
        return BootLoader::Grub;
    }

    BootLoader::None
}

/// Write a BLS entry for the given generation.
///
/// Creates `/boot/loader/entries/conary-gen-{N}.conf` with kernel, initrd,
/// and boot options derived from the generation metadata.
pub fn write_bls_entry(gen_number: i64, root_uuid: &str) -> Result<PathBuf> {
    let gen_dir = generation_path(gen_number);
    let metadata = GenerationMetadata::read_from(&gen_dir)
        .with_context(|| format!("Failed to read metadata for generation {gen_number}"))?;

    let kernel_version = metadata
        .kernel_version
        .as_deref()
        .ok_or_else(|| anyhow!("Generation {gen_number} has no kernel_version in metadata"))?;

    let cmdline = read_cmdline_options();
    let machine_id = read_machine_id().unwrap_or_default();

    // Ensure BLS directory exists
    std::fs::create_dir_all(BLS_DIR)
        .with_context(|| format!("Failed to create BLS directory {BLS_DIR}"))?;

    let entry_path = PathBuf::from(BLS_DIR).join(format!("conary-gen-{gen_number}.conf"));

    let mut options = format!("root=UUID={root_uuid} conary.generation={gen_number}");
    if !cmdline.is_empty() {
        options.push(' ');
        options.push_str(&cmdline);
    }

    let contents = format!(
        "title      Conary Generation {gen_number} ({date})\n\
         version    {kernel_version}\n\
         linux      /vmlinuz-{kernel_version}\n\
         initrd     /initramfs-{kernel_version}.img\n\
         options    {options}\n\
         sort-key   conary-{gen_number:04}\n\
         machine-id {machine_id}\n",
        date = metadata.created_at,
    );

    std::fs::write(&entry_path, &contents)
        .with_context(|| format!("Failed to write BLS entry {}", entry_path.display()))?;

    info!("Wrote BLS entry: {}", entry_path.display());
    Ok(entry_path)
}

/// Write a GRUB snippet for the given generation and regenerate GRUB config.
///
/// Creates `/etc/grub.d/42_conary` with a menuentry for the generation,
/// then runs `grub-mkconfig` (or `grub2-mkconfig`) to regenerate the config.
pub fn write_grub_snippet(gen_number: i64, root_uuid: &str) -> Result<()> {
    let gen_dir = generation_path(gen_number);
    let metadata = GenerationMetadata::read_from(&gen_dir)
        .with_context(|| format!("Failed to read metadata for generation {gen_number}"))?;

    let kernel_version = metadata
        .kernel_version
        .as_deref()
        .ok_or_else(|| anyhow!("Generation {gen_number} has no kernel_version in metadata"))?;

    let cmdline = read_cmdline_options();

    let mut options = format!("root=UUID={root_uuid} conary.generation={gen_number}");
    if !cmdline.is_empty() {
        options.push(' ');
        options.push_str(&cmdline);
    }

    let snippet_path = PathBuf::from("/etc/grub.d/42_conary");

    let contents = format!(
        r#"#!/bin/sh
exec tail -n +3 $0
menuentry "Conary Generation {gen_number} ({date})" {{
    search --no-floppy --fs-uuid --set=root {root_uuid}
    linux /vmlinuz-{kernel_version} {options}
    initrd /initramfs-{kernel_version}.img
}}
"#,
        date = metadata.created_at,
    );

    std::fs::write(&snippet_path, &contents)
        .with_context(|| format!("Failed to write GRUB snippet {}", snippet_path.display()))?;

    // Make executable (0o755)
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&snippet_path, std::fs::Permissions::from_mode(0o755))
            .with_context(|| format!("Failed to set permissions on {}", snippet_path.display()))?;
    }

    info!("Wrote GRUB snippet: {}", snippet_path.display());

    // Regenerate GRUB config
    if let Some(mkconfig) = which_grub_mkconfig() {
        let grub_cfg = if mkconfig.contains("grub2") {
            "/boot/grub2/grub.cfg"
        } else {
            "/boot/grub/grub.cfg"
        };

        let status = std::process::Command::new(&mkconfig)
            .arg("-o")
            .arg(grub_cfg)
            .status();

        match status {
            Ok(s) if s.success() => {
                info!("Regenerated GRUB config: {grub_cfg}");
            }
            Ok(s) => {
                warn!("grub-mkconfig exited with status {s}; boot entry may not be active");
            }
            Err(e) => {
                warn!("Failed to run {mkconfig}: {e}; boot entry may not be active");
            }
        }
    }

    Ok(())
}

/// Detect the boot loader and write the appropriate boot entry for the given generation.
///
/// Prints a warning if no recognized boot loader is found.
pub fn write_boot_entry(gen_number: i64) -> Result<()> {
    let root_uuid = detect_root_uuid()?;

    match detect_bootloader() {
        BootLoader::Bls => {
            let path = write_bls_entry(gen_number, &root_uuid)?;
            println!("Boot entry written: {}", path.display());
        }
        BootLoader::Grub => {
            write_grub_snippet(gen_number, &root_uuid)?;
            println!("GRUB snippet written for generation {gen_number}");
        }
        BootLoader::None => {
            warn!("No recognized boot loader found; skipping boot entry");
            println!("Warning: no recognized boot loader found, skipping boot entry");
        }
    }

    Ok(())
}

/// Detect the root filesystem UUID via `findmnt`.
fn detect_root_uuid() -> Result<String> {
    let output = std::process::Command::new("findmnt")
        .args(["-n", "-o", "UUID", "/"])
        .output()
        .context("Failed to run findmnt")?;

    let uuid = String::from_utf8_lossy(&output.stdout).trim().to_string();

    if uuid.is_empty() {
        return Err(anyhow!("Could not detect root filesystem UUID"));
    }

    Ok(uuid)
}

/// Read kernel command line, filtering out any `conary.generation=` parameter.
fn read_cmdline_options() -> String {
    let cmdline = std::fs::read_to_string("/proc/cmdline").unwrap_or_default();
    cmdline
        .split_whitespace()
        .filter(|param| !param.starts_with("conary.generation="))
        .map(|param| {
            param
                .chars()
                .filter(|c| c.is_ascii_graphic() || *c == ' ')
                .collect::<String>()
        })
        .filter(|param| !param.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Read the machine ID from `/etc/machine-id`.
fn read_machine_id() -> Option<String> {
    let contents = std::fs::read_to_string("/etc/machine-id").ok()?;
    let trimmed = contents.trim().to_string();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

/// Find `grub-mkconfig` or `grub2-mkconfig` on the system.
fn which_grub_mkconfig() -> Option<String> {
    for cmd in &["grub2-mkconfig", "grub-mkconfig"] {
        if std::process::Command::new("which")
            .arg(cmd)
            .output()
            .ok()
            .is_some_and(|o| o.status.success())
        {
            return Some((*cmd).to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_bootloader_returns_value() {
        // Just verify this doesn't panic — result depends on environment
        let _loader = detect_bootloader();
    }

    #[test]
    fn test_read_cmdline_strips_generation() {
        let result = read_cmdline_options();
        assert!(
            !result.contains("conary.generation="),
            "cmdline should not contain conary.generation param, got: {result}"
        );
    }
}
