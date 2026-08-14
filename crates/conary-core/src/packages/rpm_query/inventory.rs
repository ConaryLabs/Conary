// conary-core/src/packages/rpm_query/inventory.rs

//! Fixed-process coherent RPM installed-inventory acquisition.

use std::collections::{BTreeMap, HashSet};

use rpm::FileFlags;

use crate::error::{Error, Result};
use crate::packages::install_reason::query_package_names;
use crate::packages::{
    InstalledConfigAuthority, InstalledInventoryFile, InstalledInventoryPackage,
    InstalledInventorySnapshot, InstalledInventoryWork, InstalledLifecycleAuthority,
    NativeInstallReason, RpmInstalledLifecycleSlot, SystemPackageManager,
};

const LIFECYCLE_FORMAT: &str = concat!(
    "%{PREINPROG}\x1e%{PREIN}\x1f\x1d",
    "%{POSTINPROG}\x1e%{POSTIN}\x1f\x1d",
    "%{PREUNPROG}\x1e%{PREUN}\x1f\x1d",
    "%{POSTUNPROG}\x1e%{POSTUN}\x1f\x1d",
    "%{PRETRANSPROG}\x1e%{PRETRANS}\x1f\x1d",
    "%{POSTTRANSPROG}\x1e%{POSTTRANS}\x1f\x1d",
    "%{PREUNTRANSPROG}\x1e%{PREUNTRANS}\x1f\x1d",
    "%{POSTUNTRANSPROG}\x1e%{POSTUNTRANS}\x1f\x1d",
    "%{VERIFYSCRIPTPROG}\x1e%{VERIFYSCRIPT}\x1f\x1d",
    "[%{TRIGGERSCRIPTPROG}\x1e%{TRIGGERSCRIPTS}\x1f]\x1d",
    "[%{FILETRIGGERSCRIPTPROG}\x1e%{FILETRIGGERSCRIPTS}\x1f]\x1d",
    "[%{TRANSFILETRIGGERSCRIPTPROG}\x1e%{TRANSFILETRIGGERSCRIPTS}\x1f]",
);

/// Query all installed header fields in one rpmdb iteration and DNF reason
/// authority in at most two fixed probes.
pub fn query_installed_inventory() -> Result<InstalledInventorySnapshot> {
    let format = format!(
        "{}\x1d{}\x1d{}\x1d{}\x1d{}\x1c",
        super::RPM_PACKAGE_RECORD_FORMAT,
        super::RPM_FILE_RECORD_FORMAT,
        super::RPM_REQUIREMENT_RECORD_FORMAT,
        super::provides::RPM_PROVIDE_RECORD_FORMAT,
        LIFECYCLE_FORMAT,
    );
    let output = crate::packages::query_common::run_query_command(
        "rpm",
        &["-qa", "--queryformat", &format],
    )?;
    let mut reason_processes = 0_u64;
    let explicit = super::query_user_installed_with(|authority, command, args| {
        reason_processes += 1;
        query_package_names(authority, command, args)
    })
    .map_err(|error| Error::InitError(error.to_string()))?;
    capture_from_output(&output, &explicit, 1 + reason_processes)
}

