// conary-core/src/repository/declarations/takeover/trust_policy.rs

//! Exact projection from discovered trust evidence into persisted policy.

use super::*;
use base64::Engine;
use openpgp::cert::CertParser;
use openpgp::parse::Parse;
use sequoia_openpgp as openpgp;
use std::collections::BTreeSet;

pub(super) fn expected_trust_policy(
    selected_root: &Path,
    declarations: &DiscoveredRepositoryDeclarations,
    plan: &NativeRepositoryTrustPlan,
    format: RepositoryFormat,
) -> Result<Option<RepositoryTrustPolicy>> {
    match format {
        RepositoryFormat::Debian => Ok(Some(RepositoryTrustPolicy::Debian {
            release_keys: roots_for_role(
                selected_root,
                declarations,
                plan,
                TrustRole::DebianRelease,
            )?,
        })),
        RepositoryFormat::Fedora => {
            let metadata_urls: Vec<_> = plan
                .evidence
                .iter()
                .filter_map(|evidence| match (&evidence.role, &evidence.source) {
                    (TrustRole::RpmMetadata, TrustEvidenceSource::RpmMetalink { url }) => {
                        Some(url.clone())
                    }
                    _ => None,
                })
                .collect();
            let metadata = if metadata_urls.len() == 1 {
                RpmMetadataAuthority::Metalink {
                    url: metadata_urls[0].clone(),
                }
            } else {
                RpmMetadataAuthority::OpenPgp {
                    keys: roots_for_role(
                        selected_root,
                        declarations,
                        plan,
                        TrustRole::RpmMetadata,
                    )?,
                }
            };
            Ok(Some(RepositoryTrustPolicy::Rpm {
                metadata,
                package_keys: roots_for_role(
                    selected_root,
                    declarations,
                    plan,
                    TrustRole::RpmPackage,
                )?,
            }))
        }
        RepositoryFormat::Arch => Ok(None),
        RepositoryFormat::Eopkg => {
            let NativeRepositoryReference::Eopkg { index_url, .. } = &plan.repository else {
                return Err(Error::ConfigError(
                    "eopkg trust projection requires an eopkg declaration".to_string(),
                ));
            };
            Ok(Some(RepositoryTrustPolicy::Eopkg {
                origin: crate::repository::eopkg_origin_from_index_url(index_url)?,
            }))
        }
        RepositoryFormat::Json | RepositoryFormat::Unspecified => Err(Error::ConfigError(
            "native repository takeover requires an Arch, Debian, eopkg, or RPM parser".to_string(),
        )),
    }
}

/// Resolve only APT's legacy global-keyring ambiguity through exact manifest
/// roots. The manifest must bind primary fingerprints to native APT global
/// keyring files below the selected root; no filename selects a repository or
/// certificate implicitly.
pub(super) fn explicit_apt_global_trust_paths(
    selected_root: &Path,
    plan: &NativeRepositoryTrustPlan,
    trust: &RepositoryTrustPolicy,
) -> Option<Vec<PathBuf>> {
    let TrustImportDisposition::Ambiguous { findings } = &plan.disposition else {
        return None;
    };
    if findings.is_empty()
        || findings
            .iter()
            .any(|finding| finding.kind != TrustImportFindingKind::ImplicitGlobalAuthority)
    {
        return None;
    }
    let RepositoryTrustPolicy::Debian { release_keys } = trust else {
        return None;
    };
    if release_keys.is_empty() {
        return None;
    }

    let mut paths = BTreeSet::new();
    for root in release_keys {
        root.validate().ok()?;
        let url = url::Url::parse(&root.url).ok()?;
        if url.scheme() != "file" {
            return None;
        }
        let host_path = url.to_file_path().ok()?;
        let relative = host_path.strip_prefix(selected_root).ok()?;
        let guest_path = Path::new("/").join(relative);
        if !is_apt_global_trust_path(&guest_path)
            || safe_join(selected_root, &guest_path).ok()? != host_path
        {
            return None;
        }
        let bytes = fs::read(&host_path).ok()?;
        let parser = CertParser::from_bytes(&bytes).ok()?;
        let mut matched = false;
        for certificate in parser {
            let certificate = certificate.ok()?;
            if certificate.fingerprint().to_hex() == root.fingerprint {
                matched = true;
                break;
            }
        }
        if !matched {
            return None;
        }
        paths.insert(guest_path);
    }
    Some(paths.into_iter().collect())
}

/// Resolve Zypper's documented global-on signature defaults through an exact
/// manifest binding to the native RPM key store. This is intentionally valid
/// only when no global zypp.conf overrides exist and every ambiguous finding
/// is one of the two facts the manifest closes: inherited verification or an
/// absent per-repository gpgkey.
pub(super) fn explicit_zypper_global_trust_paths(
    selected_root: &Path,
    plan: &NativeRepositoryTrustPlan,
    trust: &RepositoryTrustPolicy,
) -> Option<Vec<PathBuf>> {
    if !matches!(plan.repository, NativeRepositoryReference::Zypper { .. })
        || safe_join(selected_root, Path::new("/etc/zypp/zypp.conf"))
            .ok()?
            .exists()
    {
        return None;
    }
    let TrustImportDisposition::Ambiguous { findings } = &plan.disposition else {
        return None;
    };
    if findings.is_empty()
        || findings
            .iter()
            .any(|finding| finding.kind != TrustImportFindingKind::MissingRequiredAuthority)
    {
        return None;
    }
    let RepositoryTrustPolicy::Rpm {
        metadata: RpmMetadataAuthority::OpenPgp {
            keys: metadata_keys,
        },
        package_keys,
    } = trust
    else {
        return None;
    };
    if metadata_keys.is_empty() || metadata_keys != package_keys {
        return None;
    }

    exact_native_rpm_key_paths(selected_root, metadata_keys)
}

