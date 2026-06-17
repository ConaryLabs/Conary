// conary-core/src/ccs/builder/package_writer.rs

//! Package emission helpers for the CCS builder.
//!
//! `builder.rs` focuses on scanning and assembling build state; this module
//! owns the final archive-writing and manifest serialization steps.

use super::{BuildResult, BuilderError};
use crate::ccs::manifest::parse_octal_mode;
use crate::hash;
use anyhow::{Context, Result};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

/// Write a CCS package to disk (unsigned).
pub fn write_ccs_package(result: &BuildResult, output_path: &Path) -> Result<()> {
    write_ccs_package_internal(result, output_path, None)
}

/// Write a signed CCS package to disk.
pub fn write_signed_ccs_package(
    result: &BuildResult,
    output_path: &Path,
    signing_key: &super::super::signing::SigningKeyPair,
) -> Result<()> {
    write_ccs_package_internal(result, output_path, Some(signing_key))
}

pub fn write_v2_ccs_package(
    authority: &crate::ccs::v2::AuthorityDocumentV2,
    payloads_by_path: &std::collections::BTreeMap<String, Vec<u8>>,
    output_path: &Path,
    signing_key: &super::super::signing::SigningKeyPair,
    debug_toml: Option<&str>,
    build_attestation: Option<&crate::ccs::attestation::BuildAttestationEnvelope>,
    foreign_conversion_boundary: Option<&crate::ccs::attestation::ForeignConversionBoundary>,
) -> Result<()> {
    use crate::ccs::builder::{ComponentData, FileEntry, FileType};
    use crate::ccs::v2::schema::{FileTypeV2, PackageKindV2};
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use std::collections::BTreeMap;
    use tar::Builder;

    crate::ccs::v2::validate_authority(authority).map_err(|error| anyhow::anyhow!("{error}"))?;
    let temp_dir = tempfile::tempdir()?;
    let manifest_cbor = authority.to_cbor()?;
    fs::write(temp_dir.path().join("MANIFEST"), &manifest_cbor)?;
    if let Some(debug_toml) = debug_toml {
        fs::write(temp_dir.path().join("MANIFEST.toml"), debug_toml)?;
    }
    if let Some(build_attestation) = build_attestation {
        fs::write(
            temp_dir.path().join("MANIFEST.attestation.json"),
            serde_json::to_string_pretty(build_attestation)?,
        )?;
    }
    if let Some(foreign_conversion_boundary) = foreign_conversion_boundary {
        fs::write(
            temp_dir.path().join("MANIFEST.conversion-boundary.json"),
            serde_json::to_string_pretty(foreign_conversion_boundary)?,
        )?;
    }
    let signature = signing_key.sign(&manifest_cbor);
    fs::write(
        temp_dir.path().join("MANIFEST.sig"),
        serde_json::to_string_pretty(&signature)?,
    )?;

    let PackageKindV2::Package(data) = &authority.kind else {
        anyhow::bail!("M4a v2 writer only writes package payloads");
    };
    let mut components: BTreeMap<String, ComponentData> = authority
        .components
        .keys()
        .map(|name| {
            (
                name.clone(),
                ComponentData {
                    name: name.clone(),
                    files: Vec::new(),
                    hash: String::new(),
                    size: 0,
                },
            )
        })
        .collect();

    let objects_dir = temp_dir.path().join("objects");
    for file in &data.files {
        let entry = FileEntry {
            path: file.path.clone(),
            hash: file.sha256.clone(),
            size: file.size,
            mode: file.mode,
            component: file.component.clone(),
            file_type: match file.file_type {
                FileTypeV2::Regular => FileType::Regular,
                FileTypeV2::Directory => FileType::Directory,
                FileTypeV2::Symlink => FileType::Symlink,
            },
            target: file.symlink_target.clone(),
            chunks: None,
        };
        if matches!(file.file_type, FileTypeV2::Regular) {
            let payload = payloads_by_path
                .get(&file.path)
                .with_context(|| format!("missing payload for {}", file.path))?;
            if crate::hash::sha256(payload) != file.sha256 || payload.len() as u64 != file.size {
                anyhow::bail!(
                    "payload for {} does not match signed v2 authority",
                    file.path
                );
            }
            if file.sha256.len() < 2 {
                anyhow::bail!("invalid signed v2 sha256 for {}", file.path);
            }
            let (prefix, suffix) = file.sha256.split_at(2);
            let object_dir = objects_dir.join(prefix);
            fs::create_dir_all(&object_dir)?;
            fs::write(object_dir.join(suffix), payload)?;
        }
        let component = components
            .get_mut(&file.component)
            .with_context(|| format!("missing component {}", file.component))?;
        component.size += file.size;
        component.files.push(entry);
    }

    let components_dir = temp_dir.path().join("components");
    fs::create_dir_all(&components_dir)?;
    for (name, component) in &mut components {
        component.hash = crate::hash::sha256_prefixed(
            &crate::ccs::attestation::canonical_json_bytes(&component.files)?,
        );
        fs::write(
            components_dir.join(format!("{name}.json")),
            serde_json::to_vec_pretty(component)?,
        )?;
    }

    let output_file = fs::File::create(output_path)?;
    let encoder = GzEncoder::new(output_file, Compression::default());
    let mut archive = Builder::new(encoder);
    archive.append_dir_all(".", temp_dir.path())?;
    archive.into_inner()?.finish()?;
    Ok(())
}

