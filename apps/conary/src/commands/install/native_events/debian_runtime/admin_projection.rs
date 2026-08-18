// apps/conary/src/commands/install/native_events/debian_runtime/admin_projection.rs

//! Exact dpkg administrative view projected inside the selected root.

use super::super::{NativeBundleOwner, PreparedNativeTransaction};
use super::{DPKG_ADMIN_COMPAT_DIRECTORY, DPKG_MAINTSCRIPT_ABI_VERSION};
use crate::commands::live_root;
use anyhow::{Context, Result, bail};
use conary_core::ccs::native_lifecycle::{
    DebControlMember, DebTriggerAwaitMode, DebTriggerDirective, NativeLifecycleBundle, SourceFormat,
};
use conary_core::ccs::native_transaction::{
    DebPackageState, NativeBundleRole, NativeEventPlacement, NativeEventStage,
    NativePackageIdentity, NativeTransactionEvent, NativeTransactionOperation,
};
use conary_core::db::models::{
    InstalledNativeLifecycleBundle, NativeLifecycleResidualState, PackagePayloadOwnership, Trove,
};
use rusqlite::Connection;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path, PathBuf};

const PROJECTION_MARKER: &str = ".conary-dpkg-projection";
const PROJECTION_FORMAT: &str = "conary.dpkg-admindir.v1\n";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::commands::install::native_events) struct DebianConfigSnapshot {
    package_name: String,
    package_version: String,
    package_arch: Option<String>,
    path: String,
    status_digest: DebianConfigStatusDigest,
    remove_on_upgrade: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum DebianConfigStatusDigest {
    Md5(String),
    NewConffile,
}

impl DebianConfigStatusDigest {
    fn as_str(&self) -> &str {
        match self {
            Self::Md5(value) => value,
            Self::NewConffile => "newconffile",
        }
    }
}

#[derive(Debug)]
pub(in crate::commands::install::native_events) struct DebianAdminSnapshot {
    pub(super) admin_directory: PathBuf,
    pub(super) projected_packages: Vec<NativePackageIdentity>,
}

#[derive(Debug, Clone)]
struct PackageCandidate {
    identity: NativePackageIdentity,
    lifecycle_state: DebPackageState,
    pending_triggers: Vec<String>,
    awaited_packages: Vec<NativePackageIdentity>,
    paths: BTreeSet<String>,
    description: Option<String>,
    bundle: NativeLifecycleBundle,
    role: Option<NativeBundleRole>,
    installed_trove: bool,
    persisted_state: bool,
    want: &'static str,
}

pub(in crate::commands::install::native_events) fn config_snapshots(
    conn: &Connection,
) -> Result<Vec<DebianConfigSnapshot>> {
    let mut statement = conn.prepare(
        "SELECT package_name, package_version, package_architecture, path,
                original_md5, remove_on_upgrade
         FROM config_files
         WHERE source = 'deb'
         ORDER BY package_name, package_version, package_architecture, path",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Option<String>>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, Option<String>>(4)?,
            row.get::<_, i32>(5)? != 0,
        ))
    })?;
    rows.map(|row| {
        let (package_name, package_version, package_arch, path, original_md5, remove) =
            row?;
        let status_digest = match original_md5 {
            Some(original_md5) => {
                validate_md5(&original_md5).with_context(|| {
                    format!(
                        "Debian conffile {path} for {package_name} {package_version} has invalid shipped MD5"
                    )
                })?;
                DebianConfigStatusDigest::Md5(original_md5)
            }
            None if remove => DebianConfigStatusDigest::NewConffile,
            None => {
                bail!(
                    "Debian conffile {path} for {package_name} {package_version} has no shipped MD5 authority"
                )
            }
        };
        Ok(DebianConfigSnapshot {
            package_name,
            package_version,
            package_arch,
            path: absolute_package_path(&path)?,
            status_digest,
            remove_on_upgrade: remove,
        })
    })
    .collect()
}

