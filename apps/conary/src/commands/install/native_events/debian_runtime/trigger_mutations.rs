// apps/conary/src/commands/install/native_events/debian_runtime/trigger_mutations.rs

//! Strict round-trip of dpkg helper mutations into Conary-owned state.

use super::super::{NativeBundleOwner, PreparedNativeTransaction};
use super::DebianAdminSnapshot;
use crate::commands::install::config_files;
use anyhow::{Context, Result, bail};
use conary_core::ccs::native_lifecycle::{
    DebControlMember, DebTriggerAwaitMode, DebTriggerDirective, SourceFormat,
};
use conary_core::ccs::native_transaction::{
    DebPackageState, NativePackageIdentity, NativeTransactionEvent,
};
use conary_core::db::models::{ConfigFile, ConfigSource};
use rusqlite::Connection;
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
struct DeferredActivation {
    trigger: String,
    awaiters: Vec<String>,
}

impl PreparedNativeTransaction {
    pub(in crate::commands::install::native_events) fn capture_debian_admin_mutations(
        &self,
        conn: &Connection,
        snapshot: &DebianAdminSnapshot,
        root: &Path,
        _event: &NativeTransactionEvent,
    ) -> Result<()> {
        let marker = fs::read_to_string(snapshot.admin_directory.join(".conary-dpkg-projection"))
            .context("read dpkg compatibility projection marker")?;
        if marker != "conary.dpkg-admindir.v1\n" {
            bail!("dpkg compatibility projection marker changed during lifecycle execution");
        }
        let deferred_path = snapshot.admin_directory.join("triggers/Unincorp");
        let deferred =
            fs::read_to_string(&deferred_path).context("read dpkg deferred trigger activations")?;
        for activation in parse_deferred_activations(&deferred)? {
            self.persist_helper_activation(conn, snapshot, &activation)?;
        }
        fs::write(deferred_path, [])?;
        super::alternatives_state::capture(conn, &snapshot.admin_directory, root)?;
        self.capture_debian_config_state(conn, root)
    }

    fn persist_helper_activation(
        &self,
        conn: &Connection,
        snapshot: &DebianAdminSnapshot,
        activation: &DeferredActivation,
    ) -> Result<()> {
        let awaiters = activation
            .awaiters
            .iter()
            .filter(|selector| selector.as_str() != "-")
            .map(|selector| projected_identity(snapshot, selector))
            .collect::<Result<Vec<_>>>()?;
        for interested in self.interested_owners(&activation.trigger) {
            let existing = self.runtime_state(conn, interested)?;
            if !trigger_interest_is_active(existing.lifecycle_state) {
                continue;
            }
            let interested_identity = owner_identity(interested);
            let mut pending = existing
                .pending_triggers
                .into_iter()
                .collect::<BTreeSet<_>>();
            pending.insert(activation.trigger.clone());
            let pending = pending.into_iter().collect::<Vec<_>>();
            let interested_state = if existing.awaited_packages.is_empty() {
                DebPackageState::TriggersPending
            } else {
                DebPackageState::TriggersAwaited
            };
            self.update_owner_state(
                conn,
                interested,
                interested_state,
                Some(&pending),
                Some(&existing.awaited_packages),
            )?;

            if interest_await_mode(interested, &activation.trigger)
                == Some(DebTriggerAwaitMode::NoAwait)
            {
                continue;
            }
            for awaiter in &awaiters {
                let owner = self
                    .deb_owner_for_runtime_identity(awaiter)?
                    .with_context(|| {
                        format!(
                            "dpkg-trigger awaiter '{} {} {:?}' has no Debian lifecycle owner",
                            awaiter.package_name, awaiter.package_version, awaiter.package_arch
                        )
                    })?;
                let existing = self.runtime_state(conn, owner)?;
                let mut awaited = existing
                    .awaited_packages
                    .into_iter()
                    .collect::<BTreeSet<_>>();
                awaited.insert(interested_identity.clone());
                let awaited = awaited.into_iter().collect::<Vec<_>>();
                self.update_owner_state(
                    conn,
                    owner,
                    DebPackageState::TriggersAwaited,
                    Some(&existing.pending_triggers),
                    Some(&awaited),
                )?;
            }
        }
        Ok(())
    }

    fn interested_owners<'a>(
        &'a self,
        trigger: &'a str,
    ) -> impl Iterator<Item = &'a NativeBundleOwner> {
        self.owners.iter().filter(move |owner| {
            owner.bundle.source_format == SourceFormat::Deb
                && interest_await_mode(owner, trigger).is_some()
        })
    }

    fn capture_debian_config_state(&self, conn: &Connection, root: &Path) -> Result<()> {
        for mut config in ConfigFile::list_all(conn)?
            .into_iter()
            .filter(|config| config.source == ConfigSource::Deb)
        {
            config_files::record_live_state(&mut config, root)?;
            let id = config
                .id
                .context("persisted Debian conffile has no database identity")?;
            let updated = conn.execute(
                "UPDATE config_files
                 SET current_hash = ?1, status = ?2, modified_at = ?3
                 WHERE id = ?4",
                rusqlite::params![
                    &config.current_hash,
                    config.status.as_str(),
                    &config.modified_at,
                    id,
                ],
            )?;
            if updated != 1 {
                bail!("Debian conffile runtime capture affected {updated} rows for id {id}");
            }
        }
        Ok(())
    }
}

