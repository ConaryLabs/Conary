// apps/conary/src/commands/ccs/test.rs

use anyhow::{Context, Result};
use conary_core::ccs::package::CcsPackage;
use conary_core::ccs::verify::{self, TrustPolicy};
use std::path::Path;

pub async fn cmd_ccs_test(
    package: &str,
    dry_run: bool,
    policy: Option<String>,
    keep_workspace: bool,
    target_profile: Option<String>,
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
    validate_target_profile_for_package(package_path, &policy, target_profile.as_deref())?;

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

fn validate_target_profile_for_package(
    package_path: &Path,
    policy: &str,
    target_profile: Option<&str>,
) -> Result<()> {
    let trust_policy =
        TrustPolicy::from_file(Path::new(policy)).context("Failed to load trust policy")?;
    let verification = verify::verify_package(package_path, &trust_policy)
        .context("Package verification failed")?;
    if !verification.valid {
        anyhow::bail!("Package verification failed");
    }
    let package = CcsPackage::parse_verified_v2(&package_path.to_string_lossy(), &verification)
        .map_err(anyhow::Error::from)
        .context("parse verified CCS package")?;
    let Some(authority) = package.v2_authority() else {
        return Ok(());
    };
    if lifecycle_is_empty(&authority.lifecycle) {
        return Ok(());
    }
    let profile = super::target_profile::resolve_target_profile(target_profile)?
        .context("m4e-target-profile-required: lifecycle authority requires --target-profile")?;
    conary_core::ccs::v2::validate_authority_with_profile(authority, profile)
        .map_err(|error| anyhow::anyhow!("m4e-lifecycle-unsupported: {error}"))?;
    Ok(())
}

fn lifecycle_is_empty(lifecycle: &conary_core::ccs::v2::schema::LifecycleAuthorityV2) -> bool {
    lifecycle.users.is_empty()
        && lifecycle.groups.is_empty()
        && lifecycle.directories.is_empty()
        && lifecycle.services.is_empty()
        && lifecycle.tmpfiles.is_empty()
        && lifecycle.sysctl.is_empty()
        && lifecycle.alternatives.is_empty()
}
