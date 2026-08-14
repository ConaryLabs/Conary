// conary-test/src/engine/release_root.rs

//! Runtime capture for authenticated Debian-derivative target evidence.

use std::collections::{BTreeSet, HashMap};
use std::time::Duration;

use anyhow::{Context, Result, bail};

use crate::config::release_root::{
    TARGET_RELEASE_EVIDENCE_SCHEMA, TargetArtifactDigestSource, TargetArtifactIdentity,
    TargetArtifactRole, TargetPackageIdentity, TargetReleaseEvidence, TargetReleaseStage,
    TargetReleaseStageResult,
};
use crate::config::{
    AuthenticatedTargetRoot, DistroReleaseRoot, ExpectedNativePackage, ExpectedOsRelease,
    NativePackageQuery, PinnedTargetFile,
};
use crate::container::{ContainerBackend, ContainerId};

const PROBE_TIMEOUT: Duration = Duration::from_secs(30);

pub async fn capture_target_release_evidence(
    backend: &dyn ContainerBackend,
    container_id: &ContainerId,
    target_profile: &str,
    root: &DistroReleaseRoot,
    checkout_commit: String,
    checkout_dirty: bool,
) -> Result<TargetReleaseEvidence> {
    root.validate()?;

    let observed_release = exec_stdout(
        backend,
        container_id,
        &[
            "sh",
            "-c",
            ". /etc/os-release; printf '%s\\n%s\\n%s\\n%s\\n' \"$ID\" \"$VERSION_ID\" \"$VERSION_CODENAME\" \"$UBUNTU_CODENAME\"",
        ],
    )
    .await?;
    let values = observed_release.lines().collect::<Vec<_>>();
    if values.len() != 4 {
        bail!("target os-release probe returned {} fields", values.len());
    }
    let observed_release = ExpectedOsRelease {
        id: values[0].to_string(),
        version_id: values[1].to_string(),
        version_codename: nonempty(values[2]),
        ubuntu_codename: nonempty(values[3]),
        id_like: None,
        build_id: None,
    };
    if observed_release != root.expected_os_release {
        bail!(
            "target os-release identity mismatch: expected {:?}, observed {:?}",
            root.expected_os_release,
            observed_release
        );
    }

    for package in [&root.keyring_package, &root.identity_package] {
        let observed = exec_stdout(
            backend,
            container_id,
            &[
                "dpkg-query",
                "--showformat=${Version}",
                "--show",
                &package.name,
            ],
        )
        .await
        .with_context(|| format!("query target package {}", package.name))?;
        if observed.trim() != package.version {
            bail!(
                "target package {} version mismatch: expected {}, observed {}",
                package.name,
                package.version,
                observed.trim()
            );
        }
    }

    exec_stdout(
        backend,
        container_id,
        &[
            "sh",
            "-c",
            "cd /etc/apt && sha256sum --check --strict /usr/lib/conary-test/apt.sha256",
        ],
    )
    .await
    .context("verify target APT declarations")?;

    let mut observed_signing_fingerprints = HashMap::new();
    for signing_root in &root.signing_roots {
        let observed = exec_stdout(backend, container_id, &["sha256sum", &signing_root.path])
            .await
            .with_context(|| format!("verify target signing root {}", signing_root.path))?;
        let observed = observed
            .split_ascii_whitespace()
            .next()
            .context("signing-root SHA-256 probe returned no digest")?;
        if observed != signing_root.sha256 {
            bail!(
                "target signing root {} digest mismatch: expected {}, observed {}",
                signing_root.path,
                signing_root.sha256,
                observed
            );
        }

        let key_listing = exec_stdout(
            backend,
            container_id,
            &[
                "gpg",
                "--batch",
                "--with-colons",
                "--show-keys",
                &signing_root.path,
            ],
        )
        .await
        .with_context(|| format!("inspect target signing root {}", signing_root.path))?;
        let observed = primary_openpgp_fingerprints(&key_listing)?;
        let expected = signing_root
            .fingerprints
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();
        if observed != expected {
            bail!(
                "target signing root {} fingerprint mismatch: expected {:?}, observed {:?}",
                signing_root.path,
                expected,
                observed
            );
        }
        observed_signing_fingerprints.insert(signing_root.path.clone(), observed);
    }

    let apt_manifest = backend
        .copy_from(container_id, "/usr/lib/conary-test/apt.sha256")
        .await
        .context("read target APT declaration identities")?;
    let apt_manifest =
        std::str::from_utf8(&apt_manifest).context("APT digest manifest is UTF-8")?;

    let conary_digest = exec_stdout(backend, container_id, &["sha256sum", "/usr/bin/conary"])
        .await?
        .split_ascii_whitespace()
        .next()
        .context("conary SHA-256 probe returned no digest")?
        .to_string();
    validate_sha256(&conary_digest, "running conary executable")?;
    let conary_version = exec_stdout(backend, container_id, &["/usr/bin/conary", "--version"])
        .await?
        .trim()
        .to_string();
    if conary_version.is_empty() {
        bail!("running conary version is empty");
    }

    let (_, base_digest) = root
        .base_image
        .rsplit_once("@sha256:")
        .context("validated base image has no digest")?;
    validate_sha256(base_digest, "base image")?;

    let mut artifacts = vec![
        TargetArtifactIdentity {
            role: TargetArtifactRole::BaseImage,
            digest_source: TargetArtifactDigestSource::PinnedBuildInput,
            identity: root.base_image.clone(),
            sha256: base_digest.to_string(),
            fingerprints: Vec::new(),
        },
        TargetArtifactIdentity {
            role: TargetArtifactRole::ReleaseMedia,
            digest_source: TargetArtifactDigestSource::PinnedReleaseAuthority,
            identity: release_media_url(root).to_string(),
            sha256: release_media_sha256(root).to_string(),
            fingerprints: Vec::new(),
        },
        TargetArtifactIdentity {
            role: TargetArtifactRole::KeyringPackage,
            digest_source: TargetArtifactDigestSource::PinnedBuildInput,
            identity: format!(
                "{}={}",
                root.keyring_package.name, root.keyring_package.version
            ),
            sha256: root.keyring_package.sha256.clone(),
            fingerprints: Vec::new(),
        },
        TargetArtifactIdentity {
            role: TargetArtifactRole::IdentityPackage,
            digest_source: TargetArtifactDigestSource::PinnedBuildInput,
            identity: format!(
                "{}={}",
                root.identity_package.name, root.identity_package.version
            ),
            sha256: root.identity_package.sha256.clone(),
            fingerprints: Vec::new(),
        },
    ];
    for line in apt_manifest.lines() {
        let (sha256, path) = line
            .split_once("  ")
            .context("malformed target APT digest manifest")?;
        validate_sha256(sha256, "APT declaration")?;
        artifacts.push(TargetArtifactIdentity {
            role: TargetArtifactRole::AptDeclaration,
            digest_source: TargetArtifactDigestSource::RunningTargetBytes,
            identity: format!("/etc/apt/{path}"),
            sha256: sha256.to_string(),
            fingerprints: Vec::new(),
        });
    }
    artifacts.extend(root.signing_roots.iter().map(|signing_root| {
        TargetArtifactIdentity {
            role: TargetArtifactRole::SigningRoot,
            digest_source: TargetArtifactDigestSource::RunningTargetBytes,
            identity: signing_root.path.clone(),
            sha256: signing_root.sha256.clone(),
            fingerprints: observed_signing_fingerprints[&signing_root.path]
                .iter()
                .cloned()
                .collect(),
        }
    }));
    artifacts.push(TargetArtifactIdentity {
        role: TargetArtifactRole::ConaryExecutable,
        digest_source: TargetArtifactDigestSource::RunningTargetBytes,
        identity: "/usr/bin/conary".to_string(),
        sha256: conary_digest,
        fingerprints: Vec::new(),
    });

    Ok(TargetReleaseEvidence {
        schema_version: TARGET_RELEASE_EVIDENCE_SCHEMA,
        target_profile: target_profile.to_string(),
        release: observed_release,
        checkout_commit,
        checkout_dirty,
        conary_version,
        packages: [&root.keyring_package, &root.identity_package]
            .into_iter()
            .map(|package| TargetPackageIdentity {
                name: package.name.clone(),
                version: package.version.clone(),
            })
            .collect(),
        artifacts,
        stages: [
            TargetReleaseStage::ReleaseIdentity,
            TargetReleaseStage::PackageIdentity,
            TargetReleaseStage::RepositoryDeclarations,
            TargetReleaseStage::ConaryRuntime,
        ]
        .into_iter()
        .map(|stage| TargetReleaseStageResult {
            stage,
            passed: true,
        })
        .collect(),
    })
}

