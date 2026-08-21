// apps/remi/src/server/canonical_fetch.rs

//! Canonical discovery fetch and publication cycle.
//!
//! Network work stays outside the SQLite writer. Each persistence phase and
//! the exact-contract rebuild acquire Remi's shared database writer so a
//! repository refresh and canonical publication cannot race SQLite's
//! single-writer boundary.

use std::path::Path;
use std::time::Duration;

use anyhow::Result;
use serde::Serialize;
use tracing::info;

use crate::server::config::CanonicalSection;
use crate::server::database_writer::DatabaseWriter;

/// Persistence result for one canonical discovery source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub(crate) enum CanonicalSourceOutcome {
    Persisted { records: usize },
    Failed { message: String },
}

impl CanonicalSourceOutcome {
    fn from_result(result: Result<usize>) -> Self {
        match result {
            Ok(records) => Self::Persisted { records },
            Err(error) => Self::Failed {
                message: error.to_string(),
            },
        }
    }

    fn persisted(&self) -> bool {
        matches!(self, Self::Persisted { .. })
    }
}

/// Result of rebuilding the mutation-authoritative canonical map.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub(crate) enum CanonicalRebuildOutcome {
    Completed { new_mappings: u64 },
    Failed { message: String },
}

/// Aggregate state of one fetch-and-rebuild cycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CanonicalCycleState {
    Complete,
    Partial,
    Failed,
}

/// Typed evidence from a complete canonical publication attempt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct CanonicalCycleReport {
    pub state: CanonicalCycleState,
    pub repology: CanonicalSourceOutcome,
    pub appstream: CanonicalSourceOutcome,
    pub rebuild: CanonicalRebuildOutcome,
}

impl CanonicalCycleReport {
    fn new(
        repology: CanonicalSourceOutcome,
        appstream: CanonicalSourceOutcome,
        rebuild: CanonicalRebuildOutcome,
    ) -> Self {
        let persisted_sources =
            usize::from(repology.persisted()) + usize::from(appstream.persisted());
        let state = match (&rebuild, persisted_sources) {
            (CanonicalRebuildOutcome::Failed { .. }, _) | (_, 0) => CanonicalCycleState::Failed,
            (CanonicalRebuildOutcome::Completed { .. }, 2) => CanonicalCycleState::Complete,
            (CanonicalRebuildOutcome::Completed { .. }, 1) => CanonicalCycleState::Partial,
            (_, _) => unreachable!("canonical cycle has exactly two discovery sources"),
        };
        Self {
            state,
            repology,
            appstream,
            rebuild,
        }
    }
}

/// Fetch both discovery sources, persist their exact outcomes, then rebuild.
///
/// The rebuild always runs because exact versioned contracts, not discovery
/// sources, own canonical mapping authority. A failed discovery source remains
/// explicit in the report and can never be rendered as a successful zero-row
/// fetch.
pub(crate) async fn run_canonical_cycle(
    db_path: &Path,
    config: &CanonicalSection,
    database_writer: DatabaseWriter,
) -> CanonicalCycleReport {
    let repology = CanonicalSourceOutcome::from_result(
        fetch_repology_data(db_path, config.repology_batch_size, database_writer.clone()).await,
    );
    let appstream = CanonicalSourceOutcome::from_result(
        fetch_appstream_data(db_path, database_writer.clone()).await,
    );

    let rebuild_db = db_path.to_path_buf();
    let rebuild_config = config.clone();
    let rebuild = match tokio::task::spawn_blocking(move || {
        database_writer.execute(|| {
            crate::server::canonical_job::rebuild_canonical_map(&rebuild_db, &rebuild_config)
        })
    })
    .await
    {
        Ok(Ok(new_mappings)) => CanonicalRebuildOutcome::Completed { new_mappings },
        Ok(Err(error)) => CanonicalRebuildOutcome::Failed {
            message: error.to_string(),
        },
        Err(error) => CanonicalRebuildOutcome::Failed {
            message: format!("canonical rebuild task did not complete: {error}"),
        },
    };

    CanonicalCycleReport::new(repology, appstream, rebuild)
}

