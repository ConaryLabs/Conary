// conary-core/src/packages/deb/dpkg_lifecycle/tests/dpkg.rs

use super::*;

fn round_trip_owned(values: Vec<Vec<u8>>) -> DpkgInvocation {
    let parsed = DpkgInvocation::parse(&values).unwrap();
    assert_eq!(DpkgInvocation::parse(&parsed.argv()).unwrap(), parsed);
    parsed
}

fn round_trip(values: &[&[u8]]) -> DpkgInvocation {
    round_trip_owned(argv(values))
}

fn text_round_trip(values: &[&str]) -> DpkgInvocation {
    round_trip_owned(text_argv(values))
}

fn assert_read_only(values: &[&str], expected: DpkgReadOnlyAction) {
    assert_eq!(
        text_round_trip(values).disposition,
        DpkgDisposition::ReadOnly(expected)
    );
}

fn assert_mutation(values: &[&str], expected: DpkgMutationAction, class: DpkgMutationClass) {
    assert_eq!(expected.class(), class);
    assert_eq!(
        text_round_trip(values).disposition,
        DpkgDisposition::RecursiveMutation(expected)
    );
}

fn assert_backend(values: &[&str], expected: DpkgArchiveBackendAction) {
    assert_eq!(
        text_round_trip(values).disposition,
        DpkgDisposition::ArchiveBackendDelegation(expected)
    );
}

#[test]
fn all_version_relations_round_trip_with_exact_obsolete_markers() {
    for relation in VersionRelation::ALL {
        let parsed = round_trip_owned(vec![
            b"--compare-versions".to_vec(),
            b"1:2.0~rc1-1".to_vec(),
            relation.as_bytes().to_vec(),
            b"2.0-1".to_vec(),
        ]);
        assert_eq!(
            parsed.disposition,
            DpkgDisposition::ReadOnly(DpkgReadOnlyAction::CompareVersions {
                left: b"1:2.0~rc1-1".to_vec(),
                relation,
                right: b"2.0-1".to_vec(),
            })
        );
    }
    assert_eq!(
        VersionRelation::ALL
            .into_iter()
            .filter(|relation| relation.is_obsolete())
            .collect::<Vec<_>>(),
        vec![
            VersionRelation::ControlLessObsolete,
            VersionRelation::ControlGreaterObsolete,
        ]
    );
}

#[test]
fn all_assertion_and_validation_tables_round_trip() {
    for feature in DpkgAssertFeature::ALL {
        let mut option = b"--assert-".to_vec();
        option.extend_from_slice(feature.as_bytes());
        assert_eq!(
            round_trip_owned(vec![option]).disposition,
            DpkgDisposition::ReadOnly(DpkgReadOnlyAction::Assert(feature))
        );
    }
    for kind in DpkgValidationKind::ALL {
        let mut option = b"--validate-".to_vec();
        option.extend_from_slice(kind.as_bytes());
        assert_eq!(
            round_trip_owned(vec![option, b"value-with-\xff".to_vec()]).disposition,
            DpkgDisposition::ReadOnly(DpkgReadOnlyAction::Validate {
                kind,
                value: b"value-with-\xff".to_vec(),
            })
        );
    }
}