impl PreparedNativeTransaction {
    pub(in crate::commands::install::native_events) fn materialize_debian_admin(
        &self,
        conn: &Connection,
        event: Option<&NativeTransactionEvent>,
        root: &Path,
    ) -> Result<Option<DebianAdminSnapshot>> {
        if event.is_some_and(|event| event.source_format != SourceFormat::Deb.as_str()) {
            return Ok(None);
        }
        if !self
            .owners
            .iter()
            .any(|owner| owner.bundle.source_format == SourceFormat::Deb)
        {
            return Ok(None);
        }
        reject_live_root(root)?;
        let current_configs = config_snapshots(conn)?;
        let candidates = self.package_candidates(conn)?;
        let mut projected = self.select_package_candidates(candidates, event)?;
        projected.sort_by(|left, right| left.identity.cmp(&right.identity));
        reject_unmodeled_multiarch(&projected)?;

        let admin_directory = prepare_admin_directory(root)?;
        write_admin_files(
            &admin_directory,
            &projected,
            &self.debian_config_before,
            &current_configs,
            event,
        )?;
        super::alternatives_state::materialize(conn, &admin_directory, root)?;
        Ok(Some(DebianAdminSnapshot {
            admin_directory,
            projected_packages: projected
                .into_iter()
                .map(|package| package.identity)
                .collect(),
        }))
    }

    pub(in crate::commands::install) fn refresh_debian_admin_projection(
        &self,
        conn: &Connection,
        root: &Path,
    ) -> Result<()> {
        self.materialize_debian_admin(conn, None, root)?;
        Ok(())
    }

    fn package_candidates(&self, conn: &Connection) -> Result<Vec<PackageCandidate>> {
        let mut candidates = Vec::new();
        for row in InstalledNativeLifecycleBundle::find_all(conn)? {
            if row.source_format != SourceFormat::Deb.as_str() {
                continue;
            }
            let bundle = row.bundle()?;
            let trove = Trove::find_by_id(conn, row.trove_id)?.with_context(|| {
                format!(
                    "installed Debian lifecycle row for {} points to missing trove {}",
                    row.source_package, row.trove_id
                )
            })?;
            let identity = NativePackageIdentity::new(
                &row.source_package,
                &row.source_version,
                row.source_arch.as_deref(),
            );
            candidates.push(PackageCandidate {
                role: owner_role(&self.owners, &identity),
                identity,
                lifecycle_state: row.lifecycle_state,
                pending_triggers: row.pending_triggers,
                awaited_packages: row.awaited_packages,
                paths: PackagePayloadOwnership::load(conn, row.trove_id)?
                    .lifecycle_paths()
                    .iter()
                    .map(|path| absolute_package_path(path))
                    .collect::<Result<_>>()?,
                description: trove.description,
                bundle,
                installed_trove: true,
                persisted_state: true,
                want: "install",
            });
        }
        for row in NativeLifecycleResidualState::find_all(conn)? {
            if row.source_format != SourceFormat::Deb.as_str() {
                continue;
            }
            let bundle = row.bundle()?;
            let identity = row.identity();
            if candidates
                .iter()
                .any(|candidate| candidate.identity == identity)
            {
                bail!(
                    "Debian package state is duplicated for '{} {} {:?}'",
                    identity.package_name,
                    identity.package_version,
                    identity.package_arch
                );
            }
            candidates.push(PackageCandidate {
                role: owner_role(&self.owners, &identity),
                identity,
                lifecycle_state: row.lifecycle_state,
                pending_triggers: row.pending_triggers,
                awaited_packages: row.awaited_packages,
                paths: BTreeSet::new(),
                description: None,
                bundle,
                installed_trove: false,
                persisted_state: true,
                want: "deinstall",
            });
        }
        let missing_owners = self
            .owners
            .iter()
            .filter(|owner| {
                owner.bundle.source_format == SourceFormat::Deb
                    && !candidates.iter().any(|candidate| {
                        candidate.identity.package_name == owner.package_name
                            && candidate.identity.package_version == owner.package_version
                            && candidate.identity.package_arch == owner.bundle.source_arch
                    })
            })
            .collect::<Vec<_>>();
        for owner in missing_owners {
            let identity = owner_identity(owner);
            let state = self.runtime_state(conn, owner)?;
            let change = change_for_owner(&self.changes, owner);
            let snapshot_paths = self
                .transaction_state
                .installed_packages_before
                .iter()
                .find(|package| {
                    package.package_name == identity.package_name
                        && package.package_version == identity.package_version
                        && package.package_arch == identity.package_arch
                })
                .map(|package| &package.paths);
            candidates.push(PackageCandidate {
                identity,
                lifecycle_state: state.lifecycle_state,
                pending_triggers: state.pending_triggers,
                awaited_packages: state.awaited_packages,
                paths: change
                    .map(|change| match owner.role {
                        NativeBundleRole::Installing => &change.new_paths,
                        NativeBundleRole::Removing | NativeBundleRole::Deconfiguring => {
                            &change.old_paths
                        }
                        NativeBundleRole::Installed => &change.old_paths,
                    })
                    .or(snapshot_paths)
                    .into_iter()
                    .flatten()
                    .map(|path| absolute_package_path(path))
                    .collect::<Result<_>>()?,
                description: None,
                bundle: owner.bundle.clone(),
                role: Some(owner.role),
                installed_trove: false,
                persisted_state: false,
                want: want_for_owner(owner, change),
            });
        }
        Ok(candidates)
    }

