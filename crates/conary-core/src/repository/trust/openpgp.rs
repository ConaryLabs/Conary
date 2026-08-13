// conary-core/src/repository/trust/openpgp.rs

//! Pinned OpenPGP trust preparation and verification.
//!
//! This file is a hub.  It owns role dispatch and the prepared-trust handle;
//! the authority models live in focused modules:
//!
//! * [`store`] -- trust-store layout and key-source transport.
//! * [`pinned`] -- Debian/rpm-md/RPM pinned-certificate verification.
//! * [`arch`] -- pacman keyring authority, trust snapshots, and ALPM
//!   signature semantics.

pub mod arch;
mod pinned;
mod store;

use super::{
    ArchKeyringTrust, OpenPgpTrustRoot, RepositoryTrustPolicy, RpmMetadataAuthority, TrustRole,
};
use crate::error::{Error, Result};
use openpgp::serialize::Serialize;
use sequoia_openpgp as openpgp;
use std::path::{Path, PathBuf};

pub use arch::snapshot::{
    ArchKeyAuthority, ArchKeyringEntry, ArchSignatureValidity, ArchTrustSnapshot,
};
pub use arch::verify::{ArchInvalidReason, ArchSignatureResult, ArchSignatureStatus};

#[derive(Debug, Clone)]
pub struct PreparedOpenPgpTrust {
    repository_name: String,
    keyring_dir: PathBuf,
    policy: RepositoryTrustPolicy,
}

impl PreparedOpenPgpTrust {
    #[cfg(test)]
    pub(crate) fn for_test(policy: RepositoryTrustPolicy) -> Self {
        policy.validate().expect("test trust policy must be valid");
        Self {
            repository_name: "test-repository".to_string(),
            keyring_dir: PathBuf::from("/unused/test-keyring"),
            policy,
        }
    }

    pub async fn prepare(
        repository_name: &str,
        keyring_dir: &Path,
        policy: &RepositoryTrustPolicy,
    ) -> Result<Self> {
        policy.validate()?;
        if let RepositoryTrustPolicy::Arch { keyring, .. } = policy {
            arch::prepare(repository_name, keyring_dir, keyring).await?;
        }
        for (role, roots) in policy_roles(policy) {
            for root in roots {
                store::prepare_root(repository_name, keyring_dir, role, root).await?;
            }
        }
        Ok(Self {
            repository_name: repository_name.to_string(),
            keyring_dir: keyring_dir.to_path_buf(),
            policy: policy.clone(),
        })
    }

    pub fn from_prepared(
        repository_name: &str,
        keyring_dir: &Path,
        policy: &RepositoryTrustPolicy,
    ) -> Result<Self> {
        policy.validate()?;
        let prepared = Self {
            repository_name: repository_name.to_string(),
            keyring_dir: keyring_dir.to_path_buf(),
            policy: policy.clone(),
        };
        if let RepositoryTrustPolicy::Arch { keyring, .. } = policy {
            prepared.arch_snapshot(keyring)?;
        }
        for (role, roots) in policy_roles(policy) {
            store::load_role_certificates(repository_name, keyring_dir, role, roots)?;
        }
        Ok(prepared)
    }

    pub fn policy(&self) -> &RepositoryTrustPolicy {
        &self.policy
    }

    /// Verifies a detached signature for one trust role.
    ///
    /// Arch database and package objects are evaluated against the
    /// authenticated keyring snapshot; every other role is a pinned
    /// certificate set.  The two authority models are never mixed.
    pub fn verify_detached(&self, role: TrustRole, data: &[u8], signature: &[u8]) -> Result<()> {
        match (&self.policy, role) {
            (
                RepositoryTrustPolicy::Arch { keyring, .. },
                TrustRole::ArchDatabase | TrustRole::ArchPackage,
            ) => {
                let snapshot = self.arch_snapshot(keyring)?;
                arch::verify::check_detached(&snapshot, data, signature)
                    .map(|_| ())
                    .map_err(|error| self.role_error(role, "signature", error))
            }
            _ => {
                let certificates = self.pinned_certificates_for_role(role)?;
                pinned::verify_detached_with_certificates(data, signature, certificates).map_err(
                    |error| {
                        self.role_error(
                            role,
                            "signature",
                            Error::GpgVerificationFailed(error.to_string()),
                        )
                    },
                )
            }
        }
    }