#[test]
fn every_read_only_action_and_alias_has_a_non_mutating_disposition() {
    assert_read_only(
        &["-V", "demo"],
        DpkgReadOnlyAction::Verify(text_argv(&["demo"])),
    );
    assert_read_only(&["--verify"], DpkgReadOnlyAction::Verify(Vec::new()));
    assert_read_only(
        &["-L", "dpkg"],
        DpkgReadOnlyAction::Query(DpkgQueryAction::ListFiles {
            packages: text_argv(&["dpkg"]),
        }),
    );
    assert_read_only(
        &["--listfiles", "dpkg"],
        DpkgReadOnlyAction::Query(DpkgQueryAction::ListFiles {
            packages: text_argv(&["dpkg"]),
        }),
    );
    assert_read_only(
        &["-s", "dpkg"],
        DpkgReadOnlyAction::Query(DpkgQueryAction::Status {
            packages: text_argv(&["dpkg"]),
        }),
    );
    assert_read_only(
        &["--status"],
        DpkgReadOnlyAction::Query(DpkgQueryAction::Status {
            packages: Vec::new(),
        }),
    );
    assert_read_only(
        &["--get-selections", "lib*"],
        DpkgReadOnlyAction::GetSelections(text_argv(&["lib*"])),
    );
    assert_read_only(
        &["-C", "demo"],
        DpkgReadOnlyAction::Audit(text_argv(&["demo"])),
    );
    assert_read_only(&["--audit"], DpkgReadOnlyAction::Audit(Vec::new()));
    assert_read_only(&["--yet-to-unpack"], DpkgReadOnlyAction::YetToUnpack);
    assert_read_only(
        &["--predep-package"],
        DpkgReadOnlyAction::PredependencyPackage,
    );
    assert_read_only(
        &["--print-architecture"],
        DpkgReadOnlyAction::PrintArchitecture,
    );
    assert_read_only(
        &["--print-foreign-architectures"],
        DpkgReadOnlyAction::PrintForeignArchitectures,
    );
    assert_read_only(
        &["-l", "lib*"],
        DpkgReadOnlyAction::Query(DpkgQueryAction::List {
            patterns: text_argv(&["lib*"]),
        }),
    );
    assert_read_only(
        &["--list"],
        DpkgReadOnlyAction::Query(DpkgQueryAction::List {
            patterns: Vec::new(),
        }),
    );
    assert_read_only(
        &["-S", "/usr/bin/dpkg"],
        DpkgReadOnlyAction::Query(DpkgQueryAction::Search {
            patterns: text_argv(&["/usr/bin/dpkg"]),
        }),
    );
    assert_read_only(
        &["--search", "*.service"],
        DpkgReadOnlyAction::Query(DpkgQueryAction::Search {
            patterns: text_argv(&["*.service"]),
        }),
    );
    assert_read_only(
        &["-p", "dpkg"],
        DpkgReadOnlyAction::Query(DpkgQueryAction::PrintAvailable {
            packages: text_argv(&["dpkg"]),
        }),
    );
    assert_read_only(
        &["--print-avail"],
        DpkgReadOnlyAction::Query(DpkgQueryAction::PrintAvailable {
            packages: Vec::new(),
        }),
    );
    assert_read_only(&["-?"], DpkgReadOnlyAction::Help);
    assert_read_only(&["--help"], DpkgReadOnlyAction::Help);
    assert_read_only(&["--force-help"], DpkgReadOnlyAction::ForceHelp);
    assert_read_only(&["-Dh"], DpkgReadOnlyAction::DebugHelp);
    assert_read_only(&["--debug=help"], DpkgReadOnlyAction::DebugHelp);
    assert_read_only(&["--version"], DpkgReadOnlyAction::Version);
}