/// Fetch Repology project data and cache it in the database.
/// Paginates through the API at 1 request/second (Repology rate limit).
async fn fetch_repology_data(
    db_path: &Path,
    batch_size: usize,
    database_writer: DatabaseWriter,
) -> Result<usize> {
    let client = conary_core::canonical::repology::RepologyClient::new()?;
    let mut all_projects = Vec::new();
    let mut start = String::new();
    let mut total_fetched = 0;

    loop {
        if total_fetched >= batch_size {
            break;
        }

        let batch = if start.is_empty() {
            client.fetch_projects_batch("0").await?
        } else {
            client.fetch_projects_batch(&start).await?
        };

        if batch.is_empty() {
            break;
        }

        if let Some(last) = batch.last() {
            start = last.name.clone();
        }

        total_fetched += batch.len();
        all_projects.extend(batch);
        tokio::time::sleep(Duration::from_secs(1)).await;
    }

    info!("Fetched {} Repology projects", all_projects.len());

    let db = db_path.to_path_buf();
    let count = tokio::task::spawn_blocking(move || {
        persist_repology_projects(&db, &all_projects, &database_writer)
    })
    .await??;

    info!("Cached {count} Repology entries to database");
    Ok(count)
}

pub(crate) fn persist_repology_projects(
    db_path: &Path,
    projects: &[conary_core::canonical::repology::RepologyProject],
    database_writer: &DatabaseWriter,
) -> Result<usize> {
    database_writer.execute(|| {
        let conn = crate::server::open_runtime_db(db_path)?;
        conary_core::canonical::repology::cache_projects_to_db(&conn, projects).map_err(Into::into)
    })
}

/// Fetch the Ubuntu AppStream catalog and persist it.
///
/// There is currently one configured AppStream source, so its failure is the
/// cycle's AppStream failure. It is deliberately not converted into `Ok(0)`.
async fn fetch_appstream_data(db_path: &Path, database_writer: DatabaseWriter) -> Result<usize> {
    let client = reqwest::Client::builder()
        .user_agent("conary/0.6.0 (https://conary.io; canonical-registry-sync)")
        .timeout(Duration::from_secs(60))
        .build()?;

    let count = fetch_ubuntu_appstream(&client, db_path, database_writer).await?;
    info!("Cached {count} AppStream components from Ubuntu");
    Ok(count)
}

async fn fetch_ubuntu_appstream(
    client: &reqwest::Client,
    db_path: &Path,
    database_writer: DatabaseWriter,
) -> Result<usize> {
    let url = "https://archive.ubuntu.com/ubuntu/dists/resolute/main/dep11/Components-amd64.yml.gz";

    let response = client.get(url).send().await?;
    if !response.status().is_success() {
        anyhow::bail!("Ubuntu AppStream fetch failed: HTTP {}", response.status());
    }

    let compressed = response.bytes().await?;

    use std::io::Read;
    let mut decoder = flate2::read::GzDecoder::new(&compressed[..]);
    let mut yaml = String::new();
    decoder.read_to_string(&mut yaml)?;

    let components = conary_core::canonical::appstream::parse_appstream_yaml(&yaml)?;
    info!(
        "Parsed {} AppStream components from Ubuntu 26.04 LTS",
        components.len()
    );

    let db = db_path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        database_writer.execute(|| {
            let conn = crate::server::open_runtime_db(&db)?;
            conary_core::canonical::appstream::cache_components_to_db(
                &conn,
                &components,
                "ubuntu-26.04",
            )
            .map_err(Into::into)
        })
    })
    .await?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_failed_sources_never_report_a_successful_cycle() {
        let report = CanonicalCycleReport::new(
            CanonicalSourceOutcome::Failed {
                message: "repology persistence failed".to_string(),
            },
            CanonicalSourceOutcome::Failed {
                message: "appstream persistence failed".to_string(),
            },
            CanonicalRebuildOutcome::Completed { new_mappings: 3 },
        );

        assert_eq!(report.state, CanonicalCycleState::Failed);
        let json = serde_json::to_value(report).expect("serialize cycle report");
        assert_eq!(json["state"], serde_json::json!("failed"));
        assert_eq!(json["repology"]["state"], serde_json::json!("failed"));
        assert_eq!(json["appstream"]["state"], serde_json::json!("failed"));
        assert_eq!(json["rebuild"]["state"], serde_json::json!("completed"));
    }

    #[test]
    fn one_failed_source_reports_a_partial_cycle() {
        let report = CanonicalCycleReport::new(
            CanonicalSourceOutcome::Persisted { records: 12 },
            CanonicalSourceOutcome::Failed {
                message: "upstream unavailable".to_string(),
            },
            CanonicalRebuildOutcome::Completed { new_mappings: 2 },
        );

        assert_eq!(report.state, CanonicalCycleState::Partial);
    }
}
