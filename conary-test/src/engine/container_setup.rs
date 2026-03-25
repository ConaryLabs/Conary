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
/// `add_distro_repo` gates whether to add the distro-specific Remi repo.
/// The runner sets this to `phase > 1`; the service always passes `true`.
pub async fn initialize_container_state(
    config: &GlobalConfig,
    distro: &str,
    add_distro_repo: bool,
    backend: &dyn ContainerBackend,
    container_id: &ContainerId,
) -> anyhow::Result<()> {
    use std::time::Duration;

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

    if add_distro_repo {
        let distro_config = config
            .distros
            .get(distro)
            .with_context(|| format!("unknown distro: {distro}"))?;
        let add_repo_cmd = format!(
            "{} repo add {} {} --default-strategy remi --remi-endpoint {} --remi-distro {} --no-gpg-check --db-path {} >/dev/null 2>&1 || true",
            config.paths.conary_bin,
            distro_config.repo_name,
            config.remi.endpoint,
            config.remi.endpoint,
            distro_config.remi_distro,
            config.paths.db
        );
        backend
            .exec(
                container_id,
                &["sh", "-c", &add_repo_cmd],
                Duration::from_secs(60),
            )
            .await?;
    }

    Ok(())
}
