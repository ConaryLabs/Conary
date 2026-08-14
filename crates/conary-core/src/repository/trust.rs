// conary-core/src/repository/trust.rs

//! Typed trust contracts for native package repositories.
//!
//! Repository format selects the metadata grammar.  This module separately
//! records the exact authority chain that must authenticate that grammar.
//! The policy is persisted as a tagged enum so an RPM repository cannot be
//! accidentally evaluated with Debian or ALPM signature semantics.

use crate::error::{Error, Result};
use crate::repository::RepositoryFormat;
use serde::{Deserialize, Serialize};
use url::Url;

pub mod openpgp;

const MAX_EMBEDDED_OPENPGP_BYTES: usize = 4 * 1024 * 1024;
const MAX_EMBEDDED_OPENPGP_BASE64: usize = MAX_EMBEDDED_OPENPGP_BYTES.div_ceil(3) * 4;

pub(crate) fn decode_embedded_openpgp_source(source: &str) -> Option<Result<Vec<u8>>> {
    let encoded = source.strip_prefix("data:application/pgp-keys;base64,")?;
    Some((|| {
        if encoded.is_empty() || encoded.len() > MAX_EMBEDDED_OPENPGP_BASE64 {
            return Err(Error::ConfigError(format!(
                "embedded OpenPGP trust root must encode 1 to {MAX_EMBEDDED_OPENPGP_BYTES} bytes"
            )));
        }
        use base64::Engine;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .map_err(|error| {
                Error::ConfigError(format!(
                    "embedded OpenPGP trust root is invalid base64: {error}"
                ))
            })?;
        if bytes.is_empty() || bytes.len() > MAX_EMBEDDED_OPENPGP_BYTES {
            return Err(Error::ConfigError(format!(
                "embedded OpenPGP trust root must contain 1 to {MAX_EMBEDDED_OPENPGP_BYTES} bytes"
            )));
        }
        Ok(bytes)
    })())
}

/// One explicitly pinned OpenPGP certificate source.
///
/// The downloaded object may be an OpenPGP keyring, but only the certificate
/// whose full fingerprint matches `fingerprint` is admitted to the repository
/// trust store.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenPgpTrustRoot {
    pub url: String,
    pub fingerprint: String,
}

impl OpenPgpTrustRoot {
    pub fn new(url: String, fingerprint: String) -> Result<Self> {
        let root = Self { url, fingerprint };
        root.validate()?;
        Ok(root)
    }

    pub fn validate(&self) -> Result<()> {
        if let Some(bytes) = decode_embedded_openpgp_source(&self.url) {
            bytes?;
            let fingerprint = self.normalized_fingerprint()?;
            if fingerprint != self.fingerprint {
                return Err(Error::ConfigError(format!(
                    "OpenPGP fingerprint '{}' must be uppercase hexadecimal without separators",
                    self.fingerprint
                )));
            }
            return Ok(());
        }
        let parsed = Url::parse(&self.url).map_err(|error| {
            Error::ConfigError(format!(
                "OpenPGP trust-root URL '{}' is invalid: {error}",
                self.url
            ))
        })?;
        if !matches!(parsed.scheme(), "https" | "file") {
            return Err(Error::ConfigError(format!(
                "OpenPGP trust-root URL '{}' must use https://, file://, or an exact embedded application/pgp-keys data URL",
                self.url
            )));
        }
        if parsed.scheme() == "https" && parsed.host_str().is_none() {
            return Err(Error::ConfigError(format!(
                "OpenPGP trust-root URL '{}' has no host",
                self.url
            )));
        }
        if parsed.username() != "" || parsed.password().is_some() || parsed.fragment().is_some() {
            return Err(Error::ConfigError(format!(
                "OpenPGP trust-root URL '{}' must not contain credentials or a fragment",
                self.url
            )));
        }

        let fingerprint = self.normalized_fingerprint()?;
        if fingerprint != self.fingerprint {
            return Err(Error::ConfigError(format!(
                "OpenPGP fingerprint '{}' must be uppercase hexadecimal without separators",
                self.fingerprint
            )));
        }
        Ok(())
    }

