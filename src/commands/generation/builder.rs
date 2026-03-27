// src/commands/generation/builder.rs
//! Generation builder — CLI wrapper around conary_core::generation::builder.
//!
//! This module handles CLI-specific concerns (convergence intent checks,
//! composefs preflight, fs-verity enablement, progress output) and delegates
//! the actual EROFS image building to the core library.

use super::composefs::preflight_composefs;
use super::metadata::{GenerationMetadata, generation_path, generations_dir};
use anyhow::{Context, Result, anyhow};
use conary_core::db::models::{InstallSource, Trove};
use conary_core::db::paths::objects_dir;
use conary_core::generation::builder as core_builder;
use conary_core::model;
use conary_core::model::ConvergenceIntent;
use tracing::{debug, info, warn};

/// Build a new generation as an EROFS image from the current system state.
///
/// This is the CLI entry point that wraps `conary_core::generation::builder`.
/// It adds CLI-specific checks (convergence intent, composefs preflight,
/// fs-verity) around the core builder.
pub fn build_generation(conn: &rusqlite::Connection, db_path: &str, summary: &str) -> Result<i64> {
    // Step 0: Check convergence intent -- generation building requires at least CAS-backed
    let convergence = if model::model_exists(None) {
        model::load_model(None)
            .ok()
            .map(|m| m.system.convergence.clone())
    } else {
        None
    };
    if let Some(ref intent) = convergence {
        info!(
            "Convergence intent: {} (target: {})",
            intent.display_name(),
            intent.target_install_source()
        );
        if *intent == ConvergenceIntent::TrackOnly {
            warn!(
                "Convergence intent is 'track-only' -- packages at AdoptedTrack \
                 lack CAS content and will be skipped in the generation image. \
                 Set convergence to 'cas-backed' or 'full-ownership' for complete generations."
            );
        }
    }

    // Check for non-CAS-backed packages that will be excluded from the generation.
    let all_troves = Trove::list_all(conn).unwrap_or_default();
    let track_only_count = all_troves
        .iter()
        .filter(|t| t.install_source == InstallSource::AdoptedTrack)
        .count();
    if track_only_count > 0 {
        warn!(
            "{track_only_count} package(s) are at AdoptedTrack (no CAS content) \
             and may have incomplete file coverage in the generation image. \
             Use 'conary adopt-system --full' or 'conary system adopt --takeover' \
             to promote them along the ownership ladder."
        );
    }

    // Step 1: Composefs preflight check
    let obj_dir = objects_dir(db_path);
    let caps = preflight_composefs(&obj_dir).context("Composefs preflight failed")?;

    // Step 2: Delegate to core builder
    let generations_root = generations_dir();
    let (gen_number, result) =
        core_builder::build_generation_from_db(conn, &generations_root, summary)
            .map_err(|e| anyhow!("Generation build failed: {e}"))?;

    info!(
        "EROFS image built: {} bytes, {} CAS objects",
        result.image_size, result.cas_objects_referenced
    );

    // Step 3: Enable fs-verity on CAS objects (if supported)
    if caps.fsverity {
        debug!("fs-verity supported, enabling on CAS objects");
        let (enabled, already, errors) =
            conary_core::filesystem::fsverity::enable_fsverity_on_cas(&obj_dir);
        info!("fs-verity: {enabled} newly enabled, {already} already enabled, {errors} errors");

        // A non-zero error count means some CAS objects could not be
        // protected. Warn rather than hard-fail: the generation is still
        // usable, but integrity verification will be incomplete.
        if errors > 0 {
            warn!(
                "fs-verity: {errors} CAS object(s) could not have verity enabled; \
                 the generation will work but may lack full integrity protection"
            );
        }

        // Update the metadata with fsverity status.
        // Propagate write errors — a stale metadata.json could mislead
        // subsequent commands about whether verity is active.
        let gen_dir = generation_path(gen_number);
        match GenerationMetadata::read_from(&gen_dir) {
            Ok(mut metadata) => {
                metadata.fsverity_enabled = true;
                metadata.write_to(&gen_dir).with_context(|| {
                    format!(
                        "Failed to update fsverity status in generation {} metadata",
                        gen_number
                    )
                })?;
            }
            Err(e) => {
                warn!(
                    "Could not read generation {} metadata to update fsverity status: {e}",
                    gen_number
                );
            }
        }
    } else {
        debug!("fs-verity not supported on CAS filesystem, skipping");
    }

    Ok(gen_number)
}

// hex_to_digest tests live in conary_core::generation::builder::tests
