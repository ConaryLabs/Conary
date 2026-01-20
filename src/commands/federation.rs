// src/commands/federation.rs
//! Command implementations for federation management

use anyhow::Result;
use rusqlite::Connection;
use tracing::info;

/// Show federation status
pub fn cmd_federation_status(db_path: &str, verbose: bool) -> Result<()> {
    let conn = Connection::open(db_path)?;

    // Get peer count by tier
    let mut stmt = conn.prepare(
        "SELECT tier, COUNT(*), SUM(CASE WHEN is_enabled = 1 THEN 1 ELSE 0 END)
         FROM federation_peers
         GROUP BY tier",
    )?;

    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, i64>(2)?,
        ))
    })?;

    let mut total_peers = 0i64;
    let mut enabled_peers = 0i64;
    let mut tiers: Vec<(String, i64, i64)> = Vec::new();

    for row in rows {
        let (tier, count, enabled) = row?;
        total_peers += count;
        enabled_peers += enabled;
        tiers.push((tier, count, enabled));
    }

    println!("Federation Status");
    println!("=================");
    println!();
    println!("Total peers: {} ({} enabled)", total_peers, enabled_peers);
    println!();

    if !tiers.is_empty() {
        println!("Peers by tier:");
        for (tier, count, enabled) in &tiers {
            println!("  {}: {} ({} enabled)", tier, count, enabled);
        }
        println!();
    }

    // Show today's stats
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let stats: Option<(i64, i64, i64, i64, i64)> = conn
        .query_row(
            "SELECT bytes_from_peers, bytes_from_upstream, chunks_from_peers,
                    chunks_from_upstream, requests_coalesced
             FROM federation_stats WHERE date = ?1",
            [&today],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .ok();

    if let Some((bytes_peers, bytes_upstream, chunks_peers, chunks_upstream, coalesced)) = stats {
        let total_bytes = bytes_peers + bytes_upstream;
        let savings_pct = if total_bytes > 0 {
            (bytes_peers as f64 / total_bytes as f64) * 100.0
        } else {
            0.0
        };

        println!("Today's statistics:");
        println!("  Bytes from peers: {}", format_bytes(bytes_peers as u64));
        println!(
            "  Bytes from upstream: {}",
            format_bytes(bytes_upstream as u64)
        );
        println!("  Bandwidth savings: {:.1}%", savings_pct);
        println!(
            "  Chunks from peers: {} / {}",
            chunks_peers,
            chunks_peers + chunks_upstream
        );
        println!("  Requests coalesced: {}", coalesced);
    } else {
        println!("Today's statistics: No data");
    }

    if verbose {
        println!();
        println!("Enabled peers:");

        let mut stmt = conn.prepare(
            "SELECT endpoint, tier, latency_ms, success_count, failure_count
             FROM federation_peers
             WHERE is_enabled = 1
             ORDER BY tier, latency_ms",
        )?;

        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, i64>(4)?,
            ))
        })?;

        for row in rows {
            let (endpoint, tier, latency, successes, failures) = row?;
            let total = successes + failures;
            let rate = if total > 0 {
                (successes as f64 / total as f64) * 100.0
            } else {
                100.0
            };
            println!(
                "  {} [{}] - {}ms, {:.1}% success",
                endpoint, tier, latency, rate
            );
        }
    }

    Ok(())
}

