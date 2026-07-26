// conary-core/src/ccs/native_transaction/tests/debian.rs

//! Debian transaction tests grouped by native ownership boundary.

use super::*;

fn deb_package_state(
    package_name: &str,
    package_version: &str,
    source_arch: Option<&str>,
    lifecycle_state: DebPackageState,
) -> NativeDebPackageState {
    NativeDebPackageState {
        package_name: package_name.to_string(),
        package_version: package_version.to_string(),
        source_arch: source_arch.map(str::to_string),
        lifecycle_state,
        pending_triggers: Vec::new(),
        awaited_packages: Vec::new(),
    }
}

mod lifecycle;
mod refcount;
mod triggers;
