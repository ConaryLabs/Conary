// apps/conary/src/commands/ccs/target_profile.rs

use anyhow::{Context, Result};
use conary_core::repository::supported_profiles::SupportedProfile;

pub(crate) fn resolve_target_profile(
    id: Option<&str>,
) -> Result<Option<&'static SupportedProfile>> {
    let Some(id) = id else {
        return Ok(None);
    };
    conary_core::repository::supported_profiles::profile_by_public_id(id)
        .map(Some)
        .with_context(|| {
            format!(
                "unsupported target profile {id}; expected one of fedora-44, ubuntu-26.04, arch"
            )
        })
}