    pub fn normalized_fingerprint(&self) -> Result<String> {
        let fingerprint = self
            .fingerprint
            .chars()
            .filter(|character| !character.is_ascii_whitespace())
            .collect::<String>()
            .to_ascii_uppercase();
        if !matches!(fingerprint.len(), 40 | 64)
            || !fingerprint.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(Error::ConfigError(format!(
                "OpenPGP fingerprint '{}' must contain exactly 40 or 64 hexadecimal digits",
                self.fingerprint
            )));
        }
        Ok(fingerprint)
    }
}

/// Whether ALPM requires an object signature or verifies it only when present.
///
/// `Never` is intentionally not representable.  An optional signature still
/// fails when present but invalid or signed by an unpinned certificate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ArchSignatureRequirement {
    Required,
    Optional,
}

/// The only trust-level policy currently accepted by Conary.
///
/// Pacman's `TrustAll` mode is deliberately absent: a cryptographically valid
/// signature from an untrusted key is not repository authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ArchTrustLevel {
    TrustedOnly,
}

/// Exact ALPM `SigLevel` projection used for database and package artifacts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArchSigLevel {
    pub database: ArchSignatureRequirement,
    pub package: ArchSignatureRequirement,
    pub trust: ArchTrustLevel,
}

impl ArchSigLevel {
    /// Arch's distribution default: packages are required; database
    /// signatures are checked when the repository publishes them.
    pub const fn distribution_default() -> Self {
        Self {
            database: ArchSignatureRequirement::Optional,
            package: ArchSignatureRequirement::Required,
            trust: ArchTrustLevel::TrustedOnly,
        }
    }

    fn validate(&self) -> Result<()> {
        if self.package != ArchSignatureRequirement::Required {
            return Err(Error::ConfigError(
                "Arch package signatures must be required; PackageOptional is not a supported \
                 repository authority policy"
                    .to_string(),
            ));
        }
        Ok(())
    }
}

/// Pacman-compatible keyring authority.
///
/// The keyring supplies packager certificates and certifications. Only paths
/// rooted at these exact master fingerprints satisfy `TrustedOnly`, so a
/// changed keyring cannot introduce an unauthenticated signing key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArchKeyringTrust {
    pub url: String,
    pub format: ArchKeyringFormat,
    pub master_fingerprints: Vec<String>,
    /// Number of distinct pinned master keys that must certify a packager key.
    pub packager_key_threshold: usize,
}

/// Exact transport grammar used by an Arch keyring source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ArchKeyringFormat {
    OpenPgp,
    AlpmPackageZstd,
}

impl ArchKeyringTrust {
    pub fn validate(&self) -> Result<()> {
        validate_https_or_file_url(&self.url, "Arch keyring")?;
        if self.master_fingerprints.is_empty() {
            return Err(Error::ConfigError(
                "Arch TrustedOnly authority requires at least one master fingerprint".to_string(),
            ));
        }
        let mut unique = std::collections::BTreeSet::new();
        for fingerprint in &self.master_fingerprints {
            validate_fingerprint(fingerprint, "Arch master")?;
            if !unique.insert(fingerprint.as_str()) {
                return Err(Error::ConfigError(format!(
                    "Arch keyring repeats master fingerprint '{fingerprint}'"
                )));
            }
        }
        if self.packager_key_threshold == 0
            || self.packager_key_threshold > self.master_fingerprints.len()
        {
            return Err(Error::ConfigError(format!(
                "Arch packager-key certification threshold {} must be between 1 and the {} pinned \
                 master fingerprints",
                self.packager_key_threshold,
                self.master_fingerprints.len()
            )));
        }
        Ok(())
    }
}