/// List federation peers
pub fn cmd_federation_peers(
    db_path: &str,
    tier: Option<&str>,
    enabled_only: bool,
) -> Result<()> {
    let conn = Connection::open(db_path)?;

    let base_query = "SELECT id, endpoint, node_name, tier, latency_ms, success_count,
                             failure_count, consecutive_failures, is_enabled, last_seen
                      FROM federation_peers";

    // Build different queries based on filters
    let peers: Vec<PeerRow> = if let Some(t) = tier {
        let query = if enabled_only {
            format!("{} WHERE tier = ?1 AND is_enabled = 1 ORDER BY tier, latency_ms", base_query)
        } else {
            format!("{} WHERE tier = ?1 ORDER BY tier, latency_ms", base_query)
        };
        let mut stmt = conn.prepare(&query)?;
        stmt.query_map([t], |row| {
            Ok(PeerRow {
                id: row.get(0)?,
                endpoint: row.get(1)?,
                name: row.get(2)?,
                tier: row.get(3)?,
                latency: row.get(4)?,
                successes: row.get(5)?,
                failures: row.get(6)?,
                consecutive_failures: row.get(7)?,
                enabled: row.get::<_, i64>(8)? == 1,
                last_seen: row.get(9)?,
            })
        })?.collect::<Result<Vec<_>, _>>()?
    } else {
        let query = if enabled_only {
            format!("{} WHERE is_enabled = 1 ORDER BY tier, latency_ms", base_query)
        } else {
            format!("{} ORDER BY tier, latency_ms", base_query)
        };
        let mut stmt = conn.prepare(&query)?;
        stmt.query_map([], |row| {
            Ok(PeerRow {
                id: row.get(0)?,
                endpoint: row.get(1)?,
                name: row.get(2)?,
                tier: row.get(3)?,
                latency: row.get(4)?,
                successes: row.get(5)?,
                failures: row.get(6)?,
                consecutive_failures: row.get(7)?,
                enabled: row.get::<_, i64>(8)? == 1,
                last_seen: row.get(9)?,
            })
        })?.collect::<Result<Vec<_>, _>>()?
    };

    println!(
        "{:<12} {:<40} {:<12} {:<8} {:>8} {:>8} {:<8}",
        "TIER", "ENDPOINT", "NAME", "STATUS", "LATENCY", "SUCCESS", "LAST SEEN"
    );
    println!("{}", "-".repeat(100));

    for peer in &peers {
        let status = if peer.enabled { "[OK]" } else { "[OFF]" };
        let total = peer.successes + peer.failures;
        let success_rate = if total > 0 {
            format!("{:.1}%", (peer.successes as f64 / total as f64) * 100.0)
        } else {
            "-".to_string()
        };
        let name = peer.name.as_deref().unwrap_or("-");
        let last_seen = if peer.last_seen.len() >= 10 {
            &peer.last_seen[..10]
        } else {
            &peer.last_seen
        };

        println!(
            "{:<12} {:<40} {:<12} {:<8} {:>6}ms {:>8} {:<8}",
            peer.tier,
            truncate(&peer.endpoint, 40),
            truncate(name, 12),
            status,
            peer.latency,
            success_rate,
            last_seen
        );
    }

    println!();
    println!("Total: {} peers", peers.len());

    Ok(())
}

/// Add a peer
pub fn cmd_federation_add_peer(
    url: &str,
    db_path: &str,
    tier: &str,
    name: Option<&str>,
) -> Result<()> {
    // Validate URL format (basic check)
    if !url.starts_with("http://") && !url.starts_with("https://") {
        anyhow::bail!("Invalid peer URL: {}. Must start with http:// or https://", url);
    }

    // Validate tier
    let tier_lower = tier.to_lowercase();
    if !["region_hub", "cell_hub", "leaf"].contains(&tier_lower.as_str()) {
        anyhow::bail!("Invalid tier: {}. Use: region_hub, cell_hub, leaf", tier);
    }

    let conn = Connection::open(db_path)?;

    // Generate peer ID
    let id = conary::hash::sha256(url.as_bytes());

    conn.execute(
        "INSERT INTO federation_peers (id, endpoint, node_name, tier)
         VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![id, url, name, tier_lower],
    )?;

    info!("Added federation peer: {} [{}]", url, tier_lower);
    println!("[OK] Added peer: {}", url);
    println!("     Tier: {}", tier_lower);
    if let Some(n) = name {
        println!("     Name: {}", n);
    }

    Ok(())
}

/// Remove a peer
pub fn cmd_federation_remove_peer(peer: &str, db_path: &str) -> Result<()> {
    let conn = Connection::open(db_path)?;

    // Try to match by URL or ID
    let deleted = conn.execute(
        "DELETE FROM federation_peers WHERE endpoint = ?1 OR id = ?1",
        [peer],
    )?;

    if deleted == 0 {
        anyhow::bail!("Peer not found: {}", peer);
    }

    info!("Removed federation peer: {}", peer);
    println!("[OK] Removed peer: {}", peer);

    Ok(())
}

