// conary-core/src/packages/rpm_query/tests.rs

use super::*;

#[test]
fn installed_requirement_records_decode_typed_flags_and_canonicalize_empty_epoch() {
    let output = concat!(
        "device-mapper-libs\x1e8\x1e:1.02.212-2.fc44\x1f",
        "/bin/sh\x1e0\x1e\x1f",
    );

    let requirements = parse_rpm_requirement_records(output).unwrap();

    assert_eq!(requirements.len(), 2);
    assert_eq!(requirements[0].alternatives[0].name, "device-mapper-libs");
    assert_eq!(
        requirements[0].alternatives[0]
            .version_constraint
            .as_deref(),
        Some("= 1.02.212-2.fc44")
    );
    assert_eq!(requirements[1].alternatives[0].name, "/bin/sh");
    assert!(requirements[1].alternatives[0].version_constraint.is_none());
}

#[test]
fn installed_requirement_records_reject_misaligned_or_malformed_header_arrays() {
    assert!(parse_rpm_requirement_records("name\x1e0\x1f").is_err());
    assert!(parse_rpm_requirement_records("\x1e0\x1e\x1f").is_err());
    assert!(parse_rpm_requirement_records("name\x1enot-hex\x1e\x1f").is_err());
}

#[test]
fn installed_and_artifact_requirements_share_rich_dependency_decoding() {
    let output = "((feature-a with feature-b) if engine else fallback)\x1e8000000\x1e\x1f";
    let requirements = parse_rpm_requirement_records(output).unwrap();

    assert_eq!(requirements.len(), 1);
    assert!(requirements[0].expression.is_conditional());
    assert_eq!(requirements[0].alternatives.len(), 4);
}

#[test]
fn test_is_rpm_available() {
    // This test just ensures the function runs without panic
    let _ = is_rpm_available();
}

#[test]
fn owner_records_are_queryformat_data_not_human_message_matches() {
    assert_eq!(
        parse_owner_records("filesystem\x1fbash\x1f").unwrap(),
        vec!["filesystem", "bash"]
    );
    assert!(parse_owner_records("file is not owned by any package").is_err());
}

#[test]
fn dnf5_userinstalled_is_an_installed_only_selector_not_a_composable_filter() {
    let expected = HashSet::from(["fixture.x86_64".to_string()]);
    let mut calls = Vec::new();

    let packages = query_user_installed_with(|authority, command, args| {
        calls.push((
            authority,
            command.to_string(),
            args.iter()
                .map(|arg| (*arg).to_string())
                .collect::<Vec<_>>(),
        ));
        Ok(expected.clone())
    })
    .unwrap();

    assert_eq!(packages, expected);
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, "DNF5");
    assert_eq!(calls[0].1, "dnf5");
    assert!(calls[0].2.contains(&"--userinstalled".to_string()));
    assert!(!calls[0].2.contains(&"--installed".to_string()));
    assert_eq!(calls[0].2, DNF5_USER_INSTALLED_ARGS);
}

#[test]
fn only_a_missing_dnf5_command_falls_back_to_dnf4s_distinct_grammar() {
    let expected = HashSet::from(["fixture.x86_64".to_string()]);
    let mut calls = Vec::new();

    let packages = query_user_installed_with(|authority, command, args| {
        calls.push((
            authority,
            command.to_string(),
            args.iter()
                .map(|arg| (*arg).to_string())
                .collect::<Vec<_>>(),
        ));
        if command == "dnf5" {
            return Err(InstallReasonAuthorityError::CommandUnavailable {
                authority,
                command: command.to_string(),
            });
        }
        Ok(expected.clone())
    })
    .unwrap();

    assert_eq!(packages, expected);
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[1].0, "DNF4");
    assert_eq!(calls[1].1, "dnf");
    assert!(calls[1].2.contains(&"--installed".to_string()));
    assert!(calls[1].2.contains(&"--userinstalled".to_string()));
    assert_eq!(calls[1].2, DNF4_USER_INSTALLED_ARGS);
}

#[test]
fn a_present_but_failing_dnf5_remains_the_authority() {
    let mut calls = Vec::new();

    let error = query_user_installed_with(|authority, command, args| {
        calls.push((
            authority,
            command.to_string(),
            args.iter()
                .map(|arg| (*arg).to_string())
                .collect::<Vec<_>>(),
        ));
        Err(InstallReasonAuthorityError::CommandFailed {
            authority,
            command: command.to_string(),
            status: Some(2),
            stderr: "invalid selector composition".to_string(),
        })
    })
    .unwrap_err();

    assert!(matches!(
        error,
        InstallReasonAuthorityError::CommandFailed {
            authority: "DNF5",
            status: Some(2),
            ..
        }
    ));
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, "DNF5");
    assert_eq!(calls[0].1, "dnf5");
    assert_eq!(calls[0].2, DNF5_USER_INSTALLED_ARGS);
}