    fn select_package_candidates(
        &self,
        candidates: Vec<PackageCandidate>,
        event: Option<&NativeTransactionEvent>,
    ) -> Result<Vec<PackageCandidate>> {
        let mut groups = BTreeMap::<(String, Option<String>), Vec<PackageCandidate>>::new();
        for candidate in candidates {
            groups
                .entry((
                    candidate.identity.package_name.clone(),
                    candidate.identity.package_arch.clone(),
                ))
                .or_default()
                .push(candidate);
        }
        groups
            .into_values()
            .filter_map(|group| match select_group(&self.changes, group, event) {
                Ok(Some(candidate)) => Some(Ok(candidate)),
                Ok(None) => None,
                Err(error) => Some(Err(error)),
            })
            .collect()
    }
}

fn select_group(
    changes: &[conary_core::ccs::native_transaction::NativeTransactionChange],
    mut group: Vec<PackageCandidate>,
    event: Option<&NativeTransactionEvent>,
) -> Result<Option<PackageCandidate>> {
    if event.is_none() {
        group.retain(|candidate| candidate.persisted_state);
        if group.is_empty() {
            return Ok(None);
        }
    }
    let relevant_change = event.and_then(event_transaction_index).and_then(|index| {
        changes
            .iter()
            .find(|change| change.transaction_index == index)
    });
    let preferred_version = match (event, relevant_change) {
        (Some(event), Some(change))
            if matches!(
                event.stage,
                NativeEventStage::DebPreRemoveUpgrade | NativeEventStage::DebPostRemoveUpgrade
            ) =>
        {
            change.old_version.as_deref()
        }
        (Some(event), Some(change)) if event.stage == NativeEventStage::PackagePreInstall => {
            match change.operation {
                NativeTransactionOperation::Upgrade => change.old_version.as_deref(),
                NativeTransactionOperation::Install => change.new_version.as_deref(),
                _ => Some(event.owner_version.as_str()),
            }
        }
        (Some(event), _) => Some(event.owner_version.as_str()),
        (None, _) => None,
    };
    let mut selected = preferred_version
        .and_then(|version| {
            group
                .iter()
                .position(|candidate| candidate.identity.package_version == version)
        })
        .map(|index| group.remove(index));
    if selected.is_none() {
        let active = group
            .iter()
            .enumerate()
            .filter(|(_, candidate)| candidate.lifecycle_state != DebPackageState::NotInstalled)
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        selected = match active.as_slice() {
            [] => None,
            [index] => Some(group.remove(*index)),
            _ => {
                let preferred = group.iter().position(|candidate| {
                    candidate.role == Some(NativeBundleRole::Removing) && candidate.installed_trove
                });
                preferred.map(|index| group.remove(index))
            }
        };
    }
    let Some(mut selected) = selected else {
        return Ok(None);
    };
    if let Some(event) = event
        && event.owner_package == selected.identity.package_name
        && event.owner_arch == selected.identity.package_arch
    {
        apply_event_boundary(&mut selected, event, relevant_change)?;
    }
    if selected.lifecycle_state == DebPackageState::NotInstalled {
        return Ok(None);
    }
    Ok(Some(selected))
}