pub async fn capture_authenticated_target_evidence(
    backend: &dyn ContainerBackend,
    container_id: &ContainerId,
    target_profile: &str,
    root: &AuthenticatedTargetRoot,
    checkout_commit: String,
    checkout_dirty: bool,
) -> Result<TargetReleaseEvidence> {
    root.validate()?;
    let observed_release = probe_os_release(backend, container_id).await?;
    if observed_release != root.expected_os_release {
        bail!(
            "target os-release identity mismatch: expected {:?}, observed {:?}",
            root.expected_os_release,
            observed_release
        );
    }

    let mut packages = Vec::new();
    for expected in &root.identity_packages {
        let version = query_package(backend, container_id, root.package_query, expected).await?;
        packages.push(TargetPackageIdentity {
            name: expected.name.clone(),
            version,
        });
    }

    let mut artifacts = Vec::new();
    let (_, base_digest) = root
        .base_image
        .rsplit_once("@sha256:")
        .context("validated base image has no digest")?;
    artifacts.push(TargetArtifactIdentity {
        role: TargetArtifactRole::BaseImage,
        digest_source: TargetArtifactDigestSource::PinnedBuildInput,
        identity: root.base_image.clone(),
        sha256: base_digest.to_string(),
        fingerprints: Vec::new(),
    });
    for declaration in &root.repository_declarations {
        artifacts.push(
            verify_target_file(
                backend,
                container_id,
                declaration,
                TargetArtifactRole::NativeRepositoryDeclaration,
                false,
            )
            .await?,
        );
    }
    for signing_root in &root.signing_roots {
        artifacts.push(
            verify_target_file(
                backend,
                container_id,
                signing_root,
                TargetArtifactRole::SigningRoot,
                true,
            )
            .await?,
        );
    }

    let conary_digest = exec_stdout(backend, container_id, &["sha256sum", "/usr/bin/conary"])
        .await?
        .split_ascii_whitespace()
        .next()
        .context("conary SHA-256 probe returned no digest")?
        .to_string();
    validate_sha256(&conary_digest, "running conary executable")?;
    let conary_version = exec_stdout(backend, container_id, &["/usr/bin/conary", "--version"])
        .await?
        .trim()
        .to_string();
    if conary_version.is_empty() {
        bail!("running conary version is empty");
    }
    artifacts.push(TargetArtifactIdentity {
        role: TargetArtifactRole::ConaryExecutable,
        digest_source: TargetArtifactDigestSource::RunningTargetBytes,
        identity: "/usr/bin/conary".to_string(),
        sha256: conary_digest,
        fingerprints: Vec::new(),
    });

    Ok(TargetReleaseEvidence {
        schema_version: TARGET_RELEASE_EVIDENCE_SCHEMA,
        target_profile: target_profile.to_string(),
        release: observed_release,
        checkout_commit,
        checkout_dirty,
        conary_version,
        packages,
        artifacts,
        stages: [
            TargetReleaseStage::ReleaseIdentity,
            TargetReleaseStage::PackageIdentity,
            TargetReleaseStage::RepositoryDeclarations,
            TargetReleaseStage::ConaryRuntime,
        ]
        .into_iter()
        .map(|stage| TargetReleaseStageResult {
            stage,
            passed: true,
        })
        .collect(),
    })
}

