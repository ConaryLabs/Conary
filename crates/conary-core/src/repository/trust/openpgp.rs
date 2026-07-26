// conary-core/src/repository/trust/openpgp.rs

//! Pinned OpenPGP certificate preparation and verification.

use super::{
    ArchKeyringFormat, ArchKeyringTrust, OpenPgpTrustRoot, RepositoryTrustPolicy,
    RpmMetadataAuthority, TrustRole,
};
use crate::error::{Error, Result};
use crate::repository::client::RepositoryClient;
use openpgp::cert::CertParser;
use openpgp::cert::amalgamation::key::PrimaryKey;
use openpgp::parse::Parse;
use openpgp::parse::stream::{
    DetachedVerifierBuilder, MessageLayer, MessageStructure, VerificationHelper, VerifierBuilder,
};
use openpgp::policy::StandardPolicy;
use openpgp::serialize::Serialize;
use sequoia_openpgp as openpgp;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use url::Url;

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
            prepare_arch_keyring(repository_name, keyring_dir, keyring).await?;
        }
        for (role, roots) in policy_roles(policy) {
            for root in roots {
                prepare_root(repository_name, keyring_dir, role, root).await?;
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
            prepared.load_arch_authenticated_certificates(keyring)?;
        }
        for (role, roots) in policy_roles(policy) {
            prepared.load_role_certificates(role, roots)?;
        }
        Ok(prepared)
    }

    pub fn policy(&self) -> &RepositoryTrustPolicy {
        &self.policy
    }

    pub fn verify_detached(&self, role: TrustRole, data: &[u8], signature: &[u8]) -> Result<()> {
        let certificates = self.certificates_for_role(role)?;
        verify_detached_with_certificates(data, signature, certificates).map_err(|error| {
            Error::GpgVerificationFailed(format!(
                "{} signature for repository '{}' failed: {error}",
                role.as_str(),
                self.repository_name
            ))
        })
    }

    pub fn verify_inline(&self, role: TrustRole, signed_data: &[u8]) -> Result<Vec<u8>> {
        let certificates = self.certificates_for_role(role)?;
        verify_inline_with_certificates(signed_data, certificates).map_err(|error| {
            Error::GpgVerificationFailed(format!(
                "{} inline signature for repository '{}' failed: {error}",
                role.as_str(),
                self.repository_name
            ))
        })
    }

    pub fn armored_keyring(&self, role: TrustRole) -> Result<Vec<u8>> {
        let certificates = self.certificates_for_role(role)?;
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

    fn certificates_for_role(&self, role: TrustRole) -> Result<Vec<openpgp::Cert>> {
        match (&self.policy, role) {
            (
                RepositoryTrustPolicy::Arch { keyring, .. },
                TrustRole::ArchDatabase | TrustRole::ArchPackage,
            ) => self.load_arch_authenticated_certificates(keyring),
            _ => {
                let roots = self.policy.roots_for_role(role)?;
                self.load_role_certificates(role, roots)
            }
        }
    }

    fn load_arch_authenticated_certificates(
        &self,
        keyring: &ArchKeyringTrust,
    ) -> Result<Vec<openpgp::Cert>> {
        let path = arch_keyring_path(&self.repository_name, &self.keyring_dir)?;
        let bytes = fs::read(&path).map_err(|error| {
            Error::GpgVerificationFailed(format!(
                "prepared Arch keyring for repository '{}' is missing at {}: {error}",
                self.repository_name,
                path.display()
            ))
        })?;
        authenticate_arch_certificates(parse_arch_keyring(&bytes, keyring)?, keyring)
    }

    fn load_role_certificates(
        &self,
        role: TrustRole,
        roots: &[OpenPgpTrustRoot],
    ) -> Result<Vec<openpgp::Cert>> {
        roots
            .iter()
            .map(|root| {
                let path = certificate_path(
                    &self.repository_name,
                    &self.keyring_dir,
                    role,
                    &root.fingerprint,
                )?;
                let bytes = fs::read(&path).map_err(|error| {
                    Error::GpgVerificationFailed(format!(
                        "prepared {} certificate '{}' for repository '{}' is missing at {}: \
                         {error}",
                        role.as_str(),
                        root.fingerprint,
                        self.repository_name,
                        path.display()
                    ))
                })?;
                let certificate = openpgp::Cert::from_bytes(&bytes).map_err(|error| {
                    Error::GpgVerificationFailed(format!(
                        "prepared {} certificate '{}' is malformed: {error}",
                        role.as_str(),
                        root.fingerprint
                    ))
                })?;
                if certificate.fingerprint().to_hex() != root.fingerprint {
                    return Err(Error::GpgVerificationFailed(format!(
                        "prepared {} certificate fingerprint changed: expected '{}', got '{}'",
                        role.as_str(),
                        root.fingerprint,
                        certificate.fingerprint()
                    )));
                }
                Ok(certificate)
            })
            .collect()
    }
}