fn apply_event_boundary(
    package: &mut PackageCandidate,
    event: &NativeTransactionEvent,
    change: Option<&conary_core::ccs::native_transaction::NativeTransactionChange>,
) -> Result<()> {
    package.lifecycle_state = match event.stage {
        NativeEventStage::DebPreRemoveUpgrade
        | NativeEventStage::DebPreDeconfigure
        | NativeEventStage::DebPreRemoveInFavour
        | NativeEventStage::DebPostInstall
        | NativeEventStage::DebAwaitedTriggerProcessing => DebPackageState::HalfConfigured,
        NativeEventStage::PackagePreInstall
        | NativeEventStage::DebPostRemoveUpgrade
        | NativeEventStage::PackagePreRemove
        | NativeEventStage::PackagePostRemove => DebPackageState::HalfInstalled,
        NativeEventStage::DebPostRemovePurge => DebPackageState::ConfigFiles,
        _ => package.lifecycle_state,
    };
    if matches!(
        package.lifecycle_state,
        DebPackageState::HalfConfigured | DebPackageState::HalfInstalled
    ) {
        package.pending_triggers.clear();
        package.awaited_packages.clear();
    }
    if event.stage == NativeEventStage::PackagePreInstall
        && change.is_some_and(|change| change.operation == NativeTransactionOperation::Install)
    {
        package.paths.clear();
    }
    validate_trigger_state(package)
}

fn write_admin_files(
    admin: &Path,
    packages: &[PackageCandidate],
    before_configs: &[DebianConfigSnapshot],
    current_configs: &[DebianConfigSnapshot],
    event: Option<&NativeTransactionEvent>,
) -> Result<()> {
    fs::write(admin.join(PROJECTION_MARKER), PROJECTION_FORMAT)?;
    fs::write(admin.join("info/format"), "1\n")?;
    for file in [
        "available",
        "diversions",
        "statoverride",
        "lock",
        "lock-frontend",
        "triggers/Lock",
        "triggers/Unincorp",
    ] {
        fs::write(admin.join(file), [])?;
    }
    fs::set_permissions(
        admin.join("triggers/Lock"),
        fs::Permissions::from_mode(0o600),
    )?;

    let mut status = String::new();
    let mut explicit_interests = BTreeMap::<String, BTreeSet<String>>::new();
    let mut file_interests = BTreeSet::<String>::new();
    for package in packages {
        let configs = configs_for_package(package, before_configs, current_configs, event);
        write_status_stanza(&mut status, package, &configs)?;
        let basename = info_basename(&package.identity)?;
        let mut paths = package.paths.clone();
        paths.extend(configs.iter().map(|config| config.path.clone()));
        fs::write(
            admin.join(format!("info/{basename}.list")),
            lines(paths.iter().map(String::as_str)),
        )?;
        if !configs.is_empty() {
            fs::write(
                admin.join(format!("info/{basename}.conffiles")),
                conffiles_control(&configs),
            )?;
        }
        let declarations = trigger_declarations(&package.bundle);
        if !declarations.is_empty() {
            fs::write(
                admin.join(format!("info/{basename}.triggers")),
                lines(
                    declarations
                        .iter()
                        .map(|declaration| declaration.raw_line.as_str()),
                ),
            )?;
        }
        for declaration in declarations
            .into_iter()
            .filter(|declaration| declaration.directive == DebTriggerDirective::Interest)
        {
            let selector = trigger_interest_selector(&package.identity, declaration.await_mode)?;
            if declaration.trigger_name.starts_with('/') {
                validate_file_trigger_name(&declaration.trigger_name)?;
                file_interests.insert(format!("{} {}", declaration.trigger_name, selector));
            } else {
                validate_explicit_trigger_name(&declaration.trigger_name)?;
                explicit_interests
                    .entry(declaration.trigger_name)
                    .or_default()
                    .insert(selector);
            }
        }
    }
    fs::write(admin.join("status"), status)?;
    fs::write(
        admin.join("triggers/File"),
        lines(file_interests.iter().map(String::as_str)),
    )?;
    for (trigger, interests) in explicit_interests {
        fs::write(
            admin.join("triggers").join(trigger),
            lines(interests.iter().map(String::as_str)),
        )?;
    }
    Ok(())
}

