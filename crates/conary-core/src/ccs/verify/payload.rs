// conary-core/src/ccs/verify/payload.rs

//! Exact CCS payload-authority verification.

use super::ContentStatus;
use crate::ccs::builder::{ComponentData, FileEntry};
use crate::ccs::v3::schema::{AuthorityDocumentV3, PackageKindV3};
use crate::payload::PayloadNodeKind;
use anyhow::Result;
use std::collections::{HashMap, HashSet};

pub(super) fn verify_v3_archive_payload(
    authority: &AuthorityDocumentV3,
    components: &HashMap<String, ComponentData>,
    blobs: &HashMap<String, Vec<u8>>,
) -> Result<ContentStatus> {
    let PackageKindV3::Package(data) = &authority.kind else {
        return Ok(ContentStatus::Skipped);
    };

    let mut errors = Vec::new();
    let signed_files: HashSet<(&str, &str)> = data
        .files
        .iter()
        .map(|file| (file.component.as_str(), file.path.as_str()))
        .collect();

    for (component_name, component) in components {
        if !authority.components.contains_key(component_name) {
            errors.push(format!(
                "v3 archive carries unsigned component {component_name}"
            ));
        }
        for component_file in &component.files {
            if !signed_files.contains(&(component_name.as_str(), component_file.path.as_str())) {
                errors.push(format!(
                    "v3 archive carries unsigned file {} in component {}",
                    component_file.path, component_name
                ));
            }
        }
    }

    for file in &data.files {
        if let Err(error) = file
            .node
            .validate()
            .and_then(|()| file.node.validate_content(file.content.as_ref()))
        {
            errors.push(format!(
                "v3 payload authority for {} is invalid: {error}",
                file.path
            ));
            continue;
        }
        let Some(component) = components.get(&file.component) else {
            errors.push(format!(
                "v3 file {} references missing component {}",
                file.path, file.component
            ));
            continue;
        };
        let Some(component_file) = component.files.iter().find(|item| item.path == file.path)
        else {
            errors.push(format!(
                "v3 signed file {} missing from component {}",
                file.path, file.component
            ));
            continue;
        };
        if component_file.node != file.node
            || component_file.content != file.content
            || component_file.component != file.component
        {
            errors.push(format!(
                "v3 file authority mismatch for {} in component {}",
                file.path, file.component
            ));
            continue;
        }
        verify_file_content(component_file, blobs, &mut errors);
    }

    if errors.is_empty() {
        Ok(ContentStatus::Valid {
            files_checked: data.files.len(),
        })
    } else {
        Ok(ContentStatus::Invalid { errors })
    }
}

fn verify_file_content(
    file: &FileEntry,
    blobs: &HashMap<String, Vec<u8>>,
    errors: &mut Vec<String>,
) {
    let PayloadNodeKind::Regular { .. } = &file.node.kind else {
        if file.chunks.is_some() {
            errors.push(format!(
                "{}: non-regular node carries chunk references",
                file.path
            ));
        }
        return;
    };
    let Some(authority) = &file.content else {
        errors.push(format!(
            "{}: regular node has no content authority",
            file.path
        ));
        return;
    };

    if let Some(chunks) = &file.chunks {
        let mut assembled = Vec::new();
        for chunk_hash in chunks {
            match blobs.get(chunk_hash) {
                Some(content) if crate::hash::sha256(content) == *chunk_hash => {
                    assembled.extend_from_slice(content);
                }
                Some(_) => errors.push(format!(
                    "{}: chunk hash mismatch for {}",
                    file.path, chunk_hash
                )),
                None => errors.push(format!("{}: missing chunk blob {}", file.path, chunk_hash)),
            }
        }
        verify_bytes(file, authority, &assembled, errors);
        return;
    }

    match blobs.get(&authority.sha256) {
        Some(content) => verify_bytes(file, authority, content, errors),
        None => errors.push(format!("{}: missing blob {}", file.path, authority.sha256)),
    }
}

fn verify_bytes(
    file: &FileEntry,
    authority: &crate::payload::PayloadContentAuthority,
    content: &[u8],
    errors: &mut Vec<String>,
) {
    if content.len() as u64 != authority.size {
        errors.push(format!(
            "{}: expected {} bytes, got {}",
            file.path,
            authority.size,
            content.len()
        ));
    }
    let actual_hash = crate::hash::sha256(content);
    if actual_hash != authority.sha256 {
        errors.push(format!(
            "{}: expected {}, got {}",
            file.path, authority.sha256, actual_hash
        ));
    }
}