fn interest_await_mode(owner: &NativeBundleOwner, trigger: &str) -> Option<DebTriggerAwaitMode> {
    owner
        .bundle
        .entries
        .iter()
        .filter_map(|entry| entry.deb_maintainer.as_ref())
        .filter(|metadata| metadata.control_member == DebControlMember::Triggers)
        .flat_map(|metadata| &metadata.trigger_declarations)
        .find(|declaration| {
            declaration.directive == DebTriggerDirective::Interest
                && declaration.trigger_name == trigger
        })
        .map(|declaration| declaration.await_mode)
}

fn trigger_interest_is_active(state: DebPackageState) -> bool {
    matches!(
        state,
        DebPackageState::TriggersAwaited
            | DebPackageState::TriggersPending
            | DebPackageState::Installed
    )
}

fn projected_identity(
    snapshot: &DebianAdminSnapshot,
    selector: &str,
) -> Result<NativePackageIdentity> {
    let (name, arch) = selector
        .split_once(':')
        .map_or((selector, None), |(name, arch)| (name, Some(arch)));
    let matches = snapshot
        .projected_packages
        .iter()
        .filter(|identity| {
            identity.package_name == name
                && arch.is_none_or(|arch| identity.package_arch.as_deref() == Some(arch))
        })
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [identity] => Ok((*identity).clone()),
        [] => bail!("dpkg deferred trigger awaiter '{selector}' is not projected"),
        _ => bail!("dpkg deferred trigger awaiter '{selector}' is ambiguous"),
    }
}

fn owner_identity(owner: &NativeBundleOwner) -> NativePackageIdentity {
    NativePackageIdentity::new(
        &owner.package_name,
        &owner.package_version,
        owner.bundle.source_arch.as_deref(),
    )
}

fn parse_deferred_activations(input: &str) -> Result<Vec<DeferredActivation>> {
    let mut activations = Vec::new();
    for (line_index, raw) in input.lines().enumerate() {
        let line = raw.trim_start_matches(char::is_whitespace);
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut tokens = line.split_ascii_whitespace();
        let trigger = tokens
            .next()
            .context("dpkg deferred trigger record has no trigger name")?;
        validate_deferred_trigger(trigger, line_index + 1)?;
        let mut awaiters = Vec::new();
        for token in tokens {
            if token.starts_with('#') {
                break;
            }
            validate_deferred_package(token, line_index + 1)?;
            awaiters.push(token.to_string());
        }
        activations.push(DeferredActivation {
            trigger: trigger.to_string(),
            awaiters,
        });
    }
    Ok(activations)
}

fn validate_deferred_trigger(trigger: &str, line: usize) -> Result<()> {
    if trigger.is_empty() || trigger.bytes().any(|byte| !(0x21..=0x7e).contains(&byte)) {
        bail!("dpkg deferred trigger line {line} has an invalid trigger name");
    }
    Ok(())
}

fn validate_deferred_package(package: &str, line: usize) -> Result<()> {
    if package == "-" {
        return Ok(());
    }
    let Some(first) = package.as_bytes().first() else {
        bail!("dpkg deferred trigger line {line} has an empty package selector");
    };
    if !(first.is_ascii_lowercase() || first.is_ascii_digit()) {
        bail!("dpkg deferred trigger line {line} has invalid package selector '{package}'");
    }
    if package.bytes().any(|byte| {
        !(byte.is_ascii_lowercase()
            || byte.is_ascii_digit()
            || matches!(byte, b'-' | b':' | b'+' | b'.'))
    }) {
        bail!("dpkg deferred trigger line {line} has invalid package selector '{package}'");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deferred_grammar_accepts_upstream_records_and_comments() {
        let parsed = parse_deferred_activations(
            "# retained by dpkg\nrefresh-cache demo:amd64\nicons - # no awaiter\n",
        )
        .unwrap();
        assert_eq!(
            parsed,
            vec![
                DeferredActivation {
                    trigger: "refresh-cache".to_string(),
                    awaiters: vec!["demo:amd64".to_string()],
                },
                DeferredActivation {
                    trigger: "icons".to_string(),
                    awaiters: vec!["-".to_string()],
                },
            ]
        );
    }

    #[test]
    fn deferred_grammar_rejects_non_dpkg_selectors() {
        assert!(parse_deferred_activations("refresh-cache Demo\n").is_err());
        assert!(parse_deferred_activations("refresh-cache demo/await\n").is_err());
        assert!(parse_deferred_activations("bad\u{7f} demo\n").is_err());
    }
}
