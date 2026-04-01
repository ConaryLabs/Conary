// apps/remi/src/server/canonical_fetch.rs

//! Daily background job to fetch Repology and AppStream data into cache tables.
//! Runs on a configurable interval (default 24h). Errors are logged, never fatal.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Result;
use tracing::{info, warn};

use crate::server::config::CanonicalSection;

/// Fetch Repology project data and cache it in the database.
/// Paginates through the API at 1 request/second (Repology rate limit).
/// Returns the number of cache entries written.
pub async fn fetch_repology_data(db_path: &Path, batch_size: usize) -> Result<usize> {
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

        // The last project name becomes the start for the next page
        if let Some(last) = batch.last() {
            start = last.name.clone();
        }

        total_fetched += batch.len();
        all_projects.extend(batch);

        // Respect Repology rate limit: 1 request per second
        tokio::time::sleep(Duration::from_secs(1)).await;
    }

    info!("Fetched {} Repology projects", all_projects.len());

    // Write to cache in a blocking task (SQLite is sync)
    let db = db_path.to_path_buf();
    let count = tokio::task::spawn_blocking(move || -> Result<usize> {
        let conn = crate::server::open_runtime_db(&db)?;
        conary_core::canonical::repology::cache_projects_to_db(&conn, &all_projects)
            .map_err(Into::into)
    })
    .await??;

    info!("Cached {count} Repology entries to database");
    Ok(count)
}

/// Fetch AppStream catalog data from well-known distro URLs and cache it.
/// Currently supports Ubuntu (DEP-11 YAML).
/// Returns the total number of components cached.
pub async fn fetch_appstream_data(db_path: &Path) -> Result<usize> {
    let client = reqwest::Client::builder()
        .user_agent("conary/0.6.0 (https://conary.io; canonical-registry-sync)")
        .timeout(Duration::from_secs(60))
        .build()?;

    let mut total = 0;

    // Ubuntu Noble DEP-11 YAML
    match fetch_ubuntu_appstream(&client, db_path).await {
        Ok(n) => {
            info!("Cached {n} AppStream components from Ubuntu");
            total += n;
        }
        Err(e) => warn!("AppStream fetch failed for Ubuntu: {e}"),
    }

    // Fedora AppStream XML (TODO: parse repomd.xml to find the appstream file)
    // For now, skip Fedora AppStream — the repomd.xml two-step fetch is complex
    // and Repology provides better cross-distro coverage anyway.

    Ok(total)
}

async fn fetch_ubuntu_appstream(client: &reqwest::Client, db_path: &Path) -> Result<usize> {
    let url = "http://archive.ubuntu.com/ubuntu/dists/noble/main/dep11/Components-amd64.yml.gz";

    let response = client.get(url).send().await?;
    if !response.status().is_success() {
        anyhow::bail!("Ubuntu AppStream fetch failed: HTTP {}", response.status());
    }

    let compressed = response.bytes().await?;

    // Decompress gzip
    use std::io::Read;
    let mut decoder = flate2::read::GzDecoder::new(&compressed[..]);
    let mut yaml = String::new();
    decoder.read_to_string(&mut yaml)?;

    let components = conary_core::canonical::appstream::parse_appstream_yaml(&yaml)?;
    info!(
        "Parsed {} AppStream components from Ubuntu Noble",
        components.len()
    );

    let db = db_path.to_path_buf();
    let count = tokio::task::spawn_blocking(move || -> Result<usize> {
        let conn = crate::server::open_runtime_db(&db)?;
        conary_core::canonical::appstream::cache_components_to_db(
            &conn,
            &components,
            "ubuntu-noble",
        )
        .map_err(Into::into)
    })
    .await??;

    Ok(count)
}

/// Spawn the background canonical fetch loop.
/// Waits 60 seconds for server warm-up then runs every `fetch_interval_hours`.
/// After each fetch cycle, triggers a canonical rebuild.
pub fn spawn_canonical_fetch_loop(config: CanonicalSection, db_path: PathBuf) {
    let interval = Duration::from_secs(config.fetch_interval_hours * 3600);

    tokio::spawn(async move {
        // Initial delay to let the server warm up
        tokio::time::sleep(Duration::from_secs(60)).await;

        loop {
            info!("Starting canonical fetch cycle (Repology + AppStream)");

            // Fetch Repology
            match fetch_repology_data(&db_path, config.repology_batch_size).await {
                Ok(n) => info!("Repology fetch complete: {n} entries cached"),
                Err(e) => warn!("Repology fetch failed: {e}"),
            }

            // Fetch AppStream
            match fetch_appstream_data(&db_path).await {
                Ok(n) => info!("AppStream fetch complete: {n} components cached"),
                Err(e) => warn!("AppStream fetch failed: {e}"),
            }

            // Trigger rebuild after fetch
            let rebuild_db = db_path.clone();
            let rebuild_config = config.clone();
            match tokio::task::spawn_blocking(move || {
                crate::server::canonical_job::rebuild_canonical_map(&rebuild_db, &rebuild_config)
            })
            .await
            {
                Ok(Ok(count)) => info!("Post-fetch canonical rebuild: {count} new mappings"),
                Ok(Err(e)) => warn!("Post-fetch canonical rebuild failed: {e}"),
                Err(e) => warn!("Post-fetch canonical rebuild task panicked: {e}"),
            }

            tokio::time::sleep(interval).await;
        }
    });
}
