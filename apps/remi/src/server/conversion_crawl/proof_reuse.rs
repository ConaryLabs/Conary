// apps/remi/src/server/conversion_crawl/proof_reuse.rs
//! Exact artifact-and-contract identity for reusable conversion proof.

use super::{
    CCS_ARTIFACT_REOPEN_PROOF_SCHEMA_V1, CcsArtifactReopenProofV1, CcsArtifactReopener,
    CcsTargetCompatibilityProofV1, ReopenedCcsArtifactEvidence, exact_prefixed_sha256,
    target_preflight, validate_sha256,
};
use crate::server::conversion::{ConversionService, ServerConversionResult};
use crate::server::database_writer::DatabaseWriter;
use anyhow::{Context, Result, ensure};
use conary_core::ccs::{CcsTransportEnvelopeV1, TargetProfileV1, supported_target_contracts};
use conary_core::db::models::{ConvertedPackage, RemiActiveProfileRevision};
use conary_core::repository::catalog::CatalogPackageRecordV1;
use rusqlite::{OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const CONVERSION_PROOF_KEY_SCHEMA_V1: u32 = 1;
pub const CONVERSION_PROOF_SCHEMA_V1: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConversionProofTargetContractV1 {
    pub target_profile: TargetProfileV1,
    pub target_contract_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConversionProofKeyV1 {
    pub schema_version: u32,
    pub source_profile: String,
    pub package_key_sha256: String,
    pub package_name: String,
    pub package_version: String,
    pub package_release: String,
    pub package_architecture: Option<String>,
    pub source_artifact_sha256: String,
    pub converter_schema_version: i32,
    pub converter_version: String,
    pub ccs_format_version: u16,
    pub targets_signer_public_key_sha256: String,
    pub target_contracts: Vec<ConversionProofTargetContractV1>,
}

impl ConversionProofKeyV1 {
    pub(super) fn current(
        package: &CatalogPackageRecordV1,
        source_artifact_sha256: String,
        targets_signer_public_key_sha256: String,
    ) -> Result<Self> {
        let key = Self {
            schema_version: CONVERSION_PROOF_KEY_SCHEMA_V1,
            source_profile: package.source_profile.clone(),
            package_key_sha256: package.package_key_sha256.clone(),
            package_name: package.name.clone(),
            package_version: package.version.clone(),
            package_release: package.package_release.clone(),
            package_architecture: package.architecture.clone(),
            source_artifact_sha256,
            converter_schema_version: conary_core::db::models::CONVERSION_VERSION,
            converter_version: env!("CARGO_PKG_VERSION").to_string(),
            ccs_format_version: conary_core::ccs::v3::FORMAT_VERSION_V3,
            targets_signer_public_key_sha256,
            target_contracts: supported_target_contracts()
                .iter()
                .map(|contract| {
                    Ok(ConversionProofTargetContractV1 {
                        target_profile: contract.target_profile,
                        target_contract_sha256: contract.sha256().map_err(anyhow::Error::msg)?,
                    })
                })
                .collect::<Result<Vec<_>>>()?,
        };
        key.validate_current()?;
        Ok(key)
    }

    pub fn validate_current(&self) -> Result<()> {
        ensure!(
            self.schema_version == CONVERSION_PROOF_KEY_SCHEMA_V1,
            "unsupported conversion proof key schema {}",
            self.schema_version
        );
        ensure!(
            conary_core::repository::supported_profiles::profile_by_public_id(&self.source_profile)
                .is_some(),
            "conversion proof key source profile is not public"
        );
        validate_sha256(&self.package_key_sha256, "conversion proof package key")?;
        validate_sha256(
            &self.source_artifact_sha256,
            "conversion proof source artifact",
        )?;
        validate_sha256(
            &self.targets_signer_public_key_sha256,
            "conversion proof targets signer",
        )?;
        ensure!(
            !self.package_name.is_empty() && !self.package_version.is_empty(),
            "conversion proof package identity is incomplete"
        );
        ensure!(
            self.package_architecture
                .as_ref()
                .is_some_and(|architecture| !architecture.is_empty()),
            "conversion proof package architecture is incomplete"
        );
        ensure!(
            self.converter_schema_version == conary_core::db::models::CONVERSION_VERSION,
            "conversion proof converter schema has drifted"
        );
        ensure!(
            self.converter_version == env!("CARGO_PKG_VERSION"),
            "conversion proof converter version has drifted"
        );
        ensure!(
            self.ccs_format_version == conary_core::ccs::v3::FORMAT_VERSION_V3,
            "conversion proof CCS schema has drifted"
        );

        let contracts = supported_target_contracts();
        ensure!(
            self.target_contracts.len() == contracts.len(),
            "conversion proof target contract set is incomplete"
        );
        for (identity, contract) in self.target_contracts.iter().zip(contracts) {
            validate_sha256(
                &identity.target_contract_sha256,
                "conversion proof target contract",
            )?;
            let expected_digest = contract.sha256().map_err(anyhow::Error::msg)?;
            ensure!(
                identity.target_profile == contract.target_profile
                    && identity.target_contract_sha256 == expected_digest,
                "conversion proof target contract order or digest has drifted"
            );
        }
        Ok(())
    }

    pub fn sha256(&self) -> Result<String> {
        self.validate_current()?;
        let canonical = conary_core::json::canonical_json(self).map_err(anyhow::Error::msg)?;
        Ok(conary_core::hash::sha256(&canonical))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversionProofDispositionV1 {
    Validated,
    Reused,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConversionProofV1 {
    pub schema_version: u32,
    pub proof_key_sha256: String,
    pub key: ConversionProofKeyV1,
    pub validated_profile_revision_sha256: String,
    pub ccs_sha256: String,
    pub ccs_reopen_proof: CcsArtifactReopenProofV1,
    pub target_compatibility_proofs: Vec<CcsTargetCompatibilityProofV1>,
}

impl ConversionProofV1 {
    fn from_reopened(
        package: &CatalogPackageRecordV1,
        profile_revision_sha256: &str,
        targets_signer_public_key_sha256: &str,
        reopened: &ReopenedCcsArtifactEvidence,
    ) -> Result<Self> {
        let key = ConversionProofKeyV1::current(
            package,
            reopened.source_artifact_sha256.clone(),
            targets_signer_public_key_sha256.to_string(),
        )?;
        let proof = Self {
            schema_version: CONVERSION_PROOF_SCHEMA_V1,
            proof_key_sha256: key.sha256()?,
            key,
            validated_profile_revision_sha256: profile_revision_sha256.to_string(),
            ccs_sha256: reopened.ccs_sha256.clone(),
            ccs_reopen_proof: reopened.reopen_proof.clone(),
            target_compatibility_proofs: reopened.target_compatibility_proofs.clone(),
        };
        proof.validate_current()?;
        Ok(proof)
    }

    pub fn validate_current(&self) -> Result<()> {
        ensure!(
            self.schema_version == CONVERSION_PROOF_SCHEMA_V1,
            "unsupported conversion proof schema {}",
            self.schema_version
        );
        self.key.validate_current()?;
        validate_sha256(
            &self.validated_profile_revision_sha256,
            "conversion proof validation profile revision",
        )?;
        validate_sha256(&self.proof_key_sha256, "conversion proof key")?;
        ensure!(
            self.proof_key_sha256 == self.key.sha256()?,
            "conversion proof key digest differs from its exact key"
        );
        validate_sha256(&self.ccs_sha256, "conversion proof CCS")?;
        self.ccs_reopen_proof.validate()?;
        ensure!(
            self.ccs_reopen_proof.schema_version == CCS_ARTIFACT_REOPEN_PROOF_SCHEMA_V1,
            "conversion proof carries an unsupported CCS reopen proof"
        );
        ensure!(
            self.ccs_reopen_proof.ccs_format_version == self.key.ccs_format_version
                && self.ccs_reopen_proof.signer_public_key_sha256
                    == self.key.targets_signer_public_key_sha256,
            "conversion proof reopen authority differs from its exact key"
        );
        target_preflight::validate_complete_target_proofs(
            &self.target_compatibility_proofs,
            &self.ccs_sha256,
        )
    }
}

#[derive(Clone)]
pub(super) struct ConversionProofStore {
    db_path: PathBuf,
    database_writer: DatabaseWriter,
}

struct StoredConversionProof {
    proof: ConversionProofV1,
    original_format: String,
    transport: CcsTransportEnvelopeV1,
    total_size: u64,
    ccs_path: PathBuf,
    scriptlets: conary_core::ccs::convert::ScriptletBundleSummary,
}

impl ConversionProofStore {
    pub(super) fn new(db_path: PathBuf, database_writer: DatabaseWriter) -> Self {
        Self {
            db_path,
            database_writer,
        }
    }

    pub(super) async fn reuse_async(
        &self,
        package: CatalogPackageRecordV1,
        profile_revision_sha256: String,
        targets_signer_public_key_sha256: String,
    ) -> Result<Option<ConversionProofV1>> {
        let store = self.clone();
        tokio::task::spawn_blocking(move || {
            store.reuse(
                &package,
                &profile_revision_sha256,
                &targets_signer_public_key_sha256,
            )
        })
        .await
        .map_err(|error| anyhow::anyhow!("conversion proof reuse task panicked: {error}"))?
    }

    pub(super) async fn publish_async(
        &self,
        package: CatalogPackageRecordV1,
        profile_revision_sha256: String,
        targets_signer_public_key_sha256: String,
        result: ServerConversionResult,
        reopened: ReopenedCcsArtifactEvidence,
    ) -> Result<ConversionProofV1> {
        let store = self.clone();
        tokio::task::spawn_blocking(move || {
            store.publish(
                &package,
                &profile_revision_sha256,
                &targets_signer_public_key_sha256,
                &result,
                &reopened,
            )
        })
        .await
        .map_err(|error| anyhow::anyhow!("conversion proof publication task panicked: {error}"))?
    }

    fn reuse(
        &self,
        package: &CatalogPackageRecordV1,
        profile_revision_sha256: &str,
        targets_signer_public_key_sha256: &str,
    ) -> Result<Option<ConversionProofV1>> {
        validate_sha256(profile_revision_sha256, "conversion proof profile revision")?;
        let source_artifact_sha256 =
            exact_prefixed_sha256(&package.checksum, "catalog source artifact")?.to_string();
        let expected_key = ConversionProofKeyV1::current(
            package,
            source_artifact_sha256,
            targets_signer_public_key_sha256.to_string(),
        )?;
        let proof_key_sha256 = expected_key.sha256()?;
        let conn = crate::server::open_runtime_db(&self.db_path)?;
        let Some(stored) = load_stored_proof(&conn, &proof_key_sha256)? else {
            return Ok(None);
        };
        stored.validate_bytes()?;
        ensure!(
            stored.proof.key == expected_key,
            "conversion proof lookup returned a conflicting exact key"
        );

        let proof = stored.proof.clone();
        self.database_writer.execute(|| {
            let mut conn = crate::server::open_runtime_db(&self.db_path)?;
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            bind_proof_to_revision(
                &tx,
                package,
                profile_revision_sha256,
                &proof_key_sha256,
                &stored,
            )?;
            tx.commit()?;
            Ok::<_, anyhow::Error>(())
        })?;
        Ok(Some(proof))
    }

    fn publish(
        &self,
        package: &CatalogPackageRecordV1,
        profile_revision_sha256: &str,
        targets_signer_public_key_sha256: &str,
        result: &ServerConversionResult,
        reopened: &ReopenedCcsArtifactEvidence,
    ) -> Result<ConversionProofV1> {
        validate_sha256(profile_revision_sha256, "conversion proof profile revision")?;
        let catalog_source = exact_prefixed_sha256(&package.checksum, "catalog source artifact")?;
        ensure!(
            catalog_source == reopened.source_artifact_sha256,
            "reopened CCS source artifact differs from the exact catalog digest"
        );
        let proof = ConversionProofV1::from_reopened(
            package,
            profile_revision_sha256,
            targets_signer_public_key_sha256,
            reopened,
        )?;
        ensure!(
            result.content_hash == format!("sha256:{}", proof.ccs_sha256),
            "conversion result CCS digest differs from reusable proof"
        );
        ensure!(
            result.ccs_path.exists(),
            "conversion proof CCS artifact is missing"
        );

        self.database_writer.execute(|| {
            let mut conn = crate::server::open_runtime_db(&self.db_path)?;
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let converted = ConvertedPackage::find_repository_by_checksum(
                &tx,
                profile_revision_sha256,
                &package.checksum,
            )?
            .context("validated conversion has no exact durable repository row")?;
            let converted_id = converted
                .id
                .context("validated conversion row has no durable identity")?;
            ConvertedPackage::require_conversion_pin(&tx, converted_id)?;
            validate_converted_row(&converted, package, result, &proof)?;

            let original_format = public_format(&package.source_profile)?;
            let proof_json = canonical_json_text(&proof)?;
            let transport_json = canonical_json_text(&result.transport)?;
            let scriptlets = conary_core::ccs::convert::ScriptletBundleSummary {
                scriptlet_fidelity: result.scriptlets.scriptlet_fidelity.clone(),
                evidence_digest: result.scriptlets.evidence_digest.clone(),
            };
            let scriptlet_summary_json = canonical_json_text(&scriptlets)?;
            tx.execute(
                "INSERT INTO remi_conversion_proofs (
                    proof_key_sha256, proof_json, original_format, transport_json,
                    total_size, ccs_path, scriptlet_summary_json
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(proof_key_sha256) DO NOTHING",
                params![
                    &proof.proof_key_sha256,
                    &proof_json,
                    original_format,
                    &transport_json,
                    i64::try_from(result.total_size)
                        .context("conversion proof size exceeds SQLite INTEGER range")?,
                    result.ccs_path.to_string_lossy().as_ref(),
                    &scriptlet_summary_json,
                ],
            )?;
            let stored = load_stored_proof(&tx, &proof.proof_key_sha256)?
                .context("conversion proof insert did not produce durable state")?;
            stored.validate_bytes()?;
            ensure!(
                stored.proof == proof
                    && stored.original_format == original_format
                    && stored.transport == result.transport
                    && stored.total_size == result.total_size
                    && stored.ccs_path == result.ccs_path
                    && stored.scriptlets == scriptlets,
                "conversion proof key conflicts with existing durable evidence"
            );
            bind_existing_row(&tx, converted_id, &proof.proof_key_sha256)?;
            tx.commit()?;
            Ok::<_, anyhow::Error>(())
        })?;
        Ok(proof)
    }
}

pub(super) async fn validate_or_reuse(
    store: ConversionProofStore,
    service: ConversionService,
    reopener: CcsArtifactReopener,
    route: String,
    package: CatalogPackageRecordV1,
    selection: RemiActiveProfileRevision,
) -> Result<(ConversionProofDispositionV1, ConversionProofV1)> {
    let signer = reopener.signer_public_key_sha256().to_string();
    if let Some(proof) = store
        .reuse_async(
            package.clone(),
            selection.profile_revision_sha256.clone(),
            signer.clone(),
        )
        .await?
    {
        let disposition =
            if proof.validated_profile_revision_sha256 == selection.profile_revision_sha256 {
                ConversionProofDispositionV1::Validated
            } else {
                ConversionProofDispositionV1::Reused
            };
        return Ok((disposition, proof));
    }

    let result = service
        .convert_catalog_package_from_selection_async(&route, package.clone(), selection.clone())
        .await?;
    let reopened = reopener.reopen(&package, &result)?;
    let proof = store
        .publish_async(
            package,
            selection.profile_revision_sha256,
            signer,
            result,
            reopened,
        )
        .await?;
    Ok((ConversionProofDispositionV1::Validated, proof))
}

impl StoredConversionProof {
    fn validate_bytes(&self) -> Result<()> {
        self.proof.validate_current()?;
        ensure!(
            self.original_format == public_format(&self.proof.key.source_profile)?,
            "reusable conversion proof source format has drifted"
        );
        ensure!(self.total_size > 0, "conversion proof CCS is empty");
        let metadata = std::fs::metadata(&self.ccs_path).with_context(|| {
            format!(
                "reusable conversion proof CCS is missing: {}",
                self.ccs_path.display()
            )
        })?;
        ensure!(
            metadata.len() == self.total_size,
            "reusable conversion proof CCS size has drifted"
        );
        let mut file = std::fs::File::open(&self.ccs_path)?;
        let actual =
            conary_core::hash::hash_reader(conary_core::hash::HashAlgorithm::Sha256, &mut file)?;
        ensure!(
            actual.as_str() == self.proof.ccs_sha256,
            "reusable conversion proof CCS bytes have drifted"
        );
        let transport_sha256 = conary_core::hash::sha256(
            &conary_core::json::canonical_json(&self.transport).map_err(anyhow::Error::msg)?,
        );
        ensure!(
            transport_sha256 == self.proof.ccs_reopen_proof.transport_sha256,
            "reusable conversion proof transport has drifted"
        );
        validate_transport_identity(&self.transport, &self.proof.key)
    }
}

fn load_stored_proof(
    conn: &rusqlite::Connection,
    proof_key_sha256: &str,
) -> Result<Option<StoredConversionProof>> {
    validate_sha256(proof_key_sha256, "stored conversion proof key")?;
    conn.query_row(
        "SELECT proof_json, original_format, transport_json, total_size,
                ccs_path, scriptlet_summary_json
         FROM remi_conversion_proofs WHERE proof_key_sha256 = ?1",
        [proof_key_sha256],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
            ))
        },
    )
    .optional()?
    .map(
        |(proof_json, original_format, transport_json, total_size, ccs_path, scriptlets_json)| {
            let proof: ConversionProofV1 =
                serde_json::from_str(&proof_json).context("parse stored conversion proof JSON")?;
            ensure!(
                canonical_json_text(&proof)? == proof_json,
                "stored conversion proof JSON is not canonical"
            );
            ensure!(
                proof.proof_key_sha256 == proof_key_sha256,
                "stored conversion proof row key differs from proof evidence"
            );
            let transport: CcsTransportEnvelopeV1 = serde_json::from_str(&transport_json)
                .context("parse stored conversion proof transport")?;
            ensure!(
                canonical_json_text(&transport)? == transport_json,
                "stored conversion proof transport is not canonical"
            );
            let scriptlets = serde_json::from_str(&scriptlets_json)
                .context("parse stored conversion proof scriptlet summary")?;
            ensure!(
                canonical_json_text(&scriptlets)? == scriptlets_json,
                "stored conversion proof scriptlet summary is not canonical"
            );
            Ok(StoredConversionProof {
                proof,
                original_format,
                transport,
                total_size: u64::try_from(total_size)
                    .context("stored conversion proof has negative CCS size")?,
                ccs_path: PathBuf::from(ccs_path),
                scriptlets,
            })
        },
    )
    .transpose()
}

fn bind_proof_to_revision(
    tx: &Transaction<'_>,
    package: &CatalogPackageRecordV1,
    profile_revision_sha256: &str,
    proof_key_sha256: &str,
    stored: &StoredConversionProof,
) -> Result<i64> {
    if let Some(converted) = ConvertedPackage::find_repository_by_checksum(
        tx,
        profile_revision_sha256,
        &package.checksum,
    )? {
        let id = converted
            .id
            .context("reused conversion row has no durable identity")?;
        ConvertedPackage::require_conversion_pin(tx, id)?;
        validate_existing_binding_row(&converted, package, stored)?;
        bind_existing_row(tx, id, proof_key_sha256)?;
        return Ok(id);
    }

    let repository_provides_digest =
        conary_core::ccs::attestation::canonical_json_hash(&package.provides)
            .map_err(anyhow::Error::msg)?;
    let mut converted = ConvertedPackage::new_repository(
        package.source_profile.clone(),
        profile_revision_sha256.to_string(),
        package.name.clone(),
        package.version.clone(),
        package
            .architecture
            .clone()
            .context("reused conversion package has no exact architecture")?,
        stored.original_format.clone(),
        package.checksum.clone(),
        &stored.transport,
        i64::try_from(stored.total_size)
            .context("reused conversion size exceeds SQLite INTEGER range")?,
        format!("sha256:{}", stored.proof.ccs_sha256),
        stored.ccs_path.to_string_lossy().to_string(),
        repository_provides_digest,
    );
    converted.set_scriptlet_metadata(&stored.scriptlets)?;
    let id = converted.insert_with_conversion_pin_in_transaction(tx, unix_seconds()?)?;
    bind_existing_row(tx, id, proof_key_sha256)?;
    Ok(id)
}

fn bind_existing_row(
    tx: &Transaction<'_>,
    converted_package_id: i64,
    proof_key_sha256: &str,
) -> Result<()> {
    let existing = tx
        .query_row(
            "SELECT proof_key_sha256 FROM remi_conversion_proof_bindings
             WHERE converted_package_id = ?1",
            [converted_package_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    match existing {
        Some(existing) => ensure!(
            existing == proof_key_sha256,
            "converted package is bound to a conflicting conversion proof"
        ),
        None => {
            tx.execute(
                "INSERT INTO remi_conversion_proof_bindings (
                    converted_package_id, proof_key_sha256
                 ) VALUES (?1, ?2)",
                params![converted_package_id, proof_key_sha256],
            )?;
        }
    }
    Ok(())
}

fn validate_converted_row(
    converted: &ConvertedPackage,
    package: &CatalogPackageRecordV1,
    result: &ServerConversionResult,
    proof: &ConversionProofV1,
) -> Result<()> {
    let artifact = converted.repository_artifact()?;
    ensure!(
        artifact.source_profile == package.source_profile
            && artifact.package_name == package.name
            && artifact.package_version == package.version
            && Some(artifact.package_architecture) == package.architecture.as_deref()
            && artifact.content_hash == result.content_hash
            && Path::new(artifact.ccs_path) == result.ccs_path
            && artifact.transport == result.transport
            && proof.key.package_key_sha256 == package.package_key_sha256,
        "validated conversion row differs from exact reusable proof authority"
    );
    Ok(())
}

fn validate_existing_binding_row(
    converted: &ConvertedPackage,
    package: &CatalogPackageRecordV1,
    stored: &StoredConversionProof,
) -> Result<()> {
    let artifact = converted.repository_artifact()?;
    ensure!(
        artifact.source_profile == package.source_profile
            && artifact.package_name == package.name
            && artifact.package_version == package.version
            && Some(artifact.package_architecture) == package.architecture.as_deref()
            && artifact.content_hash == format!("sha256:{}", stored.proof.ccs_sha256)
            && Path::new(artifact.ccs_path) == stored.ccs_path
            && artifact.transport == stored.transport,
        "current revision conversion row conflicts with reusable proof"
    );
    Ok(())
}

fn validate_transport_identity(
    transport: &CcsTransportEnvelopeV1,
    key: &ConversionProofKeyV1,
) -> Result<()> {
    let boundary_json = transport
        .foreign_conversion_boundary_json
        .as_deref()
        .context("reusable conversion proof transport has no foreign boundary")?;
    let boundary: conary_core::ccs::attestation::ForeignConversionBoundary =
        serde_json::from_str(boundary_json)
            .context("parse reusable conversion proof foreign boundary")?;
    ensure!(
        exact_prefixed_sha256(&boundary.source_checksum, "proof boundary source artifact")?
            == key.source_artifact_sha256
            && boundary.output_identity.package_name == key.package_name
            && boundary.output_identity.package_version == key.package_version
            && boundary.output_identity.package_release == key.package_release
            && boundary.output_identity.architecture == key.package_architecture,
        "reusable conversion proof boundary differs from exact proof key"
    );
    Ok(())
}

fn public_format(source_profile: &str) -> Result<&'static str> {
    conary_core::repository::supported_profiles::profile_by_public_id(source_profile)
        .map(|profile| profile.package_format().as_str())
        .context("conversion proof source profile is not public")
}

fn canonical_json_text<T: Serialize>(value: &T) -> Result<String> {
    String::from_utf8(conary_core::json::canonical_json(value).map_err(anyhow::Error::msg)?)
        .context("canonical JSON is not UTF-8")
}

fn unix_seconds() -> Result<i64> {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .context("system time precedes Unix epoch")?
        .as_secs();
    i64::try_from(seconds).context("system time exceeds SQLite integer range")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::catalog_authority::test_support::{ActiveCatalogFixture, package};
    use crate::server::conversion::ScriptletPackageMetadata;
    use conary_core::ccs::attestation::{
        BuildOutputIdentity, FOREIGN_CONVERSION_BOUNDARY_SCHEMA_V1, ForeignConversionBoundary,
    };

    fn key() -> ConversionProofKeyV1 {
        let mut package = package(
            "fedora-44",
            "demo",
            "1.0",
            "1",
            Some("x86_64"),
            1,
            "proof-key",
        );
        package.package_key_sha256 = "c".repeat(64);
        ConversionProofKeyV1::current(&package, "a".repeat(64), "b".repeat(64))
            .expect("current conversion proof key")
    }

    #[test]
    fn proof_key_binds_exact_current_converter_ccs_signer_and_targets() {
        let current_key = key();
        current_key.validate_current().expect("valid proof key");
        assert_eq!(current_key.target_contracts.len(), 3);
        assert_eq!(current_key.sha256().expect("proof key digest").len(), 64);

        let mut drifted = current_key.clone();
        drifted.converter_schema_version += 1;
        assert!(drifted.validate_current().is_err());

        let mut drifted = current_key.clone();
        drifted.target_contracts.swap(0, 1);
        assert!(drifted.validate_current().is_err());

        let mut drifted = current_key;
        drifted.targets_signer_public_key_sha256 = "c".repeat(64);
        assert_ne!(
            drifted
                .sha256()
                .expect("different signer remains a valid key"),
            key().sha256().expect("original key digest")
        );
    }

    #[test]
    fn candidate_profile_cannot_enter_public_proof_reuse() {
        let mut candidate = key();
        candidate.source_profile = "solus".to_string();
        assert!(candidate.validate_current().is_err());
    }

    #[test]
    fn proof_key_json_rejects_unknown_fields() {
        let mut value = serde_json::to_value(key()).expect("proof key JSON");
        value
            .as_object_mut()
            .expect("proof key object")
            .insert("unexpected".to_string(), serde_json::json!(true));
        assert!(serde_json::from_value::<ConversionProofKeyV1>(value).is_err());
    }

    #[test]
    fn exact_proof_reuses_across_revisions_and_changed_artifacts_validate_fresh() {
        let fixture = ActiveCatalogFixture::new();
        let directory = tempfile::tempdir().expect("proof artifact directory");
        let ccs_path = directory.path().join("artifact.ccs");
        std::fs::write(&ccs_path, b"exact reusable ccs").expect("write proof CCS");
        let ccs_sha256 = conary_core::hash::sha256(b"exact reusable ccs");
        let source_sha256 = "a".repeat(64);
        let signer_sha256 = "b".repeat(64);
        let mut package = package(
            "fedora-44",
            "demo",
            "1.0",
            "1",
            Some("x86_64"),
            18,
            "proof-reuse",
        );
        package.package_key_sha256 = "c".repeat(64);
        package.checksum = format!("sha256:{source_sha256}");
        let revision_a = fixture.activate("fedora-44", 1, vec![package.clone()]);
        let transport = transport(&package, &source_sha256);
        let reopened = reopened(&transport, &source_sha256, &ccs_sha256, &signer_sha256);
        let result = ServerConversionResult {
            name: package.name.clone(),
            version: package.version.clone(),
            source_profile: Some(package.source_profile.clone()),
            transport: transport.clone(),
            total_size: 18,
            content_hash: format!("sha256:{ccs_sha256}"),
            ccs_path: ccs_path.clone(),
            cache_state: "cold".to_string(),
            scriptlets: ScriptletPackageMetadata {
                scriptlet_fidelity: "native-free".to_string(),
                evidence_digest: None,
            },
            timing: None,
        };
        let conn = fixture.connection();
        let mut converted = ConvertedPackage::new_repository(
            package.source_profile.clone(),
            revision_a.clone(),
            package.name.clone(),
            package.version.clone(),
            "x86_64".to_string(),
            "rpm".to_string(),
            package.checksum.clone(),
            &transport,
            18,
            format!("sha256:{ccs_sha256}"),
            ccs_path.to_string_lossy().to_string(),
            conary_core::ccs::attestation::canonical_json_hash(&package.provides)
                .expect("provides digest"),
        );
        converted
            .set_scriptlet_metadata(&conary_core::ccs::convert::ScriptletBundleSummary::default())
            .expect("scriptlet summary");
        converted
            .insert_with_conversion_pin(&conn, 1)
            .expect("insert revision A conversion");
        drop(conn);

        let store =
            ConversionProofStore::new(fixture.db_path().to_path_buf(), DatabaseWriter::default());
        let proof = store
            .publish(&package, &revision_a, &signer_sha256, &result, &reopened)
            .expect("publish exact conversion proof");
        assert_eq!(proof.validated_profile_revision_sha256, revision_a);

        let revision_b = fixture.activate("fedora-44", 2, vec![package.clone()]);
        let reused = store
            .reuse(&package, &revision_b, &signer_sha256)
            .expect("reuse lookup")
            .expect("exact proof reuse");
        assert_eq!(reused, proof);
        let conn = fixture.connection();
        let revision_b_row =
            ConvertedPackage::find_repository_by_checksum(&conn, &revision_b, &package.checksum)
                .expect("revision B conversion lookup")
                .expect("revision B conversion binding");
        ConvertedPackage::require_conversion_pin(
            &conn,
            revision_b_row.id.expect("revision B row id"),
        )
        .expect("revision B exact pin");
        let bindings: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM remi_conversion_proof_bindings
                 WHERE proof_key_sha256 = ?1",
                [&proof.proof_key_sha256],
                |row| row.get(0),
            )
            .expect("count proof bindings");
        assert_eq!(bindings, 2);
        drop(conn);

        let mut changed = package.clone();
        changed.checksum = format!("sha256:{}", "d".repeat(64));
        assert!(
            store
                .reuse(&changed, &revision_b, &signer_sha256)
                .expect("changed artifact lookup")
                .is_none()
        );

        std::fs::write(&ccs_path, b"corrupt reusable ccs").expect("corrupt proof CCS");
        assert!(store.reuse(&package, &revision_b, &signer_sha256).is_err());
    }

    fn transport(package: &CatalogPackageRecordV1, source_sha256: &str) -> CcsTransportEnvelopeV1 {
        let boundary = ForeignConversionBoundary {
            schema_version: FOREIGN_CONVERSION_BOUNDARY_SCHEMA_V1,
            source_format: "rpm".to_string(),
            source_checksum: format!("sha256:{source_sha256}"),
            output_identity: BuildOutputIdentity {
                file_merkle_root: "e".repeat(64),
                package_name: package.name.clone(),
                package_version: package.version.clone(),
                package_release: package.package_release.clone(),
                architecture: package.architecture.clone(),
                origin_class: "foreign_conversion".to_string(),
                hardening_level: "converted".to_string(),
                hermetic_evidence_hash: "f".repeat(64),
                canonical_content_identity: "1".repeat(64),
            },
            build_risk_report_hash: None,
            build_risk_report: None,
            scriptlet_risk_report_hash: None,
            scriptlet_risk_report: None,
            diagnostics: Vec::new(),
        };
        CcsTransportEnvelopeV1 {
            schema_version: conary_core::ccs::transport::CCS_TRANSPORT_SCHEMA_V1,
            manifest_base64: String::new(),
            signature_json: "{}".to_string(),
            debug_toml_base64: None,
            build_attestation_json: None,
            foreign_conversion_boundary_json: Some(
                serde_json::to_string(&boundary).expect("serialize boundary"),
            ),
            objects: Vec::new(),
        }
    }

    fn reopened(
        transport: &CcsTransportEnvelopeV1,
        source_sha256: &str,
        ccs_sha256: &str,
        signer_sha256: &str,
    ) -> ReopenedCcsArtifactEvidence {
        let transport_sha256 = conary_core::hash::sha256(
            &conary_core::json::canonical_json(transport).expect("canonical transport"),
        );
        ReopenedCcsArtifactEvidence {
            source_artifact_sha256: source_sha256.to_string(),
            ccs_sha256: ccs_sha256.to_string(),
            reopen_proof: CcsArtifactReopenProofV1 {
                schema_version: CCS_ARTIFACT_REOPEN_PROOF_SCHEMA_V1,
                ccs_format_version: conary_core::ccs::v3::FORMAT_VERSION_V3,
                foreign_conversion_boundary_schema_version: FOREIGN_CONVERSION_BOUNDARY_SCHEMA_V1,
                signer_public_key_sha256: signer_sha256.to_string(),
                transport_sha256,
                verified_files: 1,
                verified_objects: 0,
            },
            target_compatibility_proofs: supported_target_contracts()
                .iter()
                .map(|contract| CcsTargetCompatibilityProofV1 {
                    schema_version: super::super::CCS_TARGET_COMPATIBILITY_PROOF_SCHEMA_V1,
                    ccs_sha256: ccs_sha256.to_string(),
                    compatibility: conary_core::ccs::StaticTargetCompatibilityProofV1 {
                        schema_version:
                            conary_core::ccs::STATIC_TARGET_COMPATIBILITY_PROOF_SCHEMA_V1,
                        target_profile: contract.target_profile,
                        target_contract_sha256: contract.sha256().expect("target contract digest"),
                        required_capabilities: Vec::new(),
                        required_systemd_operations: Vec::new(),
                        required_linux_process_capabilities: Vec::new(),
                    },
                })
                .collect(),
        }
    }
}
