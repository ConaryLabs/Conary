// apps/remi/src/server/native_publish/test_support.rs
//! Test helpers for native Remi publication.

#[cfg(test)]
pub fn assert_json_code(body: &str, expected: &str) {
    let value: serde_json::Value = serde_json::from_str(body).unwrap();
    assert_eq!(value["code"], expected);
}

#[cfg(test)]
pub fn seed_native_publication(
    conn: &rusqlite::Connection,
    distro: &str,
    name: &str,
    version: &str,
    package_release: &str,
    architecture: &str,
    package_path: &str,
) {
    use rusqlite::params;

    conn.execute(
        "INSERT OR IGNORE INTO repositories (name, url, enabled, tuf_enabled)
         VALUES (?1, ?2, 1, 1)",
        params![distro, format!("remi-release://{distro}")],
    )
    .unwrap();
    let repo_id: i64 = conn
        .query_row(
            "SELECT id FROM repositories WHERE name = ?1",
            params![distro],
            |row| row.get(0),
        )
        .unwrap();
    let content_hash = format!("sha256:{name}-{version}-{package_release}-{architecture}");
    let chunk_hash = format!("sha256:{name}-{version}-{package_release}-{architecture}-chunk");
    let metadata = serde_json::json!({
        "source_kind": "native-ccs",
        "native": true,
        "identity": {
            "name": name,
            "version": version,
            "release": package_release,
            "architecture": architecture,
        },
        "trust": {
            "status": "verified",
            "hardening_level": "hermetic",
        }
    })
    .to_string();

    conn.execute(
        "INSERT INTO repository_packages
         (repository_id, name, version, package_release, architecture, description,
          checksum, size, download_url, metadata, distro)
         VALUES (?1, ?2, ?3, ?4, ?5, 'native test package', ?6, 42, ?7, ?8, ?9)",
        params![
            repo_id,
            name,
            version,
            package_release,
            architecture,
            content_hash,
            format!("/v1/chunks/{content_hash}"),
            metadata,
            distro,
        ],
    )
    .unwrap();
    let repo_package_id = conn.last_insert_rowid();
    conn.execute(
        "INSERT INTO native_package_publications (
            repository_id, repository_package_id, distro, name, version, package_release,
            architecture, package_kind, authority_format_version, status, content_hash,
            chunk_hashes_json, total_size, package_path, target_path, trust_status
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'package', 2, 'public', ?8, ?9,
                   42, ?10, ?11, 'verified')",
        params![
            repo_id,
            repo_package_id,
            distro,
            name,
            version,
            package_release,
            architecture,
            content_hash,
            serde_json::to_string(&[chunk_hash]).unwrap(),
            package_path,
            format!("packages/{distro}/{name}-{version}-{package_release}-{architecture}.ccs"),
        ],
    )
    .unwrap();
}
