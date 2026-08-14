// apps/conary/src/commands/install/native_lifecycle.rs

//! Install-side carrier for validated native lifecycle metadata.

use super::PackageFormatType;

#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct NativeLifecycleInstallState {
    pub(crate) bundle_to_persist: Option<conary_core::ccs::native_lifecycle::NativeLifecycleBundle>,
}

impl NativeLifecycleInstallState {
    pub(crate) fn from_bundle(
        bundle: Option<&conary_core::ccs::native_lifecycle::NativeLifecycleBundle>,
    ) -> anyhow::Result<Self> {
        let Some(bundle) = bundle else {
            return Ok(Self::default());
        };
        bundle.validate()?;
        Ok(Self {
            bundle_to_persist: Some(bundle.clone()),
        })
    }

    pub(crate) fn from_native_package(
        package: &dyn conary_core::packages::PackageFormat,
        format: PackageFormatType,
        source_profile_id: Option<&str>,
    ) -> anyhow::Result<Self> {
        let source_format = match format {
            PackageFormatType::Rpm => "rpm",
            PackageFormatType::Deb => "deb",
            PackageFormatType::Arch => "arch",
            PackageFormatType::Eopkg => "eopkg",
        };
        let bundle = conary_core::ccs::convert::build_direct_native_lifecycle_bundle(
            package,
            source_format,
            source_profile_id,
        )?;
        Self::from_bundle(bundle.as_ref())
    }
}
