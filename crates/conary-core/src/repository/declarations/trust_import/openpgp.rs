// conary-core/src/repository/declarations/trust_import/openpgp.rs

use super::{
    OpenPgpCertificateEvidence, OpenPgpFingerprintSelector, PlanFindings, TrustEvidenceSource,
    TrustImportEvidence, TrustImportFindingKind, finding,
};
use crate::filesystem::path::safe_join;
use crate::repository::TrustRole;
use crate::repository::declarations::DeclarationLocation;
use openpgp::cert::CertParser;
use openpgp::parse::Parse;
use sequoia_openpgp as openpgp;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use url::Url;

pub(super) struct CollectedRoots {
    pub evidence: Vec<TrustImportEvidence>,
}

pub(super) fn collect_declared_roots(
    selected_root: &Path,
    role: TrustRole,
    values: &[String],
    location: &DeclarationLocation,
    findings: &mut PlanFindings,
) -> CollectedRoots {
    let mut evidence = Vec::new();
    let mut selectors: Vec<OpenPgpFingerprintSelector> = Vec::new();
    for raw_value in values {
        if raw_value
            .trim_start()
            .starts_with("-----BEGIN PGP PUBLIC KEY BLOCK-----")
        {
            collect_embedded(
                role,
                raw_value.trim_start().as_bytes(),
                location,
                &mut evidence,
                findings,
            );
            continue;
        }
        for value in raw_value.split_whitespace() {
            let value = value.trim_matches(',');
            if value.is_empty() {
                continue;
            }
            if let Some(selector) = fingerprint_selector(value) {
                selectors.push(selector);
                continue;
            }
            match declared_path(value) {
                DeclaredSource::SelectedRoot(path) => collect_file(
                    selected_root,
                    role,
                    &path,
                    location,
                    &mut evidence,
                    findings,
                ),
                DeclaredSource::Remote(url) => findings.ambiguous(finding(
                    TrustImportFindingKind::UnpinnedRemoteAuthority,
                    Some(role),
                    location,
                    format!(
                        "declared trust source '{url}' is remote and carries no exact certificate fingerprint pin"
                    ),
                )),
                DeclaredSource::Unsupported(value) => findings.unsupported(finding(
                    TrustImportFindingKind::UnsupportedTrustValue,
                    Some(role),
                    location,
                    format!("unsupported declared trust source '{value}'"),
                )),
            }
        }
    }
    selectors.sort_by(|left, right| {
        (&left.fingerprint, left.primary_only).cmp(&(&right.fingerprint, right.primary_only))
    });
    selectors.dedup();
    if !selectors.is_empty() && evidence.is_empty() {
        findings.ambiguous(finding(
            TrustImportFindingKind::FingerprintSelectorWithoutSource,
            Some(role),
            location,
            "fingerprint selectors refer to implicit global native trust rather than an exact declared key source",
        ));
    }
    if !selectors.is_empty() && !evidence.is_empty() {
        let available: BTreeSet<_> = evidence
            .iter()
            .flat_map(|item| {
                item.certificates
                    .iter()
                    .flat_map(|certificate| certificate.key_fingerprints.iter().cloned())
            })
            .collect();
        let missing: Vec<_> = selectors
            .iter()
            .filter(|selector| {
                if selector.primary_only {
                    !evidence.iter().any(|item| {
                        item.certificates.iter().any(|certificate| {
                            certificate.certificate_fingerprint == selector.fingerprint
                        })
                    })
                } else {
                    !available.contains(selector.fingerprint.as_str())
                }
            })
            .map(|selector| selector.fingerprint.clone())
            .collect();
        if !missing.is_empty() {
            findings.unsupported(finding(
                TrustImportFindingKind::FingerprintSelectorMismatch,
                Some(role),
                location,
                format!(
                    "declared fingerprint selectors are absent from the declared key material: {}",
                    missing.join(", ")
                ),
            ));
        } else {
            for item in &mut evidence {
                item.certificates.retain(|certificate| {
                    selectors
                        .iter()
                        .any(|selector| selector_matches_certificate(selector, certificate))
                });
                item.fingerprint_selectors = selectors
                    .iter()
                    .filter(|selector| {
                        item.certificates
                            .iter()
                            .any(|certificate| selector_matches_certificate(selector, certificate))
                    })
                    .cloned()
                    .collect();
            }
            evidence.retain(|item| !item.certificates.is_empty());
        }
    }
    CollectedRoots { evidence }
}

fn selector_matches_certificate(
    selector: &OpenPgpFingerprintSelector,
    certificate: &OpenPgpCertificateEvidence,
) -> bool {
    if selector.primary_only {
        certificate.certificate_fingerprint == selector.fingerprint
    } else {
        certificate.key_fingerprints.contains(&selector.fingerprint)
    }
}

