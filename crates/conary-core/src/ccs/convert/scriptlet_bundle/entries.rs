// conary-core/src/ccs/convert/scriptlet_bundle/entries.rs

use super::format_metadata::{FormatMetadataProjection, project_format_metadata};
use super::native_contracts::{
    encoded_native_body, native_invocation, native_lifecycle_entry_kind, native_lifecycle_paths,
    native_transaction_order, phase_from_native_lifecycle,
};
use super::types::ScriptletBundleInput;
use crate::ccs::native_lifecycle::NativeLifecycleEntry;
use crate::packages::native_abi::{
    ArchNativeScriptletMetadata, NativeScriptletEntry, NativeScriptletMetadata,
};
use crate::repository::supported_profiles;

pub(super) fn build_entries(
    input: &ScriptletBundleInput<'_>,
) -> anyhow::Result<Vec<NativeLifecycleEntry>> {
    input
        .source_metadata
        .native_scriptlet_abi
        .iter()
        .map(|entry| build_native_entry(entry, input))
        .collect()
}

fn build_native_entry(
    native: &NativeScriptletEntry,
    input: &ScriptletBundleInput<'_>,
) -> anyhow::Result<NativeLifecycleEntry> {
    if matches!(
        native.metadata,
        NativeScriptletMetadata::DebconfTemplates(_)
    ) {
        crate::packages::deb::templates::validate_native_entry(native)
            .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    }
    let phase = phase_from_native_lifecycle(native.primary_lifecycle);
    let lifecycle_paths = native_lifecycle_paths(native);
    let (body, body_encoding) = encoded_native_body(&native.body);
    let FormatMetadataProjection {
        rpm_runtime,
        rpm_sysusers,
        rpm_trigger,
        deb_maintainer,
        arch_install,
        arch_hook,
    } = project_format_metadata(native);
    let interpreter = native_interpreter(native, input.source_profile)?;

    Ok(NativeLifecycleEntry {
        id: native.id.clone(),
        native_slot: native.native_slot.clone(),
        kind: native_lifecycle_entry_kind(native.kind),
        phase,
        lifecycle_paths,
        interpreter,
        interpreter_args: native.interpreter_args.clone(),
        body_sha256: native.body.sha256.clone(),
        body,
        body_encoding,
        native_invocation: native_invocation(&native.invocation),
        transaction_order: native_transaction_order(&native.order),
        timeout_ms: 30_000,
        sandbox: None,
        capabilities: Vec::new(),
        evidence_digest: None,
        source_evidence_refs: Vec::new(),
        rpm_trigger,
        rpm_runtime,
        rpm_sysusers,
        deb_maintainer,
        arch_install,
        arch_hook,
        residual_lifecycle: None,
    })
}

fn native_interpreter(
    native: &NativeScriptletEntry,
    source_profile_id: Option<&str>,
) -> anyhow::Result<String> {
    if native.kind == crate::packages::native_abi::NativeScriptletKind::ControlArtifact {
        anyhow::ensure!(
            native.interpreter.is_none() && native.interpreter_args.is_empty(),
            "control artifact '{}' must not declare an executable interpreter",
            native.id
        );
        return Ok("package-manager-control-artifact".to_string());
    }

    if let Some(interpreter) = &native.interpreter {
        return Ok(interpreter.clone());
    }

    if matches!(
        native.metadata,
        NativeScriptletMetadata::Arch(ArchNativeScriptletMetadata::Install(_))
    ) {
        let source_profile_id = source_profile_id.ok_or_else(|| {
            anyhow::anyhow!(
                "Arch .INSTALL entry '{}' requires an exact ALPM source profile ID",
                native.id
            )
        })?;
        let profile =
            supported_profiles::alpm_source_profile(source_profile_id).ok_or_else(|| {
                anyhow::anyhow!(
                    "Arch .INSTALL entry '{}' names unsupported ALPM source profile '{}'",
                    native.id,
                    source_profile_id
                )
            })?;
        return profile
            .scriptlet_shell()
            .map(str::to_string)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Arch source profile '{}' has no scriptlet shell",
                    profile.id()
                )
            });
    }

    anyhow::bail!(
        "executable native lifecycle entry '{}' has no exact interpreter",
        native.id
    )
}