#[test]
fn every_recursive_mutation_action_and_alias_has_an_exact_class() {
    let archives = text_argv(&["one.deb", "two.deb"]);
    assert_mutation(
        &["-i", "one.deb", "two.deb"],
        DpkgMutationAction::Install(archives.clone()),
        DpkgMutationClass::PackageTransaction,
    );
    assert_mutation(
        &["--install", "one.deb"],
        DpkgMutationAction::Install(text_argv(&["one.deb"])),
        DpkgMutationClass::PackageTransaction,
    );
    assert_mutation(
        &["--unpack", "one.deb"],
        DpkgMutationAction::Unpack(text_argv(&["one.deb"])),
        DpkgMutationClass::PackageTransaction,
    );
    assert_mutation(
        &["-A", "one.deb"],
        DpkgMutationAction::RecordAvailable(text_argv(&["one.deb"])),
        DpkgMutationClass::PackageDatabase,
    );
    assert_mutation(
        &["--record-avail", "one.deb"],
        DpkgMutationAction::RecordAvailable(text_argv(&["one.deb"])),
        DpkgMutationClass::PackageDatabase,
    );
    assert_mutation(
        &["--configure", "demo"],
        DpkgMutationAction::Configure(text_argv(&["demo"])),
        DpkgMutationClass::PackageTransaction,
    );
    assert_mutation(
        &["-a", "--configure"],
        DpkgMutationAction::Configure(Vec::new()),
        DpkgMutationClass::PackageTransaction,
    );
    assert_mutation(
        &["-r", "demo"],
        DpkgMutationAction::Remove(text_argv(&["demo"])),
        DpkgMutationClass::PackageTransaction,
    );
    assert_mutation(
        &["--remove", "demo"],
        DpkgMutationAction::Remove(text_argv(&["demo"])),
        DpkgMutationClass::PackageTransaction,
    );
    assert_mutation(
        &["-P", "demo"],
        DpkgMutationAction::Purge(text_argv(&["demo"])),
        DpkgMutationClass::PackageTransaction,
    );
    assert_mutation(
        &["--purge", "demo"],
        DpkgMutationAction::Purge(text_argv(&["demo"])),
        DpkgMutationClass::PackageTransaction,
    );
    assert_mutation(
        &["--triggers-only", "demo"],
        DpkgMutationAction::ProcessTriggers(text_argv(&["demo"])),
        DpkgMutationClass::TriggerProcessing,
    );
    assert_mutation(
        &["--pending", "--triggers-only"],
        DpkgMutationAction::ProcessTriggers(Vec::new()),
        DpkgMutationClass::TriggerProcessing,
    );
    assert_mutation(
        &["--set-selections"],
        DpkgMutationAction::SetSelections,
        DpkgMutationClass::PackageDatabase,
    );
    assert_mutation(
        &["--clear-selections"],
        DpkgMutationAction::ClearSelections,
        DpkgMutationClass::PackageDatabase,
    );
    assert_mutation(
        &["--update-avail"],
        DpkgMutationAction::UpdateAvailable(None),
        DpkgMutationClass::PackageDatabase,
    );
    assert_mutation(
        &["--update-avail", "Packages"],
        DpkgMutationAction::UpdateAvailable(Some(b"Packages".to_vec())),
        DpkgMutationClass::PackageDatabase,
    );
    assert_mutation(
        &["--merge-avail"],
        DpkgMutationAction::MergeAvailable(None),
        DpkgMutationClass::PackageDatabase,
    );
    assert_mutation(
        &["--merge-avail", "Packages"],
        DpkgMutationAction::MergeAvailable(Some(b"Packages".to_vec())),
        DpkgMutationClass::PackageDatabase,
    );
    assert_mutation(
        &["--clear-avail"],
        DpkgMutationAction::ClearAvailable,
        DpkgMutationClass::PackageDatabase,
    );
    assert_mutation(
        &["--forget-old-unavail"],
        DpkgMutationAction::ForgetOldUnavailable,
        DpkgMutationClass::PackageDatabase,
    );
    assert_mutation(
        &["--add-architecture", "arm64"],
        DpkgMutationAction::AddArchitecture(b"arm64".to_vec()),
        DpkgMutationClass::ArchitectureConfiguration,
    );
    assert_mutation(
        &["--remove-architecture", "arm64"],
        DpkgMutationAction::RemoveArchitecture(b"arm64".to_vec()),
        DpkgMutationClass::ArchitectureConfiguration,
    );

    assert!(DpkgMutationAction::SetSelections.reads_stdin());
    assert!(!DpkgMutationAction::ClearSelections.reads_stdin());
    assert!(matches!(
        text_round_trip(&["--no-act", "--install", "one.deb"]).disposition,
        DpkgDisposition::RecursiveMutation(DpkgMutationAction::Install(_))
    ));
}

