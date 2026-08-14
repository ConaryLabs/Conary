// conary-core/src/repository/declarations/trust_import/tests.rs

use super::*;
use crate::repository::declarations::discover_selected_root;
use openpgp::cert::prelude::CertBuilder;
use openpgp::serialize::Serialize;
use sequoia_openpgp as openpgp;
use std::fs;
use std::path::Path;

fn certificate() -> (Vec<u8>, String) {
    let (certificate, _) = CertBuilder::new()
        .add_userid("Native repository fixture")
        .generate()
        .unwrap();
    let fingerprint = certificate.fingerprint().to_hex();
    let mut bytes = Vec::new();
    certificate.serialize(&mut bytes).unwrap();
    (bytes, fingerprint)
}

fn armored_certificate() -> (String, String) {
    let (certificate, _) = CertBuilder::new()
        .add_userid("Embedded native repository fixture")
        .generate()
        .unwrap();
    let fingerprint = certificate.fingerprint().to_hex();
    let mut bytes = Vec::new();
    certificate.armored().serialize(&mut bytes).unwrap();
    (String::from_utf8(bytes).unwrap(), fingerprint)
}

fn prepare(root: &Path) {
    for directory in [
        "etc/apt/sources.list.d",
        "etc/apt/keyrings",
        "etc/yum.repos.d",
        "etc/pki/rpm-gpg",
        "etc/zypp/repos.d",
    ] {
        fs::create_dir_all(root.join(directory)).unwrap();
    }
}

#[test]
fn apt_local_signed_by_is_importable_with_exact_fingerprint() {
    let root = tempfile::tempdir().unwrap();
    prepare(root.path());
    let (certificate, fingerprint) = certificate();
    fs::write(root.path().join("etc/apt/keyrings/vendor.gpg"), certificate).unwrap();
    fs::write(
        root.path().join("etc/apt/sources.list.d/vendor.list"),
        format!(
            "deb [signed-by=/etc/apt/keyrings/vendor.gpg,{fingerprint}!] https://apt.example stable main\n"
        ),
    )
    .unwrap();

    let declarations = discover_selected_root(root.path()).unwrap();
    let plan = plan_selected_root_trust(&declarations);
    assert_eq!(plan.repositories.len(), 1);
    assert_eq!(
        plan.repositories[0].disposition,
        TrustImportDisposition::Importable
    );
    assert_eq!(
        plan.repositories[0].evidence[0].certificates[0].certificate_fingerprint,
        fingerprint
    );
    assert_eq!(
        plan.repositories[0].evidence[0].fingerprint_selectors,
        [OpenPgpFingerprintSelector {
            fingerprint: fingerprint.clone(),
            primary_only: true,
        }]
    );
    assert_eq!(
        plan.repositories[0].evidence[0].role,
        TrustRole::DebianRelease
    );
}

#[test]
fn apt_global_and_remote_authority_are_ambiguous_not_imported() {
    let root = tempfile::tempdir().unwrap();
    prepare(root.path());
    fs::write(
        root.path().join("etc/apt/sources.list.d/vendor.list"),
        "deb https://apt.example stable main\n\
         deb [signed-by=https://keys.example/vendor.asc] https://other.example stable main\n",
    )
    .unwrap();

    let plan = plan_selected_root_trust(&discover_selected_root(root.path()).unwrap());
    assert!(plan.repositories.iter().all(|repository| matches!(
        repository.disposition,
        TrustImportDisposition::Ambiguous { .. }
    )));
}

