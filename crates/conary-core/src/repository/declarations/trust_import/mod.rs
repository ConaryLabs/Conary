// conary-core/src/repository/declarations/trust_import/mod.rs

//! Fail-closed trust-import planning for native repository declarations.
//!
//! Planning reads only explicitly declared material below the selected root.
//! It does not fetch keys, persist trust, enable repositories, or infer roles
//! from names and URLs.

use super::alpm::{AlpmDirective, AlpmRepositoryOptionName};
use super::apt::AptSourceEntry;
use super::dnf::DnfRepository;
use super::zypper::ZypperRepository;
use super::{DeclarationLocation, DiscoveredRepositoryDeclarations};
use crate::repository::TrustRole;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

mod apt;
mod openpgp;
mod rpm;

/// Exact native declaration identity. APT has no repository name, so its
/// document path and entry index remain the identity until enrollment assigns
/// a durable source ID.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "ecosystem", rename_all = "kebab-case", deny_unknown_fields)]
pub enum NativeRepositoryReference {
    Apt {
        document: PathBuf,
        entry_index: usize,
        uris: Vec<String>,
        suites: Vec<String>,
        location: DeclarationLocation,
    },
    Dnf {
        id: String,
        location: DeclarationLocation,
    },
    Zypper {
        alias: String,
        location: DeclarationLocation,
    },
    Alpm {
        name: String,
        location: DeclarationLocation,
    },
    Eopkg {
        name: String,
        index_url: String,
        location: DeclarationLocation,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum TrustEvidenceSource {
    SelectedRootFile { path: PathBuf },
    EmbeddedOpenPgp,
    RpmMetalink { url: String },
    AlpmSigLevel { values: Vec<String> },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenPgpCertificateEvidence {
    pub certificate_fingerprint: String,
    pub key_fingerprints: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenPgpFingerprintSelector {
    pub fingerprint: String,
    /// APT's trailing `!` restricts authority to the exact selected key rather
    /// than admitting its signing subkeys.
    pub primary_only: bool,
}

/// Exact evidence that can be carried into a later apply transaction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrustImportEvidence {
    pub role: TrustRole,
    pub source: TrustEvidenceSource,
    pub certificates: Vec<OpenPgpCertificateEvidence>,
    pub fingerprint_selectors: Vec<OpenPgpFingerprintSelector>,
    pub location: DeclarationLocation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TrustImportFindingKind {
    ImplicitGlobalAuthority,
    MissingRequiredAuthority,
    UnpinnedRemoteAuthority,
    MissingDeclaredTrustSource,
    SelectedRootEscape,
    MalformedOpenPgp,
    FingerprintSelectorWithoutSource,
    FingerprintSelectorMismatch,
    VerificationDisabled,
    OptionalPackageSignature,
    TrustAll,
    AlpmKeyringBindingMissing,
    UnsupportedTrustValue,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrustImportFinding {
    pub kind: TrustImportFindingKind,
    pub role: Option<TrustRole>,
    pub location: DeclarationLocation,
    pub message: String,
}

/// A plan is importable only when all required roles have exact evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "kebab-case", deny_unknown_fields)]
pub enum TrustImportDisposition {
    Importable,
    Ambiguous { findings: Vec<TrustImportFinding> },
    Unsupported { findings: Vec<TrustImportFinding> },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeRepositoryTrustPlan {
    pub repository: NativeRepositoryReference,
    pub enabled: Option<bool>,
    pub evidence: Vec<TrustImportEvidence>,
    pub disposition: TrustImportDisposition,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeTrustImportPlan {
    pub selected_root: PathBuf,
    pub repositories: Vec<NativeRepositoryTrustPlan>,
}

#[derive(Default)]
struct PlanFindings {
    ambiguous: Vec<TrustImportFinding>,
    unsupported: Vec<TrustImportFinding>,
}

type AlpmRepositoryState<'a> = (
    String,
    DeclarationLocation,
    Option<(&'a [String], &'a DeclarationLocation)>,
);

impl PlanFindings {
    fn ambiguous(&mut self, finding: TrustImportFinding) {
        self.ambiguous.push(finding);
    }

    fn unsupported(&mut self, finding: TrustImportFinding) {
        self.unsupported.push(finding);
    }

    fn is_empty(&self) -> bool {
        self.ambiguous.is_empty() && self.unsupported.is_empty()
    }

    fn has_role(&self, role: TrustRole) -> bool {
        self.ambiguous
            .iter()
            .chain(&self.unsupported)
            .any(|finding| finding.role == Some(role))
    }

    fn finish(mut self) -> TrustImportDisposition {
        if !self.unsupported.is_empty() {
            self.unsupported.append(&mut self.ambiguous);
            TrustImportDisposition::Unsupported {
                findings: self.unsupported,
            }
        } else if !self.ambiguous.is_empty() {
            TrustImportDisposition::Ambiguous {
                findings: self.ambiguous,
            }
        } else {
            TrustImportDisposition::Importable
        }
    }
}

/// Produce deterministic preview data without mutating or fetching anything.
pub fn plan_selected_root_trust(
    declarations: &DiscoveredRepositoryDeclarations,
) -> NativeTrustImportPlan {
    let mut repositories = Vec::new();
    for document in &declarations.apt {
        for (entry_index, entry) in document.entries.iter().enumerate() {
            repositories.push(apt::plan(
                &declarations.root,
                &document.path,
                entry_index,
                entry,
            ));
        }
    }
    for document in &declarations.dnf {
        for repository in &document.repositories {
            repositories.push(rpm::plan_dnf(&declarations.root, repository));
        }
    }
    for document in &declarations.zypper_repositories {
        for repository in &document.repositories {
            repositories.push(rpm::plan_zypper(&declarations.root, repository));
        }
    }
    if let Some(config) = &declarations.alpm {
        repositories.extend(plan_alpm(config));
    }
    if let Some(document) = &declarations.eopkg {
        repositories.extend(document.repositories.iter().map(plan_eopkg));
    }
    NativeTrustImportPlan {
        selected_root: declarations.root.clone(),
        repositories,
    }
}

fn plan_eopkg(repository: &super::eopkg::EopkgRepository) -> NativeRepositoryTrustPlan {
    let mut findings = PlanFindings::default();
    if repository.media != "remote" {
        findings.unsupported(finding(
            TrustImportFindingKind::UnsupportedTrustValue,
            None,
            &repository.location,
            format!(
                "eopkg repository media '{}' is not the authenticated remote-index contract",
                repository.media
            ),
        ));
    }
    if crate::repository::eopkg_origin_from_index_url(&repository.index_url).is_err() {
        findings.unsupported(finding(
            TrustImportFindingKind::MissingRequiredAuthority,
            None,
            &repository.location,
            "eopkg repository must declare an exact HTTPS eopkg-index.xml.xz URL",
        ));
    }
    NativeRepositoryTrustPlan {
        repository: NativeRepositoryReference::Eopkg {
            name: repository.name.clone(),
            index_url: repository.index_url.clone(),
            location: repository.location.clone(),
        },
        enabled: Some(repository.enabled),
        evidence: Vec::new(),
        disposition: findings.finish(),
    }
}

fn plan_alpm(config: &super::alpm::AlpmConfigSet) -> Vec<NativeRepositoryTrustPlan> {
    let global_sig_level = config
        .directives
        .iter()
        .rev()
        .find_map(|directive| match directive {
            AlpmDirective::GlobalSigLevel { values, location } => Some((values, location)),
            _ => None,
        });
    let mut repositories: Vec<AlpmRepositoryState<'_>> = Vec::new();
    for directive in &config.directives {
        let AlpmDirective::RepositoryOption {
            repository,
            name,
            values,
            location,
            ..
        } = directive
        else {
            continue;
        };
        let entry = if let Some(index) = repositories
            .iter()
            .position(|(name, _, _)| name == repository)
        {
            &mut repositories[index]
        } else {
            repositories.push((repository.clone(), location.clone(), None));
            repositories.last_mut().expect("repository was pushed")
        };
        if *name == AlpmRepositoryOptionName::SigLevel {
            entry.2 = Some((values, location));
        }
    }
    repositories
        .into_iter()
        .map(|(name, location, repository_sig_level)| {
            let (values, sig_location) = repository_sig_level
                .or_else(|| global_sig_level.map(|(values, location)| (values.as_slice(), location)))
                .unwrap_or((&[], &location));
            let mut evidence = Vec::new();
            for role in [TrustRole::ArchDatabase, TrustRole::ArchPackage] {
                evidence.push(TrustImportEvidence {
                    role,
                    source: TrustEvidenceSource::AlpmSigLevel {
                        values: values.to_vec(),
                    },
                    certificates: Vec::new(),
                    fingerprint_selectors: Vec::new(),
                    location: sig_location.clone(),
                });
            }
            let mut findings = PlanFindings::default();
            classify_alpm_sig_level(values, sig_location, &mut findings);
            if findings.unsupported.is_empty() {
                findings.ambiguous(finding(
                    TrustImportFindingKind::AlpmKeyringBindingMissing,
                    Some(TrustRole::ArchPackage),
                    sig_location,
                    "pacman SigLevel does not bind repositories to an exact authenticated keyring snapshot and master-fingerprint threshold",
                ));
            }
            NativeRepositoryTrustPlan {
                repository: NativeRepositoryReference::Alpm { name, location },
                enabled: Some(true),
                evidence,
                disposition: findings.finish(),
            }
        })
        .collect()
}

fn classify_alpm_sig_level(
    values: &[String],
    location: &DeclarationLocation,
    findings: &mut PlanFindings,
) {
    for value in values {
        if value.ends_with("TrustAll") || value == "TrustAll" {
            findings.unsupported(finding(
                TrustImportFindingKind::TrustAll,
                None,
                location,
                format!("ALPM SigLevel token '{value}' admits untrusted signing keys"),
            ));
        } else if value.ends_with("Never") || value == "Never" {
            findings.unsupported(finding(
                TrustImportFindingKind::VerificationDisabled,
                None,
                location,
                format!("ALPM SigLevel token '{value}' disables signature verification"),
            ));
        } else if value == "Optional" || value == "PackageOptional" {
            findings.unsupported(finding(
                TrustImportFindingKind::OptionalPackageSignature,
                Some(TrustRole::ArchPackage),
                location,
                format!("ALPM SigLevel token '{value}' permits unsigned packages"),
            ));
        }
    }
}

fn finding(
    kind: TrustImportFindingKind,
    role: Option<TrustRole>,
    location: &DeclarationLocation,
    message: impl Into<String>,
) -> TrustImportFinding {
    TrustImportFinding {
        kind,
        role,
        location: location.clone(),
        message: message.into(),
    }
}

fn apt_reference(
    document: &std::path::Path,
    entry_index: usize,
    entry: &AptSourceEntry,
) -> NativeRepositoryReference {
    NativeRepositoryReference::Apt {
        document: document.to_path_buf(),
        entry_index,
        uris: entry.uris.clone(),
        suites: entry.suites.clone(),
        location: entry.location.clone(),
    }
}

fn dnf_reference(repository: &DnfRepository) -> NativeRepositoryReference {
    NativeRepositoryReference::Dnf {
        id: repository.id.clone(),
        location: repository.location.clone(),
    }
}

fn zypper_reference(repository: &ZypperRepository) -> NativeRepositoryReference {
    NativeRepositoryReference::Zypper {
        alias: repository.alias.clone(),
        location: repository.location.clone(),
    }
}

#[cfg(test)]
mod tests;