#[test]
fn every_archive_backend_action_and_alias_is_explicit_delegation() {
    assert_backend(
        &["-b", "tree"],
        DpkgArchiveBackendAction::Build(text_argv(&["tree"])),
    );
    assert_backend(
        &["--build", "tree", "demo.deb"],
        DpkgArchiveBackendAction::Build(text_argv(&["tree", "demo.deb"])),
    );
    assert_backend(
        &["-c", "demo.deb"],
        DpkgArchiveBackendAction::Contents(b"demo.deb".to_vec()),
    );
    assert_backend(
        &["--contents", "demo.deb"],
        DpkgArchiveBackendAction::Contents(b"demo.deb".to_vec()),
    );
    assert_backend(
        &["-e", "demo.deb"],
        DpkgArchiveBackendAction::Control(text_argv(&["demo.deb"])),
    );
    assert_backend(
        &["--control", "demo.deb", "DEBIAN"],
        DpkgArchiveBackendAction::Control(text_argv(&["demo.deb", "DEBIAN"])),
    );
    assert_backend(
        &["-x", "demo.deb", "root"],
        DpkgArchiveBackendAction::Extract {
            archive: b"demo.deb".to_vec(),
            directory: b"root".to_vec(),
        },
    );
    assert_backend(
        &["--extract", "demo.deb", "root"],
        DpkgArchiveBackendAction::Extract {
            archive: b"demo.deb".to_vec(),
            directory: b"root".to_vec(),
        },
    );
    assert_backend(
        &["-X", "demo.deb", "root"],
        DpkgArchiveBackendAction::VerboseExtract {
            archive: b"demo.deb".to_vec(),
            directory: b"root".to_vec(),
        },
    );
    assert_backend(
        &["--vextract", "demo.deb", "root"],
        DpkgArchiveBackendAction::VerboseExtract {
            archive: b"demo.deb".to_vec(),
            directory: b"root".to_vec(),
        },
    );
    assert_backend(
        &["-f", "demo.deb", "Package", "Version"],
        DpkgArchiveBackendAction::Field(text_argv(&["demo.deb", "Package", "Version"])),
    );
    assert_backend(
        &["--field", "demo.deb"],
        DpkgArchiveBackendAction::Field(text_argv(&["demo.deb"])),
    );
    assert_backend(
        &["--ctrl-tarfile", "demo.deb"],
        DpkgArchiveBackendAction::ControlTar(b"demo.deb".to_vec()),
    );
    assert_backend(
        &["--fsys-tarfile", "demo.deb"],
        DpkgArchiveBackendAction::FilesystemTar(b"demo.deb".to_vec()),
    );
    assert_backend(
        &["-I", "demo.deb", "control"],
        DpkgArchiveBackendAction::Info(text_argv(&["demo.deb", "control"])),
    );
    assert_backend(
        &["--info", "demo.deb"],
        DpkgArchiveBackendAction::Info(text_argv(&["demo.deb"])),
    );
}

#[test]
fn every_fixed_option_spelling_is_typed() {
    for (spelling, expected) in [
        (b"-B".as_slice(), DpkgOption::AutoDeconfigure),
        (
            b"--auto-deconfigure".as_slice(),
            DpkgOption::AutoDeconfigure,
        ),
        (b"--no-act".as_slice(), DpkgOption::NoAct),
        (b"--dry-run".as_slice(), DpkgOption::NoAct),
        (b"--simulate".as_slice(), DpkgOption::NoAct),
        (b"-R".as_slice(), DpkgOption::Recursive),
        (b"--recursive".as_slice(), DpkgOption::Recursive),
        (b"-G".as_slice(), DpkgOption::RefuseDowngrade),
        (b"-O".as_slice(), DpkgOption::SelectedOnly),
        (b"--selected-only".as_slice(), DpkgOption::SelectedOnly),
        (b"-E".as_slice(), DpkgOption::SkipSameVersion),
        (
            b"--skip-same-version".as_slice(),
            DpkgOption::SkipSameVersion,
        ),
        (b"--robot".as_slice(), DpkgOption::Robot),
        (b"--no-pager".as_slice(), DpkgOption::NoPager),
        (b"--no-debsig".as_slice(), DpkgOption::NoDebsig),
        (
            b"--triggers".as_slice(),
            DpkgOption::TriggerProcessing(true),
        ),
        (
            b"--no-triggers".as_slice(),
            DpkgOption::TriggerProcessing(false),
        ),
        (b"-a".as_slice(), DpkgOption::Pending),
        (b"--pending".as_slice(), DpkgOption::Pending),
    ] {
        assert_eq!(
            round_trip(&[spelling, b"--version"]).options,
            vec![expected]
        );
    }
}

