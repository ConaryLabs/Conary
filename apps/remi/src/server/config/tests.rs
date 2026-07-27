// apps/remi/src/server/config/tests.rs

use super::*;

#[test]
fn test_parse_size() {
    assert_eq!(parse_size("1024").unwrap(), 1024);
    assert_eq!(parse_size("1KB").unwrap(), 1024);
    assert_eq!(parse_size("1MB").unwrap(), 1024 * 1024);
    assert_eq!(parse_size("1GB").unwrap(), 1024 * 1024 * 1024);
    assert_eq!(parse_size("1TB").unwrap(), 1024u64 * 1024 * 1024 * 1024);
    assert_eq!(parse_size("700GB").unwrap(), 700 * 1024 * 1024 * 1024);
    assert_eq!(
        parse_size("1.5GB").unwrap(),
        (1.5 * 1024.0 * 1024.0 * 1024.0) as u64
    );
}

#[test]
fn test_parse_duration() {
    assert_eq!(parse_duration("30").unwrap(), Duration::from_secs(30));
    assert_eq!(parse_duration("30s").unwrap(), Duration::from_secs(30));
    assert_eq!(parse_duration("15m").unwrap(), Duration::from_secs(15 * 60));
    assert_eq!(parse_duration("1h").unwrap(), Duration::from_secs(3600));
    assert_eq!(
        parse_duration("2d").unwrap(),
        Duration::from_secs(2 * 24 * 3600)
    );
}

#[test]
fn test_default_config() {
    let config = RemiConfig::default();
    assert!(config.validate().is_ok());
    assert_eq!(config.server.bind, "0.0.0.0:8080");
    assert_eq!(config.server.admin_bind, "127.0.0.1:8081");
}

#[test]
fn prewarm_contract_rejects_invalid_interval_and_profile() {
    let mut invalid_interval = RemiConfig::default();
    invalid_interval.prewarm.metadata_sync_interval = "eventually".to_string();
    assert!(invalid_interval.validate().is_err());

    let mut invalid_profile = RemiConfig::default();
    invalid_profile.prewarm.distros = vec!["debian".to_string()];
    assert!(invalid_profile.validate().is_err());
}

#[test]
fn conversion_contract_rejects_invalid_concurrency() {
    let mut invalid_concurrency = RemiConfig::default();
    invalid_concurrency.conversion.max_concurrent = 0;
    assert!(invalid_concurrency.validate().is_err());
}

#[test]
fn test_storage_dirs() {
    let config = RemiConfig::default();
    let dirs = config.storage_dirs();
    assert!(dirs.contains(&PathBuf::from("/conary/chunks")));
    assert!(dirs.contains(&PathBuf::from("/conary/metadata")));
    assert!(dirs.contains(&PathBuf::from("/conary/bootstrap")));
}

#[test]
fn test_default_remi_config_to_server_config_regression() {
    let runtime = RemiConfig::default().to_server_config().unwrap();

    assert_eq!(runtime.bind_addr, "0.0.0.0:8080".parse().unwrap());
    assert_eq!(runtime.db_path, PathBuf::from("/conary/metadata/conary.db"));
    assert_eq!(runtime.chunk_dir, PathBuf::from("/conary/chunks"));
    assert_eq!(runtime.cache_dir, PathBuf::from("/conary/cache"));
    assert_eq!(runtime.max_concurrent_conversions, 32);
    assert_eq!(runtime.cache_max_bytes, 700 * 1024 * 1024 * 1024);
    assert_eq!(runtime.chunk_ttl_days, 30);
    assert!(runtime.enable_bloom_filter);
    assert_eq!(runtime.bloom_expected_chunks, 1_000_000);
    assert_eq!(runtime.upstream_url, None);
    assert_eq!(runtime.upstream_timeout, Duration::from_secs(30));
    assert!(runtime.enable_rate_limit);
    assert_eq!(runtime.rate_limit_rps, 100);
    assert_eq!(runtime.rate_limit_burst, 200);
    assert!(runtime.cors_allowed_origins.is_empty());
    assert!(runtime.enable_audit_log);
    assert_eq!(runtime.ban_threshold, 10);
    assert_eq!(runtime.ban_duration_secs, 300);
    assert_eq!(runtime.web_root, None);
}

#[test]
fn test_to_server_config_uses_storage_max_cache_override() {
    let mut config = RemiConfig::default();
    config.storage.max_cache_size = Some("1TB".to_string());

    let runtime = config.to_server_config().unwrap();

    assert_eq!(runtime.cache_max_bytes, 1024 * 1024 * 1024 * 1024);
}

