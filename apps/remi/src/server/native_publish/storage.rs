// apps/remi/src/server/native_publish/storage.rs

use std::path::{Path, PathBuf};

use crate::server::native_publish::{
    NativePublishError, NativePublishErrorCode, VerifiedNativeArtifact,
};

pub fn safe_native_ccs_filename(
    name: &str,
    version: &str,
    package_release: &str,
    architecture: &str,
    content_hash: &str,
) -> String {
    let hash_prefix = content_hash.get(..12).unwrap_or(content_hash);
    format!(
        "{}-{}-{}-{}-{hash_prefix}.ccs",
        target_safe_segment(name),
        target_safe_segment(version),
        target_safe_segment(package_release),
        target_safe_segment(architecture),
    )
}

pub fn native_target_path(
    distro: &str,
    name: &str,
    version: &str,
    package_release: &str,
    architecture: &str,
    content_hash: &str,
) -> String {
    format!(
        "packages/{}/{}",
        target_safe_segment(distro),
        safe_native_ccs_filename(name, version, package_release, architecture, content_hash)
    )
}

fn target_safe_segment(value: &str) -> String {
    let mut output = String::new();
    for byte in value.as_bytes() {
        let ch = *byte as char;
        if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') {
            output.push(ch);
        } else {
            output.push_str(&format!("%{byte:02X}"));
        }
    }
    output
}

#[derive(Debug, Clone)]
pub struct PromotedNativeArtifact {
    pub package_path: PathBuf,
    pub target_path: String,
}

impl PromotedNativeArtifact {
    pub fn cleanup_package_path_blocking(package_path: &std::path::Path) {
        let _ = std::fs::remove_file(package_path);
    }

    pub async fn cleanup_public_objects(&self) {
        let _ = tokio::fs::remove_file(&self.package_path).await;
    }
}

pub async fn promote_native_artifact(
    cache_dir: &Path,
    chunk_dir: &Path,
    distro: &str,
    staged_path: &Path,
    artifact: &VerifiedNativeArtifact,
) -> Result<PromotedNativeArtifact, NativePublishError> {
    let packages_dir = cache_dir.join("releases").join("packages").join(distro);
    tokio::fs::create_dir_all(&packages_dir)
        .await
        .map_err(|error| {
            NativePublishError::internal(
                NativePublishErrorCode::IoError,
                format!("create native release package directory: {error}"),
            )
        })?;
    let filename = safe_native_ccs_filename(
        &artifact.name,
        &artifact.version,
        &artifact.package_release,
        &artifact.architecture,
        &artifact.content_hash,
    );
    let package_path = packages_dir.join(filename);
    tokio::fs::copy(staged_path, &package_path)
        .await
        .map_err(|error| {
            NativePublishError::internal(
                NativePublishErrorCode::IoError,
                format!("promote native release package: {error}"),
            )
        })?;

    let objects_dir = chunk_dir.join("objects");
    let verification = artifact.verification.clone();
    if let Err(error) = tokio::task::spawn_blocking(move || {
        let cas = conary_core::filesystem::CasStore::new(objects_dir)?;
        conary_core::ccs::transport::persist_verified_archive_objects(&verification, &cas)
            .map(|_| ())
    })
    .await
    .map_err(anyhow::Error::from)
    .and_then(|result| result)
    {
        let _ = tokio::fs::remove_file(&package_path).await;
        return Err(NativePublishError::internal(
            NativePublishErrorCode::IoError,
            format!("promote native release signed objects: {error}"),
        ));
    }

    Ok(PromotedNativeArtifact {
        package_path,
        target_path: native_target_path(
            distro,
            &artifact.name,
            &artifact.version,
            &artifact.package_release,
            &artifact.architecture,
            &artifact.content_hash,
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_target_path_includes_release_arch_and_hash() {
        let path = native_target_path(
            "test-distro",
            "hello/pkg",
            "1.0.0",
            "1",
            "noarch",
            "abcdef0123456789",
        );
        assert_eq!(
            path,
            "packages/test-distro/hello%2Fpkg-1.0.0-1-noarch-abcdef012345.ccs"
        );
    }

    #[test]
    fn native_target_path_percent_encodes_release_without_collisions() {
        let slash = native_target_path("fedora", "hello", "1.0.0", "1/2", "x86_64", "abcdef012345");
        let dash = native_target_path("fedora", "hello", "1.0.0", "1-2", "x86_64", "abcdef012345");

        assert!(slash.contains("1%2F2"));
        assert_ne!(slash, dash);
    }
}