pub(super) fn collect_embedded(
    role: TrustRole,
    bytes: &[u8],
    location: &DeclarationLocation,
    evidence: &mut Vec<TrustImportEvidence>,
    findings: &mut PlanFindings,
) {
    let normalized = normalize_deb822_embedded(bytes);
    match fingerprints(&normalized) {
        Ok(certificates) => evidence.push(TrustImportEvidence {
            role,
            source: TrustEvidenceSource::EmbeddedOpenPgp,
            certificates,
            fingerprint_selectors: Vec::new(),
            location: location.clone(),
        }),
        Err(message) => findings.unsupported(finding(
            TrustImportFindingKind::MalformedOpenPgp,
            Some(role),
            location,
            message,
        )),
    }
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

fn collect_file(
    selected_root: &Path,
    role: TrustRole,
    guest_path: &Path,
    location: &DeclarationLocation,
    evidence: &mut Vec<TrustImportEvidence>,
    findings: &mut PlanFindings,
) {
    let host_path = match safe_join(selected_root, guest_path) {
        Ok(path) => path,
        Err(error) => {
            findings.unsupported(finding(
                TrustImportFindingKind::SelectedRootEscape,
                Some(role),
                location,
                format!(
                    "declared trust path '{}' escapes the selected root: {error}",
                    guest_path.display()
                ),
            ));
            return;
        }
    };
    let bytes = match fs::read(&host_path) {
        Ok(bytes) => bytes,
        Err(error) => {
            findings.unsupported(finding(
                TrustImportFindingKind::MissingDeclaredTrustSource,
                Some(role),
                location,
                format!(
                    "declared trust source '{}' is not readable below the selected root: {error}",
                    guest_path.display()
                ),
            ));
            return;
        }
    };
    match fingerprints(&bytes) {
        Ok(certificates) => evidence.push(TrustImportEvidence {
            role,
            source: TrustEvidenceSource::SelectedRootFile {
                path: guest_path.to_path_buf(),
            },
            certificates,
            fingerprint_selectors: Vec::new(),
            location: location.clone(),
        }),
        Err(message) => findings.unsupported(finding(
            TrustImportFindingKind::MalformedOpenPgp,
            Some(role),
            location,
            format!(
                "declared trust source '{}' is not exact OpenPGP certificate material: {message}",
                guest_path.display()
            ),
        )),
    }
}

fn fingerprints(bytes: &[u8]) -> Result<Vec<OpenPgpCertificateEvidence>, String> {
    let parser = CertParser::from_bytes(bytes)
        .map_err(|error| format!("failed to parse OpenPGP certificate stream: {error}"))?;
    let mut certificates = Vec::new();
    for certificate in parser {
        let certificate =
            certificate.map_err(|error| format!("malformed OpenPGP certificate: {error}"))?;
        let mut key_fingerprints: Vec<_> = certificate
            .keys()
            .map(|key| key.key().fingerprint().to_hex())
            .collect();
        key_fingerprints.sort();
        key_fingerprints.dedup();
        certificates.push(OpenPgpCertificateEvidence {
            certificate_fingerprint: certificate.fingerprint().to_hex(),
            key_fingerprints,
        });
    }
    if certificates.is_empty() {
        return Err("OpenPGP source contains no certificates".to_string());
    }
    certificates.sort_by(|left, right| {
        left.certificate_fingerprint
            .cmp(&right.certificate_fingerprint)
    });
    certificates
        .dedup_by(|left, right| left.certificate_fingerprint == right.certificate_fingerprint);
    Ok(certificates)
}

enum DeclaredSource {
    SelectedRoot(PathBuf),
    Remote(String),
    Unsupported(String),
}

fn declared_path(value: &str) -> DeclaredSource {
    if value.starts_with('/') {
        return DeclaredSource::SelectedRoot(PathBuf::from(value));
    }
    let Ok(url) = Url::parse(value) else {
        return DeclaredSource::Unsupported(value.to_string());
    };
    match url.scheme() {
        "file" => url
            .to_file_path()
            .map(DeclaredSource::SelectedRoot)
            .unwrap_or_else(|_| DeclaredSource::Unsupported(value.to_string())),
        "https" => DeclaredSource::Remote(value.to_string()),
        _ => DeclaredSource::Unsupported(value.to_string()),
    }
}

fn fingerprint_selector(value: &str) -> Option<OpenPgpFingerprintSelector> {
    let primary_only = value.ends_with('!');
    let stripped = value.strip_suffix('!').unwrap_or(value);
    (matches!(stripped.len(), 40 | 64) && stripped.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then(|| OpenPgpFingerprintSelector {
            fingerprint: stripped.to_ascii_uppercase(),
            primary_only,
        })
}