async fn probe_os_release(
    backend: &dyn ContainerBackend,
    container_id: &ContainerId,
) -> Result<ExpectedOsRelease> {
    let output = exec_stdout(
        backend,
        container_id,
        &[
            "sh",
            "-c",
            ". /etc/os-release; printf '%s\\n%s\\n%s\\n%s\\n%s\\n%s\\n' \"$ID\" \"$VERSION_ID\" \"${VERSION_CODENAME:-}\" \"${UBUNTU_CODENAME:-}\" \"${ID_LIKE:-}\" \"${BUILD_ID:-}\"",
        ],
    )
    .await?;
    let values = output.lines().collect::<Vec<_>>();
    if values.len() != 6 {
        bail!("target os-release probe returned {} fields", values.len());
    }
    Ok(ExpectedOsRelease {
        id: values[0].to_string(),
        version_id: values[1].to_string(),
        version_codename: nonempty(values[2]),
        ubuntu_codename: nonempty(values[3]),
        id_like: nonempty(values[4]),
        build_id: nonempty(values[5]),
    })
}

async fn query_package(
    backend: &dyn ContainerBackend,
    container_id: &ContainerId,
    query: NativePackageQuery,
    package: &ExpectedNativePackage,
) -> Result<String> {
    let command: Vec<&str> = match query {
        NativePackageQuery::Dpkg => vec![
            "dpkg-query",
            "--showformat=${Version}",
            "--show",
            &package.name,
        ],
        NativePackageQuery::Pacman => vec!["pacman", "-Q", &package.name],
        NativePackageQuery::Rpm => vec!["rpm", "-q", "--qf", "%{EVR}", &package.name],
    };
    let output = exec_stdout(backend, container_id, &command)
        .await
        .with_context(|| format!("query target package {}", package.name))?;
    let observed = match query {
        NativePackageQuery::Pacman => output
            .split_ascii_whitespace()
            .nth(1)
            .context("pacman package query returned no version")?,
        NativePackageQuery::Dpkg | NativePackageQuery::Rpm => output.trim(),
    };
    if observed != package.version {
        bail!(
            "target package {} version mismatch: expected {}, observed {}",
            package.name,
            package.version,
            observed
        );
    }
    Ok(observed.to_string())
}

