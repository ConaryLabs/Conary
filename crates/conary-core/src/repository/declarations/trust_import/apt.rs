// conary-core/src/repository/declarations/trust_import/apt.rs

use super::openpgp::collect_declared_roots;
use super::{
    NativeRepositoryTrustPlan, PlanFindings, TrustImportFindingKind, apt_reference, finding,
};
use crate::repository::TrustRole;
use crate::repository::declarations::apt::{AptOption, AptOptionName, AptSourceEntry};
use std::path::Path;

pub(super) fn plan(
    selected_root: &Path,
    document: &Path,
    entry_index: usize,
    entry: &AptSourceEntry,
) -> NativeRepositoryTrustPlan {
    let mut findings = PlanFindings::default();
    reject_bypasses(entry, &mut findings);
    let signed_by: Vec<String> = entry
        .options
        .iter()
        .filter(|option| option.name == AptOptionName::SignedBy)
        .flat_map(|option| option.values.iter().cloned())
        .collect();
    let evidence = if signed_by.is_empty() {
        findings.ambiguous(finding(
            TrustImportFindingKind::ImplicitGlobalAuthority,
            Some(TrustRole::DebianRelease),
            &entry.location,
            "APT source has no Signed-By binding and therefore inherits global native trust",
        ));
        Vec::new()
    } else {
        collect_declared_roots(
            selected_root,
            TrustRole::DebianRelease,
            &signed_by,
            &entry.location,
            &mut findings,
        )
        .evidence
    };
    if evidence.is_empty() && findings.is_empty() {
        findings.ambiguous(finding(
            TrustImportFindingKind::MissingRequiredAuthority,
            Some(TrustRole::DebianRelease),
            &entry.location,
            "APT source has no importable Release-signing certificate authority",
        ));
    }
    NativeRepositoryTrustPlan {
        repository: apt_reference(document, entry_index, entry),
        enabled: Some(entry.enabled),
        evidence,
        disposition: findings.finish(),
    }
}

fn reject_bypasses(entry: &AptSourceEntry, findings: &mut PlanFindings) {
    for option in &entry.options {
        let is_verification_option = matches!(
            option.name,
            AptOptionName::Trusted
                | AptOptionName::AllowInsecure
                | AptOptionName::AllowWeak
                | AptOptionName::AllowDowngradeToInsecure
        );
        if is_verification_option {
            match native_bool(option) {
                Some(true) => findings.unsupported(finding(
                    TrustImportFindingKind::VerificationDisabled,
                    Some(TrustRole::DebianRelease),
                    &option.location,
                    format!(
                        "APT option '{:?}' weakens authenticated Release authority",
                        option.name
                    ),
                )),
                Some(false) => {}
                None => findings.unsupported(finding(
                    TrustImportFindingKind::UnsupportedTrustValue,
                    Some(TrustRole::DebianRelease),
                    &option.location,
                    format!(
                        "APT option '{:?}' uses unsupported trust value '{}'",
                        option.name, option.raw_value
                    ),
                )),
            }
        }
    }
}

fn native_bool(option: &AptOption) -> Option<bool> {
    match option.raw_value.trim().to_ascii_lowercase().as_str() {
        "1" | "yes" | "true" | "on" => Some(true),
        "0" | "no" | "false" | "off" => Some(false),
        _ => None,
    }
}
