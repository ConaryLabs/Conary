// apps/conary/src/commands/bootstrap/run_artifact.rs

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use conary_core::generation::builder::{FileEntryRef, SymlinkEntryRef};
use rusqlite::Connection;

pub(super) fn write_bootstrap_run_generation_artifact(
    conn: &Connection,
    cas_dir: &Path,
    gen_dir: &Path,
    profile: &conary_core::derivation::BuildProfile,
    target_triple: &str,
    system_name: &str,
) -> Result<()> {
    use conary_core::derivation::OutputManifest;
    use conary_core::derivation::compose::compose_entries;
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
    let output_manifests = load_bootstrap_run_output_manifests(conn, &cas, profile)?;
    let manifest_refs: Vec<&OutputManifest> = output_manifests.iter().collect();
    let composed_entries = compose_entries(&manifest_refs);
    let file_refs = composed_entries.files;
    let symlink_refs = composed_entries.symlinks;
    if file_refs.is_empty() {
        anyhow::bail!("bootstrap-run output has no file entries to export");
    }

    let boot_source_dir =
        tempfile::tempdir_in(gen_dir).context("Failed to create boot asset staging tempdir")?;
    let kernel_source = write_bootstrap_run_boot_asset_source(
        &cas,
        &file_refs,
        "/boot/vmlinuz",
        boot_source_dir.path(),
    )?;
    let initramfs_source = write_bootstrap_run_initramfs_source(
        &cas,
        &file_refs,
        &symlink_refs,
        boot_source_dir.path(),
    )
    .context("Failed to stage bootstrap-run initramfs")?;
    let efi_source = write_bootstrap_run_boot_asset_source(
        &cas,
        &file_refs,
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

    let cas_objects: Vec<CasObjectRef> = file_refs
        .iter()
        .map(|file| CasObjectRef {
            sha256: file.sha256_hash.clone(),
            size: file.size,
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
        cas_objects,
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
    file_refs: &[FileEntryRef],
    symlink_refs: &[SymlinkEntryRef],
    temp_dir: &Path,
) -> Result<PathBuf> {
    let root = temp_dir.join("initramfs-root");
    std::fs::create_dir_all(&root)?;

    let file_map: HashMap<&str, &conary_core::generation::builder::FileEntryRef> = file_refs
        .iter()
        .map(|file| (file.path.as_str(), file))
        .collect();
    let symlink_map: HashMap<&str, &str> = symlink_refs
        .iter()
        .map(|link| (link.path.as_str(), link.target.as_str()))
        .collect();
    let mut seen = HashSet::new();

    for rel in conary_core::bootstrap::bootstrap_initramfs_input_paths() {
        materialize_bootstrap_run_initramfs_path(
            cas,
            &root,
            Path::new(rel),
            &file_map,
            &symlink_map,
            &mut seen,
        )
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
    _file_refs: &[FileEntryRef],
    _symlink_refs: &[SymlinkEntryRef],
    _temp_dir: &Path,
) -> Result<PathBuf> {
    anyhow::bail!("bootstrap-run initramfs generation requires Unix filesystem metadata")
}
#[cfg(unix)]
fn materialize_bootstrap_run_initramfs_path<'a>(
    cas: &conary_core::filesystem::CasStore,
    root: &Path,
    rel: &Path,
    file_map: &HashMap<&'a str, &'a FileEntryRef>,
    symlink_map: &HashMap<&'a str, &'a str>,
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

    if let Some(target) = symlink_map.get(key.as_str()) {
        let _ = std::fs::remove_file(&dest);
        symlink(target, &dest)
            .with_context(|| format!("Failed to create staged initramfs symlink {key}"))?;
        let target_rel = resolve_bootstrap_run_symlink_target(&rel, target)?;
        return materialize_bootstrap_run_initramfs_path(
            cas,
            root,
            &target_rel,
            file_map,
            symlink_map,
            seen,
        );
    }

    let Some(file) = file_map.get(key.as_str()) else {
        anyhow::bail!("bootstrap-run output is missing required initramfs input {key}");
    };
    let bytes = cas
        .retrieve(&file.sha256_hash)
        .with_context(|| format!("Failed to load initramfs input {key} from CAS"))?;
    if bytes.len() as u64 != file.size {
        anyhow::bail!(
            "bootstrap-run initramfs input {key} size mismatch: manifest says {}, CAS object has {}",
            file.size,
            bytes.len()
        );
    }
    std::fs::write(&dest, bytes)?;
    std::fs::set_permissions(
        &dest,
        std::fs::Permissions::from_mode(file.permissions & 0o777),
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
fn load_bootstrap_run_output_manifests(
    conn: &Connection,
    cas: &conary_core::filesystem::CasStore,
    profile: &conary_core::derivation::BuildProfile,
) -> Result<Vec<conary_core::derivation::OutputManifest>> {
    let index = conary_core::derivation::DerivationIndex::new(conn);
    let mut manifests = Vec::new();
    for derivation in profile
        .stages
        .iter()
        .flat_map(|stage| stage.derivations.iter())
    {
        let record = index
            .lookup(&derivation.derivation_id)
            .with_context(|| {
                format!(
                    "Failed to look up derivation record for {}",
                    derivation.derivation_id
                )
            })?
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "missing derivation record for bootstrap-run output {}",
                    derivation.derivation_id
                )
            })?;
        let manifest_bytes = cas.retrieve(&record.manifest_cas_hash).with_context(|| {
            format!(
                "Failed to load output manifest {} from CAS",
                record.manifest_cas_hash
            )
        })?;
        let manifest_toml = std::str::from_utf8(&manifest_bytes)
            .context("bootstrap-run output manifest is not valid UTF-8")?;
        let manifest = toml::from_str(manifest_toml)
            .context("bootstrap-run output manifest TOML parse failed")?;
        manifests.push(manifest);
    }

    if manifests.is_empty() {
        anyhow::bail!("bootstrap-run profile has no derivation outputs to export");
    }

    Ok(manifests)
}
fn write_bootstrap_run_boot_asset_source(
    cas: &conary_core::filesystem::CasStore,
    file_refs: &[conary_core::generation::builder::FileEntryRef],
    manifest_path: &str,
    temp_dir: &Path,
) -> Result<PathBuf> {
    let file = file_refs
        .iter()
        .find(|file| file.path == manifest_path)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "bootstrap-run output is missing required boot asset {manifest_path}; ensure the bootstrap pipeline stages kernel, initramfs, and systemd-boot into /boot before generation export"
            )
        })?;
    let bytes = cas.retrieve(&file.sha256_hash).with_context(|| {
        format!(
            "Failed to retrieve bootstrap-run boot asset {} from CAS object {}",
            manifest_path, file.sha256_hash
        )
    })?;
    if bytes.len() as u64 != file.size {
        anyhow::bail!(
            "bootstrap-run boot asset {manifest_path} size mismatch: manifest says {}, CAS object has {}",
            file.size,
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

    #[test]
    fn bootstrap_run_artifact_writer_creates_loadable_generation() {
        use conary_core::db::schema::migrate;
        use conary_core::derivation::{
            BuildProfile, DerivationIndex, DerivationRecord, OutputFile, OutputManifest,
            OutputSymlink, PackageOutput, ProfileDerivation, ProfileMetadata, ProfileSeedRef,
            ProfileStage,
        };
        use conary_core::filesystem::CasStore;

        let temp = tempfile::tempdir().expect("tempdir");
        let output_dir = temp.path().join("output");
        let cas_dir = output_dir.join("objects");
        let gen_dir = output_dir.join("generations/1");
        std::fs::create_dir_all(&gen_dir).expect("generation dir");
        std::fs::write(gen_dir.join("root.erofs"), b"root-erofs").expect("root erofs");

        let conn = rusqlite::Connection::open_in_memory().expect("db");
        migrate(&conn).expect("migrate");
        let cas = CasStore::new(&cas_dir).expect("cas");
        let kernel_hash = cas.store(b"kernel").expect("kernel object");
        let efi_hash = cas.store(b"efi").expect("efi object");
        let hello_hash = cas.store(b"hello").expect("hello object");

        let derivation_id = "1111111111111111111111111111111111111111111111111111111111111111";
        let mut files = vec![
            OutputFile {
                path: "/boot/vmlinuz".to_string(),
                hash: kernel_hash,
                size: b"kernel".len() as u64,
                mode: 0o644,
            },
            OutputFile {
                path: "/boot/EFI/BOOT/BOOTX64.EFI".to_string(),
                hash: efi_hash,
                size: b"efi".len() as u64,
                mode: 0o644,
            },
            OutputFile {
                path: "/usr/bin/hello".to_string(),
                hash: hello_hash,
                size: b"hello".len() as u64,
                mode: 0o755,
            },
        ];
        let mut symlinks = Vec::new();
        for rel in conary_core::bootstrap::bootstrap_initramfs_input_paths() {
            if rel == "usr/lib/libc.so.6" {
                let target_rel = "usr/lib/libc-test.so";
                let bytes = b"fake initramfs input: usr/lib/libc-test.so\n";
                let hash = cas.store(bytes).expect("initramfs symlink target object");
                files.push(OutputFile {
                    path: format!("/{target_rel}"),
                    hash,
                    size: bytes.len() as u64,
                    mode: 0o644,
                });
                symlinks.push(OutputSymlink {
                    path: "/usr/lib/libc.so.6".to_string(),
                    target: "libc-test.so".to_string(),
                });
                continue;
            }
            let bytes = format!("fake initramfs input: {rel}\n").into_bytes();
            let hash = cas.store(&bytes).expect("initramfs input object");
            let mode = if rel.contains("/bin/") || rel.contains("/sbin/") {
                0o755
            } else {
                0o644
            };
            files.push(OutputFile {
                path: format!("/{rel}"),
                hash,
                size: bytes.len() as u64,
                mode,
            });
        }
        let output_hash = OutputManifest::compute_output_hash_v2(&files, &symlinks);
        let manifest = OutputManifest {
            derivation_id: derivation_id.to_string(),
            output_hash: output_hash.clone(),
            hash_version: 2,
            files,
            symlinks,
            build_duration_secs: 1,
            built_at: "2026-04-22T00:00:00Z".to_string(),
        };
        let package_output = PackageOutput::from_manifest(manifest).expect("package output");
        let manifest_cas_hash = cas
            .store(&package_output.manifest_bytes)
            .expect("manifest object");
        DerivationIndex::new(&conn)
            .insert(&DerivationRecord {
                derivation_id: derivation_id.to_string(),
                output_hash,
                package_name: "hello".to_string(),
                package_version: "1.0.0".to_string(),
                manifest_cas_hash,
                stage: Some("system".to_string()),
                build_env_hash: Some("seed".to_string()),
                built_at: "2026-04-22T00:00:00Z".to_string(),
                build_duration_secs: 1,
                trust_level: 2,
                provenance_cas_hash: None,
                reproducible: None,
            })
            .expect("insert derivation record");
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
            &conn,
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
}