#[test]
fn every_value_option_accepts_separate_and_attached_values() {
    let cases = vec![
        (
            b"--abort-after".as_slice(),
            b"42".as_slice(),
            DpkgOption::AbortAfter(42),
        ),
        (
            b"--ignore-depends".as_slice(),
            b"alpha,beta".as_slice(),
            DpkgOption::IgnoreDepends(text_argv(&["alpha", "beta"])),
        ),
        (
            b"--admindir".as_slice(),
            b"/state".as_slice(),
            DpkgOption::AdministrativeDirectory(b"/state".to_vec()),
        ),
        (
            b"--instdir".as_slice(),
            b"/target".as_slice(),
            DpkgOption::InstallationDirectory(b"/target".to_vec()),
        ),
        (
            b"--root".as_slice(),
            b"/root".as_slice(),
            DpkgOption::Root(b"/root".to_vec()),
        ),
        (
            b"--pre-invoke".as_slice(),
            b"prepare --exact".as_slice(),
            DpkgOption::PreInvoke(b"prepare --exact".to_vec()),
        ),
        (
            b"--post-invoke".as_slice(),
            b"finish --exact".as_slice(),
            DpkgOption::PostInvoke(b"finish --exact".to_vec()),
        ),
        (
            b"--path-exclude".as_slice(),
            b"/usr/share/doc/*".as_slice(),
            DpkgOption::PathExclude(b"/usr/share/doc/*".to_vec()),
        ),
        (
            b"--path-include".as_slice(),
            b"/usr/share/doc/demo/*".as_slice(),
            DpkgOption::PathInclude(b"/usr/share/doc/demo/*".to_vec()),
        ),
        (
            b"--verify-format".as_slice(),
            b"rpm".as_slice(),
            DpkgOption::VerifyFormat(b"rpm".to_vec()),
        ),
        (
            b"--status-fd".as_slice(),
            b"9".as_slice(),
            DpkgOption::StatusFd(9),
        ),
        (
            b"--status-logger".as_slice(),
            b"logger --tag dpkg".as_slice(),
            DpkgOption::StatusLogger(b"logger --tag dpkg".to_vec()),
        ),
        (
            b"--log".as_slice(),
            b"/var/log/dpkg.log".as_slice(),
            DpkgOption::Log(b"/var/log/dpkg.log".to_vec()),
        ),
    ];
    for (name, value, expected) in cases {
        assert_eq!(
            round_trip(&[name, value, b"--version"]).options,
            vec![expected.clone()]
        );
        let mut attached = name.to_vec();
        attached.push(b'=');
        attached.extend_from_slice(value);
        assert_eq!(
            round_trip_owned(vec![attached, b"--version".to_vec()]).options,
            vec![expected]
        );
    }
}

#[test]
fn all_debug_and_force_forms_use_source_documented_value_tables() {
    for arguments in [
        vec![b"--debug".to_vec(), b"10".to_vec(), b"--version".to_vec()],
        vec![b"-D".to_vec(), b"10".to_vec(), b"--version".to_vec()],
        vec![b"-D10".to_vec(), b"--version".to_vec()],
        vec![b"--debug=10".to_vec(), b"--version".to_vec()],
    ] {
        assert_eq!(
            round_trip_owned(arguments).options,
            vec![DpkgOption::Debug(8)]
        );
    }

    for thing in DpkgForceThing::ALL {
        for (prefix, mode) in [
            (b"--force-".as_slice(), DpkgForceMode::Force),
            (b"--refuse-".as_slice(), DpkgForceMode::Refuse),
            (b"--no-force-".as_slice(), DpkgForceMode::Refuse),
        ] {
            let mut argument = prefix.to_vec();
            argument.extend_from_slice(thing.as_bytes());
            assert_eq!(
                round_trip_owned(vec![argument, b"--version".to_vec()]).options,
                vec![DpkgOption::Force {
                    mode,
                    things: vec![thing],
                }]
            );
        }
    }

    for (spelling, mode) in [
        (b"--force".as_slice(), DpkgForceMode::Force),
        (b"--refuse".as_slice(), DpkgForceMode::Refuse),
        (b"--no-force".as_slice(), DpkgForceMode::Refuse),
    ] {
        assert_eq!(
            round_trip(&[spelling, b"depends,conflicts", b"--version"]).options,
            vec![DpkgOption::Force {
                mode,
                things: vec![DpkgForceThing::Depends, DpkgForceThing::Conflicts],
            }]
        );
    }
}

