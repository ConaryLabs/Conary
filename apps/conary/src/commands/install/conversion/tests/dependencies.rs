// apps/conary/src/commands/install/conversion/tests/dependencies.rs

use super::*;
#[test]
fn default_dependency_passes_reach_kernel_initramfs_toolchain() {
    assert_eq!(DEFAULT_CCS_DEPENDENCY_PASSES, 2);
}