async fn prepare_root(
    repository_name: &str,
    keyring_dir: &Path,
    role: TrustRole,
    root: &OpenPgpTrustRoot,
) -> Result<()> {
    root.validate()?;
    let bytes = fetch_key_source(&root.url).await?;
    let mut matches = Vec::new();
    for parsed in CertParser::from_bytes(&bytes)
        .map_err(|error| Error::ParseError(format!("failed to parse OpenPGP keyring: {error}")))?
    {
        let certificate = parsed.map_err(|error| {
            Error::ParseError(format!("malformed OpenPGP certificate: {error}"))
        })?;
        if certificate.fingerprint().to_hex() == root.fingerprint {
            matches.push(certificate);
        }
    }
    if matches.len() != 1 {
        return Err(Error::GpgVerificationFailed(format!(
            "{} trust source '{}' for repository '{}' contains {} certificates with pinned \
             fingerprint '{}'; expected exactly one",
            role.as_str(),
            root.url,
            repository_name,
            matches.len(),
            root.fingerprint
        )));
    }

    let path = certificate_path(repository_name, keyring_dir, role, &root.fingerprint)?;
    let parent = path.parent().ok_or_else(|| {
        Error::IoError(format!(
            "trusted certificate path {} has no parent",
            path.display()
        ))
    })?;
    fs::create_dir_all(parent).map_err(|error| {
        Error::IoError(format!(
            "failed to create repository trust directory {}: {error}",
            parent.display()
        ))
    })?;
    let mut serialized = Vec::new();
    matches
        .pop()
        .expect("exact match count was checked")
        .serialize(&mut serialized)
        .map_err(|error| {
            Error::IoError(format!("failed to serialize pinned certificate: {error}"))
        })?;
    fs::write(&path, serialized).map_err(|error| {
        Error::IoError(format!(
            "failed to write pinned certificate {}: {error}",
            path.display()
        ))
    })
}

async fn prepare_arch_keyring(
    repository_name: &str,
    keyring_dir: &Path,
    keyring: &ArchKeyringTrust,
) -> Result<()> {
    keyring.validate()?;
    let source = fetch_key_source(&keyring.url).await?;
    let bytes = extract_arch_keyring_source(&source, keyring.format)?;
    authenticate_arch_certificates(parse_arch_keyring(&bytes, keyring)?, keyring)?;

    let path = arch_keyring_path(repository_name, keyring_dir)?;
    let parent = path.parent().ok_or_else(|| {
        Error::IoError(format!(
            "trusted Arch keyring path {} has no parent",
            path.display()
        ))
    })?;
    fs::create_dir_all(parent).map_err(|error| {
        Error::IoError(format!(
            "failed to create repository trust directory {}: {error}",
            parent.display()
        ))
    })?;
    fs::write(&path, bytes).map_err(|error| {
        Error::IoError(format!(
            "failed to write prepared Arch keyring {}: {error}",
            path.display()
        ))
    })
}

