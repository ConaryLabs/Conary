// apps/remi/src/server/native_publish/storage.rs

use std::path::PathBuf;

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
    pub chunk_path: PathBuf,
    pub target_path: String,
}

impl PromotedNativeArtifact {
    pub fn cleanup_package_path_blocking(package_path: &std::path::Path) {
        let _ = std::fs::remove_file(package_path);
    }
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
