// apps/conary/src/commands/try_session/validation.rs
//! Try-session package and manifest policy.

use anyhow::{Result, bail};
use conary_core::ccs::CcsPackage;
use conary_core::ccs::manifest::{CcsManifest, HookExecutionRoot};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TryExecutionRoot {
    Namespace,
    Generation,
}

impl TryExecutionRoot {
    fn hook_execution_root(self) -> HookExecutionRoot {
        match self {
            Self::Namespace => HookExecutionRoot::TryRoot,
            Self::Generation => HookExecutionRoot::GenerationRoot,
        }
    }
}

pub(super) fn validate_try_package_policy(
    package: &CcsPackage,
    execution_root: TryExecutionRoot,
    activated: bool,
) -> Result<()> {
    validate_try_manifest_policy(package.manifest(), execution_root, activated)
}

fn validate_try_manifest_policy(
    manifest: &CcsManifest,
    execution_root: TryExecutionRoot,
    activated: bool,
) -> Result<()> {
    let hooks = &manifest.hooks;

    if hooks.has_script_hooks() {
        bail!("{}", script_hook_policy_error(activated));
    }

    if manifest.native_lifecycle.is_some() {
        bail!(
            "try sessions have no typed native-lifecycle filesystem-delta contract; \
             install the converted package through the normal transaction path"
        );
    }

    if hooks.has_service_hooks() {
        if activated {
            bail!(
                "service lifecycle is not generation-scoped in activated M1b try sessions; \
                 host-root lifecycle helper is M2 work"
            );
        }
        bail!(
            "service lifecycle is not generation-scoped in M1b try sessions; \
            hooks.services cannot run during try"
        );
    }

    validate_m1b_try_declarative_hook_support(manifest, activated)?;

    if hooks.has_irreversible_hooks_for_try_root(execution_root.hook_execution_root()) {
        bail!("try package contains irreversible hooks for the planned execution root");
    }

    Ok(())
}

fn validate_m1b_try_declarative_hook_support(
    manifest: &CcsManifest,
    activated: bool,
) -> Result<()> {
    let hooks = &manifest.hooks;
    if !hooks.systemd.is_empty() {
        bail!(
            "{}",
            unsupported_declarative_hook_error("hooks.systemd", activated)
        );
    }
    if !hooks.tmpfiles.is_empty() {
        bail!(
            "{}",
            unsupported_declarative_hook_error("hooks.tmpfiles", activated)
        );
    }
    if !hooks.sysctl.is_empty() {
        bail!(
            "{}",
            unsupported_declarative_hook_error("hooks.sysctl", activated)
        );
    }
    if !hooks.alternatives.is_empty() {
        bail!(
            "{}",
            unsupported_declarative_hook_error("hooks.alternatives", activated)
        );
    }
    Ok(())
}

fn unsupported_declarative_hook_error(hook_class: &str, activated: bool) -> String {
    if activated {
        format!(
            "{hook_class} are not supported in activated M1b try sessions; \
             generation-scoped effect verification for this hook class is M2 work"
        )
    } else {
        format!(
            "{hook_class} are not supported in M1b try sessions; \
             promotable try-root effect verification for this hook class is M2 work"
        )
    }
}

fn script_hook_policy_error(activated: bool) -> &'static str {
    if activated {
        "script hooks are not supported in activated M1b try sessions; \
         host-root lifecycle helper is M2 work"
    } else {
        "script hooks are not supported in M1b try sessions; \
         scripts cannot run against the host root"
    }
}

#[cfg(test)]
#[path = "validation/tests.rs"]
mod tests;