fn extract_arch_keyring_source(source: &[u8], format: ArchKeyringFormat) -> Result<Vec<u8>> {
    match format {
        ArchKeyringFormat::OpenPgp => Ok(source.to_vec()),
        ArchKeyringFormat::AlpmPackageZstd => {
            const MAX_KEYRING_PACKAGE_OUTPUT: u64 = 64 * 1024 * 1024;
            const MAX_KEYRING_BYTES: u64 = 32 * 1024 * 1024;
            const MAX_KEYRING_PACKAGE_ENTRIES: usize = 4096;
            const KEYRING_MEMBER: &str = "usr/share/pacman/keyrings/archlinux.gpg";

            let decoder = crate::compression::create_decoder_limited(
                Cursor::new(source),
                crate::compression::CompressionFormat::Zstd,
                MAX_KEYRING_PACKAGE_OUTPUT,
            )
            .map_err(|error| {
                Error::ParseError(format!(
                    "failed to decode Arch keyring ALPM package: {error}"
                ))
            })?;
            let mut archive = tar::Archive::new(decoder);
            let mut keyring = None;
            for (index, entry) in archive
                .entries()
                .map_err(|error| {
                    Error::ParseError(format!("failed to read Arch keyring ALPM package: {error}"))
                })?
                .enumerate()
            {
                if index >= MAX_KEYRING_PACKAGE_ENTRIES {
                    return Err(Error::ParseError(format!(
                        "Arch keyring ALPM package exceeds {MAX_KEYRING_PACKAGE_ENTRIES} entries"
                    )));
                }
                let entry = entry.map_err(|error| {
                    Error::ParseError(format!("failed to read Arch keyring ALPM member: {error}"))
                })?;
                let path = entry.path().map_err(|error| {
                    Error::ParseError(format!("invalid Arch keyring ALPM member path: {error}"))
                })?;
                if path.as_ref() != Path::new(KEYRING_MEMBER) {
                    continue;
                }
                if keyring.is_some() {
                    return Err(Error::ParseError(format!(
                        "Arch keyring ALPM package repeats exact member '{KEYRING_MEMBER}'"
                    )));
                }
                if !entry.header().entry_type().is_file() {
                    return Err(Error::ParseError(format!(
                        "Arch keyring ALPM member '{KEYRING_MEMBER}' is not a regular file"
                    )));
                }
                let mut bytes = Vec::new();
                entry
                    .take(MAX_KEYRING_BYTES + 1)
                    .read_to_end(&mut bytes)
                    .map_err(|error| {
                        Error::ParseError(format!(
                            "failed to read Arch keyring ALPM member: {error}"
                        ))
                    })?;
                if bytes.len() as u64 > MAX_KEYRING_BYTES {
                    return Err(Error::ParseError(format!(
                        "Arch keyring ALPM member exceeds {MAX_KEYRING_BYTES} bytes"
                    )));
                }
                keyring = Some(bytes);
            }
            keyring.ok_or_else(|| {
                Error::ParseError(format!(
                    "Arch keyring ALPM package has no exact '{KEYRING_MEMBER}' member"
                ))
            })
        }
    }
}

fn parse_arch_keyring(bytes: &[u8], keyring: &ArchKeyringTrust) -> Result<Vec<openpgp::Cert>> {
    let mut certificates = BTreeMap::<String, openpgp::Cert>::new();
    for parsed in CertParser::from_bytes(bytes)
        .map_err(|error| Error::ParseError(format!("failed to parse Arch keyring: {error}")))?
    {
        let certificate = parsed
            .map_err(|error| Error::ParseError(format!("malformed Arch keyring: {error}")))?;
        let fingerprint = certificate.fingerprint().to_hex();
        match certificates.remove(&fingerprint) {
            Some(existing) => {
                let merged = existing.merge_public(certificate).map_err(|error| {
                    Error::ParseError(format!(
                        "failed to merge Arch keyring certificate '{fingerprint}': {error}"
                    ))
                })?;
                certificates.insert(fingerprint, merged);
            }
            None => {
                certificates.insert(fingerprint, certificate);
            }
        }
    }
    if certificates.is_empty() {
        return Err(Error::GpgVerificationFailed(
            "Arch keyring contains no OpenPGP certificates".to_string(),
        ));
    }
    for master in &keyring.master_fingerprints {
        if !certificates.contains_key(master) {
            return Err(Error::GpgVerificationFailed(format!(
                "Arch keyring '{}' does not contain pinned master certificate '{}'",
                keyring.url, master
            )));
        }
    }
    Ok(certificates.into_values().collect())
}

