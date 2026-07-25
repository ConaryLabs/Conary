// src/commands/ccs/mod.rs

//! CCS package format commands
//!
//! Commands for creating, building, inspecting, signing, and installing CCS packages.

mod build;
mod enhance;
mod init;
mod init_template;
mod inspect;
mod install;
mod lint;
mod local_dev;
mod payload_paths;
mod runtime;
mod signing;
mod templates;
mod test;

// Re-export all public commands
pub use build::{CcsBuildOptions, cmd_ccs_build};
pub use enhance::cmd_ccs_enhance;
pub use init::cmd_ccs_init;
pub use init_template::CcsInitTemplate;
pub use inspect::{cmd_ccs_inspect, cmd_ccs_verify};
pub use install::cmd_ccs_install;
pub(crate) use install::validate_ccs_capability_declaration;
pub use lint::cmd_ccs_lint;
pub(crate) use local_dev::{load_or_create_local_dev_key, local_dev_trust_policy};
pub(crate) use payload_paths::{
    normalize_ccs_extracted_files, normalize_ccs_file_capabilities, normalize_ccs_package_path,
    validate_ccs_payload_paths,
};
pub use runtime::{cmd_ccs_export, cmd_ccs_run, cmd_ccs_shell};
pub use signing::{cmd_ccs_keygen, cmd_ccs_sign};
pub use test::cmd_ccs_test;
