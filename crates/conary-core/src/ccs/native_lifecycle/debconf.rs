// conary-core/src/ccs/native_lifecycle/debconf.rs

//! Package-level debconf templates authority transported by native lifecycle CCS.

use super::{
    LifecyclePath, NativeLifecycleBundle, NativeLifecycleEntry, NativeLifecycleEntryKind,
    SourceFormat,
};
use crate::packages::native_abi::{DEBCONF_TEMPLATES_ENTRY_ID, DebconfTemplateRecord};
use anyhow::{Result, bail};

/// Exact package association and byte-preserving authority for a Debian
/// `control.tar/templates` member.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DebconfTemplatesAuthority {
    pub source_package: String,
    pub source_version: String,
    pub raw_bytes: Vec<u8>,
    pub raw_sha256: String,
    pub records: Vec<DebconfTemplateRecord>,
}

impl NativeLifecycleBundle {
    /// Resolve the package-level debconf templates artifact, if present.
    ///
    /// Candidate selection uses only the closed entry id and native slot. A
    /// partial or duplicate declaration is rejected instead of guessed into
    /// one authority.
    pub fn debconf_templates(&self) -> Result<Option<DebconfTemplatesAuthority>> {
        let candidates = self
            .entries
            .iter()
            .filter(|entry| is_debconf_templates_candidate(entry))
            .collect::<Vec<_>>();
        if candidates.len() > 1 {
            bail!(
                "native lifecycle bundle has ambiguous debconf templates authority across entries {:?}",
                candidates
                    .iter()
                    .map(|entry| entry.id.as_str())
                    .collect::<Vec<_>>()
            );
        }
        let Some(entry) = candidates.first().copied() else {
            return Ok(None);
        };
        if self.source_format != SourceFormat::Deb {
            bail!(
                "native lifecycle entry '{}' declares Debian templates authority in '{}' source format",
                entry.id,
                self.source_format.as_str()
            );
        }
        validate_debconf_templates_entry(entry)?;

        let raw_bytes = entry.body_bytes()?;
        let parsed = crate::packages::deb::templates::parse(&raw_bytes)
            .map_err(|error| anyhow::anyhow!("entry '{}': {error}", entry.id))?;
        if !parsed.raw_sha256.eq_ignore_ascii_case(&entry.body_sha256) {
            bail!(
                "entry '{}' parsed templates digest does not match its preserved body",
                entry.id
            );
        }

        Ok(Some(DebconfTemplatesAuthority {
            source_package: self.source_package.clone(),
            source_version: self.source_version.clone(),
            raw_bytes,
            raw_sha256: parsed.raw_sha256,
            records: parsed.records,
        }))
    }
}

pub(super) fn is_debconf_templates_candidate(entry: &NativeLifecycleEntry) -> bool {
    entry.id == DEBCONF_TEMPLATES_ENTRY_ID
}

