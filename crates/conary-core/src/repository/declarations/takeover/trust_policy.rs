// conary-core/src/repository/declarations/takeover/trust_policy.rs

//! Exact projection from discovered trust evidence into persisted policy.

use super::*;
use base64::Engine;

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
        RepositoryFormat::Json | RepositoryFormat::Unspecified => Err(Error::ConfigError(
            "native repository takeover requires an Arch, Debian, or RPM parser".to_string(),
        )),
    }
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