#[test]
fn test_installed_rpm_info_full_version() {
    let info = InstalledRpmInfo {
        name: "test".to_string(),
        version: "1.0.0".to_string(),
        release: "1.fc44".to_string(),
        epoch: Some(2),
        arch: "x86_64".to_string(),
        description: None,
        summary: None,
        license: None,
        url: None,
        vendor: None,
        source_rpm: None,
        build_host: None,
        install_time: None,
    };

    assert_eq!(info.full_version(), "2:1.0.0-1.fc44");
    assert_eq!(info.version_only(), "2:1.0.0");
}

#[test]
fn test_installed_rpm_info_no_epoch() {
    let info = InstalledRpmInfo {
        name: "test".to_string(),
        version: "1.0.0".to_string(),
        release: "1.fc44".to_string(),
        epoch: None,
        arch: "x86_64".to_string(),
        description: None,
        summary: None,
        license: None,
        url: None,
        vendor: None,
        source_rpm: None,
        build_host: None,
        install_time: None,
    };

    assert_eq!(info.full_version(), "1.0.0-1.fc44");
    assert_eq!(info.version_only(), "1.0.0");
}

#[test]
fn package_query_records_preserve_multiline_descriptions_and_variants() {
    let output = "fixture-1.2.3-4.fc44.x86_64\x1efixture\x1e1.2.3\x1e4.fc44\x1e(none)\x1ex86_64\x1efirst line\nsecond line\x1esummary\x1eMIT\x1ehttps://example.invalid\x1evendor\x1esource.src.rpm\x1ebuilder\x1e1\x1f\
                      fixture-1.2.3-4.fc44.aarch64\x1efixture\x1e1.2.3\x1e4.fc44\x1e(none)\x1eaarch64\x1edescription\x1esummary\x1eMIT\x1ehttps://example.invalid\x1evendor\x1esource.src.rpm\x1ebuilder\x1e1\x1f";
    let records = parse_package_query_records(output).unwrap();

    assert_eq!(records.len(), 2);
    assert_eq!(
        records[0].info.description.as_deref(),
        Some("first line\nsecond line")
    );
    assert_eq!(records[1].info.arch, "aarch64");
    assert_eq!(
        records[0].identity.selector(),
        "fixture-1.2.3-4.fc44.x86_64"
    );
    assert_eq!(
        records[1].identity.selector(),
        "fixture-1.2.3-4.fc44.aarch64"
    );
}

/// Every real Fedora system imports the distro signing key at install time,
/// so `rpm -qa` always emits at least one keyring record. Adoption failed on
/// the whole system because of it:
///
/// ```text
/// Error: Parse error: installed native package selector
/// "gpg-pubkey-36f612dcf27f7d1a48a835e4dbfcf71c6d9f90a6-6786af3b"
/// disagrees with its typed identity fields
/// ```
///
/// The record is verbatim from `fedora44-guest-v1`. It cannot satisfy the
/// RPM identity invariant `name-[epoch:]version-release.architecture`,
/// because RPM reports its architecture as `(none)` and renders its NEVRA
/// without an architecture suffix at all.
#[test]
fn the_rpm_keyring_is_not_part_of_the_installed_package_inventory() {
    let key = "gpg-pubkey-36f612dcf27f7d1a48a835e4dbfcf71c6d9f90a6-6786af3b\x1egpg-pubkey\x1e36f612dcf27f7d1a48a835e4dbfcf71c6d9f90a6\x1e6786af3b\x1e(none)\x1e(none)\x1eGnupg public key\x1egpg(Fedora (44))\x1epubkey\x1e(none)\x1e(none)\x1e(none)\x1e(none)\x1e1\x1f";
    let package = "bash-5.3.9-3.fc44.x86_64\x1ebash\x1e5.3.9\x1e3.fc44\x1e(none)\x1ex86_64\x1edescription\x1esummary\x1eGPLv3\x1ehttps://example.invalid\x1evendor\x1esource.src.rpm\x1ebuilder\x1e1\x1f";

    let records = parse_package_query_records(&format!("{key}{package}")).unwrap();

    assert_eq!(records.len(), 1, "the keyring record must not be inventory");
    assert_eq!(records[0].info.name, "bash");
}

/// The name alone does not license skipping a record. A `gpg-pubkey` header
/// carrying a real architecture is not the keyring shape this recognises,
/// and must still be parsed rather than silently vanish from the inventory.
#[test]
fn only_the_architecture_less_keyring_shape_is_skipped() {
    assert!(is_rpm_public_key_record("gpg-pubkey", "(none)"));
    assert!(is_rpm_public_key_record("gpg-pubkey", ""));
    assert!(!is_rpm_public_key_record("gpg-pubkey", "x86_64"));
    assert!(!is_rpm_public_key_record("bash", "(none)"));
}