#[test]
fn option_occurrences_preserve_precedence_duplicates_and_byte_values() {
    let parsed = round_trip(&[
        b"--root=/first",
        b"--admindir=/admin",
        b"--root=/last",
        b"--pre-invoke=\xffprepare",
        b"--force-depends",
        b"--refuse-depends",
        b"--triggers",
        b"--no-triggers",
        b"--pre-invoke=again",
        b"--version",
    ]);
    assert_eq!(
        parsed.options,
        vec![
            DpkgOption::Root(b"/first".to_vec()),
            DpkgOption::AdministrativeDirectory(b"/admin".to_vec()),
            DpkgOption::Root(b"/last".to_vec()),
            DpkgOption::PreInvoke(b"\xffprepare".to_vec()),
            DpkgOption::Force {
                mode: DpkgForceMode::Force,
                things: vec![DpkgForceThing::Depends],
            },
            DpkgOption::Force {
                mode: DpkgForceMode::Refuse,
                things: vec![DpkgForceThing::Depends],
            },
            DpkgOption::TriggerProcessing(true),
            DpkgOption::TriggerProcessing(false),
            DpkgOption::PreInvoke(b"again".to_vec()),
        ]
    );
    assert_eq!(
        &parsed.argv()[..3],
        &[
            b"--root=/first".to_vec(),
            b"--admindir=/admin".to_vec(),
            b"--root=/last".to_vec(),
        ]
    );

    let delimited = round_trip(&[
        b"--root=/target",
        b"--install",
        b"--",
        b"-literal.deb",
        b"\xfepackage.deb",
    ]);
    assert_eq!(
        delimited.disposition,
        DpkgDisposition::RecursiveMutation(DpkgMutationAction::Install(vec![
            b"-literal.deb".to_vec(),
            b"\xfepackage.deb".to_vec(),
        ]))
    );
}

#[test]
fn dpkg_rejects_unknown_conflicting_invalid_and_bad_arity_forms() {
    let invalid: &[&[&str]] = &[
        &[],
        &["--"],
        &["--unknown"],
        &["--version", "--help"],
        &["--assert-future"],
        &["--assert-multi-arch", "extra"],
        &["--validate-future", "value"],
        &["--validate-pkgname"],
        &["--validate-pkgname", "one", "two"],
        &["--compare-versions", "1", "eq"],
        &["--compare-versions", "1", "future", "2"],
        &["--compare-versions", "1", "eq", "2", "extra"],
        &["--force-help", "extra"],
        &["--debug=help", "extra"],
        &["--install"],
        &["--configure"],
        &["--pending", "--configure", "demo"],
        &["--set-selections", "extra"],
        &["--update-avail", "one", "two"],
        &["--add-architecture"],
        &["--add-architecture", "arm64", "extra"],
        &["--build"],
        &["--build", "tree", "one.deb", "extra"],
        &["--contents"],
        &["--contents", "one.deb", "extra"],
        &["--extract", "one.deb"],
        &["--extract", "one.deb", "root", "extra"],
        &["--force"],
        &["--force-"],
        &["--force-future"],
        &["--force-all,"],
        &["--ignore-depends="],
        &["--ignore-depends=one,,two"],
        &["--abort-after=-1", "--version"],
        &["--abort-after=4294967296", "--version"],
        &["--status-fd=-1", "--version"],
        &["--debug=8", "--version"],
        &["--root", "--version"],
        &["--root=\0", "--version"],
        &["--install", "\0"],
    ];
    for values in invalid {
        assert!(
            DpkgInvocation::parse(&text_argv(values)).is_err(),
            "{values:?}"
        );
    }
}

#[test]
fn dpkg_special_result_tables_are_total_for_documented_codes() {
    for (status, code) in [
        (DpkgValidationStatus::Valid, 0),
        (DpkgValidationStatus::InvalidButLax, 1),
        (DpkgValidationStatus::Invalid, 2),
    ] {
        assert_eq!(DpkgValidationStatus::from_exit_code(code).unwrap(), status);
        assert_eq!(status.exit_code(), code);
    }
    for (status, code) in [
        (DpkgAssertionStatus::Supported, 0),
        (DpkgAssertionStatus::KnownUnsupported, 1),
        (DpkgAssertionStatus::Unknown, 2),
    ] {
        assert_eq!(DpkgAssertionStatus::from_exit_code(code).unwrap(), status);
        assert_eq!(status.exit_code(), code);
    }
    for (status, code) in [
        (DpkgComparisonStatus::Satisfied, 0),
        (DpkgComparisonStatus::Unsatisfied, 1),
        (DpkgComparisonStatus::Error, 2),
    ] {
        assert_eq!(DpkgComparisonStatus::from_exit_code(code).unwrap(), status);
        assert_eq!(status.exit_code(), code);
    }
    assert!(DpkgValidationStatus::from_exit_code(3).is_err());
    assert!(DpkgAssertionStatus::from_exit_code(3).is_err());
    assert!(DpkgComparisonStatus::from_exit_code(3).is_err());
}