#[test]
fn test_to_server_config_preserves_security_mapping() {
    let mut config = RemiConfig::default();
    config.security.rate_limit = false;
    config.security.rate_limit_rps = 12;
    config.security.rate_limit_burst = 34;
    config.security.cors_origins = vec!["https://example.com".to_string()];
    config.server.audit_log = false;
    config.security.ban_threshold = 56;
    config.security.ban_duration = "15m".to_string();

    let runtime = config.to_server_config().unwrap();

    assert!(!runtime.enable_rate_limit);
    assert_eq!(runtime.rate_limit_rps, 12);
    assert_eq!(runtime.rate_limit_burst, 34);
    assert_eq!(runtime.cors_allowed_origins, vec!["https://example.com"]);
    assert!(!runtime.enable_audit_log);
    assert_eq!(runtime.ban_threshold, 56);
    assert_eq!(runtime.ban_duration_secs, 15 * 60);
}

#[test]
fn test_server_config_default_matches_default_remi_config() {
    let from_remi = RemiConfig::default().to_server_config().unwrap();
    let from_server = ServerConfig::default();

    assert_eq!(from_server, from_remi);
}

#[test]
fn release_publish_trusted_signers_parse_from_config() {
    let config: RemiConfig = toml::from_str(
        r#"
        [release_publish]
        repository_keys_dir = "/var/lib/remi/keys"
        trusted_build_attestation_signers = [
          { key_id = "publisher", public_key = "cHVibGlzaGVyLXB1YmxpYy1rZXk=" }
        ]
        "#,
    )
    .unwrap();

    assert_eq!(
        config.release_publish.trusted_build_attestation_signers[0].key_id,
        "publisher"
    );
    assert_eq!(
        config.release_publish.repository_keys_dir.as_deref(),
        Some(std::path::Path::new("/var/lib/remi/keys"))
    );
}

#[test]
fn release_publish_empty_trusted_signers_fail_closed() {
    let config = RemiConfig::default();

    assert!(
        config
            .release_publish
            .trusted_build_attestation_signers
            .is_empty()
    );
}

#[test]
fn test_parse_toml() {
    let toml_str = r#"
repository_manifest = "/etc/conary/remi-repositories.toml"

[server]
bind = "0.0.0.0:8080"
admin_bind = "127.0.0.1:8081"
workers = 4

[storage]
root = "/conary"
eviction_threshold = 0.90
negative_cache_ttl = "15m"

[conversion]
chunking = true
chunk_min = 16384
chunk_avg = 65536
chunk_max = 262144

[release_publish]
repository_keys_dir = "/conary/repository-keys"

[federation]
enabled = false

[security]
rate_limit = true
rate_limit_rps = 100

[admin]
enabled = true
external_bind = "0.0.0.0:8082"
bootstrap_token = "bootstrap-token"
"#;
    let config: RemiConfig = toml::from_str(toml_str).unwrap();
    assert!(config.validate().is_ok());
    assert_eq!(config.server.workers, 4);
    assert_eq!(
        config.repository_manifest.as_deref(),
        Some(Path::new("/etc/conary/remi-repositories.toml"))
    );
    assert!(!config.federation.enabled);
    assert!(config.admin.enabled);
    assert_eq!(
        config.admin.bootstrap_token.as_deref(),
        Some("bootstrap-token")
    );
}

#[test]
fn repository_manifest_requires_absolute_signing_authority_root() {
    let mut missing = RemiConfig {
        repository_manifest: Some(PathBuf::from("/etc/conary/remi-repositories.toml")),
        ..RemiConfig::default()
    };
    let error = missing.validate().unwrap_err();
    assert!(
        error
            .to_string()
            .contains("repository_keys_dir is required"),
        "{error:#}"
    );

    missing.release_publish.repository_keys_dir = Some(PathBuf::from("relative/keys"));
    let error = missing.validate().unwrap_err();
    assert!(
        error
            .to_string()
            .contains("repository_keys_dir must be an absolute path"),
        "{error:#}"
    );

    missing.release_publish.repository_keys_dir = Some(PathBuf::from("/conary/repository-keys"));
    missing.validate().unwrap();
}

#[test]
fn test_admin_section_rejects_removed_forgejo_fields() {
    let toml_str = r#"
[server]
bind = "0.0.0.0:8080"
admin_bind = "127.0.0.1:8081"

[storage]
root = "/conary"

[admin]
enabled = true
external_bind = "0.0.0.0:8082"
forgejo_url = "https://forgejo.example"
forgejo_token = "secret"
"#;

    let err = toml::from_str::<RemiConfig>(toml_str)
        .expect_err("removed forgejo admin keys should be rejected");
    let message = err.to_string();
    assert!(message.contains("forgejo_url") || message.contains("forgejo_token"));
}

#[test]
fn test_invalid_eviction_threshold() {
    let toml_str = r#"
[storage]
eviction_threshold = 1.5
"#;
    let config: RemiConfig = toml::from_str(toml_str).unwrap();
    assert!(config.validate().is_err());
}

#[test]
fn test_invalid_chunk_sizes() {
    let toml_str = r#"
[conversion]
chunk_min = 100000
chunk_avg = 50000
"#;
    let config: RemiConfig = toml::from_str(toml_str).unwrap();
    assert!(config.validate().is_err());
}
