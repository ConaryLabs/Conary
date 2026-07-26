// apps/conary/src/commands/try_session/package_verification.rs

//! Trusted CCS package acquisition for try-session lifecycle operations.

use std::path::Path;

use anyhow::{Context, Result};
use conary_core::ccs::CcsPackage;
use conary_core::ccs::verify::{TrustPolicy, verify_package};

pub(super) struct VerifiedTryPackage {
    pub(super) package: CcsPackage,
    pub(super) signing_key: String,
}

pub(super) fn load_verified_try_package(
    path: &Path,
    trust_policy: &TrustPolicy,
    purpose: &str,
) -> Result<VerifiedTryPackage> {
    let verified = verify_package(path, trust_policy)
        .map_err(|error| anyhow::anyhow!(error))
        .with_context(|| format!("failed to verify {purpose} {}", path.display()))?;
    let signing_key = verified.signature().public_key.clone();
    let package_path = path.to_string_lossy();
    let package = CcsPackage::from_verified_archive(&package_path, &verified)
        .map_err(|error| anyhow::anyhow!(error))
        .with_context(|| format!("failed to construct verified {purpose} {}", path.display()))?;
    Ok(VerifiedTryPackage {
        package,
        signing_key,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::try_session::test_support::TryRuntimeFixture;
    use conary_core::ccs::manifest::CcsManifest;

    #[test]
    fn verified_try_package_rejects_tampered_copied_bytes() {
        let fixture = TryRuntimeFixture::new();
        let path = fixture.write_package(
            "try-tamper",
            CcsManifest::new_minimal("try-tamper", "1.0.0"),
        );
        let trusted =
            load_verified_try_package(&path, &fixture.trust_policy, "try test package").unwrap();
        assert_eq!(trusted.signing_key, fixture.signing_key.public_key_base64());

        let mut bytes = std::fs::read(&path).unwrap();
        let offset = bytes.len() / 2;
        bytes[offset] ^= 0xff;
        std::fs::write(&path, bytes).unwrap();
        let error = load_verified_try_package(&path, &fixture.trust_policy, "try test package")
            .err()
            .expect("tampered package must not produce a verification capability");
        assert!(error.to_string().contains("failed to verify"));
    }
}
