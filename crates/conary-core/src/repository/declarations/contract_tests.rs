// conary-core/src/repository/declarations/contract_tests.rs

use super::alpm::AlpmConfigFile;
use super::apt::{AptSourceDocument, AptSyntax};
use super::dnf::{DnfEndpointKind, DnfRepoDocument};
use super::zypper::{ZypperRepoDocument, ZypperServiceDocument};
use super::{DeclarationEcosystem, DeclarationErrorKind};

const FIXTURES: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/repository_declarations/"
);

fn fixture(name: &str) -> String {
    std::fs::read_to_string(format!("{FIXTURES}{name}")).unwrap()
}

#[test]
fn source_derived_fixtures_round_trip_byte_for_byte() {
    let apt_list = fixture("apt-vendor.list");
    assert_eq!(
        AptSourceDocument::parse("apt-vendor.list", AptSyntax::OneLine, &apt_list)
            .unwrap()
            .render_preserved(),
        apt_list
    );
    let apt_sources = fixture("apt-vendor.sources");
    assert_eq!(
        AptSourceDocument::parse("apt-vendor.sources", AptSyntax::Deb822, &apt_sources)
            .unwrap()
            .render_preserved(),
        apt_sources
    );
    let dnf = fixture("dnf-vendor.repo");
    assert_eq!(
        DnfRepoDocument::parse("dnf-vendor.repo", &dnf)
            .unwrap()
            .render_preserved(),
        dnf
    );
    let zypper = fixture("zypper-vendor.repo");
    assert_eq!(
        ZypperRepoDocument::parse("zypper-vendor.repo", &zypper)
            .unwrap()
            .render_preserved(),
        zypper
    );
    let service = fixture("zypper-vendor.service");
    assert_eq!(
        ZypperServiceDocument::parse("zypper-vendor.service", &service)
            .unwrap()
            .render_preserved(),
        service
    );
    let alpm = fixture("pacman.conf");
    assert_eq!(
        AlpmConfigFile::parse("pacman.conf", &alpm)
            .unwrap()
            .render_preserved(),
        alpm
    );
}

#[test]
fn dnf_endpoint_combinations_follow_one_precedence_property() {
    for bits in 0_u8..8 {
        let mut source = String::from("[vendor]\n");
        if bits & 1 != 0 {
            source.push_str("baseurl=https://base/$basearch\n");
        }
        if bits & 2 != 0 {
            source.push_str("mirrorlist=https://mirrors\n");
        }
        if bits & 4 != 0 {
            source.push_str("metalink=https://meta\n");
        }
        let document = DnfRepoDocument::parse("vendor.repo", &source).unwrap();
        assert_eq!(document.render_preserved(), source);
        let expected = if bits & 4 != 0 {
            Some(DnfEndpointKind::Metalink)
        } else if bits & 2 != 0 {
            Some(DnfEndpointKind::Mirrorlist)
        } else if bits & 1 != 0 {
            Some(DnfEndpointKind::Baseurl)
        } else {
            None
        };
        assert_eq!(
            document.repositories[0]
                .effective_endpoint()
                .map(|(kind, _)| kind),
            expected
        );
    }
}

#[test]
fn authority_name_mutations_fail_closed_in_every_grammar() {
    let cases = [
        DnfRepoDocument::parse("vendor.repo", "[vendor]\nFutureEndpoint=https://x\n").unwrap_err(),
        ZypperServiceDocument::parse("vendor.service", "[vendor]\nFutureEndpoint=https://x\n")
            .unwrap_err(),
        AlpmConfigFile::parse("pacman.conf", "[vendor]\nFutureEndpoint=https://x\n").unwrap_err(),
        AptSourceDocument::parse(
            "vendor.list",
            AptSyntax::OneLine,
            "deb [future-endpoint=https://x] https://repo stable main\n",
        )
        .unwrap_err(),
    ];
    for error in cases {
        let expected_line = if error.ecosystem == DeclarationEcosystem::Apt {
            1
        } else {
            2
        };
        assert_eq!(error.kind, DeclarationErrorKind::UnknownAuthority);
        assert_eq!(error.line, Some(expected_line));
    }
}