/// Print a concise build summary.
pub fn print_build_summary(result: &BuildResult) {
    println!();
    println!("Build Summary");
    println!("=============");
    println!();
    println!(
        "Package: {} v{}",
        result.manifest.package.name, result.manifest.package.version
    );
    println!("Total files: {}", result.files.len());
    println!("Total size: {} bytes", result.total_size);
    println!("Blobs: {} objects", result.blobs.len());

    if let Some(ref stats) = result.chunk_stats {
        println!();
        println!("CDC Chunking:");
        println!("  Chunked files: {} (files >16KB)", stats.chunked_files);
        println!("  Whole files: {} (files ≤16KB)", stats.whole_files);
        println!("  Total chunks: {}", stats.total_chunks);
        println!("  Unique chunks: {}", stats.unique_chunks);
        if stats.dedup_savings > 0 {
            println!("  Intra-package dedup: {} bytes saved", stats.dedup_savings);
        }
    }

    println!();
    println!("Components:");

    let mut comp_names: Vec<_> = result.components.keys().collect();
    comp_names.sort();

    for name in comp_names {
        let comp = &result.components[name];
        println!(
            "  :{} - {} files ({} bytes)",
            name,
            comp.files.len(),
            comp.size
        );
    }
}

fn write_ccs_package_internal(
    result: &BuildResult,
    output_path: &Path,
    signing_key: Option<&super::super::signing::SigningKeyPair>,
) -> Result<()> {
    use crate::ccs::binary_manifest::{ComponentRef, Hash, MerkleTree};
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use std::collections::BTreeMap;
    use tar::Builder;

    let temp_dir = tempfile::tempdir()?;

    let components_dir = temp_dir.path().join("components");
    fs::create_dir_all(&components_dir)?;

    let mut component_refs: BTreeMap<String, ComponentRef> = BTreeMap::new();
    let default_components = &result.manifest.components.default;

    for (name, component) in &result.components {
        let component_json = serde_json::to_string_pretty(component)?;
        let component_path = components_dir.join(format!("{}.json", name));
        fs::write(&component_path, &component_json)?;

        let hash = Hash::sha256(component_json.as_bytes());
        component_refs.insert(
            name.clone(),
            ComponentRef {
                hash,
                file_count: component.files.len() as u32,
                total_size: component.size,
                default: default_components.contains(name),
            },
        );
    }

    let content_root = MerkleTree::calculate_root(&component_refs);
    let mut binary_manifest = build_binary_manifest(result, component_refs, content_root)?;

    let manifest_toml = result.manifest.to_toml()?;
    binary_manifest.toml_integrity_hash = Some(hash::sha256(manifest_toml.as_bytes()));

    let manifest_cbor = binary_manifest
        .to_cbor()
        .map_err(|e| BuilderError::ManifestEncoding(e.to_string()))?;
    fs::write(temp_dir.path().join("MANIFEST"), &manifest_cbor)?;
    fs::write(temp_dir.path().join("MANIFEST.toml"), &manifest_toml)?;

    if let Some(key) = signing_key {
        let signature = key.sign(&manifest_cbor);
        let sig_json = serde_json::to_string_pretty(&signature)?;
        fs::write(temp_dir.path().join("MANIFEST.sig"), &sig_json)?;
    }

    let objects_dir = temp_dir.path().join("objects");
    fs::create_dir_all(&objects_dir)?;

    for (hash, content) in &result.blobs {
        let blob_path = crate::filesystem::object_path(&objects_dir, hash)?;
        if let Some(parent) = blob_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&blob_path, content)?;
    }

    let output_file = fs::File::create(output_path)?;
    let encoder = GzEncoder::new(output_file, Compression::default());
    let mut archive = Builder::new(encoder);

    if result.manifest.policy.normalize_timestamps {
        let timestamp = std::env::var("SOURCE_DATE_EPOCH")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(1704067200);
        append_dir_with_mtime(&mut archive, temp_dir.path(), "", timestamp)?;
    } else {
        archive.append_dir_all(".", temp_dir.path())?;
    }

    let encoder = archive.into_inner()?;
    encoder.finish()?;

    Ok(())
}

