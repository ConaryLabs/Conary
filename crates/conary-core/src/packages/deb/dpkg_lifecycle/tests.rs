// conary-core/src/packages/deb/dpkg_lifecycle/tests.rs

use super::*;

#[path = "tests/alternatives.rs"]
mod alternatives;
#[path = "tests/dpkg.rs"]
mod dpkg;
#[path = "tests/maintscript.rs"]
mod maintscript;
#[path = "tests/query.rs"]
mod query;

fn argv(values: &[&[u8]]) -> Vec<Vec<u8>> {
    values.iter().map(|value| value.to_vec()).collect()
}

fn text_argv(values: &[&str]) -> Vec<Vec<u8>> {
    values
        .iter()
        .map(|value| value.as_bytes().to_vec())
        .collect()
}

fn assert_tool_round_trip(command: &[u8], values: &[&[u8]]) -> DpkgLifecycleToolInvocation {
    let parsed = DpkgLifecycleToolInvocation::parse(command, &argv(values)).unwrap();
    assert_eq!(
        DpkgLifecycleToolInvocation::parse(parsed.command(), &parsed.argv()).unwrap(),
        parsed
    );
    parsed
}

#[test]
fn pinned_sources_are_complete_unique_sha256_identities() {
    assert_eq!(DPKG_LIFECYCLE_VERSION, "1.23.7");
    assert_eq!(PINNED_DPKG_LIFECYCLE_SOURCES.len(), 16);
    let mut paths = std::collections::BTreeSet::new();
    let mut digests = std::collections::BTreeSet::new();
    for source in PINNED_DPKG_LIFECYCLE_SOURCES {
        assert!(paths.insert(source.path));
        assert!(digests.insert(source.sha256));
        assert_eq!(source.sha256.len(), 64);
        assert!(source.sha256.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert!(
            source
                .source_url
                .starts_with("https://sources.debian.org/data/main/d/dpkg/1.23.7/")
        );
        assert!(source.source_url.ends_with(source.path));
    }
}

#[test]
fn endpoint_dispatch_accepts_only_exact_names_and_paths() {
    for (name, path, arguments) in [
        (
            b"dpkg-maintscript-helper".as_slice(),
            b"/usr/bin/dpkg-maintscript-helper".as_slice(),
            &[b"supports".as_slice(), b"rm_conffile".as_slice()][..],
        ),
        (
            b"dpkg-query".as_slice(),
            b"/usr/bin/dpkg-query".as_slice(),
            &[b"--version".as_slice()][..],
        ),
        (
            b"dpkg".as_slice(),
            b"/usr/bin/dpkg".as_slice(),
            &[b"--version".as_slice()][..],
        ),
        (
            b"update-alternatives".as_slice(),
            b"/usr/bin/update-alternatives".as_slice(),
            &[b"--version".as_slice()][..],
        ),
    ] {
        let by_name = assert_tool_round_trip(name, arguments);
        let by_path = assert_tool_round_trip(path, arguments);
        assert_eq!(by_name, by_path);
        assert_eq!(by_path.command(), path);
    }
    assert!(matches!(
        DpkgLifecycleToolInvocation::parse(b"/usr/local/bin/dpkg", &[]),
        Err(DpkgLifecycleGrammarError::UnknownTool(_))
    ));
}
