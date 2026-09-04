// conary-core/src/generation/gc.rs

//! Typed CAS reachability and object collection.
//!
//! Reachability is assembled completely before deletion begins. Persisted
//! authorities are parsed through their owning types or exact schema columns;
//! malformed authority aborts collection instead of silently weakening the
//! live set.

use std::collections::HashSet;
use std::path::Path;
use std::time::{Duration, SystemTime};

use crate::filesystem::CasObjectCollectionSession;
use crate::filesystem::CasStore;
use crate::generation::root_manifest::{
    CapturedSelectedRoot, GenerationRootManifest, MutableStateManifest,
};
use crate::payload::PayloadContentAuthority;
use rusqlite::Connection;
use tracing::{debug, info};

const GC_RECENT_OBJECT_GRACE_PERIOD: Duration = Duration::from_secs(60 * 60);

/// Exact set of CAS objects reachable from current typed authority.
#[derive(Debug, Clone, Default)]
pub struct CasReachability {
    hashes: HashSet<String>,
}

impl CasReachability {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn hashes(&self) -> &HashSet<String> {
        &self.hashes
    }

    pub fn into_hashes(self) -> HashSet<String> {
        self.hashes
    }

    /// Protect one exact raw SHA-256 CAS key.
    pub fn protect_hash(&mut self, context: &str, hash: &str) -> crate::Result<()> {
        PayloadContentAuthority {
            sha256: hash.to_string(),
            size: 0,
        }
        .validate()
        .map_err(|error| {
            crate::Error::ConfigError(format!(
                "invalid CAS reference in {context}: {hash:?}: {error}"
            ))
        })?;
        self.hashes.insert(hash.to_string());
        Ok(())
    }

    pub fn protect_content(
        &mut self,
        context: &str,
        content: &PayloadContentAuthority,
    ) -> crate::Result<()> {
        content.validate().map_err(|error| {
            crate::Error::ConfigError(format!(
                "invalid CAS content authority in {context}: {error}"
            ))
        })?;
        self.hashes.insert(content.sha256.clone());
        Ok(())
    }

    pub fn protect_generation_manifest(
        &mut self,
        context: &str,
        manifest: &GenerationRootManifest,
    ) -> crate::Result<()> {
        manifest.validate()?;
        for content in manifest.regular_contents() {
            self.protect_content(context, content)?;
        }
        Ok(())
    }

    pub fn protect_mutable_state_manifest(
        &mut self,
        context: &str,
        manifest: &MutableStateManifest,
    ) -> crate::Result<()> {
        manifest.validate()?;
        for content in manifest.regular_contents() {
            self.protect_content(context, content)?;
        }
        Ok(())
    }

    pub fn protect_selected_root(
        &mut self,
        context: &str,
        root: &CapturedSelectedRoot,
    ) -> crate::Result<()> {
        self.protect_generation_manifest(context, &root.generation)?;
        self.protect_mutable_state_manifest(context, &root.state)
    }