fn authenticate_arch_certificates(
    certificates: Vec<openpgp::Cert>,
    keyring: &ArchKeyringTrust,
) -> Result<Vec<openpgp::Cert>> {
    let policy = StandardPolicy::new();
    let by_fingerprint = certificates
        .iter()
        .map(|certificate| (certificate.fingerprint().to_hex(), certificate))
        .collect::<BTreeMap<_, _>>();
    let masters = keyring
        .master_fingerprints
        .iter()
        .map(|fingerprint| {
            let certificate = by_fingerprint.get(fingerprint).copied().ok_or_else(|| {
                Error::GpgVerificationFailed(format!(
                    "Arch keyring lost pinned master certificate '{fingerprint}'"
                ))
            })?;
            let usable = certificate
                .keys()
                .with_policy(&policy, None)
                .supported()
                .alive()
                .revoked(false)
                .for_certification()
                .any(|key| key.primary());
            if !usable {
                return Err(Error::GpgVerificationFailed(format!(
                    "Arch master certificate '{fingerprint}' has no live, unrevoked certification \
                     key"
                )));
            }
            Ok(certificate)
        })
        .collect::<Result<Vec<_>>>()?;
    let master_fingerprints = keyring
        .master_fingerprints
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();

    let authenticated_fingerprints = certificates
        .iter()
        .filter(|certificate| {
            let fingerprint = certificate.fingerprint().to_hex();
            master_fingerprints.contains(fingerprint.as_str())
                || certificate.userids().any(|binding| {
                    masters
                        .iter()
                        .filter(|master| {
                            binding
                                .active_certifications_by_key(
                                    &policy,
                                    None,
                                    master.primary_key().key(),
                                )
                                .next()
                                .is_some()
                        })
                        .count()
                        >= keyring.packager_key_threshold
                })
        })
        .map(|certificate| certificate.fingerprint().to_hex())
        .collect::<BTreeSet<_>>();
    drop(masters);
    drop(by_fingerprint);
    let authenticated = certificates
        .into_iter()
        .filter(|certificate| {
            authenticated_fingerprints.contains(&certificate.fingerprint().to_hex())
        })
        .collect::<Vec<_>>();
    if authenticated.is_empty() {
        return Err(Error::GpgVerificationFailed(
            "Arch keyring contains no certificate authenticated by a pinned master".to_string(),
        ));
    }
    Ok(authenticated)
}

async fn fetch_key_source(source: &str) -> Result<Vec<u8>> {
    let url = Url::parse(source)
        .map_err(|error| Error::ConfigError(format!("invalid trust-root URL: {error}")))?;
    match url.scheme() {
        "https" => RepositoryClient::new()?.download_to_bytes(source).await,
        "file" => {
            let path = url.to_file_path().map_err(|_| {
                Error::ConfigError(format!(
                    "file trust-root URL '{source}' is not a local path"
                ))
            })?;
            fs::read(&path).map_err(|error| {
                Error::IoError(format!(
                    "failed to read trust-root certificate {}: {error}",
                    path.display()
                ))
            })
        }
        other => Err(Error::ConfigError(format!(
            "unsupported trust-root URL scheme '{other}'"
        ))),
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
    }
}

fn certificate_path(
    repository_name: &str,
    keyring_dir: &Path,
    role: TrustRole,
    fingerprint: &str,
) -> Result<PathBuf> {
    let repository_name = safe_component(repository_name, "repository name")?;
    let fingerprint = safe_component(fingerprint, "OpenPGP fingerprint")?;
    Ok(keyring_dir
        .join("native")
        .join(repository_name)
        .join(role.as_str())
        .join(format!("{fingerprint}.pgp")))
}

fn arch_keyring_path(repository_name: &str, keyring_dir: &Path) -> Result<PathBuf> {
    let repository_name = safe_component(repository_name, "repository name")?;
    Ok(keyring_dir
        .join("native")
        .join(repository_name)
        .join("arch-keyring.pgp"))
}

fn safe_component<'a>(value: &'a str, label: &str) -> Result<&'a str> {
    if value.is_empty()
        || value == "."
        || value == ".."
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'+' | b'-'))
    {
        return Err(Error::ConfigError(format!(
            "{label} '{value}' is not a safe trust-store path component"
        )));
    }
    Ok(value)
}

#[derive(Clone)]
struct PinnedCertificateHelper {
    certificates: Vec<openpgp::Cert>,
}

impl VerificationHelper for PinnedCertificateHelper {
    fn get_certs(
        &mut self,
        _identities: &[openpgp::KeyHandle],
    ) -> openpgp::Result<Vec<openpgp::Cert>> {
        Ok(self.certificates.clone())
    }

    fn check(&mut self, structure: MessageStructure<'_>) -> openpgp::Result<()> {
        let mut valid = 0usize;
        for layer in structure {
            let MessageLayer::SignatureGroup { results } = layer else {
                return Err(anyhow::anyhow!(
                    "unexpected encrypted or compressed layer in repository signature"
                ));
            };
            for result in results {
                result.map_err(|error| anyhow::anyhow!("{error}"))?;
                valid += 1;
            }
        }
        if valid == 0 {
            return Err(anyhow::anyhow!(
                "repository object has no valid signature from a pinned certificate"
            ));
        }
        Ok(())
    }
}

