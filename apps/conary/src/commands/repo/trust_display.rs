// src/commands/repo/trust_display.rs

//! Human-readable projection of typed repository trust policies.

use conary_core::repository::{RepositoryTrustPolicy, RpmMetadataAuthority};

pub(super) fn describe(policy: &RepositoryTrustPolicy) -> String {
    match policy {
        RepositoryTrustPolicy::Debian { release_keys } => {
            format!(
                "Debian Release chain ({} pinned key(s))",
                release_keys.len()
            )
        }
        RepositoryTrustPolicy::Rpm {
            metadata,
            package_keys,
        } => format!(
            "RPM metadata/package chain ({}, {} package key(s))",
            match metadata {
                RpmMetadataAuthority::OpenPgp { keys } => {
                    format!("{} metadata key(s)", keys.len())
                }
                RpmMetadataAuthority::Metalink { .. } => "exact metalink identity".to_string(),
            },
            package_keys.len()
        ),
        RepositoryTrustPolicy::Arch { keyring, sig_level } => format!(
            "Arch database/package chain ({} pinned master key(s), {} certification threshold, \
             database {:?}, package {:?})",
            keyring.master_fingerprints.len(),
            keyring.packager_key_threshold,
            sig_level.database,
            sig_level.package
        ),
        RepositoryTrustPolicy::Eopkg { origin } => {
            format!("eopkg HTTPS origin {origin} with same-origin compressed-index SHA-256 sidecar")
        }
    }
}
