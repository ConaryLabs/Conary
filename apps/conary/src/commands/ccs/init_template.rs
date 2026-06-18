// apps/conary/src/commands/ccs/init_template.rs

//! CCS init authoring template selection.

#[derive(Clone, Copy, Debug, clap::ValueEnum, PartialEq, Eq)]
pub enum CcsInitTemplate {
    MinimalFile,
}
