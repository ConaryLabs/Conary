// apps/remi/src/server/conversion/test_support.rs
//! Shared test helpers for Remi conversion child modules.

use conary_core::ccs::convert::{ConversionResult, ScriptletBundleSummary};
use conary_core::db::models::{Repository, RepositoryPackage};
use conary_core::db::schema;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::NamedTempFile;
use walkdir::WalkDir;

pub(super) fn create_test_db() -> (NamedTempFile, rusqlite::Connection) {
    let temp_file = NamedTempFile::new().unwrap();
    let conn = rusqlite::Connection::open(temp_file.path()).unwrap();
    conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
    schema::ensure_current(&conn).unwrap();
    (temp_file, conn)
}

pub(super) fn insert_repo(conn: &rusqlite::Connection, name: &str, distro: &str) -> i64 {
    let mut repo = Repository::new(name.to_string(), "https://example.com".to_string());
    let profile = conary_core::repository::supported_profiles::profile_by_public_id(distro)
        .or_else(|| conary_core::repository::supported_profiles::profile_for_remi_route(distro))
        .unwrap_or_else(|| {
            panic!("test source '{distro}' must name an exact profile or supported Remi route")
        });
    repo.source_profile = Some(profile.id().to_string());
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
        conary_core::repository::versioning::VersionScheme::Rpm,
        format!("sha256:{name}-{version}"),
        size,
        format!("https://example.com/{name}-{version}.rpm"),
    );
    pkg.architecture = Some("x86_64".to_string());
    pkg.insert(conn).unwrap();
}

fn production_source_without_comments(path: &Path) -> String {
    let source = fs::read_to_string(path)
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

pub(super) fn production_rust_sources(relative_root: &str) -> Vec<(PathBuf, String)> {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let root = manifest_dir.join(relative_root);
    let mut sources = WalkDir::new(&root)
        .into_iter()
        .map(|entry| entry.unwrap_or_else(|error| panic!("walk {}: {error}", root.display())))
        .filter(|entry| {
            entry.file_type().is_file()
                && entry
                    .path()
                    .extension()
                    .is_some_and(|extension| extension == "rs")
        })
        .map(|entry| {
            let path = entry.path().to_path_buf();
            let relative = path
                .strip_prefix(manifest_dir)
                .expect("server source stays below the Remi crate")
                .to_path_buf();
            let source = production_source_without_comments(&path);
            (relative, source)
        })
        .collect::<Vec<_>>();
    sources.sort_by(|left, right| left.0.cmp(&right.0));
    sources
}

pub(super) fn make_conversion_result(package_path: Option<std::path::PathBuf>) -> ConversionResult {
    use conary_core::ccs::builder::{BuildResult, FileEntry};
    use conary_core::ccs::manifest::{CcsManifest, Hooks, Package};

    let manifest = CcsManifest {
        package: Package {
            name: "test".to_string(),
            version: "1.0".to_string(),
            version_scheme: conary_core::repository::versioning::VersionScheme::Conary,
            release: "1".to_string(),
            kind: conary_core::ccs::v2::PackageKindTagV2::Package,
            debian_multi_arch: None,
            description: "test package".to_string(),
            license: None,
            homepage: None,
            repository: None,
            platform: None,
            authors: None,
        },
        provides: Default::default(),
        requirements: Vec::new(),
        relations: Vec::new(),
        suggests: Default::default(),
        components: Default::default(),
        hooks: Hooks::default(),
        scriptlets: Default::default(),
        native_lifecycle: None,
        config: Default::default(),
        build: None,
        native_export: None,
        policy: Default::default(),
        file_capabilities: Vec::new(),
        provenance: None,
        capabilities: None,
        redirects: Default::default(),
    };

    let build_result = BuildResult {
        manifest,
        components: std::collections::HashMap::new(),
        files: Vec::<FileEntry>::new(),
        blobs: std::collections::HashMap::new(),
        total_size: 0,
        chunked: false,
        chunk_stats: None,
    };

    ConversionResult {
        build_result,
        package_path,
        original_format: "rpm".to_string(),
        original_checksum: "sha256:test".to_string(),
        signing_public_key: conary_core::ccs::SigningKeyPair::generate().public_key_base64(),
        native_provenance: None,
        native_lifecycle: None,
        scriptlet_metadata: ScriptletBundleSummary::default(),
    }
}
