// apps/conary/src/commands/ccs/test.rs

use anyhow::{Context, Result};
use std::path::Path;

pub async fn cmd_ccs_test(
    package: &str,
    dry_run: bool,
    policy: Option<String>,
    keep_workspace: bool,
) -> Result<()> {
    if !dry_run {
        anyhow::bail!("M4b supports only conary ccs test --dry-run");
    }
    let package_path = Path::new(package);
    if !package_path.exists() {
        anyhow::bail!("Package not found: {package}");
    }

    let workspace = tempfile::tempdir().context("create isolated CCS test workspace")?;
    let root = workspace.path().join("root");
    let db_path = workspace.path().join("conary.db");
    let policy_path = workspace.path().join("trust-policy.toml");
    std::fs::create_dir_all(&root)?;
    conary_core::db::init(&db_path).context("initialize isolated test database")?;

    let policy = if let Some(policy) = policy {
        policy
    } else {
        let key = super::local_dev::load_or_create_local_dev_key()?;
        super::local_dev::write_local_dev_policy(&policy_path, &key)?;
        policy_path.to_string_lossy().into_owned()
    };

    println!("Testing CCS package in isolated dry-run workspace:");
    println!("  root: {}", root.display());
    println!("  db: {}", db_path.display());

    let db_path_string = db_path.to_string_lossy().into_owned();
    let root_string = root.to_string_lossy().into_owned();

    // SandboxMode::None is acceptable in M4b because minimal-file authoring
    // emits no script hooks and ccs test forces dry-run against an isolated
    // root/database. Future lifecycle/template slices must reevaluate this and
    // prefer SandboxMode::Always before any script execution is admitted.
    super::cmd_ccs_install_with_replay_options(
        package,
        &db_path_string,
        &root_string,
        true,
        false,
        Some(policy),
        None,
        crate::commands::SandboxMode::None,
        false,
        true,
        false,
        false,
        None,
        crate::commands::LegacyReplayOptions::default(),
    )
    .await?;

    if keep_workspace {
        let kept = workspace.keep();
        println!("Kept isolated CCS test workspace: {}", kept.display());
    }
    Ok(())
}