fn append_dir_with_mtime<W: std::io::Write>(
    archive: &mut tar::Builder<W>,
    base_path: &Path,
    archive_path: &str,
    mtime: u64,
) -> Result<()> {
    for entry in fs::read_dir(base_path)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let file_name = entry.file_name();
        let file_name_str = file_name.to_string_lossy();

        let entry_archive_path = if archive_path.is_empty() {
            file_name_str.to_string()
        } else {
            format!("{}/{}", archive_path, file_name_str)
        };

        if file_type.is_dir() {
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(tar::EntryType::Directory);
            header.set_mode(0o755);
            header.set_size(0);
            header.set_mtime(mtime);
            header.set_cksum();

            archive.append_data(&mut header, &entry_archive_path, std::io::empty())?;
            append_dir_with_mtime(archive, &entry.path(), &entry_archive_path, mtime)?;
        } else if file_type.is_file() {
            let content = fs::read(entry.path())?;
            let metadata = entry.metadata()?;

            let mut header = tar::Header::new_gnu();
            header.set_entry_type(tar::EntryType::Regular);
            header.set_mode(metadata.permissions().mode());
            header.set_size(content.len() as u64);
            header.set_mtime(mtime);
            header.set_cksum();

            archive.append_data(&mut header, &entry_archive_path, content.as_slice())?;
        } else if file_type.is_symlink() {
            let target = fs::read_link(entry.path())?;

            let mut header = tar::Header::new_gnu();
            header.set_entry_type(tar::EntryType::Symlink);
            header.set_mode(0o777);
            header.set_size(0);
            header.set_mtime(mtime);
            header.set_cksum();

            archive.append_link(&mut header, &entry_archive_path, &target)?;
        }
    }

    Ok(())
}

fn build_binary_manifest(
    result: &BuildResult,
    components: std::collections::BTreeMap<String, super::super::binary_manifest::ComponentRef>,
    content_root: super::super::binary_manifest::Hash,
) -> Result<super::super::binary_manifest::BinaryManifest> {
    use crate::ccs::binary_manifest::{
        BinaryBuildInfo, BinaryCapability, BinaryManifest, BinaryPlatform, BinaryRequirement,
        FORMAT_VERSION,
    };

    let manifest = &result.manifest;

    let platform = manifest.package.platform.as_ref().map(|p| BinaryPlatform {
        os: p.os.clone(),
        arch: p.arch.clone(),
        libc: p.libc.clone(),
        abi: p.abi.clone(),
    });

    let provides: Vec<BinaryCapability> = manifest
        .provides
        .capabilities
        .iter()
        .map(|cap| BinaryCapability {
            name: cap.clone(),
            version: None,
        })
        .collect();

    let requires: Vec<BinaryRequirement> = manifest
        .requires
        .capabilities
        .iter()
        .map(|cap| BinaryRequirement {
            name: cap.name().to_string(),
            version: cap.version().map(String::from),
            kind: "capability".to_string(),
        })
        .chain(
            manifest
                .requires
                .packages
                .iter()
                .map(|pkg| BinaryRequirement {
                    name: pkg.name.clone(),
                    version: pkg.version.clone(),
                    kind: "package".to_string(),
                }),
        )
        .collect();

    let binary_hooks = convert_hooks_to_binary(&manifest.hooks)?;

    let build = manifest.build.as_ref().map(|b| BinaryBuildInfo {
        source: b.source.clone(),
        commit: b.commit.clone(),
        timestamp: b.timestamp.clone(),
        reproducible: b.reproducible,
    });

    Ok(BinaryManifest {
        format_version: FORMAT_VERSION,
        name: manifest.package.name.clone(),
        version: manifest.package.version.clone(),
        description: manifest.package.description.clone(),
        license: manifest.package.license.clone(),
        platform,
        provides,
        requires,
        components,
        hooks: binary_hooks,
        build,
        capabilities: manifest.capabilities.clone(),
        content_root,
        toml_integrity_hash: None,
    })
}