#[test]
fn apt_deb822_embedded_key_unescapes_blank_lines() {
    let root = tempfile::tempdir().unwrap();
    prepare(root.path());
    let (armor, fingerprint) = armored_certificate();
    let continuation = armor
        .lines()
        .map(|line| {
            if line.is_empty() {
                " .".to_string()
            } else {
                format!(" {line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(
        root.path().join("etc/apt/sources.list.d/vendor.sources"),
        format!(
            "Types: deb\nURIs: https://apt.example\nSuites: stable\nComponents: main\nSigned-By:\n{continuation}\n"
        ),
    )
    .unwrap();

    let plan = plan_selected_root_trust(&discover_selected_root(root.path()).unwrap());
    assert_eq!(
        plan.repositories[0].disposition,
        TrustImportDisposition::Importable
    );
    assert_eq!(
        plan.repositories[0].evidence[0].certificates[0].certificate_fingerprint,
        fingerprint
    );
    assert!(matches!(
        plan.repositories[0].evidence[0].source,
        TrustEvidenceSource::EmbeddedOpenPgp
    ));
}

#[test]
fn dnf_metalink_and_local_package_key_keep_roles_separate() {
    let root = tempfile::tempdir().unwrap();
    prepare(root.path());
    let (certificate, fingerprint) = certificate();
    fs::write(root.path().join("etc/pki/rpm-gpg/VENDOR"), certificate).unwrap();
    fs::write(
        root.path().join("etc/yum.repos.d/vendor.repo"),
        "[vendor]\nenabled=1\nmetalink=https://mirrors.example/metalink\ngpgcheck=1\nrepo_gpgcheck=0\ngpgkey=file:///etc/pki/rpm-gpg/VENDOR\n",
    )
    .unwrap();

    let plan = plan_selected_root_trust(&discover_selected_root(root.path()).unwrap());
    let repository = &plan.repositories[0];
    assert_eq!(repository.disposition, TrustImportDisposition::Importable);
    assert!(repository.evidence.iter().any(|item| {
        item.role == TrustRole::RpmMetadata
            && matches!(item.source, TrustEvidenceSource::RpmMetalink { .. })
    }));
    assert!(repository.evidence.iter().any(|item| {
        item.role == TrustRole::RpmPackage
            && item.certificates[0].certificate_fingerprint == fingerprint
    }));
}

#[test]
fn dnf_repeated_gpgkey_lines_append_exact_sources_for_each_role() {
    let root = tempfile::tempdir().unwrap();
    prepare(root.path());
    let (first, first_fingerprint) = certificate();
    let (second, second_fingerprint) = certificate();
    fs::write(root.path().join("etc/pki/rpm-gpg/FIRST"), first).unwrap();
    fs::write(root.path().join("etc/pki/rpm-gpg/SECOND"), second).unwrap();
    fs::write(
        root.path().join("etc/yum.repos.d/vendor.repo"),
        "[vendor]\ngpgcheck=1\nrepo_gpgcheck=1\ngpgkey=file:///etc/pki/rpm-gpg/FIRST\ngpgkey=file:///etc/pki/rpm-gpg/SECOND\n",
    )
    .unwrap();

    let plan = plan_selected_root_trust(&discover_selected_root(root.path()).unwrap());
    let repository = &plan.repositories[0];
    assert_eq!(repository.disposition, TrustImportDisposition::Importable);
    for role in [TrustRole::RpmMetadata, TrustRole::RpmPackage] {
        let role_evidence: Vec<_> = repository
            .evidence
            .iter()
            .filter(|item| item.role == role)
            .collect();
        assert_eq!(role_evidence.len(), 2);
        assert_eq!(
            role_evidence
                .iter()
                .map(|item| item.certificates[0].certificate_fingerprint.as_str())
                .collect::<Vec<_>>(),
            [first_fingerprint.as_str(), second_fingerprint.as_str()]
        );
        assert_eq!(
            role_evidence
                .iter()
                .map(|item| item.location.line)
                .collect::<Vec<_>>(),
            [4, 5]
        );
    }
}

#[test]
fn rpm_metalink_outside_persisted_transport_contract_is_unsupported() {
    let root = tempfile::tempdir().unwrap();
    prepare(root.path());
    let (certificate, _) = certificate();
    fs::write(root.path().join("etc/pki/rpm-gpg/VENDOR"), certificate).unwrap();
    fs::write(
        root.path().join("etc/yum.repos.d/vendor.repo"),
        "[vendor]\nmetalink=http://mirrors.example/metalink\ngpgcheck=1\nrepo_gpgcheck=0\ngpgkey=file:///etc/pki/rpm-gpg/VENDOR\n",
    )
    .unwrap();

    let plan = plan_selected_root_trust(&discover_selected_root(root.path()).unwrap());
    assert!(matches!(
        plan.repositories[0].disposition,
        TrustImportDisposition::Unsupported { ref findings }
            if findings.iter().any(|finding| finding.kind == TrustImportFindingKind::UnsupportedTrustValue
                && finding.role == Some(TrustRole::RpmMetadata))
    ));
}

#[cfg(unix)]
#[test]
fn file_metalink_cannot_escape_selected_root() {
    use std::os::unix::fs::symlink;

    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    prepare(root.path());
    let (certificate, _) = certificate();
    fs::write(root.path().join("etc/pki/rpm-gpg/VENDOR"), certificate).unwrap();
    fs::write(outside.path().join("metalink.xml"), "outside").unwrap();
    symlink(outside.path(), root.path().join("etc/metalink")).unwrap();
    fs::write(
        root.path().join("etc/yum.repos.d/vendor.repo"),
        "[vendor]\nmetalink=file:///etc/metalink/metalink.xml\ngpgcheck=1\nrepo_gpgcheck=0\ngpgkey=file:///etc/pki/rpm-gpg/VENDOR\n",
    )
    .unwrap();

    let plan = plan_selected_root_trust(&discover_selected_root(root.path()).unwrap());
    assert!(matches!(
        plan.repositories[0].disposition,
        TrustImportDisposition::Unsupported { ref findings }
            if findings.iter().any(|finding| finding.kind == TrustImportFindingKind::SelectedRootEscape
                && finding.role == Some(TrustRole::RpmMetadata))
    ));
}

#[test]
fn zypper_openpgp_metadata_and_package_roles_are_explicit() {
    let root = tempfile::tempdir().unwrap();
    prepare(root.path());
    let (certificate, fingerprint) = certificate();
    fs::write(root.path().join("etc/pki/rpm-gpg/VENDOR"), certificate).unwrap();
    fs::write(
        root.path().join("etc/zypp/repos.d/vendor.repo"),
        "[vendor]\nenabled=0\nbaseurl=https://rpm.example\nrepo_gpgcheck=1\npkg_gpgcheck=1\ngpgkey=file:///etc/pki/rpm-gpg/VENDOR\n",
    )
    .unwrap();

    let plan = plan_selected_root_trust(&discover_selected_root(root.path()).unwrap());
    let repository = &plan.repositories[0];
    assert_eq!(repository.enabled, Some(false));
    assert_eq!(repository.disposition, TrustImportDisposition::Importable);
    for role in [TrustRole::RpmMetadata, TrustRole::RpmPackage] {
        assert!(repository.evidence.iter().any(|item| {
            item.role == role && item.certificates[0].certificate_fingerprint == fingerprint
        }));
    }
}

#[test]
fn alpm_safe_siglevel_is_ambiguous_until_keyring_is_bound() {
    let root = tempfile::tempdir().unwrap();
    prepare(root.path());
    fs::write(
        root.path().join("etc/pacman.conf"),
        "[options]\nSigLevel=Required DatabaseOptional\n[core]\nServer=https://mirror.example/$repo/$arch\n",
    )
    .unwrap();

    let plan = plan_selected_root_trust(&discover_selected_root(root.path()).unwrap());
    assert!(matches!(
        plan.repositories[0].disposition,
        TrustImportDisposition::Ambiguous { ref findings }
            if findings.iter().any(|finding| finding.kind == TrustImportFindingKind::AlpmKeyringBindingMissing)
    ));
}

#[test]
fn unsafe_native_verification_modes_are_unsupported() {
    let root = tempfile::tempdir().unwrap();
    prepare(root.path());
    fs::write(
        root.path().join("etc/pacman.conf"),
        "[options]\nSigLevel=Required TrustAll\n[core]\nServer=https://mirror.example/$repo/$arch\n",
    )
    .unwrap();

    let plan = plan_selected_root_trust(&discover_selected_root(root.path()).unwrap());
    assert!(matches!(
        plan.repositories[0].disposition,
        TrustImportDisposition::Unsupported { ref findings }
            if findings.iter().any(|finding| finding.kind == TrustImportFindingKind::TrustAll)
    ));
}

#[test]
fn malformed_apt_trust_bypass_value_is_unsupported() {
    let root = tempfile::tempdir().unwrap();
    prepare(root.path());
    let (certificate, _) = certificate();
    fs::write(root.path().join("etc/apt/keyrings/vendor.gpg"), certificate).unwrap();
    fs::write(
        root.path().join("etc/apt/sources.list.d/vendor.list"),
        "deb [trusted=maybe signed-by=/etc/apt/keyrings/vendor.gpg] https://apt.example stable main\n",
    )
    .unwrap();

    let plan = plan_selected_root_trust(&discover_selected_root(root.path()).unwrap());
    assert!(matches!(
        plan.repositories[0].disposition,
        TrustImportDisposition::Unsupported { ref findings }
            if findings.iter().any(|finding| finding.kind == TrustImportFindingKind::UnsupportedTrustValue)
    ));
}

#[cfg(unix)]
#[test]
fn declared_key_symlink_cannot_escape_selected_root() {
    use std::os::unix::fs::symlink;

    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    prepare(root.path());
    let (certificate, _) = certificate();
    fs::write(outside.path().join("vendor.gpg"), certificate).unwrap();
    fs::remove_dir(root.path().join("etc/apt/keyrings")).unwrap();
    symlink(outside.path(), root.path().join("etc/apt/keyrings")).unwrap();
    fs::write(
        root.path().join("etc/apt/sources.list.d/vendor.list"),
        "deb [signed-by=/etc/apt/keyrings/vendor.gpg] https://apt.example stable main\n",
    )
    .unwrap();

    let plan = plan_selected_root_trust(&discover_selected_root(root.path()).unwrap());
    assert!(matches!(
        plan.repositories[0].disposition,
        TrustImportDisposition::Unsupported { ref findings }
            if findings.iter().any(|finding| finding.kind == TrustImportFindingKind::SelectedRootEscape)
    ));
}

#[test]
fn preview_json_rejects_unknown_fields() {
    let root = tempfile::tempdir().unwrap();
    prepare(root.path());
    let plan = plan_selected_root_trust(&discover_selected_root(root.path()).unwrap());
    let mut value = serde_json::to_value(plan).unwrap();
    value
        .as_object_mut()
        .unwrap()
        .insert("future".to_string(), serde_json::Value::Bool(true));
    assert!(serde_json::from_value::<NativeTrustImportPlan>(value).is_err());
}

#[test]
fn repeated_planning_serializes_identically() {
    let root = tempfile::tempdir().unwrap();
    prepare(root.path());
    let (certificate, _) = certificate();
    fs::write(root.path().join("etc/apt/keyrings/vendor.gpg"), certificate).unwrap();
    fs::write(
        root.path().join("etc/apt/sources.list.d/vendor.list"),
        "deb [signed-by=/etc/apt/keyrings/vendor.gpg] https://apt.example stable main\n",
    )
    .unwrap();

    let declarations = discover_selected_root(root.path()).unwrap();
    let first = serde_json::to_vec(&plan_selected_root_trust(&declarations)).unwrap();
    let second = serde_json::to_vec(&plan_selected_root_trust(&declarations)).unwrap();
    assert_eq!(first, second);
}