fn write_status_stanza(
    status: &mut String,
    package: &PackageCandidate,
    configs: &[DebianConfigSnapshot],
) -> Result<()> {
    validate_trigger_state(package)?;
    status.push_str(&format!(
        "Package: {}\nStatus: {} ok {}\nArchitecture: {}\nVersion: {}\nMaintainer: Conary dpkg compatibility projection\nDescription: {}\n",
        package.identity.package_name,
        package.want,
        package.lifecycle_state.as_str(),
        package
            .identity
            .package_arch
            .as_deref()
            .context("projected Debian package has no architecture")?,
        package.identity.package_version,
        package
            .description
            .as_deref()
            .unwrap_or("Conary-managed foreign package")
    ));
    if !configs.is_empty() {
        status.push_str("Conffiles:\n");
        for config in configs {
            status.push_str(&format!(
                " {} {}",
                config.path,
                config.status_digest.as_str()
            ));
            if config.remove_on_upgrade {
                status.push_str(" remove-on-upgrade");
            }
            status.push('\n');
        }
    }
    if !package.pending_triggers.is_empty() {
        status.push_str(&format!(
            "Triggers-Pending: {}\n",
            package.pending_triggers.join(" ")
        ));
    }
    if !package.awaited_packages.is_empty() {
        status.push_str("Triggers-Awaited:");
        for awaited in &package.awaited_packages {
            status.push(' ');
            status.push_str(&package_selector(awaited)?);
        }
        status.push('\n');
    }
    status.push_str(&format!(
        "Conary-Dpkg-Projection-Version: {}\n\n",
        DPKG_MAINTSCRIPT_ABI_VERSION
    ));
    Ok(())
}

fn configs_for_package<'a>(
    package: &PackageCandidate,
    before: &'a [DebianConfigSnapshot],
    current: &'a [DebianConfigSnapshot],
    event: Option<&NativeTransactionEvent>,
) -> Vec<DebianConfigSnapshot> {
    let use_before = event.is_some_and(|event| {
        matches!(
            event.stage,
            NativeEventStage::DebPreRemoveUpgrade
                | NativeEventStage::PackagePreInstall
                | NativeEventStage::DebPostRemoveUpgrade
        ) && package.role == Some(NativeBundleRole::Removing)
    });
    let source = if use_before { before } else { current };
    source
        .iter()
        .filter(|config| {
            config.package_name == package.identity.package_name
                && config.package_version == package.identity.package_version
                && config.package_arch == package.identity.package_arch
        })
        .cloned()
        .collect()
}