fn validate_debconf_templates_entry(entry: &NativeLifecycleEntry) -> Result<()> {
    if entry.id != DEBCONF_TEMPLATES_ENTRY_ID {
        bail!(
            "entry '{}' is an ambiguous partial declaration of debconf templates authority",
            entry.id
        );
    }
    if entry.native_slot.is_some() {
        bail!(
            "entry '{}' debconf templates artifact carries an RPM slot class",
            entry.id
        );
    }
    if entry.kind != NativeLifecycleEntryKind::ControlArtifact
        || entry.phase != LifecyclePath::PackageControl
        || entry.lifecycle_paths != [LifecyclePath::PackageControl.as_str()]
        || entry.interpreter != "package-manager-control-artifact"
        || !entry.interpreter_args.is_empty()
    {
        bail!(
            "entry '{}' does not use the exact package-level debconf templates control-artifact contract",
            entry.id
        );
    }
    if !entry.native_invocation.args.is_empty()
        || !entry.native_invocation.environment.is_empty()
        || entry.native_invocation.stdin.is_some()
        || entry.native_invocation.chroot.as_deref() != Some("package-manager-default")
    {
        bail!(
            "entry '{}' debconf templates artifact declares an executable invocation",
            entry.id
        );
    }
    if entry.transaction_order.position != "control-artifact"
        || !entry.transaction_order.before.is_empty()
        || !entry.transaction_order.after.is_empty()
    {
        bail!(
            "entry '{}' debconf templates artifact declares transaction execution ordering",
            entry.id
        );
    }
    if entry.rpm_runtime.is_some()
        || entry.rpm_trigger.is_some()
        || entry.rpm_sysusers.is_some()
        || entry.deb_maintainer.is_some()
        || entry.arch_install.is_some()
        || entry.arch_hook.is_some()
    {
        bail!(
            "entry '{}' debconf templates artifact carries script or foreign-format metadata",
            entry.id
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ccs::native_lifecycle::{
        NATIVE_LIFECYCLE_SCHEMA_REVISION, NATIVE_LIFECYCLE_SCHEMA_V1, NativeInvocation,
        ScriptletFidelity, TransactionOrder, VersionScheme,
    };

    const BODY: &str = "Template: demo/question\nType: string\nDescription: Demo\n Continuation\nDescription-de_DE.UTF-8: Beispiel\n";

    fn entry() -> NativeLifecycleEntry {
        NativeLifecycleEntry {
            id: DEBCONF_TEMPLATES_ENTRY_ID.to_string(),
            native_slot: None,
            kind: NativeLifecycleEntryKind::ControlArtifact,
            phase: LifecyclePath::PackageControl,
            lifecycle_paths: vec!["package-control".to_string()],
            interpreter: "package-manager-control-artifact".to_string(),
            interpreter_args: Vec::new(),
            body_sha256: crate::hash::sha256_prefixed(BODY.as_bytes()),
            body: BODY.to_string(),
            body_encoding: None,
            native_invocation: NativeInvocation {
                args: Vec::new(),
                environment: Vec::new(),
                stdin: None,
                chroot: Some("package-manager-default".to_string()),
            },
            transaction_order: TransactionOrder {
                position: "control-artifact".to_string(),
                before: Vec::new(),
                after: Vec::new(),
            },
            timeout_ms: 30_000,
            sandbox: None,
            capabilities: Vec::new(),
            evidence_digest: None,
            source_evidence_refs: Vec::new(),
            rpm_trigger: None,
            rpm_runtime: None,
            rpm_sysusers: None,
            deb_maintainer: None,
            arch_install: None,
            arch_hook: None,
            residual_lifecycle: None,
        }
    }

    fn bundle() -> NativeLifecycleBundle {
        NativeLifecycleBundle {
            schema: NATIVE_LIFECYCLE_SCHEMA_V1.to_string(),
            schema_revision: NATIVE_LIFECYCLE_SCHEMA_REVISION,
            source_format: SourceFormat::Deb,
            source_family: "deb".to_string(),
            source_profile: Some("ubuntu-26.04".to_string()),
            source_release: Some("unstable".to_string()),
            source_arch: Some("amd64".to_string()),
            source_package: "demo".to_string(),
            source_version: "1.0-1".to_string(),
            source_checksum: None,
            version_scheme: VersionScheme::Deb,
            conversion_tool: "test".to_string(),
            conversion_tool_version: "1".to_string(),
            conversion_policy: "test".to_string(),
            evidence_digest: None,
            scriptlet_fidelity: ScriptletFidelity::NativeLifecycle,
            entries: vec![entry()],
        }
    }

    #[test]
    fn round_trip_preserves_package_binding_raw_bytes_and_typed_records() {
        let encoded = toml::to_string_pretty(&bundle()).expect("serialize bundle");
        let decoded: NativeLifecycleBundle = toml::from_str(&encoded).expect("deserialize bundle");
        decoded.validate().expect("validate round trip");

        let authority = decoded
            .debconf_templates()
            .expect("resolve templates")
            .expect("templates authority");
        assert_eq!(authority.source_package, "demo");
        assert_eq!(authority.source_version, "1.0-1");
        assert_eq!(authority.raw_bytes, BODY.as_bytes());
        assert_eq!(
            authority.raw_sha256,
            crate::hash::sha256_prefixed(BODY.as_bytes())
        );
        assert_eq!(authority.records[0].template, "demo/question");
        assert!(
            authority.records[0]
                .fields
                .iter()
                .any(|field| field.name == "Description-de_DE.UTF-8")
        );
    }

    #[test]
    fn rejects_partial_duplicate_or_tampered_authority() {
        let mut partial = bundle();
        partial.entries[0].id = "deb:other".to_string();
        assert!(partial.validate().is_err());

        let mut duplicate = bundle();
        let mut second = entry();
        second.id = "deb:other".to_string();
        duplicate.entries.push(second);
        assert!(duplicate.validate().is_err());

        let mut tampered = bundle();
        tampered.entries[0].body.push_str("Type: boolean\n");
        assert!(tampered.validate().is_err());
    }
}