fn verify_detached_with_certificates(
    data: &[u8],
    signature: &[u8],
    certificates: Vec<openpgp::Cert>,
) -> anyhow::Result<()> {
    let policy = StandardPolicy::new();
    let helper = PinnedCertificateHelper { certificates };
    let mut verifier =
        DetachedVerifierBuilder::from_bytes(signature)?.with_policy(&policy, None, helper)?;
    verifier.verify_bytes(data)?;
    Ok(())
}

fn verify_inline_with_certificates(
    signed_data: &[u8],
    certificates: Vec<openpgp::Cert>,
) -> anyhow::Result<Vec<u8>> {
    let policy = StandardPolicy::new();
    let helper = PinnedCertificateHelper { certificates };
    let mut verifier =
        VerifierBuilder::from_bytes(signed_data)?.with_policy(&policy, None, helper)?;
    let mut payload = Vec::new();
    verifier.read_to_end(&mut payload)?;
    Ok(payload)
}

#[cfg(test)]
mod tests {
    use super::*;
    use openpgp::cert::prelude::CertBuilder;
    use openpgp::packet::signature::SignatureBuilder;
    use openpgp::serialize::stream::{Message, Signer};
    use openpgp::types::SignatureType;
    use std::io::Write;

    fn arch_trust(master_fingerprint: String) -> ArchKeyringTrust {
        ArchKeyringTrust {
            url: "https://keys.example.test/archlinux.gpg".to_string(),
            format: ArchKeyringFormat::OpenPgp,
            master_fingerprints: vec![master_fingerprint],
            packager_key_threshold: 1,
        }
    }

    fn alpm_keyring_package(members: &[(&str, &[u8])]) -> Vec<u8> {
        let mut archive = tar::Builder::new(Vec::new());
        for (path, content) in members {
            let mut header = tar::Header::new_gnu();
            header.set_mode(0o644);
            header.set_size(content.len() as u64);
            header.set_cksum();
            archive.append_data(&mut header, path, *content).unwrap();
        }
        let tar = archive.into_inner().unwrap();
        zstd::encode_all(Cursor::new(tar), 0).unwrap()
    }

    fn detached_signature(certificate: &openpgp::Cert, data: &[u8]) -> Vec<u8> {
        let policy = StandardPolicy::new();
        let keypair = certificate
            .keys()
            .unencrypted_secret()
            .with_policy(&policy, None)
            .supported()
            .alive()
            .revoked(false)
            .for_signing()
            .next()
            .unwrap()
            .key()
            .clone()
            .into_keypair()
            .unwrap();
        let mut signature = Vec::new();
        let message = Message::new(&mut signature);
        let mut signer = Signer::new(message, keypair)
            .unwrap()
            .detached()
            .build()
            .unwrap();
        signer.write_all(data).unwrap();
        signer.finalize().unwrap();
        signature
    }

    fn certify_packager(master: &openpgp::Cert, packager: openpgp::Cert) -> openpgp::Cert {
        let policy = StandardPolicy::new();
        let mut master_signer = master
            .keys()
            .unencrypted_secret()
            .with_policy(&policy, None)
            .supported()
            .alive()
            .revoked(false)
            .for_certification()
            .next()
            .unwrap()
            .key()
            .clone()
            .into_keypair()
            .unwrap();
        let packager_userid = packager.userids().next().unwrap().userid().clone();
        let certification = SignatureBuilder::new(SignatureType::GenericCertification)
            .sign_userid_binding(
                &mut master_signer,
                packager.primary_key().key(),
                &packager_userid,
            )
            .unwrap();
        packager.insert_packets(certification).unwrap().0
    }

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
    fn unsafe_repository_name_cannot_escape_trust_store() {
        let error = certificate_path(
            "../repo",
            Path::new("/tmp/keyring"),
            TrustRole::DebianRelease,
            &"A".repeat(40),
        )
        .unwrap_err();
        assert!(error.to_string().contains("safe"));
    }