fn prepare_admin_directory(root: &Path) -> Result<PathBuf> {
    let admin = live_root::target_path(root, DPKG_ADMIN_COMPAT_DIRECTORY)?;
    let parent = admin
        .parent()
        .context("dpkg administrative projection path has no parent")?;
    ensure_plain_directories(root, parent)?;
    match fs::symlink_metadata(&admin) {
        Ok(metadata) if metadata.file_type().is_dir() => {
            if !admin.join(PROJECTION_MARKER).is_file() {
                bail!(
                    "reserved dpkg compatibility directory {} is not a Conary projection",
                    admin.display()
                );
            }
            fs::remove_dir_all(&admin)?;
        }
        Ok(_) => bail!(
            "reserved dpkg administrative projection path {} is not a directory",
            admin.display()
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    fs::create_dir(&admin)?;
    for directory in ["info", "updates", "triggers"] {
        fs::create_dir(admin.join(directory))?;
    }
    Ok(admin)
}

pub(super) fn ensure_plain_directories(root: &Path, target: &Path) -> Result<()> {
    let relative = target
        .strip_prefix(root)
        .context("dpkg compatibility directory escaped selected root")?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        let Component::Normal(component) = component else {
            bail!("dpkg compatibility directory is not normalized");
        };
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_dir() => {}
            Ok(_) => bail!(
                "dpkg compatibility parent {} is not a plain directory",
                current.display()
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(&current)?;
            }
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn trigger_declarations(
    bundle: &NativeLifecycleBundle,
) -> Vec<conary_core::ccs::native_lifecycle::DebTriggerMetadata> {
    bundle
        .entries
        .iter()
        .filter_map(|entry| entry.deb_maintainer.as_ref())
        .filter(|metadata| metadata.control_member == DebControlMember::Triggers)
        .flat_map(|metadata| metadata.trigger_declarations.iter().cloned())
        .collect()
}

fn validate_trigger_state(package: &PackageCandidate) -> Result<()> {
    match package.lifecycle_state {
        DebPackageState::TriggersAwaited if package.awaited_packages.is_empty() => {
            bail!("triggers-awaited package has no awaited package identities")
        }
        DebPackageState::TriggersPending if package.pending_triggers.is_empty() => {
            bail!("triggers-pending package has no pending trigger names")
        }
        DebPackageState::NotInstalled
        | DebPackageState::ConfigFiles
        | DebPackageState::HalfInstalled
        | DebPackageState::Unpacked
        | DebPackageState::HalfConfigured
        | DebPackageState::Installed
            if !package.pending_triggers.is_empty() || !package.awaited_packages.is_empty() =>
        {
            bail!(
                "Debian package state {} carries incompatible trigger state",
                package.lifecycle_state.as_str()
            )
        }
        _ => Ok(()),
    }
}

fn reject_live_root(root: &Path) -> Result<()> {
    if root == Path::new("/") {
        bail!("dpkg compatibility projection requires a selected root, not '/'");
    }
    Ok(())
}

fn reject_unmodeled_multiarch(packages: &[PackageCandidate]) -> Result<()> {
    let mut arches = BTreeMap::<&str, BTreeSet<Option<&str>>>::new();
    for package in packages {
        arches
            .entry(&package.identity.package_name)
            .or_default()
            .insert(package.identity.package_arch.as_deref());
    }
    if let Some((name, package_arches)) = arches.iter().find(|(_, arches)| arches.len() > 1) {
        bail!(
            "Debian package '{name}' has {} co-installed architectures but no typed Multi-Arch control authority",
            package_arches.len()
        );
    }
    Ok(())
}

fn event_transaction_index(event: &NativeTransactionEvent) -> Option<usize> {
    match event.placement {
        NativeEventPlacement::TransactionElement { transaction_index } => Some(transaction_index),
        NativeEventPlacement::TransactionBefore | NativeEventPlacement::TransactionAfter => None,
    }
}

fn owner_identity(owner: &NativeBundleOwner) -> NativePackageIdentity {
    NativePackageIdentity::new(
        &owner.package_name,
        &owner.package_version,
        owner.bundle.source_arch.as_deref(),
    )
}

fn owner_role(
    owners: &[NativeBundleOwner],
    identity: &NativePackageIdentity,
) -> Option<NativeBundleRole> {
    owners
        .iter()
        .find(|owner| owner_identity(owner) == *identity)
        .map(|owner| owner.role)
}

fn change_for_owner<'a>(
    changes: &'a [conary_core::ccs::native_transaction::NativeTransactionChange],
    owner: &NativeBundleOwner,
) -> Option<&'a conary_core::ccs::native_transaction::NativeTransactionChange> {
    changes.iter().find(|change| {
        change.package_name == owner.package_name
            && match owner.role {
                NativeBundleRole::Installing => {
                    change.new_version.as_deref() == Some(owner.package_version.as_str())
                        && change.new_arch == owner.bundle.source_arch
                }
                NativeBundleRole::Removing | NativeBundleRole::Deconfiguring => {
                    change.old_version.as_deref() == Some(owner.package_version.as_str())
                        && change.old_arch == owner.bundle.source_arch
                }
                NativeBundleRole::Installed => false,
            }
    })
}

fn want_for_owner(
    owner: &NativeBundleOwner,
    change: Option<&conary_core::ccs::native_transaction::NativeTransactionChange>,
) -> &'static str {
    match change.map(|change| change.operation) {
        Some(NativeTransactionOperation::Purge) => "purge",
        Some(operation) if operation.removes_package() => "deinstall",
        _ if owner.role == NativeBundleRole::Removing => "deinstall",
        _ => "install",
    }
}

