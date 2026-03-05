// src/commands/generation/commands.rs
//! CLI implementations for generation list, info, and gc commands

use super::metadata::{GenerationMetadata, gc_roots_dir, generation_path, generations_dir};
use super::switch::current_generation;
use anyhow::{Result, anyhow};
use tracing::info;

/// List all generations with a summary table.
///
/// Prints each generation's number, creation date, package count, kernel version,
/// and whether it is the currently active generation.
pub fn cmd_generation_list() -> Result<()> {
    let dir = generations_dir();

    if !dir.exists() {
        println!("No generations found. Run 'conary system takeover' to create the first.");
        return Ok(());
    }

    let current = current_generation()?;

    let mut generations: Vec<(i64, GenerationMetadata)> = Vec::new();

    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        if let Ok(number) = name_str.parse::<i64>() {
            let gen_dir = entry.path();
            match GenerationMetadata::read_from(&gen_dir) {
                Ok(meta) => generations.push((number, meta)),
                Err(e) => {
                    eprintln!("Warning: skipping generation {number}: {e}");
                }
            }
        }
    }

    generations.sort_by_key(|(number, _)| *number);

    if generations.is_empty() {
        println!("No valid generations found.");
        return Ok(());
    }

    for (number, meta) in &generations {
        let kernel = meta
            .kernel_version
            .as_deref()
            .unwrap_or("none");
        let active = if current == Some(*number) {
            " [active]"
        } else {
            ""
        };
        println!(
            "{number}  {date}  {count} packages  kernel {kernel}{active}",
            date = meta.created_at,
            count = meta.package_count,
        );
    }

    Ok(())
}

/// Print detailed information about a specific generation.
pub fn cmd_generation_info(gen_number: i64) -> Result<()> {
    let gen_dir = generation_path(gen_number);

    if !gen_dir.exists() {
        return Err(anyhow!("Generation {gen_number} does not exist"));
    }

    let meta = GenerationMetadata::read_from(&gen_dir)?;
    let current = current_generation()?;
    let is_active = current == Some(gen_number);

    let status = if is_active { "active" } else { "inactive" };
    let kernel = meta
        .kernel_version
        .as_deref()
        .unwrap_or("none");
    let size = dir_size_bytes(&gen_dir);

    println!("Generation {gen_number}");
    println!("  Status:   {status}");
    println!("  Created:  {}", meta.created_at);
    println!("  Packages: {}", meta.package_count);
    println!("  Kernel:   {kernel}");
    println!("  Summary:  {}", meta.summary);
    println!("  Size:     {}", format_bytes(size));

    Ok(())
}

/// Garbage-collect old generations, keeping the current generation, GC roots,
/// and the most recent `keep` generations.
///
/// Also removes the corresponding BLS boot loader entry files.
pub fn cmd_generation_gc(keep: usize) -> Result<()> {
    let current = current_generation()?;
    let gc_roots = load_gc_roots();
    let dir = generations_dir();

    if !dir.exists() {
        println!("No generations directory found. Nothing to collect.");
        return Ok(());
    }

    let mut all_numbers: Vec<i64> = Vec::new();

    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if let Ok(number) = name_str.parse::<i64>() {
            all_numbers.push(number);
        }
    }

    all_numbers.sort();

    // Build the keep set: current + gc_roots + last N generations
    let mut keep_set = std::collections::HashSet::new();

    if let Some(cur) = current {
        keep_set.insert(cur);
    }

    for root in &gc_roots {
        keep_set.insert(*root);
    }

    // Keep the last N generations (by highest number)
    let start = all_numbers.len().saturating_sub(keep);
    for &num in &all_numbers[start..] {
        keep_set.insert(num);
    }

    let to_remove: Vec<i64> = all_numbers
        .iter()
        .filter(|n| !keep_set.contains(n))
        .copied()
        .collect();

    if to_remove.is_empty() {
        println!("Nothing to collect. All generations are kept.");
        return Ok(());
    }

    let mut removed_count = 0u64;
    let mut freed_bytes = 0u64;

    for gen_number in &to_remove {
        let gen_dir = generation_path(*gen_number);
        let size = dir_size_bytes(&gen_dir);

        match std::fs::remove_dir_all(&gen_dir) {
            Ok(()) => {
                info!("Removed generation {gen_number}");
                removed_count += 1;
                freed_bytes += size;
            }
            Err(e) => {
                eprintln!("Warning: failed to remove generation {gen_number}: {e}");
            }
        }

        // Remove corresponding BLS entry
        let bls_path = std::path::PathBuf::from(format!(
            "/boot/loader/entries/conary-gen-{gen_number}.conf"
        ));
        if bls_path.exists() {
            if let Err(e) = std::fs::remove_file(&bls_path) {
                eprintln!("Warning: failed to remove BLS entry {}: {e}", bls_path.display());
            } else {
                info!("Removed BLS entry for generation {gen_number}");
            }
        }
    }

    println!(
        "Collected {removed_count} generation(s), freed {}.",
        format_bytes(freed_bytes)
    );

    Ok(())
}

/// Read GC root entries from the gc-roots directory.
///
/// Each entry name is expected to parse as an i64 generation number.
fn load_gc_roots() -> Vec<i64> {
    let dir = gc_roots_dir();

    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };

    entries
        .flatten()
        .filter_map(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .parse::<i64>()
                .ok()
        })
        .collect()
}

/// Calculate total size of all files under `path` recursively.
fn dir_size_bytes(path: &std::path::Path) -> u64 {
    walkdir::WalkDir::new(path)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.metadata().ok())
        .filter(|meta| meta.is_file())
        .map(|meta| meta.len())
        .sum()
}

/// Format a byte count as a human-readable string (GiB, MiB, or KiB).
fn format_bytes(bytes: u64) -> String {
    const GIB: u64 = 1024 * 1024 * 1024;
    const MIB: u64 = 1024 * 1024;
    const KIB: u64 = 1024;

    if bytes >= GIB {
        format!("{:.2} GiB", bytes as f64 / GIB as f64)
    } else if bytes >= MIB {
        format!("{:.2} MiB", bytes as f64 / MIB as f64)
    } else if bytes >= KIB {
        format!("{:.2} KiB", bytes as f64 / KIB as f64)
    } else {
        format!("{bytes} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_bytes() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1024), "1.00 KiB");
        assert_eq!(format_bytes(1_048_576), "1.00 MiB");
        assert_eq!(format_bytes(1_073_741_824), "1.00 GiB");
        assert_eq!(format_bytes(2_684_354_560), "2.50 GiB");
    }
}