    /// Protect exact current-schema roots outside generation manifests.
    ///
    /// Each query is mandatory for the current schema. A missing table,
    /// malformed hash, or malformed transport descriptor aborts before callers may
    /// delete anything.
    pub fn protect_current_database(&mut self, conn: &Connection) -> crate::Result<()> {
        for (context, sql) in [
            (
                "installed file content",
                "SELECT DISTINCT content_sha256 FROM files
                 WHERE content_sha256 IS NOT NULL",
            ),
            (
                "config original content",
                "SELECT DISTINCT original_hash FROM config_files
                 WHERE original_hash IS NOT NULL",
            ),
            (
                "config backup content",
                "SELECT DISTINCT backup_hash FROM config_backups",
            ),
            (
                "derived build artifact",
                "SELECT DISTINCT build_artifact_hash FROM derived_packages
                 WHERE build_artifact_hash IS NOT NULL",
            ),
            (
                "derived patch content",
                "SELECT DISTINCT patch_hash FROM derived_patches",
            ),
            (
                "derived override content",
                "SELECT DISTINCT source_hash FROM derived_overrides
                 WHERE source_hash IS NOT NULL",
            ),
            (
                "derivation index manifest",
                "SELECT DISTINCT manifest_cas_hash FROM derivation_index",
            ),
            (
                "derivation index provenance",
                "SELECT DISTINCT provenance_cas_hash FROM derivation_index
                 WHERE provenance_cas_hash IS NOT NULL",
            ),
            (
                "derivation cache manifest",
                "SELECT DISTINCT manifest_cas_hash FROM derivation_cache",
            ),
            (
                "derivation cache provenance",
                "SELECT DISTINCT provenance_cas_hash FROM derivation_cache
                 WHERE provenance_cas_hash IS NOT NULL",
            ),
            (
                "protected chunk",
                "SELECT DISTINCT hash FROM chunk_access WHERE protected = 1",
            ),
            (
                "Remi seed image",
                "SELECT DISTINCT image_cas_hash FROM seeds",
            ),
        ] {
            self.protect_hash_query(conn, context, sql)?;
        }

        for converted in crate::db::models::ConvertedPackage::list_repository_conversions(conn)? {
            if !converted.repository_conversion_is_current()? {
                continue;
            }
            let id = converted.id.ok_or_else(|| {
                crate::Error::InternalError(
                    "current converted repository row has no ID".to_string(),
                )
            })?;
            crate::db::models::ConvertedPackage::require_conversion_pin(conn, id)?;
            converted.scriptlet_summary()?;
            for hash in converted.object_hashes()? {
                self.protect_hash(
                    &format!("current converted package transport row {id}"),
                    &hash,
                )?;
            }
        }
        let mut stmt = conn.prepare(
            "SELECT id, transport_json FROM native_package_publications
             WHERE status = 'public'",
        )?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let id = row.get::<_, i64>(0)?;
            let json = row.get::<_, String>(1)?;
            let transport = serde_json::from_str::<crate::ccs::CcsTransportEnvelopeV1>(&json)
                .map_err(|error| {
                    crate::Error::ConfigError(format!(
                        "malformed native package publication transport for row {id}; refusing CAS GC: {error}"
                    ))
                })?;
            for object in transport.objects {
                self.protect_hash(
                    &format!("native package publication transport row {id}"),
                    &object.sha256,
                )?;
            }
        }
        Ok(())
    }

    /// Verify that every authoritative live key resolves in this local CAS.
    pub fn validate_objects_exist(&self, objects_dir: &Path) -> crate::Result<()> {
        if self.hashes.is_empty() {
            return Ok(());
        }
        if !objects_dir.is_dir() {
            return Err(crate::Error::NotFound(format!(
                "CAS objects directory {} is missing while {} live objects are authoritative",
                objects_dir.display(),
                self.hashes.len()
            )));
        }
        let cas = CasStore::new(objects_dir)?;
        let mut missing = self
            .hashes
            .iter()
            .filter(|hash| !cas.exists(hash))
            .cloned()
            .collect::<Vec<_>>();
        missing.sort();
        if !missing.is_empty() {
            return Err(crate::Error::NotFound(format!(
                "CAS reachability references {} missing object(s): {}",
                missing.len(),
                missing.join(", ")
            )));
        }
        Ok(())
    }

    /// Follow typed derived-artifact manifests to their output blobs.
    pub fn protect_derived_artifact_contents(
        &mut self,
        conn: &Connection,
        objects_dir: &Path,
    ) -> crate::Result<()> {
        let mut stmt = conn.prepare(
            "SELECT id, build_artifact_hash FROM derived_packages
             WHERE build_artifact_hash IS NOT NULL",
        )?;
        let artifacts = stmt
            .query_map([], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        if artifacts.is_empty() {
            return Ok(());
        }
        if !objects_dir.is_dir() {
            return Err(crate::Error::NotFound(format!(
                "CAS objects directory {} is missing while derived artifacts are authoritative",
                objects_dir.display()
            )));
        }
        let cas = CasStore::new(objects_dir)?;
        for (id, artifact_hash) in artifacts {
            for content in crate::derived::load_build_artifact_contents(&cas, &artifact_hash)? {
                self.protect_content(
                    &format!("derived build artifact contents row {id}"),
                    &content,
                )?;
            }
        }
        Ok(())
    }

    fn protect_hash_query(
        &mut self,
        conn: &Connection,
        context: &str,
        sql: &str,
    ) -> crate::Result<()> {
        let mut stmt = conn.prepare(sql)?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        for hash in rows {
            self.protect_hash(context, &hash?)?;
        }
        Ok(())
    }
}

/// Statistics from a CAS garbage collection run.
#[derive(Debug, Clone, Default)]
pub struct GcStats {
    pub objects_checked: u64,
    pub objects_removed: u64,
    pub bytes_freed: u64,
    pub deleted_hashes: Vec<String>,
}

/// Remove CAS objects absent from a live set resolved under `collection`.
///
/// The collection session must be acquired before assembling the live set so
/// generation completion cannot race the reachability snapshot.
pub fn gc_cas_objects(
    collection: &CasObjectCollectionSession,
    live_hashes: &HashSet<String>,
) -> crate::Result<GcStats> {
    gc_cas_objects_at(
        collection.objects_dir(),
        live_hashes,
        SystemTime::now(),
        GC_RECENT_OBJECT_GRACE_PERIOD,
    )
}

fn gc_cas_objects_at(
    objects_dir: &Path,
    live_hashes: &HashSet<String>,
    now: SystemTime,
    grace_period: Duration,
) -> crate::Result<GcStats> {
    let mut stats = GcStats::default();
    if !objects_dir.exists() {
        info!("CAS objects directory does not exist, nothing to collect");
        return Ok(stats);
    }

    let cas = CasStore::new(objects_dir)?;
    for result in cas.iter_objects() {
        let (hash, path) = result?;
        stats.objects_checked += 1;
        if live_hashes.contains(&hash) || should_skip_recent_object(&path, now, grace_period) {
            continue;
        }
        if let Ok(metadata) = path.metadata() {
            stats.bytes_freed += metadata.len();
        }
        std::fs::remove_file(&path)?;
        stats.objects_removed += 1;
        stats.deleted_hashes.push(hash.clone());
        debug!("Removed unreferenced CAS object: {hash}");
    }

    for entry in std::fs::read_dir(objects_dir)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() && entry.path().read_dir()?.next().is_none() {
            std::fs::remove_dir(entry.path())?;
        }
    }
    info!(
        "CAS GC: checked {}, removed {}, freed {} bytes",
        stats.objects_checked, stats.objects_removed, stats.bytes_freed
    );
    Ok(stats)
}

fn should_skip_recent_object(path: &Path, now: SystemTime, grace_period: Duration) -> bool {
    path.metadata()
        .ok()
        .and_then(|metadata| metadata.modified().ok())
        .and_then(|modified| now.duration_since(modified).ok())
        .is_some_and(|age| age < grace_period)
}

#[cfg(test)]
mod tests;
