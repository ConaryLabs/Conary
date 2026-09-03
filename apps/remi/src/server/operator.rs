// apps/remi/src/server/operator.rs

use super::*;

pub(crate) fn acquire_existing_runtime_storage(
    remi_config: &RemiConfig,
    server_config: &ServerConfig,
) -> Result<runtime_lock::RuntimeRootLock> {
    let locked_db_path = remi_config.storage_root().join("metadata/conary.db");
    if server_config.db_path != locked_db_path {
        anyhow::bail!(
            "Remi runtime database {} is outside the locked storage-root authority {}",
            server_config.db_path.display(),
            locked_db_path.display()
        );
    }
    let runtime_lock = runtime_lock::RuntimeRootLock::acquire(remi_config.storage_root())?;
    let metadata = std::fs::symlink_metadata(&server_config.db_path).with_context(|| {
        format!(
            "inspect existing Remi database {}",
            server_config.db_path.display()
        )
    })?;
    anyhow::ensure!(
        metadata.file_type().is_file(),
        "Remi stopped-runtime operation requires an existing plain runtime database"
    );
    let _conn = conary_core::db::open(&server_config.db_path)?;
    Ok(runtime_lock)
}

pub(super) fn create_runtime_storage_directories(remi_config: &RemiConfig) -> Result<()> {
    for dir in remi_config.storage_dirs() {
        if !dir.exists() {
            tracing::info!("Creating directory: {:?}", dir);
            std::fs::create_dir_all(&dir)?;
        }
    }
    Ok(())
}

/// Initialize storage directories without starting listeners.
///
/// The one-shot CLI path uses the same process-wide ownership boundary as the
/// long-running server, then releases it when initialization completes.
pub fn initialize_storage_directories(remi_config: &RemiConfig) -> Result<()> {
    let _runtime_lock = runtime_lock::RuntimeRootLock::acquire(remi_config.storage_root())?;
    create_runtime_storage_directories(remi_config)
}

/// Run one exclusive, evidence-consuming public promotion against a stopped
/// Remi runtime root.
pub async fn run_promotion_activation_from_config(
    remi_config: &RemiConfig,
    promotion_evidence_path: PathBuf,
    conversion_crawl_path: PathBuf,
) -> Result<RemiPromotionActivationOutcome> {
    remi_config.validate()?;
    let server_config = remi_config.to_server_config()?;
    let _runtime_lock = super::startup::prepare_runtime_storage(remi_config, &server_config)?;
    let database_writer = database_writer::DatabaseWriter::default();
    let catalog_authority = catalog_authority::CatalogAuthority::from_paths(
        server_config.db_path.clone(),
        server_config.catalog_dir.clone(),
        database_writer.clone(),
    );
    let r2_store = if remi_config.r2.enabled {
        let endpoint = remi_config
            .r2
            .endpoint
            .as_ref()
            .context("r2.endpoint is required when R2 authority is enabled")?;
        Some(Arc::new(R2Store::new(&r2::R2Config {
            endpoint: endpoint.clone(),
            bucket: remi_config.r2.bucket.clone(),
            prefix: remi_config.r2.prefix.clone(),
            region: "auto".to_string(),
        })?))
    } else {
        None
    };
    let repository_keys_dir = server_config
        .release_publish
        .repository_keys_dir
        .clone()
        .context("release_publish.repository_keys_dir is required for promotion")?;
    promotion::activate_remi_promotion(
        &RemiPromotionActivationConfig {
            db_path: server_config.db_path,
            catalog_dir: server_config.catalog_dir,
            catalog_candidate_dir: server_config.catalog_candidate_dir,
            chunk_dir: server_config.chunk_dir,
            repository_keys_dir,
            promotion_evidence_path,
            conversion_crawl_path,
        },
        &database_writer,
        &catalog_authority,
        r2_store,
    )
    .await
}

/// Produce exact candidate-resolution and promotion evidence under the normal
/// exclusive runtime-root authority.
pub fn run_promotion_proof_from_config(
    remi_config: &RemiConfig,
    conversion_crawl_path: PathBuf,
    output_dir: PathBuf,
    profiles: Vec<RemiPromotionProofProfileInput>,
) -> Result<RemiPromotionProofOutcome> {
    remi_config.validate()?;
    let server_config = remi_config.to_server_config()?;
    let _runtime_lock = acquire_existing_runtime_storage(remi_config, &server_config)?;
    let database_writer = database_writer::DatabaseWriter::default();
    let catalog_authority = catalog_authority::CatalogAuthority::from_paths(
        server_config.db_path.clone(),
        server_config.catalog_dir.clone(),
        database_writer,
    );
    promotion_proof::produce_remi_promotion_proof(
        &RemiPromotionProofConfig {
            db_path: server_config.db_path,
            catalog_dir: server_config.catalog_dir,
            conversion_crawl_path,
            output_dir,
            profiles,
        },
        &catalog_authority,
    )
}

/// Produce diagnostics-only candidate and comparison surveys while holding
/// exclusive stopped-runtime authority.
pub fn run_resolution_surveys_from_config(
    remi_config: &RemiConfig,
    output_dir: PathBuf,
    profiles: Vec<RemiPromotionProofProfileInput>,
    workers: conary_core::repository::catalog::ResolutionWorkerRequest,
) -> Result<RemiResolutionSurveyOutcome> {
    remi_config.validate()?;
    let server_config = remi_config.to_server_config()?;
    let _runtime_lock = acquire_existing_runtime_storage(remi_config, &server_config)?;
    let authority = catalog_authority::CatalogAuthority::from_paths(
        server_config.db_path,
        server_config.catalog_dir,
        database_writer::DatabaseWriter::default(),
    );
    resolution_survey::produce_remi_resolution_surveys(
        &RemiResolutionSurveyConfig {
            output_dir,
            profiles,
            workers,
        },
        &authority,
    )
}