/// Show federation statistics
pub fn cmd_federation_stats(db_path: &str, days: u32) -> Result<()> {
    let conn = Connection::open(db_path)?;

    let mut stmt = conn.prepare(
        "SELECT date, bytes_from_peers, bytes_from_upstream, chunks_from_peers,
                chunks_from_upstream, requests_coalesced, circuit_breaker_trips, peer_count
         FROM federation_stats
         ORDER BY date DESC
         LIMIT ?1",
    )?;

    let rows = stmt.query_map([days], |row| {
        Ok(StatsRow {
            date: row.get(0)?,
            bytes_peers: row.get(1)?,
            bytes_upstream: row.get(2)?,
            chunks_peers: row.get(3)?,
            chunks_upstream: row.get(4)?,
            coalesced: row.get(5)?,
            circuit_trips: row.get(6)?,
            peer_count: row.get(7)?,
        })
    })?;

    println!(
        "{:<12} {:>12} {:>12} {:>10} {:>10} {:>10}",
        "DATE", "FROM PEERS", "UPSTREAM", "SAVINGS", "COALESCED", "CB TRIPS"
    );
    println!("{}", "-".repeat(70));

    let mut total_peers = 0i64;
    let mut total_upstream = 0i64;
    let mut total_coalesced = 0i64;
    let mut count = 0;

    for row in rows {
        let stats = row?;
        let total = stats.bytes_peers + stats.bytes_upstream;
        let savings = if total > 0 {
            format!(
                "{:.1}%",
                (stats.bytes_peers as f64 / total as f64) * 100.0
            )
        } else {
            "-".to_string()
        };

        println!(
            "{:<12} {:>12} {:>12} {:>10} {:>10} {:>10}",
            stats.date,
            format_bytes(stats.bytes_peers as u64),
            format_bytes(stats.bytes_upstream as u64),
            savings,
            stats.coalesced,
            stats.circuit_trips
        );

        total_peers += stats.bytes_peers;
        total_upstream += stats.bytes_upstream;
        total_coalesced += stats.coalesced;
        count += 1;
    }

    if count > 0 {
        println!("{}", "-".repeat(70));
        let total = total_peers + total_upstream;
        let overall_savings = if total > 0 {
            (total_peers as f64 / total as f64) * 100.0
        } else {
            0.0
        };
        println!(
            "Total: {} from peers, {} from upstream ({:.1}% savings)",
            format_bytes(total_peers as u64),
            format_bytes(total_upstream as u64),
            overall_savings
        );
        println!("Coalesced requests: {}", total_coalesced);
    } else {
        println!("No statistics available");
    }

    Ok(())
}

/// Enable or disable a peer
pub fn cmd_federation_enable_peer(peer: &str, db_path: &str, enable: bool) -> Result<()> {
    let conn = Connection::open(db_path)?;

    let enabled_val: i32 = if enable { 1 } else { 0 };
    let updated = conn.execute(
        "UPDATE federation_peers SET is_enabled = ?1 WHERE endpoint = ?2 OR id = ?2",
        rusqlite::params![enabled_val, peer],
    )?;

    if updated == 0 {
        anyhow::bail!("Peer not found: {}", peer);
    }

    let action = if enable { "enabled" } else { "disabled" };
    info!("Federation peer {}: {}", action, peer);
    println!("[OK] Peer {}: {}", action, peer);

    Ok(())
}

/// Test connectivity to peers
pub fn cmd_federation_test(
    db_path: &str,
    peer: Option<&str>,
    timeout: u64,
) -> Result<()> {
    let conn = Connection::open(db_path)?;

    let endpoints: Vec<String> = if let Some(p) = peer {
        vec![p.to_string()]
    } else {
        let mut stmt = conn.prepare(
            "SELECT endpoint FROM federation_peers WHERE is_enabled = 1",
        )?;
        stmt.query_map([], |row| row.get(0))?
            .collect::<Result<Vec<_>, _>>()?
    };

    if endpoints.is_empty() {
        println!("No peers to test");
        return Ok(());
    }

    println!("Testing {} peer(s)...", endpoints.len());
    println!();

    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_millis(timeout))
        .build()?;

    let mut success_count = 0;
    let mut failure_count = 0;

    for endpoint in &endpoints {
        let health_url = format!("{}/health", endpoint.trim_end_matches('/'));
        let start = std::time::Instant::now();

        match client.get(&health_url).send() {
            Ok(response) if response.status().is_success() => {
                let elapsed = start.elapsed().as_millis();
                println!("[OK] {} - {}ms", endpoint, elapsed);
                success_count += 1;

                // Update latency in database
                let _ = conn.execute(
                    "UPDATE federation_peers SET latency_ms = ?1, last_seen = CURRENT_TIMESTAMP
                     WHERE endpoint = ?2",
                    rusqlite::params![elapsed as i64, endpoint],
                );
            }
            Ok(response) => {
                println!("[FAIL] {} - HTTP {}", endpoint, response.status());
                failure_count += 1;
            }
            Err(e) => {
                println!("[FAIL] {} - {}", endpoint, e);
                failure_count += 1;
            }
        }
    }

    println!();
    println!(
        "Results: {} OK, {} failed",
        success_count, failure_count
    );

    Ok(())
}

