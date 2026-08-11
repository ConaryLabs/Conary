// conary-core/src/repository/declarations/trust_import/rpm.rs

use super::openpgp::collect_declared_roots;
use super::{
    NativeRepositoryTrustPlan, PlanFindings, TrustEvidenceSource, TrustImportEvidence,
    TrustImportFindingKind, dnf_reference, finding, zypper_reference,
};
use crate::repository::TrustRole;
use crate::repository::declarations::DeclarationLocation;
use crate::repository::declarations::dnf::{DnfEndpointKind, DnfOptionName, DnfRepository};
use crate::repository::declarations::zypper::{
    ZypperRepository, ZypperRepositoryOption, ZypperRepositoryOptionName,
};
use std::path::Path;

pub(super) fn plan_dnf(
    selected_root: &Path,
    repository: &DnfRepository,
) -> NativeRepositoryTrustPlan {
    let mut findings = PlanFindings::default();
    let mut evidence = Vec::new();
    let package_check = dnf_last_bool(
        repository,
        &[DnfOptionName::Gpgcheck, DnfOptionName::PkgGpgcheck],
    );
    require_verification(
        &package_check,
        TrustRole::RpmPackage,
        &repository.location,
        "DNF package",
        &mut findings,
    );
    if package_check.is_true() {
        evidence.extend(dnf_keys(
            selected_root,
            repository,
            TrustRole::RpmPackage,
            &mut findings,
        ));
    }

    let metadata_check = dnf_last_bool(repository, &[DnfOptionName::RepoGpgcheck]);
    if metadata_check.is_true() {
        evidence.extend(dnf_keys(
            selected_root,
            repository,
            TrustRole::RpmMetadata,
            &mut findings,
        ));
    } else if let Some((DnfEndpointKind::Metalink, url)) = repository.effective_endpoint() {
        let location = repository
            .options
            .iter()
            .rev()
            .find(|option| option.name == DnfOptionName::Metalink)
            .map_or(&repository.location, |option| &option.location);
        evidence.push(TrustImportEvidence {
            role: TrustRole::RpmMetadata,
            source: TrustEvidenceSource::RpmMetalink {
                url: url.to_string(),
            },
            certificates: Vec::new(),
            fingerprint_selectors: Vec::new(),
            location: location.clone(),
        });
    } else {
        require_verification(
            &metadata_check,
            TrustRole::RpmMetadata,
            &repository.location,
            "DNF repository metadata",
            &mut findings,
        );
    }
    ensure_roles(&evidence, &repository.location, &mut findings);
    let enabled = match repository.explicit_enabled() {
        Ok(Some(value)) => Some(value),
        Ok(None) => Some(true),
        Err(error) => {
            findings.unsupported(finding(
                TrustImportFindingKind::UnsupportedTrustValue,
                None,
                &repository.location,
                format!("DNF enabled state is invalid: {error}"),
            ));
            None
        }
    };
    NativeRepositoryTrustPlan {
        repository: dnf_reference(repository),
        enabled,
        evidence,
        disposition: findings.finish(),
    }
}

pub(super) fn plan_zypper(
    selected_root: &Path,
    repository: &ZypperRepository,
) -> NativeRepositoryTrustPlan {
    let mut findings = PlanFindings::default();
    let mut evidence = Vec::new();
    let general = zypper_last_bool(repository, ZypperRepositoryOptionName::Gpgcheck);
    let package_check =
        zypper_last_bool(repository, ZypperRepositoryOptionName::PkgGpgcheck).or(general.clone());
    let metadata_check =
        zypper_last_bool(repository, ZypperRepositoryOptionName::RepoGpgcheck).or(general);
    require_verification(
        &package_check,
        TrustRole::RpmPackage,
        &repository.location,
        "Zypper package",
        &mut findings,
    );
    if package_check.is_true() {
        evidence.extend(zypper_keys(
            selected_root,
            repository,
            TrustRole::RpmPackage,
            &mut findings,
        ));
    }
    if metadata_check.is_true() {
        evidence.extend(zypper_keys(
            selected_root,
            repository,
            TrustRole::RpmMetadata,
            &mut findings,
        ));
    } else if let Some((url, location)) =
        zypper_last_value(repository, ZypperRepositoryOptionName::Metalink)
    {
        evidence.push(TrustImportEvidence {
            role: TrustRole::RpmMetadata,
            source: TrustEvidenceSource::RpmMetalink {
                url: url.trim().to_string(),
            },
            certificates: Vec::new(),
            fingerprint_selectors: Vec::new(),
            location: location.clone(),
        });
    } else {
        require_verification(
            &metadata_check,
            TrustRole::RpmMetadata,
            &repository.location,
            "Zypper repository metadata",
            &mut findings,
        );
    }
    ensure_roles(&evidence, &repository.location, &mut findings);
    let enabled = match repository.explicit_enabled() {
        Ok(Some(value)) => Some(value),
        Ok(None) => Some(true),
        Err(error) => {
            findings.unsupported(finding(
                TrustImportFindingKind::UnsupportedTrustValue,
                None,
                &repository.location,
                format!("Zypper enabled state is invalid: {error}"),
            ));
            None
        }
    };
    NativeRepositoryTrustPlan {
        repository: zypper_reference(repository),
        enabled,
        evidence,
        disposition: findings.finish(),
    }
}

