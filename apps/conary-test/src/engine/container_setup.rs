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
/// seed when a non-production endpoint is configured. `system init` creates
/// every built-in source feed; the container harness keeps only its selected
/// feed so an unqualified test sync remains deterministic.
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
    let selected_seed = format!("remi-{}", distro_config.remi_distro);
    if distro_config.repo_name != selected_seed {
        bail!(
            "distro {distro} must use packaged repository seed '{selected_seed}', not '{}'",
            distro_config.repo_name,
        );
    }
    let db_parent = std::path::Path::new(&config.paths.db)
        .parent()
        .context("db path has no parent directory")?
        .display()
        .to_string();
    let init_cmd = format!(
        "mkdir -p {db_parent} && {} system init --db-path {}",
        config.paths.conary_bin, config.paths.db
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

    for feed in conary_core::repository::supported_profiles::public_profiles() {
        let repo = format!("remi-{}", feed.id());
        if repo == selected_seed {
            continue;
        }
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
            "{} repo remove {} --db-path {} && {} repo add {} {} --package-format json --default-strategy remi --remi-endpoint {} --source-profile {} --db-path {}",
            config.paths.conary_bin,
            distro_config.repo_name,
            config.paths.db,
            config.paths.conary_bin,
            distro_config.repo_name,
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
        "repo_output=\"$({} repo list --all --db-path {})\" && count=\"$(printf '%s\\n' \"$repo_output\" | grep -Ec '^[[:space:]]+\\[[x ]\\][[:space:]]+{}[[:space:]]')\" && [ \"$count\" -eq 1 ]",
        config.paths.conary_bin, config.paths.db, distro_config.repo_name
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
            "packaged onboarding must leave its selected Remi source: {}{}",
            verify_result.stdout,
            verify_result.stderr
        );
    }

    Ok(())
}
