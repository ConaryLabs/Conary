// apps/remi/src/server/native_publish/verify.rs

use std::fs::File;
use std::path::Path;

use conary_core::ccs::v3::PackageKindTagV3;
use conary_core::db::models::normalize_native_architecture;
use conary_core::hash;
use conary_core::repository::static_repo::publish_gate::{
    AcceptedStaticSignerSet, TrustedArtifactSigner, format_publish_gate_failures,
    verify_static_artifact_publish_candidate,
};

use crate::server::config::ReleasePublishSection;
use crate::server::native_publish::{
    NativePublishError, NativePublishErrorCode, VerifiedNativeArtifact,
};

pub(crate) fn validate_supported_release_distro(distro: &str) -> Result<(), NativePublishError> {
    if conary_core::repository::supported_profiles::route_by_slug(distro).is_some() {
        Ok(())
    } else {
        Err(NativePublishError::unprocessable(
            NativePublishErrorCode::UnsupportedDistro,
            format!("unsupported release distro {distro}"),
        ))
    }
}

pub(crate) fn release_feed_for_route(
    distro: &str,
) -> Result<
    &'static conary_core::repository::supported_profiles::SupportedProfile,
    NativePublishError,
> {
    conary_core::repository::supported_profiles::profile_for_remi_route(distro).ok_or_else(|| {
        NativePublishError::unprocessable(
            NativePublishErrorCode::UnsupportedDistro,
            format!("release distro {distro} does not map to exactly one public repository feed"),
        )
    })
}

pub(crate) fn accepted_release_signers(
    release_publish: &ReleasePublishSection,
) -> Result<AcceptedStaticSignerSet, NativePublishError> {
    let trusted = release_publish
        .trusted_build_attestation_signers
        .iter()
        .map(|signer| TrustedArtifactSigner {
            key_id: signer.key_id.clone(),
            public_key: signer.public_key.clone(),
        })
        .collect::<Vec<_>>();

    AcceptedStaticSignerSet::from_trusted_artifact_signers(&trusted).map_err(|error| {
        NativePublishError::unprocessable(
            NativePublishErrorCode::PublishGateFailed,
            error.to_string(),
        )
    })
}

pub(crate) fn verify_native_artifact(
    artifact_path: &Path,
    route_slug: &str,
    accepted_signers: &AcceptedStaticSignerSet,
    accepted_policy_digest: &str,
) -> Result<VerifiedNativeArtifact, NativePublishError> {
    release_feed_for_route(route_slug)?;
    let candidate = verify_static_artifact_publish_candidate(
        artifact_path,
        accepted_signers,
        accepted_policy_digest,
    )
    .map_err(|error| {
        NativePublishError::unprocessable(
            NativePublishErrorCode::InvalidCcs,
            format!("release artifact gate failed: {error}"),
        )
    })?;

    if !candidate.lint.is_passed() {
        return Err(NativePublishError::unprocessable(
            NativePublishErrorCode::PublishGateFailed,
            format_publish_gate_failures(&candidate.lint),
        ));
    }

    let authority = candidate.package.v3_authority().ok_or_else(|| {
        NativePublishError::unprocessable(
            NativePublishErrorCode::UnsupportedCcsFormat,
            "release upload requires a native CCS v3 authority document",
        )
    })?;
    conary_core::ccs::v3::validate_authority(authority).map_err(|error| {
        NativePublishError::unprocessable(
            NativePublishErrorCode::InvalidAuthority,
            format!("native release authority validation failed: {error}"),
        )
    })?;
    let identity = &authority.identity;
    let name = identity.name.clone();
    let version = identity.version.clone();
    let package_release = identity.release.clone();
    let architecture = normalize_native_architecture(identity.architecture.as_deref());
    let package_kind = match identity.kind {
        PackageKindTagV3::Package => "package",
        PackageKindTagV3::Group => "group",
        PackageKindTagV3::Redirect => "redirect",
    }
    .to_string();
    let authority_format_version = i64::from(authority.format_version);
    let total_size = std::fs::metadata(artifact_path)
        .map_err(|error| {
            NativePublishError::internal(
                NativePublishErrorCode::IoError,
                format!("read native artifact metadata: {error}"),
            )
        })?
        .len();
    let mut reader = File::open(artifact_path).map_err(|error| {
        NativePublishError::internal(
            NativePublishErrorCode::IoError,
            format!("open native artifact for hashing: {error}"),
        )
    })?;
    let content_hash = hash::sha256_reader_hex(&mut reader).map_err(|error| {
        NativePublishError::internal(
            NativePublishErrorCode::IoError,
            format!("hash native artifact: {error}"),
        )
    })?;

    Ok(VerifiedNativeArtifact {
        package: candidate.package,
        verification: candidate.verification,
        lint: candidate.lint,
        name,
        version,
        package_release,
        architecture,
        package_kind,
        authority_format_version,
        content_hash,
        total_size,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::config::TrustedBuildAttestationSigner;

    #[test]
    fn validate_supported_release_distro_rejects_unknown_distro() {
        let error = validate_supported_release_distro("test-distro").unwrap_err();

        assert_eq!(error.code, NativePublishErrorCode::UnsupportedDistro);
        assert_eq!(error.status, axum::http::StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[test]
    fn release_route_resolves_to_supported_profile() {
        let feed = release_feed_for_route("fedora").expect("fedora repository feed");
        assert_eq!(feed.id(), "fedora-44");
    }

    #[test]
    fn release_route_rejects_unknown_profile_before_artifact_verification() {
        let error = release_feed_for_route("debian").unwrap_err();
        assert_eq!(error.code, NativePublishErrorCode::UnsupportedDistro);
    }

    #[test]
    fn accepted_release_signers_fail_closed_when_empty() {
        let error = accepted_release_signers(&ReleasePublishSection::default()).unwrap_err();

        assert_eq!(error.code, NativePublishErrorCode::PublishGateFailed);
        assert!(
            error
                .message
                .contains("no trusted release signers configured")
        );
    }

    #[test]
    fn accepted_release_signers_accept_configured_signer() {
        let release_publish = ReleasePublishSection {
            repository_keys_dir: None,
            trusted_build_attestation_signers: vec![TrustedBuildAttestationSigner {
                key_id: "publish".to_string(),
                public_key: "pub".to_string(),
            }],
        };

        let accepted = accepted_release_signers(&release_publish).unwrap();

        assert!(accepted.accepts_key_id("publish"));
    }
}
