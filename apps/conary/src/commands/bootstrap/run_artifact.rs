// apps/conary/src/commands/bootstrap/run_artifact.rs

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use conary_core::generation::root_manifest::{
    GenerationRootEntry, GenerationRootManifest, MutableStateManifest,
};
use conary_core::payload::PayloadNodeKind;

pub(super) fn write_bootstrap_run_generation_artifact(
    cas_dir: &Path,
    gen_dir: &Path,
    profile: &conary_core::derivation::BuildProfile,
    target_triple: &str,
    system_name: &str,
) -> Result<()> {
    use conary_core::filesystem::CasStore;
    use conary_core::generation::artifact::{
        ArtifactWriteInputs, BootAssetSources, CasObjectRef, CasObjectVerification,
        deduplicate_sort_cas_objects, stage_boot_assets, write_generation_artifact,
    };
    use conary_core::generation::metadata::{GENERATION_FORMAT, GenerationMetadata};

    let architecture = architecture_from_target_triple(target_triple)?;
    if architecture != "x86_64" {
        anyhow::bail!(
            "bootstrap-run generation artifacts currently support only x86_64, got {architecture}"
        );
    }

    let cas = CasStore::new(cas_dir).context("Failed to open bootstrap-run CAS")?;
    let root_manifest = GenerationRootManifest::read_from(gen_dir)
        .context("Failed to load exact bootstrap-run generation root")?;
    let state_manifest = MutableStateManifest::read_from(gen_dir)
        .context("Failed to load exact bootstrap-run mutable state")?;
    if root_manifest.regular_contents().next().is_none() {
        anyhow::bail!("bootstrap-run output has no regular-file entries to export");
    }

    let boot_source_dir =
        tempfile::tempdir_in(gen_dir).context("Failed to create boot asset staging tempdir")?;
    let kernel_source = write_bootstrap_run_boot_asset_source(
        &cas,
        &root_manifest.entries,
        "/boot/vmlinuz",
        boot_source_dir.path(),
    )?;
    let initramfs_source =
        write_bootstrap_run_initramfs_source(&cas, &root_manifest.entries, boot_source_dir.path())
            .context("Failed to stage bootstrap-run initramfs")?;
    let efi_source = write_bootstrap_run_boot_asset_source(
        &cas,
        &root_manifest.entries,
        "/boot/EFI/BOOT/BOOTX64.EFI",
        boot_source_dir.path(),
    )?;

    let boot_assets = stage_boot_assets(BootAssetSources {
        generation_dir: gen_dir,
        generation: 1,
        architecture,
        kernel_version: "bootstrap",
        kernel: &kernel_source,
        initramfs: &initramfs_source,
        efi_bootloader: &efi_source,
    })
    .context("Failed to stage bootstrap-run boot assets")?;

    let cas_objects: Vec<CasObjectRef> = root_manifest
        .regular_contents()
        .chain(
            state_manifest
                .entries
                .iter()
                .filter_map(|entry| entry.content.as_ref()),
        )
        .map(|content| CasObjectRef {
            sha256: content.sha256.clone(),
            size: content.size,
        })
        .collect();
    let cas_object_count = deduplicate_sort_cas_objects(cas_objects.clone())?.len();
    let erofs_path = gen_dir.join("root.erofs");
    let erofs_size = std::fs::metadata(&erofs_path)
        .with_context(|| format!("Failed to stat {}", erofs_path.display()))?
        .len();
    let erofs_size = i64::try_from(erofs_size)
        .context("root.erofs is too large to record in generation metadata")?;
    let artifact_manifest_sha256 = write_generation_artifact(ArtifactWriteInputs {
        generation_dir: gen_dir,
        generation: 1,
        architecture,
        erofs_path: &erofs_path,
        cas_base_rel: "../../objects",
        cas_verification: CasObjectVerification::Deep,
        boot_assets,
    })
    .context("Failed to write bootstrap-run generation artifact")?;

    let package_count: usize = profile
        .stages
        .iter()
        .map(|stage| stage.derivations.len())
        .sum();
    let metadata = GenerationMetadata {
        generation: 1,
        format: GENERATION_FORMAT.to_string(),
        erofs_size: Some(erofs_size),
        cas_objects_referenced: Some(i64::try_from(cas_object_count).unwrap_or(i64::MAX)),
        fsverity_enabled: false,
        erofs_verity_digest: None,
        artifact_manifest_sha256: Some(artifact_manifest_sha256),
        security_capability_xattr_count: None,
        created_at: chrono::Utc::now().to_rfc3339(),
        package_count: i64::try_from(package_count).unwrap_or(i64::MAX),
        kernel_version: Some("bootstrap".to_string()),
        summary: format!(
            "Bootstrap-run generation 1 for {system_name} ({})",
            profile.profile.profile_hash
        ),
    };
    metadata
        .write_to(gen_dir)
        .context("Failed to write bootstrap-run generation metadata")?;

    Ok(())
}
#[cfg(unix)]
fn write_bootstrap_run_initramfs_source(
    cas: &conary_core::filesystem::CasStore,
    entries: &[GenerationRootEntry],
    temp_dir: &Path,
) -> Result<PathBuf> {
    let root = temp_dir.join("initramfs-root");
    std::fs::create_dir_all(&root)?;

    let entry_map: HashMap<&str, &GenerationRootEntry> = entries
        .iter()
        .map(|entry| (entry.path.as_str(), entry))
        .collect();
    let mut seen = HashSet::new();

    for rel in conary_core::bootstrap::bootstrap_initramfs_input_paths() {
        materialize_bootstrap_run_initramfs_path(cas, &root, Path::new(rel), &entry_map, &mut seen)
            .with_context(|| {
                format!("Failed to materialize bootstrap initramfs input /{rel} from CAS output")
            })?;
    }

    let dest = temp_dir.join("initramfs.img");
    conary_core::bootstrap::write_bootstrap_initramfs(&root, &dest)?;
    Ok(dest)
}
#[cfg(not(unix))]
fn write_bootstrap_run_initramfs_source(
    _cas: &conary_core::filesystem::CasStore,
    _entries: &[GenerationRootEntry],
    _temp_dir: &Path,
) -> Result<PathBuf> {
    anyhow::bail!("bootstrap-run initramfs generation requires Unix filesystem metadata")
}
#[cfg(unix)]
fn materialize_bootstrap_run_initramfs_path<'a>(
    cas: &conary_core::filesystem::CasStore,
    root: &Path,
    rel: &Path,
    entry_map: &HashMap<&'a str, &'a GenerationRootEntry>,
    seen: &mut HashSet<PathBuf>,
) -> Result<()> {
    use std::os::unix::fs::{PermissionsExt, symlink};

    let rel = normalize_bootstrap_run_relative_path(rel)?;
    if !seen.insert(rel.clone()) {
        return Ok(());
    }
    let key = format!("/{}", rel.display());
    let dest = root.join(&rel);
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let Some(entry) = entry_map.get(key.as_str()) else {
        anyhow::bail!("bootstrap-run output is missing required initramfs input {key}");
    };
    if let PayloadNodeKind::Symlink { target } = &entry.node.source.kind {
        let _ = std::fs::remove_file(&dest);
        symlink(target, &dest)
            .with_context(|| format!("Failed to create staged initramfs symlink {key}"))?;
        let target_rel = resolve_bootstrap_run_symlink_target(&rel, target)?;
        return materialize_bootstrap_run_initramfs_path(cas, root, &target_rel, entry_map, seen);
    }
    if !matches!(entry.node.source.kind, PayloadNodeKind::Regular { .. }) {
        anyhow::bail!("bootstrap-run initramfs input {key} is not a regular file or symlink");
    }
    let content = entry.content.as_ref().ok_or_else(|| {
        anyhow::anyhow!("bootstrap-run regular initramfs input {key} has no content authority")
    })?;
    let bytes = cas
        .retrieve(&content.sha256)
        .with_context(|| format!("Failed to load initramfs input {key} from CAS"))?;
    if bytes.len() as u64 != content.size {
        anyhow::bail!(
            "bootstrap-run initramfs input {key} size mismatch: manifest says {}, CAS object has {}",
            content.size,
            bytes.len()
        );
    }
    std::fs::write(&dest, bytes)?;
    std::fs::set_permissions(
        &dest,
        std::fs::Permissions::from_mode(entry.node.source.mode & 0o777),
    )?;
    Ok(())
}
#[cfg(unix)]
fn resolve_bootstrap_run_symlink_target(rel: &Path, target: &str) -> Result<PathBuf> {
    let target_path = Path::new(target);
    let combined = if target_path.is_absolute() {
        target_path
            .strip_prefix("/")
            .with_context(|| format!("Invalid absolute initramfs symlink target {target}"))?
            .to_path_buf()
    } else {
        rel.parent()
            .unwrap_or_else(|| Path::new(""))
            .join(target_path)
    };
    normalize_bootstrap_run_relative_path(&combined)
}
#[cfg(unix)]
fn normalize_bootstrap_run_relative_path(path: &Path) -> Result<PathBuf> {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::Normal(part) => normalized.push(part),
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if !normalized.pop() {
                    anyhow::bail!(
                        "initramfs input path escapes generation root: {}",
                        path.display()
                    );
                }
            }
            std::path::Component::RootDir | std::path::Component::Prefix(_) => {}
        }
    }
    if normalized.as_os_str().is_empty() {
        anyhow::bail!("empty initramfs input path");
    }
    Ok(normalized)
}
fn architecture_from_target_triple(target_triple: &str) -> Result<&'static str> {
    if target_triple == "x86_64" || target_triple.starts_with("x86_64-") {
        Ok("x86_64")
    } else if target_triple == "aarch64" || target_triple.starts_with("aarch64-") {
        Ok("aarch64")
    } else if target_triple == "riscv64" || target_triple.starts_with("riscv64-") {
        Ok("riscv64")
    } else {
        anyhow::bail!("unsupported bootstrap target triple for generation export: {target_triple}")
    }
}
fn write_bootstrap_run_boot_asset_source(
    cas: &conary_core::filesystem::CasStore,
    entries: &[GenerationRootEntry],
    manifest_path: &str,
    temp_dir: &Path,
) -> Result<PathBuf> {
    let entry = entries
        .iter()
        .find(|entry| entry.path == manifest_path)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "bootstrap-run output is missing required boot asset {manifest_path}; ensure the bootstrap pipeline stages kernel, initramfs, and systemd-boot into /boot before generation export"
            )
        })?;
    if !matches!(entry.node.source.kind, PayloadNodeKind::Regular { .. }) {
        anyhow::bail!("bootstrap-run boot asset {manifest_path} is not a regular file");
    }
    let content = entry.content.as_ref().ok_or_else(|| {
        anyhow::anyhow!("bootstrap-run boot asset {manifest_path} has no content authority")
    })?;
    let bytes = cas.retrieve(&content.sha256).with_context(|| {
        format!(
            "Failed to retrieve bootstrap-run boot asset {} from CAS object {}",
            manifest_path, content.sha256
        )
    })?;
    if bytes.len() as u64 != content.size {
        anyhow::bail!(
            "bootstrap-run boot asset {manifest_path} size mismatch: manifest says {}, CAS object has {}",
            content.size,
            bytes.len()
        );
    }

    let file_name = manifest_path
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
        .ok_or_else(|| anyhow::anyhow!("invalid boot asset path {manifest_path}"))?;
    let dest = temp_dir.join(file_name);
    std::fs::write(&dest, bytes)
        .with_context(|| format!("Failed to write temporary boot asset {}", dest.display()))?;
    Ok(dest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn bootstrap_run_artifact_writer_creates_loadable_generation() {
        use conary_core::derivation::{
            BuildProfile, ProfileDerivation, ProfileMetadata, ProfileSeedRef, ProfileStage,
        };
        use conary_core::filesystem::CasStore;
        use conary_core::generation::root_manifest::{
            GENERATION_ROOT_MANIFEST_VERSION, GenerationRootManifest, MutableStateManifest,
            scan_payload_tree,
        };

        let temp = tempfile::tempdir().expect("tempdir");
        let output_dir = temp.path().join("output");
        let cas_dir = output_dir.join("objects");
        let gen_dir = output_dir.join("generations/1");
        let source_root = temp.path().join("selected-root");
        std::fs::create_dir_all(&gen_dir).expect("generation dir");
        std::fs::create_dir(&source_root).expect("selected root");
        std::fs::write(gen_dir.join("root.erofs"), b"root-erofs").expect("root erofs");

        let cas = CasStore::new(&cas_dir).expect("cas");
        let derivation_id = "1111111111111111111111111111111111111111111111111111111111111111";
        write_source_file(&source_root, "boot/vmlinuz", b"kernel");
        write_source_file(&source_root, "boot/EFI/BOOT/BOOTX64.EFI", b"efi");
        write_source_file(&source_root, "usr/bin/hello", b"hello");
        for rel in conary_core::bootstrap::bootstrap_initramfs_input_paths() {
            if rel == "usr/lib/libc.so.6" {
                let target_rel = "usr/lib/libc-test.so";
                write_source_file(
                    &source_root,
                    target_rel,
                    b"fake initramfs input: usr/lib/libc-test.so\n",
                );
                let link = source_root.join(rel);
                std::os::unix::fs::symlink("libc-test.so", link).expect("libc symlink");
                continue;
            }
            let bytes = format!("fake initramfs input: {rel}\n").into_bytes();
            write_source_file(&source_root, rel, &bytes);
        }
        let (root, entries) =
            scan_payload_tree(&source_root, &cas, derivation_id).expect("exact root scan");
        GenerationRootManifest {
            version: GENERATION_ROOT_MANIFEST_VERSION,
            root,
            entries,
        }
        .write_to(&gen_dir)
        .expect("generation root manifest");
        MutableStateManifest::empty()
            .write_to(&gen_dir)
            .expect("mutable state manifest");

        let profile = BuildProfile {
            profile: ProfileMetadata {
                manifest: "test".to_string(),
                profile_hash: "profile-xyz".to_string(),
                generated_at: "2026-04-22T00:00:00Z".to_string(),
                target: "x86_64-conary-linux-gnu".to_string(),
            },
            seed: ProfileSeedRef {
                id: "seed".to_string(),
                source: "local".to_string(),
            },
            stages: vec![ProfileStage {
                name: "system".to_string(),
                build_env: "seed".to_string(),
                derivations: vec![ProfileDerivation {
                    package: "hello".to_string(),
                    version: "1.0.0".to_string(),
                    derivation_id: derivation_id.to_string(),
                }],
            }],
        };

        write_bootstrap_run_generation_artifact(
            &cas_dir,
            &gen_dir,
            &profile,
            "x86_64-conary-linux-gnu",
            "test-system",
        )
        .expect("artifact writer");

        conary_core::generation::artifact::load_generation_artifact(&gen_dir)
            .expect("load generated artifact");
        assert!(gen_dir.join(".conary-artifact.json").is_file());
        assert!(gen_dir.join("cas-manifest.json").is_file());
        assert!(gen_dir.join("boot-assets/manifest.json").is_file());
        let staged_initramfs =
            std::fs::read(gen_dir.join("boot-assets/initramfs.img")).expect("initramfs");
        assert!(
            String::from_utf8_lossy(&staged_initramfs).contains("conary-initramfs"),
            "bootstrap-run artifact writer must stage a generated Conary initramfs"
        );
    }

    #[cfg(unix)]
    fn write_source_file(root: &Path, relative: &str, bytes: &[u8]) {
        let path = root.join(relative);
        std::fs::create_dir_all(path.parent().expect("source parent")).expect("source parents");
        std::fs::write(path, bytes).expect("source file");
    }
}
