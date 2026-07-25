// conary-core/src/scriptlet/rpm_runtime/query_format/tests.rs

use super::format;
use crate::ccs::native_lifecycle::{
    RpmHeaderContext, RpmHeaderFact, RpmHeaderFactSource, RpmHeaderValue, RpmMacroContext,
    RpmQueryFormatTimezone,
};

fn header() -> RpmHeaderContext {
    RpmHeaderContext {
        facts: vec![
            RpmHeaderFact {
                tag: 1000,
                name: Some("NAME".to_string()),
                value: RpmHeaderValue::String("rpm".to_string()),
                source: RpmHeaderFactSource::Header,
            },
            RpmHeaderFact {
                tag: 1030,
                name: Some("FILEMODES".to_string()),
                value: RpmHeaderValue::Integer(vec![0o100755, 0o100644]),
                source: RpmHeaderFactSource::Header,
            },
            RpmHeaderFact {
                tag: 5000,
                name: Some("FILENAMES".to_string()),
                value: RpmHeaderValue::StringArray(vec![
                    "/usr/bin/rpm".to_string(),
                    "/usr/share/man/rpm.8".to_string(),
                ]),
                source: RpmHeaderFactSource::TransactionDerived,
            },
            RpmHeaderFact {
                tag: 5011,
                name: Some("FILEDIGESTALGO".to_string()),
                value: RpmHeaderValue::Integer(vec![8]),
                source: RpmHeaderFactSource::Header,
            },
            RpmHeaderFact {
                tag: 6000,
                name: Some("TESTBLOB".to_string()),
                value: RpmHeaderValue::Binary("AP8=".to_string()),
                source: RpmHeaderFactSource::Header,
            },
        ],
        format_environment: Default::default(),
        database_instance: 0,
    }
}

fn fact(tag: rpm::IndexTag, value: RpmHeaderValue) -> RpmHeaderFact {
    RpmHeaderFact {
        tag: tag as u32,
        name: tag.to_string().strip_prefix("RPMTAG_").map(str::to_string),
        value,
        source: RpmHeaderFactSource::Header,
    }
}

fn render(input: &str, header: &RpmHeaderContext) -> String {
    format(input, header, &RpmMacroContext::default()).expect("query format")
}

#[test]
fn formats_official_scalar_iterator_and_locked_scalar_examples() {
    let output = format(
        "%-6{NAME}\\n[%{=NAME} %{FILEMODES:perms} %{FILENAMES}\\n]",
        &header(),
        &RpmMacroContext::default(),
    )
    .expect("query format");

    assert_eq!(
        output,
        "rpm   \nrpm -rwxr-xr-x /usr/bin/rpm\nrpm -rw-r--r-- /usr/share/man/rpm.8\n"
    );
}

#[test]
fn field_width_counts_utf8_bytes_like_rpm_printf() {
    let mut header = header();
    header.facts[0].value = RpmHeaderValue::String("é".to_string());

    assert_eq!(render("%3{NAME}|%-3{NAME}", &header), " é|é ");
}

#[test]
fn supports_rpms_complete_character_escape_and_array_size_shorthand() {
    assert_eq!(
        render("\\a\\b%{#FILENAMES}", &header()),
        "\u{0007}\u{0008}2"
    );
}

#[test]
fn formats_official_presence_expression() {
    assert_eq!(
        format(
            "%|PREINPROG?{ %{PREINPROG}}:{ no}|",
            &header(),
            &RpmMacroContext::default(),
        )
        .expect("query expression"),
        " no"
    );
}

#[test]
fn rejects_parallel_arrays_with_different_cardinality() {
    let mut header = header();
    header.facts.push(RpmHeaderFact {
        tag: 1049,
        name: Some("REQUIRENAME".to_string()),
        value: RpmHeaderValue::StringArray(vec!["one".to_string()]),
        source: RpmHeaderFactSource::Header,
    });

    let error = format(
        "[%{FILENAMES} %{REQUIRENAME}]",
        &header,
        &RpmMacroContext::default(),
    )
    .expect_err("mismatched arrays");

    assert!(error.to_string().contains("mismatched lengths"));
}

#[test]
fn formats_typed_binary_numeric_and_flag_values_like_rpm() {
    let output = format(
        "%{TESTBLOB:string} %{TESTBLOB:base64} %{FILEDIGESTALGO:json} %{FILEDIGESTALGO:hashalgo}",
        &header(),
        &RpmMacroContext::default(),
    )
    .expect("typed formats");

    assert_eq!(output, "00ff AP8= 8 SHA256");
}