fn capture_from_output(
    output: &str,
    explicit: &HashSet<String>,
    manager_processes: u64,
) -> Result<InstalledInventorySnapshot> {
    let mut packages = Vec::new();
    let mut ownership_records = 0_u64;
    for (record_index, record) in output
        .split('\x1c')
        .filter(|record| !record.is_empty())
        .enumerate()
    {
        let sections = record.splitn(5, '\x1d').collect::<Vec<_>>();
        if sections.len() != 5 {
            return Err(Error::ParseError(format!(
                "RPM bulk inventory record {} has {} sections; expected 5",
                record_index + 1,
                sections.len()
            )));
        }
        let mut identity_records = super::parse_package_query_records(sections[0])?;
        if identity_records.is_empty() {
            // RPM public-key headers share the database but are not packages.
            continue;
        }
        if identity_records.len() != 1 {
            return Err(Error::ParseError(format!(
                "RPM bulk inventory record {} produced more than one identity",
                record_index + 1
            )));
        }
        let package_record = identity_records.pop().expect("one identity");
        let identity = package_record.identity;
        let config = parse_file_config(sections[1])?;
        let files = super::parse_rpm_file_records(sections[1])?
            .into_iter()
            .map(|file| InstalledInventoryFile {
                config: config.get(&file.path).cloned(),
                file,
            })
            .collect::<Vec<_>>();
        ownership_records = ownership_records
            .checked_add(u64::try_from(files.len()).map_err(|_| {
                Error::ParseError("RPM ownership record count exceeds u64".to_string())
            })?)
            .ok_or_else(|| {
                Error::ParseError("RPM ownership record count overflowed u64".to_string())
            })?;
        let requirements = super::parse_rpm_requirement_records(sections[2])?;
        let provides = super::provides::parse_rpm_provide_records(&identity, sections[3])?;
        let lifecycle = parse_lifecycle(sections[4])?;
        let is_explicit = explicit.contains(&identity.install_reason_selector());
        packages.push(InstalledInventoryPackage {
            identity,
            description: package_record
                .info
                .description
                .or(package_record.info.summary),
            install_reason: if is_explicit {
                NativeInstallReason::Explicit
            } else {
                NativeInstallReason::Dependency
            },
            files,
            requirements,
            provides,
            lifecycle,
        });
    }
    InstalledInventorySnapshot::from_packages(
        SystemPackageManager::Rpm,
        packages,
        InstalledInventoryWork {
            manager_processes,
            database_bulk_reads: 1,
            ownership_records,
        },
    )
}

fn parse_file_config(output: &str) -> Result<BTreeMap<String, InstalledConfigAuthority>> {
    let mut config = BTreeMap::new();
    for (record_index, record) in output
        .split('\x1f')
        .filter(|record| !record.is_empty())
        .enumerate()
    {
        let fields = record.split('\x1e').collect::<Vec<_>>();
        if fields.len() != 10 {
            return Err(Error::ParseError(format!(
                "RPM file config record {} has {} fields; expected 10",
                record_index + 1,
                fields.len()
            )));
        }
        match fields[9].parse::<i32>() {
            Ok(0 | 3) => {}
            Ok(1 | 2 | 4) => continue,
            Ok(state) => {
                return Err(Error::ParseError(format!(
                    "RPM file config record {} has unsupported FILESTATES value {state}",
                    record_index + 1
                )));
            }
            Err(error) => {
                return Err(Error::ParseError(format!(
                    "RPM file config record {} has invalid FILESTATES value {:?}: {error}",
                    record_index + 1,
                    fields[9]
                )));
            }
        }
        let bits = u32::from_str_radix(fields[8], 16).map_err(|error| {
            Error::ParseError(format!(
                "RPM file config record {} has invalid FILEFLAGS {:?}: {error}",
                record_index + 1,
                fields[8]
            ))
        })?;
        let flags = FileFlags::from_bits_retain(bits);
        if flags.intersects(
            FileFlags::CONFIG | FileFlags::NOREPLACE | FileFlags::MISSINGOK | FileFlags::GHOST,
        ) {
            let path = crate::packages::archive_utils::normalize_path(fields[0])
                .map_err(|error| Error::ParseError(error.to_string()))?;
            if config
                .insert(
                    path.clone(),
                    InstalledConfigAuthority::Rpm {
                        config: flags.contains(FileFlags::CONFIG),
                        noreplace: flags.contains(FileFlags::NOREPLACE),
                        missing_ok: flags.contains(FileFlags::MISSINGOK),
                        ghost: flags.contains(FileFlags::GHOST),
                    },
                )
                .is_some()
            {
                return Err(Error::ConflictError(format!(
                    "RPM file config authority repeats path {path:?}"
                )));
            }
        }
    }
    Ok(config)
}

