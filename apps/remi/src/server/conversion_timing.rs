// apps/remi/src/server/conversion_timing.rs
//! Timing evidence for Remi package conversion.

use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

use conary_core::filesystem::VerifiedObjectBatchMetrics;

/// Encode exact duration values inside the portable JSON integer range.
/// `u64` milliseconds span roughly 585 million years, so overflow is an
/// invalid timing record rather than a value to truncate or stringify.
pub(crate) mod json_u128 {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(value: &u128, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let value = u64::try_from(*value).map_err(serde::ser::Error::custom)?;
        serializer.serialize_u64(value)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<u128, D::Error>
    where
        D: Deserializer<'de>,
    {
        u64::deserialize(deserializer).map(u128::from)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConversionSourceIdentity {
    pub source_profile: String,
    pub version: String,
    pub architecture: Option<String>,
    pub checksum: String,
    pub declared_size_bytes: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConversionR2Work {
    pub head_requests: u64,
    pub hits: u64,
    pub misses: u64,
    pub put_requests: u64,
    pub bytes_written: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConversionWorkMetrics {
    pub downloaded_bytes: u64,
    pub admitted_local_bytes: u64,
    pub repository_checksum_bytes_hashed: u64,
    pub source_artifact_bytes: u64,
    pub source_bytes_hashed: u64,
    pub native_payload_entries: u64,
    pub native_payload_regular_files: u64,
    pub native_payload_declared_bytes: u64,
    pub native_source_archive_opens: u64,
    pub native_source_archive_bytes_read: u64,
    pub native_archive_passes: u64,
    pub native_archive_entries_traversed: u64,
    pub native_decompressed_archive_bytes_read: u64,
    pub native_intermediate_archive_bytes_written: u64,
    pub native_intermediate_archive_bytes_read: u64,
    pub native_intermediate_archive_file_syncs: u64,
    pub native_payload_files_spooled: u64,
    pub native_payload_bytes_spooled: u64,
    pub native_payload_spool_bytes_reread: u64,
    pub native_payload_spool_file_syncs: u64,
    pub native_payload_bytes_hashed: u64,
    pub payload_files_examined: u64,
    pub payload_reference_bytes_read: u64,
    pub payload_reference_bytes_hashed: u64,
    pub payload_chunks_derived: u64,
    pub unique_payload_chunks_derived: u64,
    pub payload_files_reopened: u64,
    pub payload_object_bytes_read: u64,
    pub second_pass_chunk_reference_bytes_hashed: u64,
    pub second_pass_reconstructed_content_bytes_hashed: u64,
    pub temporary_object_incoming_bytes_hashed: u64,
    pub temporary_object_bytes_written: u64,
    pub temporary_object_canonical_bytes_reread: u64,
    pub temporary_object_hits: u64,
    pub temporary_object_misses: u64,
    pub temporary_object_file_syncs: u64,
    pub temporary_object_shard_syncs: u64,
    pub archive_members_traversed: u64,
    pub archive_input_bytes: u64,
    pub ccs_output_bytes: u64,
    pub immediate_converter_reopen_ccs_bytes: u64,
    pub immediate_converter_reopen_object_bytes_hashed: u64,
    pub independent_transport_reopen_ccs_bytes: u64,
    pub independent_transport_reopen_object_bytes_hashed: u64,
    pub complete_archive_hash_bytes: u64,
    pub complete_archive_copy_bytes: u64,
    pub maximum_retained_staging_bytes: u64,
    pub signed_object_count: u64,
    pub signed_object_bytes: u64,
    pub cas_incoming_bytes_hashed: u64,
    pub cas_persistent_bytes_written: u64,
    pub cas_objects_hashed: u64,
    pub cas_hits: u64,
    pub cas_misses: u64,
    pub cas_race_losers: u64,
    pub cas_object_syncs: u64,
    pub cas_shard_syncs: u64,
    pub cas_root_syncs: u64,
    pub cas_canonical_bytes_reread: u64,
    pub r2: ConversionR2Work,
}

impl ConversionWorkMetrics {
    pub fn record_native_parse(
        &mut self,
        metrics: &conary_core::packages::NativePackageParseMetrics,
    ) {
        self.native_source_archive_opens = metrics.source_archive_opens;
        self.native_source_archive_bytes_read = metrics.source_archive_bytes_read;
        self.native_archive_passes = metrics.archive_passes;
        self.native_archive_entries_traversed = metrics.archive_entries_traversed;
        self.native_decompressed_archive_bytes_read = metrics.decompressed_archive_bytes_read;
        self.native_intermediate_archive_bytes_written = metrics.intermediate_archive_bytes_written;
        self.native_intermediate_archive_bytes_read = metrics.intermediate_archive_bytes_read;
        self.native_intermediate_archive_file_syncs = metrics.intermediate_archive_file_syncs;
        self.native_payload_files_spooled = metrics.payload_files_spooled;
        self.native_payload_bytes_spooled = metrics.payload_bytes_spooled;
        self.native_payload_spool_bytes_reread = metrics.payload_spool_bytes_reread;
        self.native_payload_spool_file_syncs = metrics.payload_spool_file_syncs;
        self.native_payload_bytes_hashed = metrics.payload_bytes_hashed;
    }

    pub fn record_native_conversion(
        &mut self,
        metrics: &conary_core::ccs::convert::NativeConversionMetrics,
    ) {
        self.payload_files_examined = metrics.payload_files_examined;
        self.payload_reference_bytes_read = metrics.payload_reference_bytes_read;
        self.payload_reference_bytes_hashed = metrics.payload_reference_bytes_hashed;
        self.payload_chunks_derived = metrics.payload_chunks_derived;
        self.unique_payload_chunks_derived = metrics.unique_payload_chunks_derived;
        self.payload_files_reopened = metrics.ccs_write.payload_files_traversed;
        self.payload_object_bytes_read = metrics.ccs_write.payload_bytes_read;
        self.second_pass_chunk_reference_bytes_hashed =
            metrics.ccs_write.chunk_reference_bytes_hashed;
        self.second_pass_reconstructed_content_bytes_hashed =
            metrics.ccs_write.reconstructed_content_bytes_hashed;
        self.temporary_object_incoming_bytes_hashed =
            metrics.ccs_write.temporary_object_incoming_bytes_hashed;
        self.temporary_object_bytes_written = metrics.ccs_write.temporary_object_bytes_written;
        self.temporary_object_canonical_bytes_reread =
            metrics.ccs_write.temporary_object_canonical_bytes_reread;
        self.temporary_object_hits = metrics.ccs_write.temporary_object_hits;
        self.temporary_object_misses = metrics.ccs_write.temporary_object_misses;
        self.temporary_object_file_syncs = metrics.ccs_write.temporary_object_file_syncs;
        self.temporary_object_shard_syncs = metrics.ccs_write.temporary_object_shard_syncs;
        self.archive_members_traversed = metrics.ccs_write.archive_members_traversed;
        self.archive_input_bytes = metrics.ccs_write.archive_input_bytes;
        self.ccs_output_bytes = metrics.ccs_write.ccs_output_bytes;
        self.immediate_converter_reopen_ccs_bytes = metrics.ccs_write.ccs_output_bytes;
        self.maximum_retained_staging_bytes = metrics.ccs_write.maximum_retained_staging_bytes;
    }

    pub fn record_cas(&mut self, metrics: VerifiedObjectBatchMetrics) {
        self.cas_incoming_bytes_hashed = metrics.incoming_bytes_hashed;
        self.cas_persistent_bytes_written = metrics.persistent_bytes_written;
        self.cas_objects_hashed = metrics.objects_hashed;
        self.cas_hits = metrics.hits;
        self.cas_misses = metrics.misses;
        self.cas_race_losers = metrics.race_losers;
        self.cas_object_syncs = metrics.object_syncs;
        self.cas_shard_syncs = metrics.shard_syncs;
        self.cas_root_syncs = metrics.root_syncs;
        self.cas_canonical_bytes_reread = metrics.canonical_bytes_reread;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversionPhase {
    PackageLookup,
    LocalArtifactAdmission,
    Download,
    Checksum,
    CacheLookup,
    NativeArchiveParseAndSpool,
    ArtifactIdentityAndAuthorityValidation,
    MetadataLifecycleAndAuthorityProjection,
    PayloadReferenceDerivation,
    OutputWorkspacePreparation,
    ControlProjectionAndSigning,
    PayloadObjectEmission,
    ArchiveAssemblyAndGzip,
    ImmediateConverterReopen,
    NativeProvenanceProjection,
    IndependentTransportReopen,
    DurableCasIngestion,
    R2WriteThrough,
    CompleteArchiveHash,
    CompleteArchiveCopy,
    DatabasePersistence,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversionNestedPhase {
    TemporaryObjectDurability,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConversionNestedPhaseTiming {
    pub phase: ConversionNestedPhase,
    pub included_in: ConversionPhase,
    #[serde(with = "json_u128")]
    pub duration_ms: u128,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConversionPhaseTiming {
    pub phase: ConversionPhase,
    #[serde(with = "json_u128")]
    pub duration_ms: u128,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConversionSkippedPhase {
    pub phase: ConversionPhase,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConversionTimingReport {
    pub distro: String,
    pub package: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<ConversionSourceIdentity>,
    pub phases: Vec<ConversionPhaseTiming>,
    pub nested_phases: Vec<ConversionNestedPhaseTiming>,
    pub skipped_phases: Vec<ConversionSkippedPhase>,
    pub work: ConversionWorkMetrics,
    #[serde(with = "json_u128")]
    pub total_ms: u128,
    pub success: bool,
    #[serde(skip, default = "Instant::now")]
    started_at: Instant,
}

impl ConversionTimingReport {
    pub fn new(distro: &str, package: &str, version: Option<&str>) -> Self {
        Self {
            distro: distro.to_string(),
            package: package.to_string(),
            version: version.map(ToString::to_string),
            source: None,
            phases: Vec::new(),
            nested_phases: Vec::new(),
            skipped_phases: Vec::new(),
            work: ConversionWorkMetrics::default(),
            total_ms: 0,
            success: false,
            started_at: Instant::now(),
        }
    }

    pub fn record(&mut self, phase: ConversionPhase, duration: Duration) {
        self.phases.push(ConversionPhaseTiming {
            phase,
            duration_ms: duration.as_millis(),
        });
    }

    pub fn record_skipped(&mut self, phase: ConversionPhase, reason: impl Into<String>) {
        self.skipped_phases.push(ConversionSkippedPhase {
            phase,
            reason: reason.into(),
        });
    }

    pub fn record_nested(
        &mut self,
        phase: ConversionNestedPhase,
        included_in: ConversionPhase,
        duration: Duration,
    ) {
        self.nested_phases.push(ConversionNestedPhaseTiming {
            phase,
            included_in,
            duration_ms: duration.as_millis(),
        });
    }

    pub fn finish(&mut self, success: bool) {
        self.success = success;
        self.total_ms = self.started_at.elapsed().as_millis();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::time::Duration;

    #[test]
    fn timing_report_serializes_phase_durations() {
        let mut report = ConversionTimingReport::new("fedora", "nginx", Some("1.28.0"));
        report.record(ConversionPhase::PackageLookup, Duration::from_millis(11));
        report.record(ConversionPhase::Download, Duration::from_millis(22));
        report.record_nested(
            ConversionNestedPhase::TemporaryObjectDurability,
            ConversionPhase::PayloadObjectEmission,
            Duration::from_millis(7),
        );
        report.record_skipped(ConversionPhase::R2WriteThrough, "r2 store not configured");
        report.finish(true);

        let value = serde_json::to_value(&report).expect("timing report serializes");
        assert_eq!(value["distro"], json!("fedora"));
        assert_eq!(value["package"], json!("nginx"));
        assert_eq!(value["version"], json!("1.28.0"));
        assert_eq!(value["success"], json!(true));
        assert_eq!(value["work"]["downloaded_bytes"], json!(0));
        assert_eq!(value["phases"][0]["phase"], json!("package_lookup"));
        assert_eq!(value["phases"][0]["duration_ms"], json!(11));
        assert_eq!(value["phases"][1]["phase"], json!("download"));
        assert_eq!(value["phases"][1]["duration_ms"], json!(22));
        assert_eq!(
            value["nested_phases"][0],
            json!({
                "phase": "temporary_object_durability",
                "included_in": "payload_object_emission",
                "duration_ms": 7
            })
        );
        assert_eq!(
            value["skipped_phases"][0]["phase"],
            json!("r2_write_through")
        );
        assert_eq!(
            value["skipped_phases"][0]["reason"],
            json!("r2 store not configured")
        );
    }

    #[test]
    fn timing_report_strictly_round_trips_json_durations() {
        let mut report = ConversionTimingReport::new("arch", "fixture", None);
        report.record(
            ConversionPhase::NativeArchiveParseAndSpool,
            Duration::from_millis(11),
        );
        report.record_nested(
            ConversionNestedPhase::TemporaryObjectDurability,
            ConversionPhase::PayloadObjectEmission,
            Duration::from_millis(7),
        );
        report.total_ms = 19;

        let encoded = serde_json::to_vec(&report).expect("serialize timing report");
        let reopened: ConversionTimingReport =
            serde_json::from_slice(&encoded).expect("reopen timing report");

        assert_eq!(reopened.phases[0].duration_ms, 11);
        assert_eq!(reopened.nested_phases[0].duration_ms, 7);
        assert_eq!(reopened.total_ms, 19);
    }

    #[test]
    fn native_conversion_work_maps_without_collapsing_payload_passes() {
        let metrics = conary_core::ccs::convert::NativeConversionMetrics {
            payload_files_examined: 5,
            payload_reference_bytes_read: 100,
            payload_reference_bytes_hashed: 100,
            payload_chunks_derived: 4,
            unique_payload_chunks_derived: 3,
            ccs_write: conary_core::ccs::builder::CcsPackageWriteMetrics {
                payload_files_traversed: 5,
                payload_bytes_read: 120,
                chunk_reference_bytes_hashed: 100,
                reconstructed_content_bytes_hashed: 100,
                temporary_object_incoming_bytes_hashed: 120,
                temporary_object_bytes_written: 120,
                temporary_object_canonical_bytes_reread: 20,
                temporary_object_hits: 1,
                temporary_object_misses: 3,
                temporary_object_file_syncs: 4,
                temporary_object_shard_syncs: 3,
                archive_members_traversed: 12,
                archive_input_bytes: 240,
                ccs_output_bytes: 180,
                maximum_retained_staging_bytes: 420,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut work = ConversionWorkMetrics::default();

        work.record_native_conversion(&metrics);

        assert_eq!(work.payload_reference_bytes_read, 100);
        assert_eq!(work.payload_object_bytes_read, 120);
        assert_eq!(work.temporary_object_file_syncs, 4);
        assert_eq!(work.archive_input_bytes, 240);
        assert_eq!(work.immediate_converter_reopen_ccs_bytes, 180);
        assert_eq!(work.maximum_retained_staging_bytes, 420);
    }

    #[test]
    fn native_parse_work_maps_archive_spool_and_hash_passes_separately() {
        let metrics = conary_core::packages::NativePackageParseMetrics {
            source_archive_opens: 2,
            source_archive_bytes_read: 200,
            archive_passes: 3,
            archive_entries_traversed: 40,
            decompressed_archive_bytes_read: 500,
            intermediate_archive_bytes_written: 100,
            intermediate_archive_bytes_read: 200,
            intermediate_archive_file_syncs: 1,
            payload_files_spooled: 10,
            payload_bytes_spooled: 300,
            payload_spool_bytes_reread: 30,
            payload_spool_file_syncs: 10,
            payload_bytes_hashed: 600,
        };
        let mut work = ConversionWorkMetrics::default();

        work.record_native_parse(&metrics);

        assert_eq!(work.native_source_archive_bytes_read, 200);
        assert_eq!(work.native_archive_passes, 3);
        assert_eq!(work.native_intermediate_archive_file_syncs, 1);
        assert_eq!(work.native_payload_spool_bytes_reread, 30);
        assert_eq!(work.native_payload_bytes_hashed, 600);
    }
}