/// Exact authority for the rpm-md root object.
///
/// DNF's `repo_gpgcheck=true` uses a detached OpenPGP signature. Fedora's
/// official repositories instead publish an HTTPS metalink containing the
/// exact `repomd.xml` size and hashes. Both authenticate the same rpm-md root;
/// neither is a toggle or an opportunistic fallback.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum RpmMetadataAuthority {
    OpenPgp { keys: Vec<OpenPgpTrustRoot> },
    Metalink { url: String },
}

impl RpmMetadataAuthority {
    fn validate(&self) -> Result<()> {
        match self {
            Self::OpenPgp { keys } => validate_roots(keys, "RPM repository metadata"),
            Self::Metalink { url } => validate_https_or_file_url(url, "RPM metalink"),
        }
    }
}

/// Ecosystem-native repository authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "ecosystem", rename_all = "kebab-case", deny_unknown_fields)]
pub enum RepositoryTrustPolicy {
    /// APT authenticates `InRelease` or `Release` + `Release.gpg`; the signed
    /// Release SHA-256 entry authenticates `Packages`, whose SHA-256 entries
    /// authenticate `.deb` payloads.
    Debian { release_keys: Vec<OpenPgpTrustRoot> },
    /// RPM keeps repository-metadata and package-signature trust roots
    /// distinct, matching `repo_gpgcheck` and `gpgcheck`.
    Rpm {
        metadata: RpmMetadataAuthority,
        package_keys: Vec<OpenPgpTrustRoot>,
    },
    /// ALPM applies one explicit `SigLevel` to repository databases and
    /// packages using its configured keyring.
    Arch {
        keyring: ArchKeyringTrust,
        sig_level: ArchSigLevel,
    },
    /// Current eopkg repositories publish no signature. The exact HTTPS
    /// origin authenticates its SHA-256 sidecar, which authenticates the
    /// compressed index and its per-package SHA-1 identities.
    Eopkg { origin: String },
}

/// Derive the exact HTTPS repository origin from eopkg's native index URL.
pub fn eopkg_origin_from_index_url(index_url: &str) -> Result<String> {
    let mut url = Url::parse(index_url)
        .map_err(|error| Error::ConfigError(format!("eopkg index URL is invalid: {error}")))?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(Error::ConfigError(
            "eopkg index URL must be an absolute HTTPS URL without credentials, query, or fragment"
                .to_string(),
        ));
    }
    let origin_path = url
        .path()
        .strip_suffix("eopkg-index.xml.xz")
        .ok_or_else(|| {
            Error::ConfigError(
                "eopkg index URL must end with exact eopkg-index.xml.xz basename".to_string(),
            )
        })?
        .to_string();
    if !origin_path.ends_with('/') {
        return Err(Error::ConfigError(
            "eopkg index URL must place eopkg-index.xml.xz below a repository directory"
                .to_string(),
        ));
    }
    url.set_path(&origin_path);
    Ok(url.to_string())
}

impl RepositoryTrustPolicy {
    pub fn format(&self) -> RepositoryFormat {
        match self {
            Self::Debian { .. } => RepositoryFormat::Debian,
            Self::Rpm { .. } => RepositoryFormat::Fedora,
            Self::Arch { .. } => RepositoryFormat::Arch,
            Self::Eopkg { .. } => RepositoryFormat::Eopkg,
        }
    }

    pub fn validate(&self) -> Result<()> {
        match self {
            Self::Debian { release_keys } => {
                validate_roots(release_keys, "Debian Release")?;
            }
            Self::Rpm {
                metadata,
                package_keys,
            } => {
                metadata.validate()?;
                validate_roots(package_keys, "RPM package")?;
            }
            Self::Arch { keyring, sig_level } => {
                keyring.validate()?;
                sig_level.validate()?;
            }
            Self::Eopkg { origin } => {
                let url = Url::parse(origin).map_err(|error| {
                    Error::ConfigError(format!("eopkg repository origin is invalid: {error}"))
                })?;
                if url.scheme() != "https" || url.host_str().is_none() || !url.path().ends_with('/')
                {
                    return Err(Error::ConfigError(
                        "eopkg trust origin must be an absolute HTTPS base URL ending in '/'"
                            .to_string(),
                    ));
                }
                let index_url = format!("{origin}eopkg-index.xml.xz");
                if eopkg_origin_from_index_url(&index_url)? != *origin {
                    return Err(Error::ConfigError(
                        "eopkg trust origin is not canonical".to_string(),
                    ));
                }
            }
        }
        Ok(())
    }