#[test]
fn reports_unknown_formatter_by_exact_name() {
    let error = format(
        "%{NAME:definitely-not-rpm}",
        &header(),
        &RpmMacroContext::default(),
    )
    .expect_err("unknown formatter");

    assert!(error.to_string().contains("definitely-not-rpm"));
    assert!(error.to_string().contains("1000 (NAME)"));
}

#[test]
fn known_absent_header_facts_render_none() {
    assert_eq!(
        format("%{PREINPROG}", &header(), &RpmMacroContext::default(),)
            .expect("known absent header fact"),
        "(none)"
    );
}

#[test]
fn formats_date_and_day_with_persisted_c_locale_and_timezone() {
    let mut header = RpmHeaderContext {
        facts: vec![fact(
            rpm::IndexTag::RPMTAG_BUILDTIME,
            RpmHeaderValue::Integer(vec![946_684_800]),
        )],
        format_environment: Default::default(),
        database_instance: 0,
    };
    assert_eq!(
        render("%{BUILDTIME:date}|%{BUILDTIME:day}", &header),
        "Sat Jan  1 00:00:00 2000|Sat Jan 01 2000"
    );

    header.format_environment.timezone = RpmQueryFormatTimezone::FixedOffset {
        seconds_east_of_utc: 3_600,
    };
    assert_eq!(
        render("%{BUILDTIME:date}", &header),
        "Sat Jan  1 01:00:00 2000"
    );
}

#[test]
fn formats_ascii_armor_exactly_like_rpm_openpgp_armor() {
    let header = RpmHeaderContext {
        facts: vec![RpmHeaderFact {
            tag: 60_001,
            name: Some("TESTSIGNATURE".to_string()),
            value: RpmHeaderValue::Binary("SGVsbG8gd29ybGQh".to_string()),
            source: RpmHeaderFactSource::Header,
        }],
        format_environment: Default::default(),
        database_instance: 0,
    };

    assert_eq!(
        render("%{TESTSIGNATURE:armor}", &header),
        "-----BEGIN PGP SIGNATURE-----\n\nSGVsbG8gd29ybGQh\n=s4Gu\n-----END PGP SIGNATURE-----\n"
    );
}

#[test]
fn formats_pgp_signature_identity_and_creation_time_like_rpm() {
    const SIGNATURE_PACKET_BASE64: &str = "wsBzBAABCgAdFiEEwD+mQRsDrhJXZGEYciO1ZnjgJSgFAlnclx8ACgkQciO1ZnjgJShldAf+NBvUTVPnVPhYM4KihWOUlup8lbD6g1IduSM5rpsGvOVb+uKF6ik+GOBBRlMT4s183r3teFxiTkDx2pRhUz0MnOMPfbXovjF6Y93fKCOxCQWLBa0ukjNmE+axgu9nZ3XXDGXZW22iGE52uVjPGSfuLfqvdMy5bKHn8xow/kepuGHZwy8yn7uFv7slLnOBUz1FKA7iRl457XKPUhw5K7BnfRW/I2BRlnrwTDkjfXaJZC+bUTIJvm682BvtZNn8zc0JucyEkuL9WXYNuZg0znDE3T7D/6+tzfEdSf706unsXFXWHf83vL2eHCcwqhImm1lmcC+agFtWQ6/qD923LR9xmg==";
    let header = RpmHeaderContext {
        facts: vec![RpmHeaderFact {
            tag: 60_002,
            name: Some("TESTPGPSIG".to_string()),
            value: RpmHeaderValue::Binary(SIGNATURE_PACKET_BASE64.to_string()),
            source: RpmHeaderFactSource::Header,
        }],
        format_environment: Default::default(),
        database_instance: 0,
    };

    assert_eq!(
        render("%{TESTPGPSIG:pgpsig}", &header),
        "RSA/SHA512, Tue Oct 10 09:47:11 2017, Key ID 7223b56678e02528"
    );
}