    /// Verifies an inline-signed object for one trust role.
    ///
    /// ALPM has no inline-signed object, so this is a pinned-certificate path
    /// only; asking for an Arch role here is a typed error rather than a
    /// silent fallback onto keyring semantics.
    pub fn verify_inline(&self, role: TrustRole, signed_data: &[u8]) -> Result<Vec<u8>> {
        let certificates = self.pinned_certificates_for_role(role)?;
        pinned::verify_inline_with_certificates(signed_data, certificates).map_err(|error| {
            self.role_error(
                role,
                "inline signature",
                Error::GpgVerificationFailed(error.to_string()),
            )
        })
    }

    /// Detailed, inspectable results for an Arch object signature.
    pub fn verify_arch_signature(
        &self,
        role: TrustRole,
        data: &[u8],
        signature: &[u8],
    ) -> Result<Vec<ArchSignatureResult>> {
        let RepositoryTrustPolicy::Arch { keyring, .. } = &self.policy else {
            return Err(Error::ConfigError(format!(
                "repository '{}' does not use ALPM keyring authority",
                self.repository_name
            )));
        };
        if !matches!(role, TrustRole::ArchDatabase | TrustRole::ArchPackage) {
            return Err(Error::ConfigError(format!(
                "trust role {} is not an ALPM object role",
                role.as_str()
            )));
        }
        arch::verify::verify_detached(&self.arch_snapshot(keyring)?, data, signature)
    }

    /// Serializes the certificates a role currently trusts.
    pub fn armored_keyring(&self, role: TrustRole) -> Result<Vec<u8>> {
        let certificates = match (&self.policy, role) {
            (
                RepositoryTrustPolicy::Arch { keyring, .. },
                TrustRole::ArchDatabase | TrustRole::ArchPackage,
            ) => self.arch_snapshot(keyring)?.fully_valid_certificates(),
            _ => self.pinned_certificates_for_role(role)?,
        };
        let mut output = Vec::new();
        for certificate in certificates {
            certificate
                .armored()
                .serialize(&mut output)
                .map_err(|error| {
                    Error::IoError(format!(
                        "failed to serialize trusted OpenPGP certificate: {error}"
                    ))
                })?;
        }
        Ok(output)
    }

    fn arch_snapshot(&self, keyring: &ArchKeyringTrust) -> Result<ArchTrustSnapshot> {
        arch::load_snapshot(&self.repository_name, &self.keyring_dir, keyring)
    }

    fn pinned_certificates_for_role(&self, role: TrustRole) -> Result<Vec<openpgp::Cert>> {
        if matches!(self.policy, RepositoryTrustPolicy::Arch { .. }) {
            return Err(Error::ConfigError(format!(
                "trust role {} has no pinned certificate set under ALPM keyring authority",
                role.as_str()
            )));
        }
        let roots = self.policy.roots_for_role(role)?;
        store::load_role_certificates(&self.repository_name, &self.keyring_dir, role, roots)
    }

    fn role_error(&self, role: TrustRole, object: &str, error: Error) -> Error {
        Error::GpgVerificationFailed(format!(
            "{} {object} for repository '{}' failed: {error}",
            role.as_str(),
            self.repository_name
        ))
    }
}

