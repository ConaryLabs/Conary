// conary-core/src/ccs/native_lifecycle/validation.rs
//! Validation for lifecycle evidence and secondary format metadata.

use super::*;

impl NativeLifecycleEntry {
    pub(super) fn validate_source_format(
        &self,
        source_format: &SourceFormat,
    ) -> anyhow::Result<()> {
        let has_rpm =
            self.rpm_runtime.is_some() || self.rpm_trigger.is_some() || self.rpm_sysusers.is_some();
        let has_deb = self.deb_maintainer.is_some();
        let is_debconf_templates = super::debconf::is_debconf_templates_candidate(self);
        let arch_metadata_count =
            usize::from(self.arch_install.is_some()) + usize::from(self.arch_hook.is_some());
        let expected_kind = if is_debconf_templates
            || self.arch_hook.is_some()
            || self
                .deb_maintainer
                .as_ref()
                .is_some_and(|metadata| metadata.control_member == DebControlMember::Triggers)
        {
            NativeLifecycleEntryKind::ControlArtifact
        } else {
            NativeLifecycleEntryKind::Executable
        };
        if self.kind != expected_kind {
            bail!(
                "{} native lifecycle entry '{}' requires kind '{expected_kind:?}', got '{:?}'",
                source_format.as_str(),
                self.id,
                self.kind
            );
        }

        match source_format {
            SourceFormat::Rpm if self.native_slot.is_none() => bail!(
                "RPM native lifecycle entry '{}' has no typed RPM slot class",
                self.id
            ),
            SourceFormat::Rpm if self.rpm_runtime.is_none() => bail!(
                "RPM native lifecycle entry '{}' has no typed RPM runtime contract",
                self.id
            ),
            SourceFormat::Rpm if has_deb || arch_metadata_count != 0 => bail!(
                "RPM native lifecycle entry '{}' carries foreign format metadata",
                self.id
            ),
            SourceFormat::Deb if !has_deb && !is_debconf_templates => bail!(
                "DEB native lifecycle entry '{}' has no typed maintainer-script contract",
                self.id
            ),
            SourceFormat::Deb if self.native_slot.is_some() => bail!(
                "DEB native lifecycle entry '{}' carries an RPM slot class without a declared class table",
                self.id
            ),
            SourceFormat::Deb if has_rpm || arch_metadata_count != 0 => bail!(
                "DEB native lifecycle entry '{}' carries foreign format metadata",
                self.id
            ),
            SourceFormat::Arch if arch_metadata_count != 1 => bail!(
                "Arch native lifecycle entry '{}' must carry exactly one .INSTALL or ALPM hook contract",
                self.id
            ),
            SourceFormat::Arch if self.native_slot.is_some() => bail!(
                "Arch native lifecycle entry '{}' carries an RPM slot class without a declared class table",
                self.id
            ),
            SourceFormat::Arch if has_rpm || has_deb => bail!(
                "Arch native lifecycle entry '{}' carries foreign format metadata",
                self.id
            ),
            _ => Ok(()),
        }
    }
}

impl ArchInstallMetadata {
    pub(super) fn validate(&self, entry_id: &str, body_sha256: &str) -> anyhow::Result<()> {
        validate_sha256("arch_install.install_digest", &self.install_digest)
            .map_err(|error| anyhow!("entry '{entry_id}' {error}"))?;
        if self.install_digest != body_sha256 {
            bail!(
                "entry '{}' Arch .INSTALL digest does not match its preserved body",
                entry_id
            );
        }
        if !matches!(
            self.called_function.as_str(),
            "pre_install"
                | "post_install"
                | "pre_upgrade"
                | "post_upgrade"
                | "pre_remove"
                | "post_remove"
        ) {
            bail!(
                "entry '{}' has unknown Arch .INSTALL function '{}'",
                entry_id,
                self.called_function
            );
        }
        Ok(())
    }
}

impl ArchHookMetadata {
    pub(super) fn validate(&self, entry_id: &str) -> anyhow::Result<()> {
        crate::packages::arch::package_hook_basename(&self.hook_path)
            .map_err(|error| anyhow!("entry '{entry_id}' {error}"))?;
        if self.triggers.is_empty() {
            bail!("entry '{entry_id}' ALPM hook has no triggers");
        }
        for (trigger_index, trigger) in self.triggers.iter().enumerate() {
            if trigger.operations.is_empty() {
                bail!("entry '{entry_id}' ALPM hook trigger {trigger_index} has no operations");
            }
            if trigger.targets.is_empty() {
                bail!("entry '{entry_id}' ALPM hook trigger {trigger_index} has no targets");
            }
            if trigger.targets.iter().any(|target| target.contains('\0')) {
                bail!("entry '{entry_id}' ALPM hook trigger {trigger_index} contains a NUL target");
            }
        }
        required_string("arch_hook.action.exec", &self.action.exec)
            .map_err(|error| anyhow!("entry '{entry_id}' {error}"))?;
        let parsed_argv = crate::packages::arch::alpm_hook_wordsplit(&self.action.exec)
            .filter(|argv| !argv.is_empty())
            .ok_or_else(|| {
                anyhow!("entry '{entry_id}' ALPM hook Exec is not valid wordsplit input")
            })?;
        if parsed_argv != self.action.argv {
            bail!(
                "entry '{entry_id}' ALPM hook argv does not match the exact wordsplit projection of Exec"
            );
        }
        Ok(())
    }
}

impl ResidualLifecycleMetadata {
    pub(super) fn validate(&self, entry_id: &str) -> anyhow::Result<()> {
        validate_optional_sha256(
            "residual_lifecycle.residual_body_digest",
            self.residual_body_digest.as_deref(),
        )
        .map_err(|error| anyhow!("entry '{entry_id}' {error}"))?;
        Ok(())
    }
}