#[test]
fn computes_file_class_mime_and_hardlink_extensions_from_typed_header_facts() {
    use rpm::IndexTag::*;

    let header = RpmHeaderContext {
        facts: vec![
            fact(
                RPMTAG_BASENAMES,
                RpmHeaderValue::StringArray(vec![
                    "regular-a".to_string(),
                    "regular-b".to_string(),
                    "link".to_string(),
                    "directory".to_string(),
                ]),
            ),
            fact(
                RPMTAG_FILEMODES,
                RpmHeaderValue::Integer(vec![0o100644, 0o100644, 0o120777, 0o040755]),
            ),
            fact(
                RPMTAG_FILELINKTOS,
                RpmHeaderValue::StringArray(vec![
                    String::new(),
                    String::new(),
                    "target".to_string(),
                    String::new(),
                ]),
            ),
            fact(RPMTAG_FILECLASS, RpmHeaderValue::Integer(vec![0, 0, 0, 0])),
            fact(
                RPMTAG_CLASSDICT,
                RpmHeaderValue::StringArray(vec![String::new()]),
            ),
            fact(RPMTAG_FILEMIMES, RpmHeaderValue::Integer(vec![0, 0, 0, 0])),
            fact(
                RPMTAG_MIMEDICT,
                RpmHeaderValue::StringArray(vec![String::new()]),
            ),
            fact(RPMTAG_FILEFLAGS, RpmHeaderValue::Integer(vec![0, 0, 0, 0])),
            fact(
                RPMTAG_FILEINODES,
                RpmHeaderValue::Integer(vec![42, 42, 7, 8]),
            ),
            fact(
                RPMTAG_FILEDEVICES,
                RpmHeaderValue::Integer(vec![3, 3, 3, 3]),
            ),
        ],
        format_environment: Default::default(),
        database_instance: 0,
    };

    assert_eq!(
        render("[%{FILECLASS}|%{FILEMIMES}|%{FILENLINKS}\\n]", &header),
        "||2\n||2\nsymbolic link to `target'|inode/symlink|1\ndirectory|inode/directory|1\n"
    );
}

#[test]
fn computes_file_dependency_nevr_sysuser_and_trigger_extensions() {
    use rpm::IndexTag::*;

    let header = RpmHeaderContext {
        facts: vec![
            fact(
                RPMTAG_BASENAMES,
                RpmHeaderValue::StringArray(vec!["one".to_string(), "two".to_string()]),
            ),
            fact(RPMTAG_FILEDEPENDSX, RpmHeaderValue::Integer(vec![0, 1])),
            fact(RPMTAG_FILEDEPENDSN, RpmHeaderValue::Integer(vec![1, 1])),
            fact(
                RPMTAG_DEPENDSDICT,
                RpmHeaderValue::Integer(vec![(u64::from(b'P')) << 24, (u64::from(b'R')) << 24]),
            ),
            fact(
                RPMTAG_PROVIDENAME,
                RpmHeaderValue::StringArray(vec![
                    "capability".to_string(),
                    "user(service)".to_string(),
                ]),
            ),
            fact(
                RPMTAG_PROVIDEVERSION,
                RpmHeaderValue::StringArray(vec![
                    "1".to_string(),
                    "dSBzZXJ2aWNlIC0gL2hvbWUvU2VydmljZSAvc2Jpbi9ub2xvZ2lu".to_string(),
                ]),
            ),
            fact(RPMTAG_PROVIDEFLAGS, RpmHeaderValue::Integer(vec![8, 8])),
            fact(
                RPMTAG_REQUIRENAME,
                RpmHeaderValue::StringArray(vec!["libfoo".to_string()]),
            ),
            fact(
                RPMTAG_REQUIREVERSION,
                RpmHeaderValue::StringArray(vec!["2".to_string()]),
            ),
            fact(RPMTAG_REQUIREFLAGS, RpmHeaderValue::Integer(vec![12])),
            fact(
                RPMTAG_TRIGGERSCRIPTS,
                RpmHeaderValue::StringArray(vec!["one".to_string(), "two".to_string()]),
            ),
            fact(
                RPMTAG_TRIGGERNAME,
                RpmHeaderValue::StringArray(vec!["pkg".to_string(), "path".to_string()]),
            ),
            fact(
                RPMTAG_TRIGGERVERSION,
                RpmHeaderValue::StringArray(vec!["1".to_string(), String::new()]),
            ),
            fact(
                RPMTAG_TRIGGERFLAGS,
                RpmHeaderValue::Integer(vec![(1 << 25) | 12, 1 << 16]),
            ),
            fact(RPMTAG_TRIGGERINDEX, RpmHeaderValue::Integer(vec![0, 1])),
        ],
        format_environment: Default::default(),
        database_instance: 0,
    };

    assert_eq!(
        render("[%{FILEPROVIDE}|%{FILEREQUIRE}\\n]", &header),
        "capability = 1|\n|libfoo >= 2\n"
    );
    assert_eq!(render("[%{REQUIRENEVRS}\\n]", &header), "libfoo >= 2\n");
    assert_eq!(
        render("[%{SYSUSERS}\\n]", &header),
        "u service - /home/Service /sbin/nologin\n"
    );
    assert_eq!(
        render("[%{TRIGGERCONDS}|%{TRIGGERTYPE}\\n]", &header),
        "pkg >= 1|prein\npath|in\n"
    );
}

