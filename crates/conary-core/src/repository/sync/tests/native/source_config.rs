// conary-core/src/repository/sync/tests/native/source_config.rs

use super::*;

pub(super) fn configure(repo: &mut Repository, ecosystem: NativeSourceEcosystem) {
    configure_with_policy(repo, ecosystem, RepositoryUpdateMode::Follow, None);
}

pub(super) fn configure_with_policy(
    repo: &mut Repository,
    ecosystem: NativeSourceEcosystem,
    update_mode: RepositoryUpdateMode,
    pinned_snapshot: Option<AuthenticatedSnapshotIdentity>,
) {
    let root = OpenPgpTrustRoot {
        url: "https://keys.example.test/repository.gpg".to_string(),
        fingerprint: "A".repeat(40),
    };
    match ecosystem {
        NativeSourceEcosystem::Rpm => {
            repo.set_parser_config(RepositoryParserConfig::Rpm {
                architecture: "x86_64".to_string(),
            })
            .unwrap();
            repo.set_trust_policy(RepositoryTrustPolicy::Rpm {
                metadata: RpmMetadataAuthority::OpenPgp {
                    keys: vec![root.clone()],
                },
                package_keys: vec![root],
            })
            .unwrap();
        }
        NativeSourceEcosystem::Deb => {
            repo.set_parser_config(RepositoryParserConfig::Deb {
                distribution: "stable".to_string(),
                component: "main".to_string(),
                architecture: "amd64".to_string(),
            })
            .unwrap();
            repo.set_trust_policy(RepositoryTrustPolicy::Debian {
                release_keys: vec![root],
            })
            .unwrap();
        }
        NativeSourceEcosystem::Alpm => {
            repo.set_parser_config(RepositoryParserConfig::Arch {
                database: "core".to_string(),
            })
            .unwrap();
            repo.set_trust_policy(RepositoryTrustPolicy::Arch {
                keyring: ArchKeyringTrust {
                    url: "https://keys.example.test/archlinux-keyring.pkg.tar.zst".to_string(),
                    format: ArchKeyringFormat::AlpmPackageZstd,
                    master_fingerprints: vec!["A".repeat(40)],
                    packager_key_threshold: 1,
                },
                sig_level: ArchSigLevel::distribution_default(),
            })
            .unwrap();
        }
        NativeSourceEcosystem::Eopkg => {
            repo.set_parser_config(RepositoryParserConfig::Eopkg {
                architecture: "x86_64".to_string(),
            })
            .unwrap();
            repo.set_trust_policy(RepositoryTrustPolicy::Eopkg {
                origin: "https://repo.example.test/".to_string(),
            })
            .unwrap();
        }
    }
    let identity = format!("test:{}", repo.name);
    let policy = RepositorySourcePolicy::new(
        "test-source",
        RepositoryPolicyScope::repository(identity.clone()).unwrap(),
        ecosystem,
        NativeSourceStream::channel("test").unwrap(),
        update_mode,
    )
    .unwrap();
    repo.set_native_source_policy(policy, identity, pinned_snapshot)
        .unwrap();
}
