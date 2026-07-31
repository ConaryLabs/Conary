// apps/conary/src/commands/model/apply/packages.rs

//! Atomic package-set execution for declarative model actions.

use anyhow::Result;
use conary_core::model::DiffAction;

use crate::commands::install::{PackageSetRequest, install_package_set};
use crate::commands::{SandboxMode, cmd_remove};

/// Apply removal actions, then one atomic install/update package set.
///
/// Returns `(applied_count, error_list)`.
pub(super) async fn apply_package_changes(
    db_path: &str,
    _root: &str,
    actions: &[&DiffAction],
    strict: bool,
) -> Result<(usize, Vec<String>)> {
    let mut applied = 0usize;
    let mut errors = Vec::new();

    for action in actions {
        if let DiffAction::Remove {
            package,
            current_version,
            architectures,
        } = action
        {
            let iterations = architectures.len().max(1);
            for arch in architectures
                .iter()
                .map(Some)
                .chain(std::iter::once(None))
                .take(iterations)
            {
                match arch {
                    Some(arch) => {
                        println!("Removing {} {} [{}]...", package, current_version, arch)
                    }
                    None => println!("Removing {} {}...", package, current_version),
                }

                match cmd_remove(
                    package,
                    db_path,
                    Some(current_version.clone()),
                    arch.cloned(),
                    SandboxMode::Always,
                    false,
                )
                .await
                {
                    Ok(()) => {
                        println!("  Removed {}", package);
                        applied += 1;
                    }
                    Err(error) => {
                        let message = match arch {
                            Some(arch) => format!(
                                "Remove '{}' {} [{}]: {}",
                                package, current_version, arch, error
                            ),
                            None => {
                                format!("Remove '{}' {}: {}", package, current_version, error)
                            }
                        };
                        eprintln!(
                            "{}",
                            crate::ui::row_line(crate::ui::Status::Fail, &[&message])
                        );
                        if strict {
                            anyhow::bail!(message);
                        }
                        errors.push(message);
                    }
                }
            }
        }
    }

    let requests = package_set_requests(actions);
    if requests.is_empty() {
        return Ok((applied, errors));
    }
    for action in actions {
        match action {
            DiffAction::Install { package, pin, .. } => match pin {
                Some(pin) => println!("Installing {} ({})...", package, pin),
                None => println!("Installing {}...", package),
            },
            DiffAction::Update {
                package,
                current_version,
                target_version,
            } => println!(
                "Updating {} from {} to {}...",
                package, current_version, target_version
            ),
            _ => {}
        }
    }

    match install_package_set(db_path, SandboxMode::Always, requests.clone()).await {
        Ok(count) => {
            for action in actions {
                match action {
                    DiffAction::Install { package, .. } => println!("  Installed {}", package),
                    DiffAction::Update {
                        package,
                        target_version,
                        ..
                    } => println!("  Updated {} to {}", package, target_version),
                    _ => {}
                }
            }
            applied += count;
        }
        Err(error) => {
            let message = format!("Model package set: {error}");
            eprintln!(
                "{}",
                crate::ui::row_line(crate::ui::Status::Fail, &[&message])
            );
            if strict {
                anyhow::bail!(message);
            }
            errors.push(message);
        }
    }

    Ok((applied, errors))
}

fn package_set_requests(actions: &[&DiffAction]) -> Vec<PackageSetRequest> {
    actions
        .iter()
        .filter_map(|action| match action {
            DiffAction::Install { package, pin, .. } => Some(PackageSetRequest {
                name: package.clone(),
                version: pin.clone(),
                selection_reason: "Installed by model apply".to_string(),
                allow_downgrade: false,
            }),
            DiffAction::Update {
                package,
                target_version,
                ..
            } => Some(PackageSetRequest {
                name: package.clone(),
                version: Some(target_version.clone()),
                selection_reason: "Updated by model apply".to_string(),
                allow_downgrade: true,
            }),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_and_update_actions_form_one_typed_package_set() {
        let install = DiffAction::Install {
            package: "dbus-broker".to_string(),
            pin: None,
            optional: false,
        };
        let remove = DiffAction::Remove {
            package: "old".to_string(),
            current_version: "1-1".to_string(),
            architectures: vec!["x86_64".to_string()],
        };
        let update = DiffAction::Update {
            package: "systemd".to_string(),
            current_version: "258-1".to_string(),
            target_version: "259-1".to_string(),
        };

        assert_eq!(
            package_set_requests(&[&install, &remove, &update]),
            [
                PackageSetRequest {
                    name: "dbus-broker".to_string(),
                    version: None,
                    selection_reason: "Installed by model apply".to_string(),
                    allow_downgrade: false,
                },
                PackageSetRequest {
                    name: "systemd".to_string(),
                    version: Some("259-1".to_string()),
                    selection_reason: "Updated by model apply".to_string(),
                    allow_downgrade: true,
                },
            ]
        );
    }
}