fn dnf_keys(
    selected_root: &Path,
    repository: &DnfRepository,
    role: TrustRole,
    findings: &mut PlanFindings,
) -> Vec<TrustImportEvidence> {
    let Some(option) = repository
        .options
        .iter()
        .rev()
        .find(|option| option.name == DnfOptionName::Gpgkey)
    else {
        findings.ambiguous(finding(
            TrustImportFindingKind::MissingRequiredAuthority,
            Some(role),
            &repository.location,
            format!("DNF repository '{}' has no declared gpgkey", repository.id),
        ));
        return Vec::new();
    };
    collect_declared_roots(
        selected_root,
        role,
        std::slice::from_ref(&option.raw_value),
        &option.location,
        findings,
    )
    .evidence
}

fn zypper_keys(
    selected_root: &Path,
    repository: &ZypperRepository,
    role: TrustRole,
    findings: &mut PlanFindings,
) -> Vec<TrustImportEvidence> {
    let Some((value, location)) = zypper_last_value(repository, ZypperRepositoryOptionName::Gpgkey)
    else {
        findings.ambiguous(finding(
            TrustImportFindingKind::MissingRequiredAuthority,
            Some(role),
            &repository.location,
            format!(
                "Zypper repository '{}' has no declared gpgkey",
                repository.alias
            ),
        ));
        return Vec::new();
    };
    let value = value.to_string();
    collect_declared_roots(
        selected_root,
        role,
        std::slice::from_ref(&value),
        location,
        findings,
    )
    .evidence
}

fn ensure_roles(
    evidence: &[TrustImportEvidence],
    location: &DeclarationLocation,
    findings: &mut PlanFindings,
) {
    for role in [TrustRole::RpmMetadata, TrustRole::RpmPackage] {
        if !evidence.iter().any(|item| item.role == role) && !findings.has_role(role) {
            findings.ambiguous(finding(
                TrustImportFindingKind::MissingRequiredAuthority,
                Some(role),
                location,
                format!("no exact {} authority was discovered", role.as_str()),
            ));
        }
    }
}

#[derive(Debug, Clone)]
enum NativeBool {
    Missing,
    True,
    False(DeclarationLocation),
    Unsupported(String, DeclarationLocation),
}

impl NativeBool {
    fn is_true(&self) -> bool {
        matches!(self, Self::True)
    }

    fn or(self, fallback: Self) -> Self {
        if matches!(self, Self::Missing) {
            fallback
        } else {
            self
        }
    }
}

fn require_verification(
    value: &NativeBool,
    role: TrustRole,
    default_location: &DeclarationLocation,
    label: &str,
    findings: &mut PlanFindings,
) {
    match value {
        NativeBool::True => {}
        NativeBool::Missing => findings.ambiguous(finding(
            TrustImportFindingKind::MissingRequiredAuthority,
            Some(role),
            default_location,
            format!("{label} verification is inherited from unmodeled global native configuration"),
        )),
        NativeBool::False(location) => findings.unsupported(finding(
            TrustImportFindingKind::VerificationDisabled,
            Some(role),
            location,
            format!("{label} verification is disabled"),
        )),
        NativeBool::Unsupported(value, location) => findings.unsupported(finding(
            TrustImportFindingKind::UnsupportedTrustValue,
            Some(role),
            location,
            format!("{label} uses unsupported trust value '{value}'"),
        )),
    }
}

fn dnf_last_bool(repository: &DnfRepository, names: &[DnfOptionName]) -> NativeBool {
    repository
        .options
        .iter()
        .rev()
        .find(|option| names.contains(&option.name))
        .map_or(NativeBool::Missing, |option| {
            parse_native_bool(&option.raw_value, &option.location)
        })
}

fn zypper_last_bool(repository: &ZypperRepository, name: ZypperRepositoryOptionName) -> NativeBool {
    zypper_last_value(repository, name).map_or(NativeBool::Missing, |(value, location)| {
        parse_native_bool(value, location)
    })
}

fn zypper_last_value(
    repository: &ZypperRepository,
    name: ZypperRepositoryOptionName,
) -> Option<(&str, &DeclarationLocation)> {
    repository
        .options
        .iter()
        .rev()
        .find_map(|option| match option {
            ZypperRepositoryOption::Known {
                name: candidate,
                raw_value,
                location,
            } if *candidate == name => Some((raw_value.as_str(), location)),
            _ => None,
        })
}

fn parse_native_bool(value: &str, location: &DeclarationLocation) -> NativeBool {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "yes" | "true" | "on" => NativeBool::True,
        "0" | "no" | "false" | "off" => NativeBool::False(location.clone()),
        _ => NativeBool::Unsupported(value.to_string(), location.clone()),
    }
}