/// Scan for peers on the local network using mDNS
#[cfg(feature = "server")]
pub fn cmd_federation_scan(db_path: &str, duration_secs: u64, add_peers: bool) -> Result<()> {
    use conary::federation::{MdnsDiscovery, PeerTier};
    use std::time::Duration;

    println!("Scanning for Conary CAS peers on the local network...");
    println!();

    let mdns = MdnsDiscovery::new()?;
    let duration = Duration::from_secs(duration_secs);
    let peers = mdns.scan(duration)?;

    if peers.is_empty() {
        println!("No peers found on the local network.");
        println!();
        println!("Tip: Make sure other Conary nodes have mDNS enabled:");
        println!("     [federation]");
        println!("     enable_mdns = true");
        return Ok(());
    }

    println!(
        "{:<20} {:<30} {:<12} {:<8}",
        "INSTANCE", "ADDRESS", "TIER", "VERSION"
    );
    println!("{}", "-".repeat(75));

    for peer in &peers {
        let addr = peer
            .addresses
            .first()
            .map(|a| format!("{}:{}", a, peer.port))
            .unwrap_or_else(|| "unknown".to_string());

        let tier_str = match peer.tier {
            PeerTier::RegionHub => "region_hub",
            PeerTier::CellHub => "cell_hub",
            PeerTier::Leaf => "leaf",
        };

        println!(
            "{:<20} {:<30} {:<12} {:<8}",
            truncate(&peer.instance_name, 20),
            truncate(&addr, 30),
            tier_str,
            &peer.version
        );
    }

    println!();
    println!("Found {} peer(s)", peers.len());

    if add_peers {
        let conn = rusqlite::Connection::open(db_path)?;
        let mut added = 0;

        for discovered in &peers {
            if let Ok(peer) = discovered.to_peer() {
                // Check if peer already exists
                let exists: bool = conn
                    .query_row(
                        "SELECT 1 FROM federation_peers WHERE endpoint = ?1",
                        [&peer.endpoint],
                        |_| Ok(true),
                    )
                    .unwrap_or(false);

                if !exists {
                    let tier_str = match discovered.tier {
                        PeerTier::RegionHub => "region_hub",
                        PeerTier::CellHub => "cell_hub",
                        PeerTier::Leaf => "leaf",
                    };

                    conn.execute(
                        "INSERT INTO federation_peers (id, endpoint, node_name, tier)
                         VALUES (?1, ?2, ?3, ?4)",
                        rusqlite::params![
                            peer.id,
                            peer.endpoint,
                            peer.name,
                            tier_str
                        ],
                    )?;

                    println!("[OK] Added peer: {}", peer.endpoint);
                    added += 1;
                }
            }
        }

        if added > 0 {
            println!();
            println!("Added {} new peer(s) to the database", added);
        } else {
            println!();
            println!("All discovered peers already in database");
        }
    }

    Ok(())
}

// Helper types

#[allow(dead_code)]
struct PeerRow {
    id: String,
    endpoint: String,
    name: Option<String>,
    tier: String,
    latency: i64,
    successes: i64,
    failures: i64,
    consecutive_failures: i64,
    enabled: bool,
    last_seen: String,
}

#[allow(dead_code)]
struct StatsRow {
    date: String,
    bytes_peers: i64,
    bytes_upstream: i64,
    chunks_peers: i64,
    chunks_upstream: i64,
    coalesced: i64,
    circuit_trips: i64,
    peer_count: i64,
}

fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len.saturating_sub(3)])
    }
}