fn parse_lifecycle(output: &str) -> Result<Vec<InstalledLifecycleAuthority>> {
    let slots = [
        RpmInstalledLifecycleSlot::PreInstall,
        RpmInstalledLifecycleSlot::PostInstall,
        RpmInstalledLifecycleSlot::PreRemove,
        RpmInstalledLifecycleSlot::PostRemove,
        RpmInstalledLifecycleSlot::PreTransaction,
        RpmInstalledLifecycleSlot::PostTransaction,
        RpmInstalledLifecycleSlot::PreUntransaction,
        RpmInstalledLifecycleSlot::PostUntransaction,
        RpmInstalledLifecycleSlot::Verify,
        RpmInstalledLifecycleSlot::Trigger,
        RpmInstalledLifecycleSlot::FileTrigger,
        RpmInstalledLifecycleSlot::TransactionFileTrigger,
    ];
    let sections = output.split('\x1d').collect::<Vec<_>>();
    if sections.len() != slots.len() {
        return Err(Error::ParseError(format!(
            "RPM lifecycle inventory has {} slot sections; expected {}",
            sections.len(),
            slots.len()
        )));
    }
    let mut lifecycle = Vec::new();
    for (slot, section) in slots.into_iter().zip(sections) {
        for (record_index, record) in section
            .split('\x1f')
            .filter(|record| !record.is_empty())
            .enumerate()
        {
            let fields = record.split('\x1e').collect::<Vec<_>>();
            if fields.len() != 2 {
                return Err(Error::ParseError(format!(
                    "RPM {slot:?} lifecycle record {} has {} fields; expected 2",
                    record_index + 1,
                    fields.len()
                )));
            }
            if fields
                .iter()
                .all(|field| field.is_empty() || *field == "(none)")
            {
                continue;
            }
            let bytes = [fields[0].as_bytes(), b"\0", fields[1].as_bytes()].concat();
            lifecycle.push(InstalledLifecycleAuthority::Rpm {
                slot,
                sha256: format!("sha256:{}", crate::hash::sha256(&bytes)),
                size: bytes.len() as u64,
            });
        }
    }
    Ok(lifecycle)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lifecycle(post: &str) -> String {
        let mut sections = vec!["(none)\x1e(none)\x1f".to_string(); 12];
        sections[1] = format!("/bin/sh\x1e{post}\x1f");
        sections.join("\x1d")
    }

    fn output(version: &str, post: &str) -> String {
        let package = format!(
            "fixture-{version}-1.x86_64\x1efixture\x1e{version}\x1e1\x1e(none)\x1ex86_64\x1edescription\x1esummary\x1eMIT\x1e(none)\x1e(none)\x1e(none)\x1e(none)\x1e1\x1f"
        );
        let files = concat!(
            "/etc/fixture.conf\x1e1\x1e1\x1e0123456789abcdef\x1e100644\x1eroot\x1eroot\x1e\x1e11\x1e0\x1f",
            "/usr/bin/fixture\x1e1\x1e1\x1efedcba9876543210\x1e100755\x1eroot\x1eroot\x1e\x1e0\x1e0\x1f"
        );
        let requirements = "glibc.so.6()(64bit)\x1e0\x1e\x1f";
        let provides = "fixture\x1e8\x1e1.0-1\x1f";
        format!(
            "{package}\x1d{files}\x1d{requirements}\x1d{provides}\x1d{}\x1c",
            lifecycle(post)
        )
    }

    #[test]
    fn rpm_inventory_uses_a_fixed_process_count() {
        let explicit = HashSet::from(["fixture.x86_64".to_string()]);
        let snapshot = capture_from_output(&output("1.0", "exit 0"), &explicit, 2).unwrap();
        let package = snapshot.package("fixture-1.0-1.x86_64").unwrap();

        assert_eq!(package.install_reason, NativeInstallReason::Explicit);
        assert_eq!(package.files.len(), 2);
        assert!(package.files[0].config.is_some());
        assert_eq!(package.requirements.len(), 1);
        assert_eq!(package.provides.len(), 2);
        assert_eq!(package.lifecycle.len(), 1);
        assert_eq!(
            snapshot.work(),
            InstalledInventoryWork {
                manager_processes: 2,
                database_bulk_reads: 1,
                ownership_records: 2,
            }
        );
    }

    #[test]
    fn rpm_version_reason_config_and_lifecycle_change_token() {
        let explicit = HashSet::from(["fixture.x86_64".to_string()]);
        let first = capture_from_output(&output("1.0", "exit 0"), &explicit, 2).unwrap();
        let second = capture_from_output(&output("2.0", "changed"), &HashSet::new(), 3).unwrap();

        assert_ne!(first.token(), second.token());
        assert_eq!(
            second
                .package("fixture-2.0-1.x86_64")
                .unwrap()
                .install_reason,
            NativeInstallReason::Dependency
        );
    }
}