    pub fn to_json(&self) -> Result<String> {
        self.validate()?;
        serde_json::to_string(self).map_err(|error| {
            Error::ParseError(format!(
                "failed to serialize repository trust policy: {error}"
            ))
        })
    }

    pub fn from_json(value: &str) -> Result<Self> {
        let policy = serde_json::from_str::<Self>(value).map_err(|error| {
            Error::ParseError(format!(
                "invalid persisted repository trust policy: {error}"
            ))
        })?;
        policy.validate()?;
        Ok(policy)
    }

    pub fn roots_for_role(&self, role: TrustRole) -> Result<&[OpenPgpTrustRoot]> {
        match (self, role) {
            (Self::Debian { release_keys }, TrustRole::DebianRelease) => Ok(release_keys),
            (
                Self::Rpm {
                    metadata: RpmMetadataAuthority::OpenPgp { keys },
                    ..
                },
                TrustRole::RpmMetadata,
            ) => Ok(keys),
            (Self::Rpm { package_keys, .. }, TrustRole::RpmPackage) => Ok(package_keys),
            _ => Err(Error::ConfigError(format!(
                "trust role '{}' does not belong to '{}' policy",
                role.as_str(),
                self.format().as_str()
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TrustRole {
    DebianRelease,
    RpmMetadata,
    RpmPackage,
    ArchDatabase,
    ArchPackage,
}

impl TrustRole {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::DebianRelease => "debian-release",
            Self::RpmMetadata => "rpm-metadata",
            Self::RpmPackage => "rpm-package",
            Self::ArchDatabase => "arch-database",
            Self::ArchPackage => "arch-package",
        }
    }
}

fn validate_roots(roots: &[OpenPgpTrustRoot], label: &str) -> Result<()> {
    if roots.is_empty() {
        return Err(Error::ConfigError(format!(
            "{label} trust requires at least one pinned OpenPGP certificate"
        )));
    }
    let mut fingerprints = std::collections::BTreeSet::new();
    for root in roots {
        root.validate()?;
        if !fingerprints.insert(root.fingerprint.as_str()) {
            return Err(Error::ConfigError(format!(
                "{label} trust repeats fingerprint '{}'",
                root.fingerprint
            )));
        }
    }
    Ok(())
}

fn validate_fingerprint(fingerprint: &str, label: &str) -> Result<()> {
    if !matches!(fingerprint.len(), 40 | 64)
        || !fingerprint.bytes().all(|byte| byte.is_ascii_hexdigit())
        || fingerprint.bytes().any(|byte| byte.is_ascii_lowercase())
    {
        return Err(Error::ConfigError(format!(
            "{label} fingerprint '{fingerprint}' must be exactly 40 or 64 uppercase hexadecimal \
             digits"
        )));
    }
    Ok(())
}

pub(crate) fn validate_https_or_file_url(value: &str, label: &str) -> Result<()> {
    let parsed = Url::parse(value).map_err(|error| {
        Error::ConfigError(format!("{label} URL '{value}' is invalid: {error}"))
    })?;
    if !matches!(parsed.scheme(), "https" | "file") {
        return Err(Error::ConfigError(format!(
            "{label} URL '{value}' must use https:// or file://"
        )));
    }
    if parsed.scheme() == "https" && parsed.host_str().is_none() {
        return Err(Error::ConfigError(format!(
            "{label} URL '{value}' has no host"
        )));
    }
    if parsed.username() != "" || parsed.password().is_some() || parsed.fragment().is_some() {
        return Err(Error::ConfigError(format!(
            "{label} URL '{value}' must not contain credentials or a fragment"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> OpenPgpTrustRoot {
        OpenPgpTrustRoot {
            url: "https://keys.example.test/archive.asc".to_string(),
            fingerprint: "A".repeat(40),
        }
    }

    #[test]
    fn bounded_embedded_openpgp_data_authority_is_exact() {
        use base64::Engine;
        let embedded = OpenPgpTrustRoot {
            url: format!(
                "data:application/pgp-keys;base64,{}",
                base64::engine::general_purpose::STANDARD.encode(b"certificate bytes")
            ),
            fingerprint: "A".repeat(40),
        };
        embedded.validate().unwrap();

        let malformed = OpenPgpTrustRoot {
            url: "data:application/pgp-keys;base64,%%%".to_string(),
            fingerprint: "A".repeat(40),
        };
        assert!(malformed.validate().is_err());

        let oversized = OpenPgpTrustRoot {
            url: format!(
                "data:application/pgp-keys;base64,{}",
                "A".repeat(MAX_EMBEDDED_OPENPGP_BASE64 + 1)
            ),
            fingerprint: "A".repeat(40),
        };
        assert!(oversized.validate().is_err());
    }

    #[test]
    fn old_boolean_bypass_fields_are_rejected() {
        let old = serde_json::json!({
            "ecosystem": "rpm",
            "metadata": {
                "kind": "open-pgp",
                "keys": [root()]
            },
            "package_keys": [root()],
            "gpg_check": false
        });

        let error = RepositoryTrustPolicy::from_json(&old.to_string()).unwrap_err();
        assert!(error.to_string().contains("unknown field"));
    }

    #[test]
    fn rpm_roles_are_distinct() {
        let metadata = root();
        let mut package = root();
        package.fingerprint = "B".repeat(40);
        let policy = RepositoryTrustPolicy::Rpm {
            metadata: RpmMetadataAuthority::OpenPgp {
                keys: vec![metadata.clone()],
            },
            package_keys: vec![package.clone()],
        };

        assert_eq!(
            policy.roots_for_role(TrustRole::RpmMetadata).unwrap(),
            &[metadata]
        );
        assert_eq!(
            policy.roots_for_role(TrustRole::RpmPackage).unwrap(),
            &[package]
        );
    }

    #[test]
    fn arch_rejects_package_optional() {
        let policy = RepositoryTrustPolicy::Arch {
            keyring: ArchKeyringTrust {
                url: "https://keys.example.test/archlinux.gpg".to_string(),
                format: ArchKeyringFormat::OpenPgp,
                master_fingerprints: vec!["A".repeat(40)],
                packager_key_threshold: 1,
            },
            sig_level: ArchSigLevel {
                database: ArchSignatureRequirement::Optional,
                package: ArchSignatureRequirement::Optional,
                trust: ArchTrustLevel::TrustedOnly,
            },
        };

        assert!(policy.validate().is_err());
    }

    #[test]
    fn arch_packager_threshold_must_fit_pinned_master_set() {
        let keyring = ArchKeyringTrust {
            url: "https://keys.example.test/archlinux.gpg".to_string(),
            format: ArchKeyringFormat::OpenPgp,
            master_fingerprints: vec!["A".repeat(40), "B".repeat(40)],
            packager_key_threshold: 3,
        };

        let error = keyring.validate().unwrap_err();
        assert!(error.to_string().contains("between 1 and the 2"));
    }

    #[test]
    fn fingerprints_are_exact_not_fuzzy() {
        let root = OpenPgpTrustRoot {
            url: "https://keys.example.test/archive.asc".to_string(),
            fingerprint: "AA BB".repeat(10),
        };
        assert!(root.validate().is_err());
    }
}