fn convert_hooks_to_binary(
    hooks: &crate::ccs::manifest::Hooks,
) -> crate::Result<Option<super::super::binary_manifest::BinaryHooks>> {
    use crate::ccs::binary_manifest::{
        BinaryAlternativeHook, BinaryDirectoryHook, BinaryGroupHook, BinaryHooks,
        BinaryServiceHook, BinarySysctlHook, BinarySystemdHook, BinaryTmpfilesHook, BinaryUserHook,
    };

    let binary = BinaryHooks {
        users: hooks
            .users
            .iter()
            .map(|u| BinaryUserHook {
                name: u.name.clone(),
                system: u.system,
                home: u.home.clone(),
                shell: u.shell.clone(),
                group: u.group.clone(),
                reversible: u.reversible,
            })
            .collect(),
        groups: hooks
            .groups
            .iter()
            .map(|g| BinaryGroupHook {
                name: g.name.clone(),
                system: g.system,
                reversible: g.reversible,
            })
            .collect(),
        directories: hooks
            .directories
            .iter()
            .map(|d| {
                Ok(BinaryDirectoryHook {
                    path: d.path.clone(),
                    mode: parse_octal_mode(&d.mode)?,
                    owner: d.owner.clone(),
                    group: d.group.clone(),
                    reversible: d.reversible,
                })
            })
            .collect::<crate::Result<Vec<_>>>()?,
        services: hooks
            .services
            .iter()
            .map(|s| BinaryServiceHook {
                name: s.name.clone(),
                action: s.action.clone(),
                reversible: s.reversible,
            })
            .collect(),
        systemd: hooks
            .systemd
            .iter()
            .map(|s| BinarySystemdHook {
                unit: s.unit.clone(),
                enable: s.enable,
                reversible: s.reversible,
            })
            .collect(),
        tmpfiles: hooks
            .tmpfiles
            .iter()
            .map(|t| {
                Ok(BinaryTmpfilesHook {
                    entry_type: t.entry_type.clone(),
                    path: t.path.clone(),
                    mode: parse_octal_mode(&t.mode)?,
                    owner: t.owner.clone(),
                    group: t.group.clone(),
                    reversible: t.reversible,
                })
            })
            .collect::<crate::Result<Vec<_>>>()?,
        sysctl: hooks
            .sysctl
            .iter()
            .map(|s| BinarySysctlHook {
                key: s.key.clone(),
                value: s.value.clone(),
                only_if_lower: s.only_if_lower,
                reversible: s.reversible,
            })
            .collect(),
        alternatives: hooks
            .alternatives
            .iter()
            .map(|a| BinaryAlternativeHook {
                name: a.name.clone(),
                path: a.path.clone(),
                priority: a.priority,
                reversible: a.reversible,
            })
            .collect(),
        post_install: hooks.post_install.as_ref().map(|h| h.script.clone()),
        post_install_reversible: hooks.post_install.as_ref().and_then(|h| h.reversible),
        pre_remove: hooks.pre_remove.as_ref().map(|h| h.script.clone()),
        pre_remove_reversible: hooks.pre_remove.as_ref().and_then(|h| h.reversible),
    };

    if binary.is_empty() {
        Ok(None)
    } else {
        Ok(Some(binary))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_v2_package_preserves_signed_authority() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("hello-v2.ccs");
        let authority = crate::ccs::v2::test_support::package_authority_with_one_file("hello-v2");
        let payloads = crate::ccs::v2::test_support::one_file_payloads_for_tests();
        let key = crate::ccs::signing::SigningKeyPair::generate();

        write_v2_ccs_package(
            &authority,
            &payloads,
            &path,
            &key,
            Some("[package]\nname = \"hello-v2\"\nversion = \"1.0.0\"\ndescription = \"debug\"\n"),
            None,
            None,
        )
        .unwrap();

        let contents =
            crate::ccs::archive_reader::read_ccs_archive(std::fs::File::open(path).unwrap())
                .unwrap();
        assert_eq!(
            contents.v2_authority.as_ref().unwrap().identity.name,
            "hello-v2"
        );
        assert!(contents.binary_manifest.is_none());
        assert_eq!(contents.components["main"].files.len(), 1);
        assert!(
            contents
                .blobs
                .contains_key(&crate::hash::sha256(b"hello world\n"))
        );
    }
}