    #[test]
    fn detached_signature_tampering_is_fatal() {
        let (certificate, _) = CertBuilder::new()
            .add_userid("Repository Signer")
            .add_signing_subkey()
            .generate()
            .unwrap();
        let signature = detached_signature(&certificate, b"trusted metadata");

        verify_detached_with_certificates(
            b"trusted metadata",
            &signature,
            vec![certificate.clone()],
        )
        .unwrap();
        assert!(
            verify_detached_with_certificates(b"tampered metadata", &signature, vec![certificate],)
                .is_err()
        );
    }

    #[test]
    fn arch_alpm_keyring_source_requires_the_exact_member() {
        let content = b"openpgp keyring";
        let package = alpm_keyring_package(&[
            (".PKGINFO", b"pkgname = archlinux-keyring"),
            ("usr/share/pacman/keyrings/archlinux.gpg", content),
        ]);
        assert_eq!(
            extract_arch_keyring_source(&package, ArchKeyringFormat::AlpmPackageZstd).unwrap(),
            content
        );

        let missing =
            alpm_keyring_package(&[("usr/share/pacman/keyrings/lookalike-archlinux.gpg", content)]);
        let error =
            extract_arch_keyring_source(&missing, ArchKeyringFormat::AlpmPackageZstd).unwrap_err();
        assert!(error.to_string().contains("no exact"));
    }

    #[test]
    fn arch_alpm_keyring_source_rejects_duplicate_authority_members() {
        let package = alpm_keyring_package(&[
            ("usr/share/pacman/keyrings/archlinux.gpg", b"first"),
            ("usr/share/pacman/keyrings/archlinux.gpg", b"second"),
        ]);
        let error =
            extract_arch_keyring_source(&package, ArchKeyringFormat::AlpmPackageZstd).unwrap_err();
        assert!(error.to_string().contains("repeats exact member"));
    }

    #[test]
    fn arch_keyring_requires_every_pinned_master() {
        let (certificate, _) = CertBuilder::new()
            .add_userid("Other Key")
            .generate()
            .unwrap();
        let mut bytes = Vec::new();
        certificate.serialize(&mut bytes).unwrap();
        let error = parse_arch_keyring(&bytes, &arch_trust("A".repeat(40))).unwrap_err();
        assert!(error.to_string().contains("does not contain pinned master"));
    }

    #[test]
    fn arch_packager_key_requires_direct_master_certification() {
        let (master, _) = CertBuilder::new()
            .add_userid("Arch Master")
            .generate()
            .unwrap();
        let (packager, _) = CertBuilder::new()
            .add_userid("Arch Packager")
            .add_signing_subkey()
            .generate()
            .unwrap();
        let (uncertified, _) = CertBuilder::new()
            .add_userid("Uncertified Packager")
            .add_signing_subkey()
            .generate()
            .unwrap();

        let packager = certify_packager(&master, packager);
        let trust = arch_trust(master.fingerprint().to_hex());
        let expected = BTreeSet::from([
            master.fingerprint().to_hex(),
            packager.fingerprint().to_hex(),
        ]);

        let authenticated =
            authenticate_arch_certificates(vec![master, packager, uncertified], &trust).unwrap();
        let actual = authenticated
            .iter()
            .map(|certificate| certificate.fingerprint().to_hex())
            .collect::<BTreeSet<_>>();
        assert_eq!(actual, expected);
    }

    #[test]
    fn arch_packager_key_must_meet_the_exact_master_threshold() {
        let (master_a, _) = CertBuilder::new()
            .add_userid("Arch Master A")
            .generate()
            .unwrap();
        let (master_b, _) = CertBuilder::new()
            .add_userid("Arch Master B")
            .generate()
            .unwrap();
        let (packager, _) = CertBuilder::new()
            .add_userid("Arch Packager")
            .add_signing_subkey()
            .generate()
            .unwrap();
        let packager = certify_packager(&master_a, packager);
        let packager_fingerprint = packager.fingerprint().to_hex();
        let trust = ArchKeyringTrust {
            url: "https://keys.example.test/archlinux.gpg".to_string(),
            format: ArchKeyringFormat::OpenPgp,
            master_fingerprints: vec![
                master_a.fingerprint().to_hex(),
                master_b.fingerprint().to_hex(),
            ],
            packager_key_threshold: 2,
        };

        let authenticated =
            authenticate_arch_certificates(vec![master_a, master_b, packager], &trust).unwrap();
        assert!(
            !authenticated
                .iter()
                .any(|certificate| certificate.fingerprint().to_hex() == packager_fingerprint)
        );
    }
}
