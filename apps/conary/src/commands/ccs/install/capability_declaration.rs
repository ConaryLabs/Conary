// apps/conary/src/commands/ccs/install/capability_declaration.rs

use anyhow::{Context, Result};
use conary_core::ccs::CcsPackage;
use conary_core::packages::traits::PackageFormat;

/// Validate the package's versioned capability contract against the exact
/// current syscall ABI and running-kernel Linux capability inventory.
pub(crate) fn validate_ccs_capability_declaration(ccs_pkg: &CcsPackage) -> Result<()> {
    let Some(declaration) = ccs_pkg.manifest().capabilities.as_ref() else {
        return Ok(());
    };

    declaration.validate_for_current_target().with_context(|| {
        format!(
            "Package {} has capability requirements unavailable on this target",
            ccs_pkg.name()
        )
    })?;
    Ok(())
}
