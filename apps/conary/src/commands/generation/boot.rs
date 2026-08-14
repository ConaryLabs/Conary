// apps/conary/src/commands/generation/boot.rs
//! BLS boot entries and GRUB fallback for generation switching

use super::metadata::{GenerationMetadata, generation_path};
use anyhow::{Context, Result, anyhow};
use std::ffi::OsStr;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use tracing::info;

/// BLS entries directory
const BLS_DIR: &str = "/boot/loader/entries";
const GRUB_CONFIG: &str = "/boot/grub/grub.cfg";
const GRUB2_CONFIG: &str = "/boot/grub2/grub.cfg";

/// Exact GRUB provider selected for boot-entry mutation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrubConfiguration {
    executable: PathBuf,
    output: PathBuf,
}

/// Detected boot loader type
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BootLoader {
    /// Boot Loader Specification (systemd-boot, etc.)
    Bls,
    /// GRUB (legacy config generation)
    Grub(GrubConfiguration),
    /// No recognized boot loader
    None,
}

/// Detect the system boot loader by checking for BLS directory or GRUB config tools.
#[must_use]
pub fn detect_bootloader() -> BootLoader {
    detect_bootloader_in(Path::new(BLS_DIR), std::env::var_os("PATH").as_deref())
}

fn detect_bootloader_in(bls_dir: &Path, path: Option<&OsStr>) -> BootLoader {
    if bls_dir.exists() {
        return BootLoader::Bls;
    }

    if let Some(configuration) = find_grub_configuration(path) {
        return BootLoader::Grub(configuration);
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

    let cmdline = read_cmdline_options()?;
    let machine_id_line = read_machine_id()?
        .map(|machine_id| format!("machine-id {machine_id}\n"))
        .unwrap_or_default();

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
         {machine_id_line}",
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
pub fn write_grub_snippet(
    gen_number: i64,
    root_uuid: &str,
    configuration: &GrubConfiguration,
) -> Result<()> {
    let gen_dir = generation_path(gen_number);
    let metadata = GenerationMetadata::read_from(&gen_dir)
        .with_context(|| format!("Failed to read metadata for generation {gen_number}"))?;

    let kernel_version = metadata
        .kernel_version
        .as_deref()
        .ok_or_else(|| anyhow!("Generation {gen_number} has no kernel_version in metadata"))?;

    let cmdline = read_cmdline_options()?;

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

    let status = grub_mkconfig_command(configuration)
        .status()
        .with_context(|| format!("Failed to run {}", configuration.executable.display()))?;

    if !status.success() {
        return Err(anyhow!(
            "{} exited with status {status}; boot entry is not active",
            configuration.executable.display()
        ));
    }
    info!(
        "Regenerated GRUB config: {}",
        configuration.output.display()
    );

    Ok(())
}

fn grub_mkconfig_command(configuration: &GrubConfiguration) -> std::process::Command {
    let mut command = std::process::Command::new(&configuration.executable);
    command.arg("-o").arg(&configuration.output);
    command
}

/// Detect the boot loader and write the appropriate boot entry for the given generation.
///
/// Returns an error unless a supported loader is available and its configuration
/// is updated successfully.
pub fn write_boot_entry(gen_number: i64, bootloader: &BootLoader) -> Result<()> {
    let root_uuid = detect_root_uuid()?;

    match bootloader {
        BootLoader::Bls => {
            let path = write_bls_entry(gen_number, &root_uuid)?;
            println!("Boot entry written: {}", path.display());
        }
        BootLoader::Grub(configuration) => {
            write_grub_snippet(gen_number, &root_uuid, configuration)?;
            println!("GRUB snippet written for generation {gen_number}");
        }
        BootLoader::None => {
            return Err(anyhow!(
                "No supported boot loader is available; expected BLS entries or grub-mkconfig"
            ));
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
fn read_cmdline_options() -> Result<String> {
    let cmdline =
        std::fs::read_to_string("/proc/cmdline").context("Failed to read /proc/cmdline")?;
    Ok(cmdline
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
        .join(" "))
}

/// Read the machine ID from `/etc/machine-id`.
fn read_machine_id() -> Result<Option<String>> {
    let contents = match std::fs::read_to_string("/etc/machine-id") {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("Failed to read /etc/machine-id"),
    };
    let trimmed = contents.trim().to_string();
    if trimmed.is_empty() {
        Ok(None)
    } else {
        Ok(Some(trimmed))
    }
}

/// Resolve the first exact supported GRUB provider from `PATH`.
fn find_grub_configuration(path: Option<&OsStr>) -> Option<GrubConfiguration> {
    let providers = [
        ("grub2-mkconfig", GRUB2_CONFIG),
        ("grub-mkconfig", GRUB_CONFIG),
    ];

    for (executable_name, output) in providers {
        for directory in std::env::split_paths(path?) {
            let candidate = directory.join(executable_name);
            let Some(executable) = candidate.canonicalize().ok() else {
                continue;
            };
            let is_executable = executable.metadata().is_ok_and(|metadata| {
                metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
            });
            if is_executable {
                return Some(GrubConfiguration {
                    executable,
                    output: PathBuf::from(output),
                });
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn create_executable(path: &Path) {
        fs::write(path, "#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    fn test_detect_bootloader_returns_value() {
        // Just verify this doesn't panic — result depends on environment
        let _loader = detect_bootloader();
    }

    #[test]
    fn exact_grub2_identity_owns_its_output_path() {
        let temp = tempfile::tempdir().unwrap();
        let executable = temp.path().join("grub2-mkconfig");
        create_executable(&executable);

        assert_eq!(
            find_grub_configuration(Some(temp.path().as_os_str())),
            Some(GrubConfiguration {
                executable,
                output: PathBuf::from(GRUB2_CONFIG),
            })
        );
    }

    #[test]
    fn bls_directory_precedes_an_available_grub_provider() {
        let temp = tempfile::tempdir().unwrap();
        let bls_dir = temp.path().join("loader/entries");
        fs::create_dir_all(&bls_dir).unwrap();
        create_executable(&temp.path().join("grub2-mkconfig"));

        assert_eq!(
            detect_bootloader_in(&bls_dir, Some(temp.path().as_os_str())),
            BootLoader::Bls
        );
    }

    #[test]
    fn exact_grub_identity_owns_its_output_path() {
        let temp = tempfile::tempdir().unwrap();
        let executable = temp.path().join("grub-mkconfig");
        create_executable(&executable);

        assert_eq!(
            find_grub_configuration(Some(temp.path().as_os_str())),
            Some(GrubConfiguration {
                executable,
                output: PathBuf::from(GRUB_CONFIG),
            })
        );
    }

    #[test]
    fn discovery_preserves_exact_grub2_provider_priority() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        create_executable(&first.path().join("grub-mkconfig"));
        let grub2 = second.path().join("grub2-mkconfig");
        create_executable(&grub2);
        let search_path = std::env::join_paths([first.path(), second.path()]).unwrap();

        assert_eq!(
            find_grub_configuration(Some(&search_path)),
            Some(GrubConfiguration {
                executable: grub2,
                output: PathBuf::from(GRUB2_CONFIG),
            })
        );
    }

    #[test]
    fn substring_aliases_and_non_executable_providers_are_not_authority() {
        let temp = tempfile::tempdir().unwrap();
        create_executable(&temp.path().join("vendor-grub2-mkconfig-helper"));
        fs::write(temp.path().join("grub2-mkconfig"), "not executable").unwrap();

        assert_eq!(find_grub_configuration(Some(temp.path().as_os_str())), None);
    }

    #[test]
    fn command_uses_the_discovered_executable_and_output_without_reinterpretation() {
        let configuration = GrubConfiguration {
            executable: PathBuf::from("/exact/provider/grub2-mkconfig"),
            output: PathBuf::from(GRUB2_CONFIG),
        };
        let command = grub_mkconfig_command(&configuration);

        assert_eq!(
            command.get_program(),
            OsStr::new("/exact/provider/grub2-mkconfig")
        );
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            [OsStr::new("-o"), OsStr::new(GRUB2_CONFIG)]
        );
    }

    #[test]
    fn test_read_cmdline_strips_generation() {
        let result = read_cmdline_options().unwrap();
        assert!(
            !result.contains("conary.generation="),
            "cmdline should not contain conary.generation param, got: {result}"
        );
    }
}
