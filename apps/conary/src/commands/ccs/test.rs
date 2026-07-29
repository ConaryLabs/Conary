// apps/conary/src/commands/ccs/test.rs

use anyhow::{Context, Result};
use conary_core::ccs::{
    ExecutableInterface, HOST_CAPABILITY_INVENTORY_SCHEMA_VERSION, HostCapabilityInventory,
    InitSystemCapability, SystemdInterface, TmpfilesInterface,
};
use std::fs;
use std::os::unix::fs::PermissionsExt;
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
    persist_isolated_test_capabilities(workspace.path(), &db_path)?;

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
    println!("  host model: complete typed lifecycle interfaces (dry-run only)");

    let db_path_string = db_path.to_string_lossy().into_owned();
    let root_string = root.to_string_lossy().into_owned();

    // A dry-run plans and reports signed lifecycle authority but returns before
    // any declarative or executable hook runs. SandboxMode::Always therefore
    // describes the intentionally absent execution path, not permission for a
    // script to escape isolation.
    super::cmd_ccs_install(
        package,
        &db_path_string,
        &root_string,
        true,
        Some(policy),
        None,
        crate::commands::SandboxMode::Always,
        false,
        false,
    )
    .await?;

    if keep_workspace {
        let kept = workspace.keep();
        println!("Kept isolated CCS test workspace: {}", kept.display());
    }
    Ok(())
}

fn persist_isolated_test_capabilities(workspace: &Path, db_path: &Path) -> Result<()> {
    let interfaces = workspace.join("host-interfaces");
    fs::create_dir_all(&interfaces).context("create isolated host-interface directory")?;

    let systemctl = write_test_interface(
        &interfaces,
        "systemctl",
        r#"case "${1:-}" in
    --version) printf 'systemd 257\n' ;;
    show) printf '257\n' ;;
esac
"#,
    )?;
    let sysusers = write_test_interface(
        &interfaces,
        "systemd-sysusers",
        "if [ \"${1:-}\" = \"--version\" ]; then printf 'systemd 257\\n'; fi\n",
    )?;
    let tmpfiles = write_test_interface(
        &interfaces,
        "systemd-tmpfiles",
        "if [ \"${1:-}\" = \"--version\" ]; then printf 'systemd 257\\n'; fi\n",
    )?;
    let sysctl = write_test_interface(
        &interfaces,
        "sysctl",
        "if [ \"${1:-}\" = \"--version\" ]; then printf 'sysctl from procps-ng 4.0.6\\n'; fi\n",
    )?;
    let ldconfig = write_test_interface(
        &interfaces,
        "ldconfig",
        "if [ \"${1:-}\" = \"--version\" ]; then printf 'ldconfig (GNU libc) 2.40\\n'; fi\n",
    )?;

    let inventory = HostCapabilityInventory {
        schema_version: HOST_CAPABILITY_INVENTORY_SCHEMA_VERSION,
        init_system: InitSystemCapability::Systemd,
        systemd: Some(
            SystemdInterface::probe(systemctl, true)
                .context("probe isolated systemctl contract")?,
        ),
        sysusers: Some(
            ExecutableInterface::probe_sysusers(sysusers)
                .context("probe isolated systemd-sysusers contract")?,
        ),
        tmpfiles: Some(
            TmpfilesInterface::probe(tmpfiles)
                .context("probe isolated systemd-tmpfiles contract")?,
        ),
        sysctl: Some(
            ExecutableInterface::probe_sysctl(sysctl).context("probe isolated sysctl contract")?,
        ),
        ldconfig: Some(
            ExecutableInterface::probe_ldconfig(ldconfig)
                .context("probe isolated ldconfig contract")?,
        ),
        immutable_backing_security: None,
    };
    let conn = conary_core::db::open(db_path).context("open isolated test database")?;
    inventory
        .persist(&conn)
        .context("persist isolated typed host capability inventory")
}

fn write_test_interface(directory: &Path, name: &str, body: &str) -> Result<std::path::PathBuf> {
    let path = directory.join(name);
    fs::write(&path, format!("#!/bin/sh\nset -eu\n{body}exit 0\n"))
        .with_context(|| format!("write isolated {name} interface"))?;
    let mut permissions = fs::metadata(&path)
        .with_context(|| format!("stat isolated {name} interface"))?
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&path, permissions)
        .with_context(|| format!("make isolated {name} interface executable"))?;
    Ok(path)
}