fn info_basename(identity: &NativePackageIdentity) -> Result<String> {
    validate_debian_package_name(&identity.package_name)?;
    Ok(identity.package_name.clone())
}

fn package_selector(identity: &NativePackageIdentity) -> Result<String> {
    validate_debian_package_name(&identity.package_name)?;
    Ok(identity.package_name.clone())
}

fn trigger_interest_selector(
    identity: &NativePackageIdentity,
    await_mode: DebTriggerAwaitMode,
) -> Result<String> {
    let mut selector = package_selector(identity)?;
    if await_mode == DebTriggerAwaitMode::NoAwait {
        selector.push_str("/noawait");
    }
    Ok(selector)
}

fn validate_debian_package_name(name: &str) -> Result<()> {
    if name.len() < 2
        || !name.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'+' | b'-' | b'.')
        })
        || !name
            .as_bytes()
            .first()
            .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
    {
        bail!("invalid Debian package name '{name}'");
    }
    Ok(())
}

fn validate_explicit_trigger_name(name: &str) -> Result<()> {
    validate_debian_package_name(name)
        .with_context(|| format!("invalid explicit Debian trigger name '{name}'"))
}

fn validate_file_trigger_name(name: &str) -> Result<()> {
    if !name.starts_with('/')
        || name.contains("//")
        || name.bytes().any(|byte| !(0x21..0x7f).contains(&byte))
    {
        bail!("invalid Debian file trigger name '{name}'");
    }
    Ok(())
}

fn validate_md5(value: &str) -> Result<()> {
    if value.len() != 32 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("invalid MD5 digest '{value}'");
    }
    Ok(())
}

pub(super) fn absolute_package_path(path: &str) -> Result<String> {
    let mut parts = Vec::new();
    for component in Path::new(path).components() {
        match component {
            Component::RootDir => {}
            Component::Normal(component) => parts.push(
                component
                    .to_str()
                    .context("Debian package path is not UTF-8")?,
            ),
            Component::CurDir | Component::ParentDir | Component::Prefix(_) => {
                bail!("Debian package path '{path}' is not normalized")
            }
        }
    }
    if parts.is_empty() {
        bail!("Debian package path '{path}' does not name a path below root");
    }
    Ok(format!("/{}", parts.join("/")))
}

fn lines<'a>(values: impl Iterator<Item = &'a str>) -> String {
    let mut output = values.collect::<Vec<_>>().join("\n");
    if !output.is_empty() {
        output.push('\n');
    }
    output
}

fn conffiles_control(configs: &[DebianConfigSnapshot]) -> String {
    let mut output = String::new();
    for config in configs {
        if config.remove_on_upgrade {
            output.push_str("remove-on-upgrade ");
            output.push_str(&config.path);
        } else {
            output.push_str(&config.path);
        }
        output.push('\n');
    }
    output
}

#[cfg(test)]
#[path = "admin_projection/tests.rs"]
mod tests;