fn policy_roles(policy: &RepositoryTrustPolicy) -> Vec<(TrustRole, &[OpenPgpTrustRoot])> {
    match policy {
        RepositoryTrustPolicy::Debian { release_keys } => {
            vec![(TrustRole::DebianRelease, release_keys)]
        }
        RepositoryTrustPolicy::Rpm {
            metadata,
            package_keys,
        } => {
            let mut roles = Vec::with_capacity(2);
            if let RpmMetadataAuthority::OpenPgp { keys } = metadata {
                roles.push((TrustRole::RpmMetadata, keys.as_slice()));
            }
            roles.push((TrustRole::RpmPackage, package_keys));
            roles
        }
        RepositoryTrustPolicy::Arch { .. } => Vec::new(),
        RepositoryTrustPolicy::Eopkg { .. } => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn missing_prepared_key_is_fatal() {
        let temp = tempfile::tempdir().unwrap();
        let root = OpenPgpTrustRoot {
            url: "https://keys.example.test/archive.asc".to_string(),
            fingerprint: "A".repeat(40),
        };
        let policy = RepositoryTrustPolicy::Debian {
            release_keys: vec![root],
        };

        let error =
            PreparedOpenPgpTrust::from_prepared("debian", temp.path(), &policy).unwrap_err();
        assert!(error.to_string().contains("missing"));
    }

    #[test]
    fn missing_prepared_arch_keyring_is_fatal() {
        let temp = tempfile::tempdir().unwrap();
        let policy = RepositoryTrustPolicy::Arch {
            keyring: arch::testkeys::trust(vec!["A".repeat(40)], 1),
            sig_level: super::super::ArchSigLevel::distribution_default(),
        };
        let error =
            PreparedOpenPgpTrust::from_prepared("arch-core", temp.path(), &policy).unwrap_err();
        assert!(error.to_string().contains("Arch keyring certificates"));
    }

    /// End-to-end proof of the Arch role: prepare from a keyring source, load
    /// the persisted snapshot back off disk, and verify a real package.
    ///
    /// Unlike the pinned-time corpus in `arch::corpus`, this runs at wall-clock
    /// now, which is the reference time production uses.
    #[tokio::test]
    async fn arch_package_trust_round_trips_through_the_prepared_store() {
        const KEYRING: &[u8] = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/arch/archlinux-keyring-corpus.pkg.tar.zst"
        ));
        const PACKAGE: &[u8] = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/arch/packages/alpine-keyring-2.6-1-any.pkg.tar.zst"
        ));
        const SIGNATURE: &[u8] = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/arch/packages/alpine-keyring-2.6-1-any.pkg.tar.zst.sig"
        ));

        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("archlinux-keyring.pkg.tar.zst");
        std::fs::write(&source, KEYRING).unwrap();
        let policy = RepositoryTrustPolicy::Arch {
            keyring: super::super::ArchKeyringTrust {
                url: url::Url::from_file_path(&source).unwrap().to_string(),
                format: super::super::ArchKeyringFormat::AlpmPackageZstd,
                master_fingerprints: vec![
                    "2AC0A42EFB0B5CBC7A0402ED4DC95B6D7BE9892E".to_string(),
                    "3572FA2A1B067F22C58AF155F8B821B42A6FDCD7".to_string(),
                    "69E6471E3AE065297529832E6BA0F5A2037F4F41".to_string(),
                    "99B6618472A3B3B814185BAED7D3D823B88BDB9B".to_string(),
                    "D8AFDDA07A5B6EDFA7D8CCDAD6D055F927843F1C".to_string(),
                ],
                packager_key_threshold: 3,
            },
            sig_level: super::super::ArchSigLevel::distribution_default(),
        };

        let store = temp.path().join("keyrings");
        PreparedOpenPgpTrust::prepare("arch-extra", &store, &policy)
            .await
            .expect("preparing the bounded corpus keyring must succeed");
        let prepared = PreparedOpenPgpTrust::from_prepared("arch-extra", &store, &policy)
            .expect("the prepared Arch snapshot must load back");

        prepared
            .verify_detached(TrustRole::ArchPackage, PACKAGE, SIGNATURE)
            .expect(
                "the pinned alpine-keyring fixture must verify at wall-clock now; if this starts \
                 failing, refresh tests/fixtures/arch and its recorded oracle classes",
            );
        let results = prepared
            .verify_arch_signature(TrustRole::ArchPackage, PACKAGE, SIGNATURE)
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].status, ArchSignatureStatus::Valid);
        assert_eq!(results[0].validity, ArchSignatureValidity::Full);

        let mut tampered = PACKAGE.to_vec();
        let last = tampered.len() - 1;
        tampered[last] ^= 0xff;
        assert!(
            prepared
                .verify_detached(TrustRole::ArchPackage, &tampered, SIGNATURE)
                .is_err()
        );

        // ALPM has no inline-signed object, and Arch roles have no pinned
        // certificate set; both are typed errors rather than fallbacks.
        assert!(
            prepared
                .verify_inline(TrustRole::ArchDatabase, SIGNATURE)
                .is_err()
        );
        assert!(
            !prepared
                .armored_keyring(TrustRole::ArchPackage)
                .unwrap()
                .is_empty()
        );
    }
}
