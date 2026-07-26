// apps/remi/src/server/publication.rs
//! CAS visibility checks for current converted and native publications.

use conary_core::db::models::{ChunkConversionState, ConvertedPackage, NativePackagePublication};
use std::path::Path;

/// Return whether a local CAS object is reachable from a current publication.
///
/// Native publications are authoritative directly. Converted-package chunks
/// require a current validated conversion reference; malformed current metadata
/// is surfaced by the model as a data error.
pub fn local_chunk_servable(db_path: &Path, hash: &str) -> anyhow::Result<bool> {
    let conn = crate::server::open_runtime_db(db_path)?;
    if NativePackagePublication::active_by_content_hash(&conn, hash)?.is_some() {
        return Ok(true);
    }
    Ok(matches!(
        ConvertedPackage::chunk_conversion_state(&conn, hash)?,
        ChunkConversionState::CurrentConversion
    ))
}
