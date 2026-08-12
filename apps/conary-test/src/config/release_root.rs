// conary-test/src/config/release_root.rs

use std::collections::{BTreeSet, HashMap};

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

pub const TARGET_RELEASE_EVIDENCE_SCHEMA: u32 = 2;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PinnedDebPackage {
    pub name: String,
    pub version: String,
    pub url: String,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ExpectedOsRelease {
    pub id: String,
    pub version_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version_codename: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ubuntu_codename: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id_like: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ExpectedNativePackage {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum NativePackageQuery {
    Dpkg,
    Pacman,
    Rpm,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "ecosystem", rename_all = "kebab-case", deny_unknown_fields)]
pub enum NativeRepositoryTrustBinding {
    Arch {
        repository: String,
        metadata_url: String,
        keyring_url: String,
        master_fingerprints: Vec<String>,
        packager_key_threshold: u32,
    },
    RpmGlobal {
        repository: String,
        signing_root_paths: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PinnedTargetFile {
    pub path: String,
    pub sha256: String,
    pub fingerprints: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AptGlobalTrustBinding {
    pub document: String,
    pub entry_indexes: Vec<usize>,
    pub signing_root_paths: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "checksum_authority", rename_all = "kebab-case")]
pub enum ReleaseMediaAuthority {
    OpenPgp {
        media_url: String,
        media_sha256: String,
        checksums_url: String,
        signature_url: String,
        signing_fingerprint: String,
        squashfs_path: String,
    },
    HttpsMetadata {
        media_url: String,
        media_sha256: String,
        metadata_url: String,
        squashfs_path: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DistroReleaseRoot {
    pub base_image: String,
    pub fixture: String,
    pub keyring_package: PinnedDebPackage,
    pub identity_package: PinnedDebPackage,
    pub expected_os_release: ExpectedOsRelease,
    pub required_apt_uris: Vec<String>,
    pub signing_roots: Vec<PinnedTargetFile>,
    #[serde(default)]
    pub apt_global_trust: Vec<AptGlobalTrustBinding>,
    pub release_media: ReleaseMediaAuthority,
}

/// Authenticated identity and repository inputs for a first-party container
/// image whose root does not need to be reconstructed from release media.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AuthenticatedTargetRoot {
    pub base_image: String,
    pub package_query: NativePackageQuery,
    pub expected_os_release: ExpectedOsRelease,
    pub identity_packages: Vec<ExpectedNativePackage>,
    pub repository_declarations: Vec<PinnedTargetFile>,
    pub signing_roots: Vec<PinnedTargetFile>,
    pub repository_trust: Vec<NativeRepositoryTrustBinding>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetArtifactRole {
    BaseImage,
    ReleaseMedia,
    KeyringPackage,
    IdentityPackage,
    AptDeclaration,
    NativeRepositoryDeclaration,
    SigningRoot,
    ConaryExecutable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetArtifactDigestSource {
    PinnedReleaseAuthority,
    PinnedBuildInput,
    RunningTargetBytes,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TargetArtifactIdentity {
    pub role: TargetArtifactRole,
    pub digest_source: TargetArtifactDigestSource,
    pub identity: String,
    pub sha256: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub fingerprints: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TargetPackageIdentity {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetReleaseStage {
    ReleaseIdentity,
    PackageIdentity,
    RepositoryDeclarations,
    ConaryRuntime,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TargetReleaseStageResult {
    pub stage: TargetReleaseStage,
    pub passed: bool,
}

/// Typed evidence captured from the running derivative target before a suite.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TargetReleaseEvidence {
    pub schema_version: u32,
    pub target_profile: String,
    pub release: ExpectedOsRelease,
    pub checkout_commit: String,
    pub checkout_dirty: bool,
    pub conary_version: String,
    pub packages: Vec<TargetPackageIdentity>,
    pub artifacts: Vec<TargetArtifactIdentity>,
    pub stages: Vec<TargetReleaseStageResult>,
}

impl DistroReleaseRoot {
    pub fn validate(&self) -> Result<()> {
        let Some((image_name, image_digest)) = self.base_image.rsplit_once("@sha256:") else {
            bail!("release-root base_image must be digest-pinned");
        };
        if image_name.is_empty() {
            bail!("release-root base_image must name an image before its digest");
        }
        validate_sha256(image_digest, "base_image digest")?;
        validate_token(&self.base_image, "base_image")?;
        validate_slug(&self.fixture, "fixture")?;
        validate_package(&self.keyring_package, "keyring_package")?;
        validate_package(&self.identity_package, "identity_package")?;
        validate_token(&self.expected_os_release.id, "expected_os_release.id")?;
        validate_token(
            &self.expected_os_release.version_id,
            "expected_os_release.version_id",
        )?;
        validate_os_release(&self.expected_os_release)?;
        if self.required_apt_uris.is_empty() {
            bail!("release-root required_apt_uris must not be empty");
        }
        for uri in &self.required_apt_uris {
            validate_http_url(uri, "required_apt_uris")?;
        }
        if self.signing_roots.is_empty() {
            bail!("release-root signing_roots must not be empty");
        }
        for root in &self.signing_roots {
            if !root.path.starts_with('/') || root.path.contains("..") {
                bail!("release-root signing root must be an absolute non-traversing path");
            }
            validate_token(&root.path, "signing_roots.path")?;
            validate_sha256(&root.sha256, "signing_roots.sha256")?;
            if root.fingerprints.is_empty()
                || root.fingerprints.iter().any(|fingerprint| {
                    fingerprint.len() != 40
                        || !fingerprint.chars().all(|ch| ch.is_ascii_hexdigit())
                        || fingerprint != &fingerprint.to_ascii_uppercase()
                })
            {
                bail!("release-root signing roots require exact 40-digit fingerprints");
            }
        }
        let mut bound_entries = BTreeSet::new();
        for binding in &self.apt_global_trust {
            if !binding.document.starts_with("/etc/apt/")
                || binding.document.contains("..")
                || binding.entry_indexes.is_empty()
                || binding.signing_root_paths.is_empty()
            {
                bail!("APT global trust binding requires an exact document, entry, and root");
            }
            validate_token(&binding.document, "apt_global_trust.document")?;
            if binding
                .signing_root_paths
                .iter()
                .any(|path| !self.signing_roots.iter().any(|root| &root.path == path))
            {
                bail!("APT global trust binding refers to an undeclared signing root");
            }
            for entry_index in &binding.entry_indexes {
                if !bound_entries.insert((binding.document.as_str(), *entry_index)) {
                    bail!("APT global trust binding repeats a declaration entry");
                }
            }
        }
        validate_media(&self.release_media)?;
        Ok(())
    }

    pub fn docker_build_args(&self) -> Result<HashMap<String, String>> {
        self.validate()?;
        Ok(HashMap::from([
            ("BASE_IMAGE".into(), self.base_image.clone()),
            ("DERIVATIVE_ROOT".into(), self.fixture.clone()),
            (
                "KEYRING_PACKAGE_NAME".into(),
                self.keyring_package.name.clone(),
            ),
            (
                "KEYRING_PACKAGE_VERSION".into(),
                self.keyring_package.version.clone(),
            ),
            (
                "KEYRING_PACKAGE_URL".into(),
                self.keyring_package.url.clone(),
            ),
            (
                "KEYRING_PACKAGE_SHA256".into(),
                self.keyring_package.sha256.clone(),
            ),
            (
                "IDENTITY_PACKAGE_NAME".into(),
                self.identity_package.name.clone(),
            ),
            (
                "IDENTITY_PACKAGE_VERSION".into(),
                self.identity_package.version.clone(),
            ),
            (
                "IDENTITY_PACKAGE_URL".into(),
                self.identity_package.url.clone(),
            ),
            (
                "IDENTITY_PACKAGE_SHA256".into(),
                self.identity_package.sha256.clone(),
            ),
            ("EXPECTED_OS_ID".into(), self.expected_os_release.id.clone()),
            (
                "EXPECTED_VERSION_ID".into(),
                self.expected_os_release.version_id.clone(),
            ),
            (
                "EXPECTED_VERSION_CODENAME".into(),
                self.expected_os_release
                    .version_codename
                    .clone()
                    .unwrap_or_default(),
            ),
            (
                "EXPECTED_UBUNTU_CODENAME".into(),
                self.expected_os_release
                    .ubuntu_codename
                    .clone()
                    .unwrap_or_default(),
            ),
            ("REQUIRED_APT_URIS".into(), self.required_apt_uris.join(" ")),
            (
                "REQUIRED_SIGNING_ROOTS".into(),
                self.signing_roots
                    .iter()
                    .map(|root| format!("{}={}", root.sha256, root.path))
                    .collect::<Vec<_>>()
                    .join(" "),
            ),
        ]))
    }
}

impl AuthenticatedTargetRoot {
    pub fn validate(&self) -> Result<()> {
        validate_base_image(&self.base_image, "target-root")?;
        validate_os_release(&self.expected_os_release)?;
        if self.identity_packages.is_empty() {
            bail!("target-root identity_packages must not be empty");
        }
        for package in &self.identity_packages {
            validate_token(&package.name, "identity_packages.name")?;
            validate_token(&package.version, "identity_packages.version")?;
        }
        if self.repository_declarations.is_empty() {
            bail!("target-root repository_declarations must not be empty");
        }
        validate_target_files(&self.repository_declarations, false)?;
        if self.signing_roots.is_empty() {
            bail!("target-root signing_roots must not be empty");
        }
        validate_target_files(&self.signing_roots, true)?;
        if self.repository_trust.is_empty() {
            bail!("target-root repository_trust must not be empty");
        }
        let mut repositories = BTreeSet::new();
        for binding in &self.repository_trust {
            let (repository, paths) = match binding {
                NativeRepositoryTrustBinding::Arch {
                    repository,
                    metadata_url,
                    keyring_url,
                    master_fingerprints,
                    packager_key_threshold,
                } => {
                    validate_https_url(metadata_url, "repository_trust.metadata_url")?;
                    validate_https_url(keyring_url, "repository_trust.keyring_url")?;
                    if master_fingerprints.is_empty() || *packager_key_threshold == 0 {
                        bail!("Arch repository trust requires master fingerprints and a threshold");
                    }
                    for fingerprint in master_fingerprints {
                        validate_fingerprint(fingerprint)?;
                    }
                    (repository, None)
                }
                NativeRepositoryTrustBinding::RpmGlobal {
                    repository,
                    signing_root_paths,
                } => (repository, Some(signing_root_paths)),
            };
            validate_token(repository, "repository_trust.repository")?;
            if !repositories.insert(repository) {
                bail!("target-root repository_trust repeats a repository");
            }
            if let Some(paths) = paths
                && (paths.is_empty()
                    || paths
                        .iter()
                        .any(|path| !self.signing_roots.iter().any(|root| &root.path == path)))
            {
                bail!("RPM global trust must refer to declared signing roots");
            }
        }
        Ok(())
    }

    pub fn docker_build_args(&self) -> Result<HashMap<String, String>> {
        self.validate()?;
        Ok(HashMap::from([(
            "BASE_IMAGE".into(),
            self.base_image.clone(),
        )]))
    }
}

fn validate_base_image(value: &str, owner: &str) -> Result<()> {
    let Some((image_name, image_digest)) = value.rsplit_once("@sha256:") else {
        bail!("{owner} base_image must be digest-pinned");
    };
    if image_name.is_empty() {
        bail!("{owner} base_image must name an image before its digest");
    }
    validate_sha256(image_digest, "base_image digest")?;
    validate_token(value, "base_image")
}

fn validate_os_release(release: &ExpectedOsRelease) -> Result<()> {
    validate_token(&release.id, "expected_os_release.id")?;
    validate_token(&release.version_id, "expected_os_release.version_id")?;
    for (field, value) in [
        ("version_codename", &release.version_codename),
        ("ubuntu_codename", &release.ubuntu_codename),
        ("id_like", &release.id_like),
        ("build_id", &release.build_id),
    ] {
        if let Some(value) = value
            && (value.is_empty() || value.chars().any(char::is_control))
        {
            bail!("expected_os_release.{field} must be a nonempty scalar");
        }
    }
    Ok(())
}

fn validate_target_files(files: &[PinnedTargetFile], require_fingerprints: bool) -> Result<()> {
    for file in files {
        if !file.path.starts_with('/') || file.path.contains("..") {
            bail!("target-root files must use absolute non-traversing paths");
        }
        validate_token(&file.path, "target-root file path")?;
        validate_sha256(&file.sha256, "target-root file sha256")?;
        if require_fingerprints && file.fingerprints.is_empty() {
            bail!("target-root signing roots require fingerprints");
        }
        for fingerprint in &file.fingerprints {
            validate_fingerprint(fingerprint)?;
        }
    }
    Ok(())
}

fn validate_fingerprint(fingerprint: &str) -> Result<()> {
    if fingerprint.len() != 40
        || !fingerprint.chars().all(|ch| ch.is_ascii_hexdigit())
        || fingerprint != fingerprint.to_ascii_uppercase()
    {
        bail!("target-root fingerprints must be uppercase 40-digit values");
    }
    Ok(())
}

fn validate_media(media: &ReleaseMediaAuthority) -> Result<()> {
    let (media_url, media_sha256, squashfs_path) = match media {
        ReleaseMediaAuthority::OpenPgp {
            media_url,
            media_sha256,
            checksums_url,
            signature_url,
            signing_fingerprint,
            squashfs_path,
        } => {
            validate_https_url(checksums_url, "release_media.checksums_url")?;
            validate_https_url(signature_url, "release_media.signature_url")?;
            if signing_fingerprint.len() != 40
                || !signing_fingerprint.chars().all(|ch| ch.is_ascii_hexdigit())
                || signing_fingerprint != &signing_fingerprint.to_ascii_uppercase()
            {
                bail!(
                    "release_media.signing_fingerprint must be an uppercase 40-digit OpenPGP fingerprint"
                );
            }
            (media_url, media_sha256, squashfs_path)
        }
        ReleaseMediaAuthority::HttpsMetadata {
            media_url,
            media_sha256,
            metadata_url,
            squashfs_path,
        } => {
            validate_https_url(metadata_url, "release_media.metadata_url")?;
            (media_url, media_sha256, squashfs_path)
        }
    };
    validate_https_url(media_url, "release_media.media_url")?;
    validate_sha256(media_sha256, "release_media.media_sha256")?;
    if squashfs_path.starts_with('/') || squashfs_path.contains("..") {
        bail!("release_media.squashfs_path must be a relative non-traversing path");
    }
    validate_token(squashfs_path, "release_media.squashfs_path")?;
    Ok(())
}

fn validate_package(package: &PinnedDebPackage, field: &str) -> Result<()> {
    validate_token(&package.name, &format!("{field}.name"))?;
    validate_token(&package.version, &format!("{field}.version"))?;
    validate_http_url(&package.url, &format!("{field}.url"))?;
    validate_sha256(&package.sha256, &format!("{field}.sha256"))
}

fn validate_sha256(value: &str, field: &str) -> Result<()> {
    if value.len() != 64 || !value.chars().all(|ch| ch.is_ascii_hexdigit()) {
        bail!("{field} must be a 64-digit SHA-256 digest");
    }
    Ok(())
}

fn validate_http_url(value: &str, field: &str) -> Result<()> {
    if !value.starts_with("https://") && !value.starts_with("http://") {
        bail!("{field} must use an explicit HTTP or HTTPS URL");
    }
    validate_token(value, field)
}

fn validate_https_url(value: &str, field: &str) -> Result<()> {
    if !value.starts_with("https://") {
        bail!("{field} must use HTTPS");
    }
    validate_token(value, field)
}

fn validate_slug(value: &str, field: &str) -> Result<()> {
    if value.is_empty()
        || !value
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || matches!(ch, '-' | '.'))
    {
        bail!("{field} must contain only lowercase ASCII letters, digits, dots, and hyphens");
    }
    Ok(())
}

fn validate_token(value: &str, field: &str) -> Result<()> {
    if value.is_empty()
        || value.chars().any(|ch| {
            ch.is_ascii_whitespace()
                || matches!(
                    ch,
                    '\'' | '"' | '`' | '$' | '\\' | ';' | '&' | '|' | '<' | '>' | '(' | ')'
                )
        })
    {
        bail!("{field} contains an empty or shell-unsafe token");
    }
    Ok(())
}
