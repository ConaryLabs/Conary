// apps/remi/src/server/conversion/test_support.rs
//! Shared test helpers for Remi conversion child modules.

use conary_core::ccs::convert::{ConversionResult, ScriptletBundleSummary};
use conary_core::db::models::{Repository, RepositoryPackage};
use conary_core::db::schema;
use std::fs;
use std::path::Path;
use tempfile::NamedTempFile;

pub(super) fn create_test_db() -> (NamedTempFile, rusqlite::Connection) {
    let temp_file = NamedTempFile::new().unwrap();
    let conn = rusqlite::Connection::open(temp_file.path()).unwrap();
    conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
    schema::migrate(&conn).unwrap();
    (temp_file, conn)
}

pub(super) fn insert_repo(conn: &rusqlite::Connection, name: &str, distro: &str) -> i64 {
    let mut repo = Repository::new(name.to_string(), "https://example.com".to_string());
    repo.default_strategy_distro = Some(distro.to_string());
    repo.insert(conn).unwrap()
}

pub(super) fn insert_package(
    conn: &rusqlite::Connection,
    repo_id: i64,
    name: &str,
    version: &str,
    size: i64,
) {
    let mut pkg = RepositoryPackage::new(
        repo_id,
        name.to_string(),
        version.to_string(),
        format!("sha256:{name}-{version}"),
        size,
        format!("https://example.com/{name}-{version}.rpm"),
    );
    pkg.architecture = Some("x86_64".to_string());
    pkg.dependencies = Some(r#"["glibc","openssl"]"#.to_string());
    pkg.insert(conn).unwrap();
}

pub(super) fn production_source_without_comments(relative_path: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(relative_path);
    let source = fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("failed to read {}: {err}", path.display()));
    let mut stripped = String::new();
    let mut in_block_comment = false;
    let mut pending_test_cfg = false;

    for line in source.lines() {
        let trimmed = line.trim_start();
        if pending_test_cfg {
            if trimmed.starts_with("#[") {
                continue;
            }
            if trimmed.starts_with("mod tests") {
                break;
            }
            pending_test_cfg = false;
        }
        if trimmed.starts_with("#[cfg(test)]") {
            pending_test_cfg = true;
        }

        let mut chars = line.chars().peekable();
        while let Some(ch) = chars.next() {
            if in_block_comment {
                if ch == '*' && chars.peek() == Some(&'/') {
                    let _ = chars.next();
                    in_block_comment = false;
                }
                continue;
            }

            if ch == '/' && chars.peek() == Some(&'/') {
                break;
            }

            if ch == '/' && chars.peek() == Some(&'*') {
                let _ = chars.next();
                in_block_comment = true;
                continue;
            }

            stripped.push(ch);
        }

        stripped.push('\n');
    }

    stripped
}

pub(super) fn make_conversion_result(
    blobs: std::collections::HashMap<String, Vec<u8>>,
) -> ConversionResult {
    use conary_core::ccs::builder::{BuildResult, FileEntry};
    use conary_core::ccs::convert::FidelityReport;
    use conary_core::ccs::manifest::{CcsManifest, Hooks, Package};

    let manifest = CcsManifest {
        package: Package {
            name: "test".to_string(),
            version: "1.0".to_string(),
            release: None,
            kind: None,
            description: "test package".to_string(),
            license: None,
            homepage: None,
            repository: None,
            platform: None,
            authors: None,
        },
        provides: Default::default(),
        requires: Default::default(),
        suggests: Default::default(),
        components: Default::default(),
        hooks: Hooks::default(),
        scriptlets: Default::default(),
        legacy_scriptlets: None,
        config: Default::default(),
        build: None,
        legacy: None,
        policy: Default::default(),
        provenance: None,
        capabilities: None,
        redirects: Default::default(),
    };

    let build_result = BuildResult {
        manifest,
        components: std::collections::HashMap::new(),
        files: Vec::<FileEntry>::new(),
        blobs,
        total_size: 0,
        chunked: false,
        chunk_stats: None,
    };

    ConversionResult {
        build_result,
        package_path: None,
        fidelity: FidelityReport::default(),
        original_format: "rpm".to_string(),
        original_checksum: "sha256:test".to_string(),
        detected_hooks: Hooks::default(),
        inferred_capabilities: None,
        inference_error: None,
        legacy_provenance: None,
        scriptlet_classification: Default::default(),
        legacy_scriptlets: None,
        scriptlet_metadata: ScriptletBundleSummary::default(),
    }
}

pub(super) fn goal8a_scriptlet_summary(
    scriptlet_fidelity: &str,
    target_compatibility: &str,
    publication_status: &str,
) -> ScriptletBundleSummary {
    ScriptletBundleSummary {
        scriptlet_fidelity: scriptlet_fidelity.to_string(),
        target_compatibility: target_compatibility.to_string(),
        publication_status: publication_status.to_string(),
        evidence_digest: Some(conary_core::hash::sha256_prefixed(
            format!("{scriptlet_fidelity}:{publication_status}").as_bytes(),
        )),
        ..ScriptletBundleSummary::default()
    }
}