async fn verify_target_file(
    backend: &dyn ContainerBackend,
    container_id: &ContainerId,
    file: &PinnedTargetFile,
    role: TargetArtifactRole,
    inspect_fingerprints: bool,
) -> Result<TargetArtifactIdentity> {
    let output = exec_stdout(backend, container_id, &["sha256sum", &file.path]).await?;
    let observed = output
        .split_ascii_whitespace()
        .next()
        .context("target file SHA-256 probe returned no digest")?;
    if observed != file.sha256 {
        bail!(
            "target file {} digest mismatch: expected {}, observed {}",
            file.path,
            file.sha256,
            observed
        );
    }
    let fingerprints = if inspect_fingerprints {
        let listing = exec_stdout(
            backend,
            container_id,
            &["gpg", "--batch", "--with-colons", "--show-keys", &file.path],
        )
        .await?;
        let observed = primary_openpgp_fingerprints(&listing)?;
        let expected = file.fingerprints.iter().cloned().collect::<BTreeSet<_>>();
        if observed != expected {
            bail!(
                "target signing root {} fingerprint mismatch: expected {:?}, observed {:?}",
                file.path,
                expected,
                observed
            );
        }
        observed.into_iter().collect()
    } else {
        Vec::new()
    };
    Ok(TargetArtifactIdentity {
        role,
        digest_source: TargetArtifactDigestSource::RunningTargetBytes,
        identity: file.path.clone(),
        sha256: file.sha256.clone(),
        fingerprints,
    })
}

fn nonempty(value: &str) -> Option<String> {
    (!value.is_empty()).then(|| value.to_string())
}

async fn exec_stdout(
    backend: &dyn ContainerBackend,
    container_id: &ContainerId,
    command: &[&str],
) -> Result<String> {
    let result = backend.exec(container_id, command, PROBE_TIMEOUT).await?;
    if result.exit_code != 0 {
        bail!(
            "target release probe {:?} failed with exit {}: {}",
            command,
            result.exit_code,
            result.stderr.trim()
        );
    }
    Ok(result.stdout)
}

fn validate_sha256(value: &str, subject: &str) -> Result<()> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("{subject} has an invalid SHA-256 digest");
    }
    Ok(())
}

fn primary_openpgp_fingerprints(listing: &str) -> Result<BTreeSet<String>> {
    let mut fingerprints = BTreeSet::new();
    let mut expect_primary_fingerprint = false;
    for line in listing.lines() {
        let fields = line.split(':').collect::<Vec<_>>();
        match fields.first().copied() {
            Some("pub") => expect_primary_fingerprint = true,
            Some("sub" | "sec" | "ssb") => expect_primary_fingerprint = false,
            Some("fpr") if expect_primary_fingerprint => {
                let fingerprint = fields
                    .get(9)
                    .copied()
                    .context("OpenPGP primary fingerprint record is malformed")?;
                if fingerprint.len() != 40
                    || !fingerprint.bytes().all(|byte| byte.is_ascii_hexdigit())
                    || fingerprint != fingerprint.to_ascii_uppercase()
                {
                    bail!("OpenPGP primary fingerprint record is invalid");
                }
                fingerprints.insert(fingerprint.to_string());
                expect_primary_fingerprint = false;
            }
            _ => {}
        }
    }
    if fingerprints.is_empty() {
        bail!("OpenPGP key listing contains no primary certificate fingerprints");
    }
    Ok(fingerprints)
}

fn release_media_url(root: &DistroReleaseRoot) -> &str {
    match &root.release_media {
        crate::config::ReleaseMediaAuthority::OpenPgp { media_url, .. }
        | crate::config::ReleaseMediaAuthority::HttpsMetadata { media_url, .. } => media_url,
    }
}

fn release_media_sha256(root: &DistroReleaseRoot) -> &str {
    match &root.release_media {
        crate::config::ReleaseMediaAuthority::OpenPgp { media_sha256, .. }
        | crate::config::ReleaseMediaAuthority::HttpsMetadata { media_sha256, .. } => media_sha256,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primary_fingerprint_parser_ignores_subkeys_and_requires_typed_records() {
        let listing = concat!(
            "pub:-:255:22:KEY:0:0:::::::ed25519:::0:\n",
            "fpr:::::::::AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA:\n",
            "uid:-::::0::HASH::Release Key::::::::::0:\n",
            "sub:-:255:18:SUB:0:0:::::::cv25519::::\n",
            "fpr:::::::::BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB:\n",
        );

        assert_eq!(
            primary_openpgp_fingerprints(listing).unwrap(),
            BTreeSet::from(["A".repeat(40)])
        );
        assert!(primary_openpgp_fingerprints("sub:::::::::\n").is_err());
    }
}