#[test]
fn computes_legacy_weak_dependency_nevrs_with_rpms_strong_partition() {
    use rpm::IndexTag::*;

    let header = RpmHeaderContext {
        facts: vec![
            fact(
                RPMTAG_OLDSUGGESTSNAME,
                RpmHeaderValue::StringArray(vec!["strong-suggest".into(), "weak-suggest".into()]),
            ),
            fact(
                RPMTAG_OLDSUGGESTSVERSION,
                RpmHeaderValue::StringArray(vec!["1".into(), "2".into()]),
            ),
            fact(
                RPMTAG_OLDSUGGESTSFLAGS,
                RpmHeaderValue::Integer(vec![(1 << 27) | 8, 8]),
            ),
            fact(
                RPMTAG_OLDENHANCESNAME,
                RpmHeaderValue::StringArray(vec!["strong-enhance".into(), "weak-enhance".into()]),
            ),
            fact(
                RPMTAG_OLDENHANCESVERSION,
                RpmHeaderValue::StringArray(vec!["3".into(), "4".into()]),
            ),
            fact(
                RPMTAG_OLDENHANCESFLAGS,
                RpmHeaderValue::Integer(vec![(1 << 27) | 8, 8]),
            ),
        ],
        format_environment: Default::default(),
        database_instance: 0,
    };

    assert_eq!(
        render(
            "%{RECOMMENDNEVRS}|%{SUGGESTNEVRS}|%{SUPPLEMENTNEVRS}|%{ENHANCENEVRS}",
            &header
        ),
        "strong-suggest = 1|weak-suggest = 2|strong-enhance = 3|weak-enhance = 4"
    );
}

#[test]
fn computes_numeric_signature_and_format_extensions() {
    use rpm::IndexTag::*;

    let mut header = RpmHeaderContext {
        facts: vec![
            fact(RPMTAG_FILESIZES, RpmHeaderValue::Integer(vec![9, 10])),
            fact(RPMTAG_ARCHIVESIZE, RpmHeaderValue::Integer(vec![20])),
            fact(RPMTAG_SIZE, RpmHeaderValue::Integer(vec![30])),
            fact(RPMTAG_SIGSIZE, RpmHeaderValue::Integer(vec![40])),
            fact(RPMTAG_FILECOLORS, RpmHeaderValue::Integer(vec![1, 2, 16])),
            fact(RPMTAG_HEADERIMMUTABLE, RpmHeaderValue::Null),
            fact(RPMTAG_RSAHEADER, RpmHeaderValue::Binary("AAE=".to_string())),
        ],
        format_environment: Default::default(),
        database_instance: 42,
    };
    header.format_environment.verbose = true;

    assert_eq!(
        render(
            "%{LONGFILESIZES}|%{LONGARCHIVESIZE}|%{LONGSIZE}|%{LONGSIGSIZE}|%{DBINSTANCE}|%{HEADERCOLOR}|%{VERBOSE}|%{RPMFORMAT}|%{OPENPGP:arraysize}",
            &header
        ),
        "9|20|30|40|42|3|1|4|1"
    );
}

#[test]
fn every_bundled_rpm_header_extension_has_a_runtime_resolution_path() {
    let header = RpmHeaderContext::default();
    for tag in [
        "FILECLASS",
        "FILEPROVIDE",
        "FILEREQUIRE",
        "TRIGGERCONDS",
        "FILETRIGGERCONDS",
        "TRANSFILETRIGGERCONDS",
        "TRIGGERTYPE",
        "FILETRIGGERTYPE",
        "TRANSFILETRIGGERTYPE",
        "LONGFILESIZES",
        "LONGARCHIVESIZE",
        "LONGSIZE",
        "LONGSIGSIZE",
        "DBINSTANCE",
        "HEADERCOLOR",
        "VERBOSE",
        "REQUIRENEVRS",
        "RECOMMENDNEVRS",
        "SUGGESTNEVRS",
        "SUPPLEMENTNEVRS",
        "ENHANCENEVRS",
        "PROVIDENEVRS",
        "OBSOLETENEVRS",
        "CONFLICTNEVRS",
        "FILENLINKS",
        "SYSUSERS",
        "FILEMIMES",
        "OPENPGP",
        "RPMFORMAT",
    ] {
        render(&format!("%{{{tag}}}"), &header);
    }
}
