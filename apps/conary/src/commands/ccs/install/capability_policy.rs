// src/commands/ccs/install/capability_policy.rs

use anyhow::Result;
use conary_core::ccs::CcsPackage;
use conary_core::packages::traits::PackageFormat;

pub(crate) fn enforce_ccs_capability_policy(
    ccs_pkg: &CcsPackage,
    allow_capabilities: bool,
    capability_policy: Option<&str>,
) -> Result<()> {
    let Some(cap_decl) = ccs_pkg.manifest().capabilities.as_ref() else {
        return Ok(());
    };

    use conary_core::capability::policy::{
        CapabilityPolicy, PolicyDecision, infer_linux_capabilities,
    };

    let cap_policy = CapabilityPolicy::load(capability_policy)?;
    let required_caps = infer_linux_capabilities(cap_decl);

    // Evaluate all caps, checking denied first so a denied capability is not
    // masked by an earlier prompted capability bailing first.
    for cap in &required_caps {
        if let PolicyDecision::Denied(msg) = cap_policy.evaluate(cap) {
            anyhow::bail!(
                "Package {} capability policy rejected: {} -- {}",
                ccs_pkg.name(),
                cap,
                msg,
            );
        }
    }

    for cap in &required_caps {
        match cap_policy.evaluate(cap) {
            PolicyDecision::Allowed | PolicyDecision::Denied(_) => {}
            PolicyDecision::Prompt(msg) => {
                if allow_capabilities {
                    println!("Capability {cap} approved via --allow-capabilities");
                } else {
                    anyhow::bail!(
                        "Package {} requires capability {}: {}. \
                         Use --allow-capabilities to approve.",
                        ccs_pkg.name(),
                        cap,
                        msg,
                    );
                }
            }
        }
    }

    Ok(())
}
