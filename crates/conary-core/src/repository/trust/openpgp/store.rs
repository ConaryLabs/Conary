// conary-core/src/repository/trust/openpgp/store.rs

//! Trust-store layout, key-source transport, and pinned single-certificate
//! roots.
//!
//! This module owns *where* authenticated key material lives on disk and
//! *how* a configured trust source is fetched.  It deliberately owns no
//! signature semantics: Debian/RPM pinned-certificate verification lives in
//! [`super::pinned`] and Arch keyring authority lives in [`super::arch`].

use crate::error::{Error, Result};
use crate::repository::client::RepositoryClient;
use openpgp::cert::CertParser;
use openpgp::parse::Parse;
use openpgp::serialize::Serialize;
use sequoia_openpgp as openpgp;
use std::fs;
use std::path::{Path, PathBuf};
use url::Url;

use super::super::{OpenPgpTrustRoot, TrustRole};

/// Downloads a configured trust source.
///
/// Only `https://`, `file://`, and the bounded exact embedded-key data form are
/// representable; anything else is a typed configuration error.
pub(super) async fn fetch_key_source(source: &str) -> Result<Vec<u8>> {
    if let Some(bytes) = super::super::decode_embedded_openpgp_source(source) {
        return bytes;
    }
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

/// Fetches a pinned trust root and writes the single matching certificate
/// into the repository trust store.
pub(super) async fn prepare_root(
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
    let mut serialized = Vec::new();
    matches
        .pop()
        .expect("exact match count was checked")
        .serialize(&mut serialized)
        .map_err(|error| {
            Error::IoError(format!("failed to serialize pinned certificate: {error}"))
        })?;
    write_trust_object(&path, &serialized)
}

/// Loads the prepared certificate for every pinned root of one role.
pub(super) fn load_role_certificates(
    repository_name: &str,
    keyring_dir: &Path,
    role: TrustRole,
    roots: &[OpenPgpTrustRoot],
) -> Result<Vec<openpgp::Cert>> {
    roots
        .iter()
        .map(|root| {
            let path = certificate_path(repository_name, keyring_dir, role, &root.fingerprint)?;
            let bytes = fs::read(&path).map_err(|error| {
                Error::GpgVerificationFailed(format!(
                    "prepared {} certificate '{}' for repository '{}' is missing at {}: {error}",
                    role.as_str(),
                    root.fingerprint,
                    repository_name,
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

/// Writes one trust-store object, creating the owning directory first.
pub(super) fn write_trust_object(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().ok_or_else(|| {
        Error::IoError(format!("trust-store path {} has no parent", path.display()))
    })?;
    fs::create_dir_all(parent).map_err(|error| {
        Error::IoError(format!(
            "failed to create repository trust directory {}: {error}",
            parent.display()
        ))
    })?;
    fs::write(path, bytes).map_err(|error| {
        Error::IoError(format!(
            "failed to write trust-store object {}: {error}",
            path.display()
        ))
    })
}

/// Reads one prepared trust-store object, failing closed when it is absent.
pub(super) fn read_trust_object(
    path: &Path,
    label: &str,
    repository_name: &str,
) -> Result<Vec<u8>> {
    fs::read(path).map_err(|error| {
        Error::GpgVerificationFailed(format!(
            "prepared {label} for repository '{repository_name}' is missing at {}: {error}",
            path.display()
        ))
    })
}

pub(super) fn certificate_path(
    repository_name: &str,
    keyring_dir: &Path,
    role: TrustRole,
    fingerprint: &str,
) -> Result<PathBuf> {
    Ok(repository_trust_dir(repository_name, keyring_dir)?
        .join(role.as_str())
        .join(format!(
            "{}.pgp",
            safe_component(fingerprint, "OpenPGP fingerprint")?
        )))
}

/// Directory holding the authenticated Arch keyring snapshot.
///
/// Pacman's keyring authority is not a single object: it is the certificate
/// material plus the companion revoked/disabled list.  Both are persisted
/// side by side so a snapshot cannot be half-applied.
pub(super) fn arch_snapshot_dir(repository_name: &str, keyring_dir: &Path) -> Result<PathBuf> {
    Ok(repository_trust_dir(repository_name, keyring_dir)?.join("arch"))
}

fn repository_trust_dir(repository_name: &str, keyring_dir: &Path) -> Result<PathBuf> {
    Ok(keyring_dir
        .join("native")
        .join(safe_component(repository_name, "repository name")?))
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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn unsafe_repository_name_cannot_escape_the_arch_snapshot_dir() {
        let error = arch_snapshot_dir("..", Path::new("/tmp/keyring")).unwrap_err();
        assert!(error.to_string().contains("safe"));
    }

    #[test]
    fn missing_trust_object_is_fatal() {
        let error = read_trust_object(
            Path::new("/nonexistent/trust/object"),
            "Arch keyring",
            "arch-core",
        )
        .unwrap_err();
        assert!(error.to_string().contains("is missing at"));
    }
}