#[test]
fn malformed_rpm_keyring_records_fail_before_classification() {
    let malformed_epoch = "gpg-pubkey-keyid-created\x1egpg-pubkey\x1ekeyid\x1ecreated\x1enot-an-epoch\x1e(none)\x1edescription\x1esummary\x1epubkey\x1e(none)\x1e(none)\x1e(none)\x1e(none)\x1e1\x1f";
    let mismatched_nevra = "gpg-pubkey-wrong-created\x1egpg-pubkey\x1ekeyid\x1ecreated\x1e(none)\x1e(none)\x1edescription\x1esummary\x1epubkey\x1e(none)\x1e(none)\x1e(none)\x1e(none)\x1e1\x1f";

    assert!(parse_package_query_records(malformed_epoch).is_err());
    assert!(parse_package_query_records(mismatched_nevra).is_err());
}

#[test]
fn package_inventory_rejects_malformed_or_duplicate_records() {
    assert!(parse_package_query_records("missing-fields\x1f").is_err());

    let record = "fixture-1-1.x86_64\x1efixture\x1e1\x1e1\x1e(none)\x1ex86_64\x1edescription\x1esummary\x1eMIT\x1ehttps://example.invalid\x1evendor\x1esource.src.rpm\x1ebuilder\x1e1\x1f";
    assert!(parse_package_query_records(&format!("{record}{record}")).is_err());
}

#[test]
fn file_query_uses_exact_parallel_array_records() {
    let records = parse_rpm_file_records(
            "/usr/lib/libfixture.so\x1e42\x1e1700000000\x1eabcdef12\x1e0120777\x1eroot\x1eroot\x1elibfixture.so.1\x1e0\x1e0\x1f\
             /usr/share/fixture data\x1e0\x1e1700000001\x1e\x1e040755\x1eroot\x1eroot\x1e\x1e40\x1e3\x1f",
        )
        .unwrap();

    assert_eq!(records.len(), 2);
    assert_eq!(records[0].path, "/usr/lib/libfixture.so");
    assert_eq!(records[0].link_target.as_deref(), Some("libfixture.so.1"));
    assert_eq!(records[1].path, "/usr/share/fixture data");
    assert!(records[1].digest.is_none());
    assert_eq!(records[1].mode, 0o40755);
    assert_eq!(
        records[0].absence_policy,
        InstalledFileAbsencePolicy::Required
    );
    assert_eq!(
        records[1].absence_policy,
        InstalledFileAbsencePolicy::RpmGhost
    );

    let zero_digest = parse_rpm_file_records(
            "/usr/bin/zero\x1e1\x1e1700000002\x1e00000000\x1e0100755\x1eroot\x1eroot\x1e\x1e48\x1e0\x1f",
        )
        .unwrap();
    assert_eq!(zero_digest[0].digest.as_deref(), Some("00000000"));
    assert_eq!(
        zero_digest[0].absence_policy,
        InstalledFileAbsencePolicy::RpmGhostAndMissingOk
    );

    let missing_ok = parse_rpm_file_records(
        "/etc/optional\x1e1\x1e1700000002\x1e00000000\x1e0100644\x1eroot\x1eroot\x1e\x1e8\x1e0\x1f",
    )
    .unwrap();
    assert_eq!(
        missing_ok[0].absence_policy,
        InstalledFileAbsencePolicy::RpmMissingOk
    );
}

#[test]
fn file_query_uses_rpms_persisted_installation_state_as_live_authority() {
    let records = parse_rpm_file_records(
        "/normal\x1e1\x1e1\x1e00\x1e0100644\x1eroot\x1eroot\x1e\x1e0\x1e0\x1f\
             /replaced\x1e1\x1e1\x1e00\x1e0100644\x1eroot\x1eroot\x1e\x1e0\x1e1\x1f\
             /not-installed\x1e1\x1e1\x1e00\x1e0100644\x1eroot\x1eroot\x1e\x1e0\x1e2\x1f\
             /net-shared\x1e1\x1e1\x1e00\x1e0100644\x1eroot\x1eroot\x1e\x1e0\x1e3\x1f\
             /wrong-color\x1e1\x1e1\x1e00\x1e0100644\x1eroot\x1eroot\x1e\x1e0\x1e4\x1f",
    )
    .unwrap();

    assert_eq!(
        records
            .iter()
            .map(|record| record.path.as_str())
            .collect::<Vec<_>>(),
        ["/normal", "/net-shared"]
    );
}

#[test]
fn file_query_rejects_malformed_parallel_array_records() {
    assert!(parse_rpm_file_records("/usr/bin/fixture\x1e42\x1f").is_err());
    assert!(
            parse_rpm_file_records(
                "/usr/bin/fixture\x1e42\x1e1700000000\x1enot-a-digest\x1e0100755\x1eroot\x1eroot\x1e\x1e0\x1e0\x1f"
            )
            .is_err()
        );
    assert!(
            parse_rpm_file_records(
                "/usr/bin/fixture\x1e42\x1e1700000000\x1eabcdef12\x1e0100755\x1eroot\x1eroot\x1e\x1enot-hex\x1e0\x1f"
            )
            .is_err()
        );
    assert!(
            parse_rpm_file_records(
                "/usr/bin/fixture\x1e42\x1e1700000000\x1eabcdef12\x1e0100755\x1eroot\x1eroot\x1e\x1e0\x1e5\x1f"
            )
            .is_err()
        );
}