fn exact_native_rpm_key_paths(
    selected_root: &Path,
    roots: &[OpenPgpTrustRoot],
) -> Option<Vec<PathBuf>> {
    let mut paths = BTreeSet::new();
    for root in roots {
        root.validate().ok()?;
        let url = url::Url::parse(&root.url).ok()?;
        if url.scheme() != "file" {
            return None;
        }
        let host_path = url.to_file_path().ok()?;
        let relative = host_path.strip_prefix(selected_root).ok()?;
        let guest_path = Path::new("/").join(relative);
        if guest_path.parent() != Some(Path::new("/usr/lib/rpm/gnupg/keys"))
            || safe_join(selected_root, &guest_path).ok()? != host_path
        {
            return None;
        }
        let bytes = fs::read(&host_path).ok()?;
        let parser = CertParser::from_bytes(&bytes).ok()?;
        let mut matched = false;
        for certificate in parser {
            let certificate = certificate.ok()?;
            if certificate.fingerprint().to_hex() == root.fingerprint {
                matched = true;
                break;
            }
        }
        if !matched {
            return None;
        }
        paths.insert(guest_path);
    }
    Some(paths.into_iter().collect())
}

fn is_apt_global_trust_path(path: &Path) -> bool {
    if path == Path::new("/etc/apt/trusted.gpg") {
        return true;
    }
    path.parent() == Some(Path::new("/etc/apt/trusted.gpg.d"))
        && path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| matches!(extension, "gpg" | "asc"))
}

fn roots_for_role(
    selected_root: &Path,
    declarations: &DiscoveredRepositoryDeclarations,
    plan: &NativeRepositoryTrustPlan,
    role: TrustRole,
) -> Result<Vec<OpenPgpTrustRoot>> {
    let mut roots = Vec::new();
    for evidence in plan
        .evidence
        .iter()
        .filter(|evidence| evidence.role == role)
    {
        let Some(url) = evidence_openpgp_url(selected_root, declarations, plan, evidence)? else {
            continue;
        };
        for certificate in &evidence.certificates {
            roots.push(OpenPgpTrustRoot {
                url: url.clone(),
                fingerprint: certificate.certificate_fingerprint.clone(),
            });
        }
    }
    roots.sort_by(|left, right| {
        (&left.fingerprint, &left.url).cmp(&(&right.fingerprint, &right.url))
    });
    roots.dedup_by(|left, right| left.fingerprint == right.fingerprint);
    Ok(roots)
}

fn evidence_openpgp_url(
    selected_root: &Path,
    declarations: &DiscoveredRepositoryDeclarations,
    plan: &NativeRepositoryTrustPlan,
    evidence: &TrustImportEvidence,
) -> Result<Option<String>> {
    match &evidence.source {
        TrustEvidenceSource::SelectedRootFile { path } => {
            let host_path = safe_join(selected_root, path)?;
            let url = url::Url::from_file_path(&host_path).map_err(|_| {
                Error::ConfigError(format!(
                    "selected-root trust path {} cannot be represented as a file URL",
                    host_path.display()
                ))
            })?;
            Ok(Some(url.to_string()))
        }
        TrustEvidenceSource::EmbeddedOpenPgp => {
            let bytes = embedded_openpgp_bytes(declarations, plan)?;
            Ok(Some(format!(
                "data:application/pgp-keys;base64,{}",
                base64::engine::general_purpose::STANDARD.encode(bytes)
            )))
        }
        TrustEvidenceSource::RpmMetalink { .. } | TrustEvidenceSource::AlpmSigLevel { .. } => {
            Ok(None)
        }
    }
}

fn embedded_openpgp_bytes(
    declarations: &DiscoveredRepositoryDeclarations,
    plan: &NativeRepositoryTrustPlan,
) -> Result<Vec<u8>> {
    let NativeRepositoryReference::Apt {
        document,
        entry_index,
        ..
    } = &plan.repository
    else {
        return Err(Error::ConfigError(
            "embedded OpenPGP evidence is only valid for an APT declaration".to_string(),
        ));
    };
    let entry = declarations
        .apt
        .iter()
        .find(|candidate| &candidate.path == document)
        .and_then(|candidate| candidate.entries.get(*entry_index))
        .ok_or_else(|| {
            Error::ConfigError(format!(
                "embedded OpenPGP declaration {} entry {} is absent",
                document.display(),
                entry_index
            ))
        })?;
    let values: Vec<_> = entry
        .options
        .iter()
        .filter(|option| option.name == super::super::apt::AptOptionName::SignedBy)
        .flat_map(|option| option.values.iter())
        .filter(|value| {
            value
                .trim_start()
                .starts_with("-----BEGIN PGP PUBLIC KEY BLOCK-----")
        })
        .collect();
    if values.len() != 1 {
        return Err(Error::ConfigError(format!(
            "embedded OpenPGP evidence at {}:{} has {} exact source values",
            entry.location.path.display(),
            entry.location.line,
            values.len()
        )));
    }
    Ok(normalize_deb822_embedded(values[0].trim_start().as_bytes()))
}

fn normalize_deb822_embedded(bytes: &[u8]) -> Vec<u8> {
    let Ok(value) = std::str::from_utf8(bytes) else {
        return bytes.to_vec();
    };
    let mut normalized = String::new();
    for (index, line) in value.lines().enumerate() {
        if index > 0 {
            normalized.push('\n');
        }
        if line == "." {
            continue;
        }
        if let Some(rest) = line.strip_prefix("..") {
            normalized.push('.');
            normalized.push_str(rest);
        } else {
            normalized.push_str(line);
        }
    }
    normalized.push('\n');
    normalized.into_bytes()
}
