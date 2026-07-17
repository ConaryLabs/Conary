// conary-test/src/engine/container_setup.rs
//! Shared container initialization logic for test runners and service code.
//!
//! Extracted from the runner and service so both paths use identical
//! database-init + repo-setup sequences.

use anyhow::{Context, bail};

use crate::config::distro::GlobalConfig;
use crate::container::{ContainerBackend, ContainerId};

/// Initialize conary database and repos inside a test container.
///
/// `configure_remi_override` gates test-only replacement of the packaged Remi
/// seed when a non-production endpoint is configured. Normal preview tests use
/// the one `remi` source created by `system init`.
pub async fn initialize_container_state(
    config: &GlobalConfig,
    distro: &str,
    configure_remi_override: bool,
    backend: &dyn ContainerBackend,
    container_id: &ContainerId,
) -> anyhow::Result<()> {
    use std::time::Duration;

    let distro_config = config
        .distros
        .get(distro)
        .with_context(|| format!("unknown distro: {distro}"))?;
    if distro_config.repo_name != "remi" {
        bail!(
            "distro {distro} must use the packaged 'remi' repository seed, not '{}'",
            distro_config.repo_name
        );
    }
    let db_parent = std::path::Path::new(&config.paths.db)
        .parent()
        .context("db path has no parent directory")?
        .display()
        .to_string();
    let init_cmd = format!(
        "mkdir -p {db_parent} && {} system init --profile {} --db-path {}",
        config.paths.conary_bin, distro_config.remi_distro, config.paths.db
    );
    let init_result = backend
        .exec(
            container_id,
            &["sh", "-c", &init_cmd],
            Duration::from_secs(120),
        )
        .await?;
    if init_result.exit_code != 0 {
        bail!(
            "failed to initialize conary database: {}{}",
            init_result.stdout,
            init_result.stderr
        );
    }

    for repo in &config.setup.remove_default_repos {
        let remove_cmd = format!(
            "{} repo remove {} --db-path {} >/dev/null 2>&1 || true",
            config.paths.conary_bin, repo, config.paths.db
        );
        backend
            .exec(
                container_id,
                &["sh", "-c", &remove_cmd],
                Duration::from_secs(30),
            )
            .await?;
    }

    if configure_remi_override
        && config.remi.endpoint.trim_end_matches('/') != "https://remi.conary.io"
    {
        let replace_repo_cmd = format!(
            "{} repo remove remi --db-path {} && {} repo add remi {} --default-strategy remi --remi-endpoint {} --remi-distro {} --no-gpg-check --db-path {}",
            config.paths.conary_bin,
            config.paths.db,
            config.paths.conary_bin,
            config.remi.endpoint,
            config.remi.endpoint,
            distro_config.remi_distro,
            config.paths.db
        );
        let replace_result = backend
            .exec(
                container_id,
                &["sh", "-c", &replace_repo_cmd],
                Duration::from_secs(60),
            )
            .await?;
        if replace_result.exit_code != 0 {
            bail!(
                "failed to replace packaged Remi seed for test endpoint: {}{}",
                replace_result.stdout,
                replace_result.stderr
            );
        }
    }

    let verify_seed_cmd = format!(
        "repo_output=\"$({} repo list --all --db-path {})\" && count=\"$(printf '%s\\n' \"$repo_output\" | grep -Ec '^[[:space:]]+\\[[x ]\\][[:space:]]+remi[[:space:]]')\" && [ \"$count\" -eq 1 ]",
        config.paths.conary_bin, config.paths.db
    );
    let verify_result = backend
        .exec(
            container_id,
            &["sh", "-c", &verify_seed_cmd],
            Duration::from_secs(30),
        )
        .await?;
    if verify_result.exit_code != 0 {
        bail!(
            "packaged onboarding must leave exactly one Remi repository: {}{}",
            verify_result.stdout,
            verify_result.stderr
        );
    }

    Ok(())
}
