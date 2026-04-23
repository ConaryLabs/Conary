// apps/conary/src/commands/generation/export.rs
//! Generation disk-image export command wrapper.

use anyhow::Result;

pub async fn cmd_generation_export(
    _generation: Option<i64>,
    _path: Option<&str>,
    _format: &str,
    _output: &str,
    _size: Option<&str>,
) -> Result<()> {
    Err(anyhow::anyhow!(
        "generation export backend is not implemented yet"
    ))
}
