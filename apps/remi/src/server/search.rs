// apps/remi/src/server/search.rs
//! Full-text search engine for the Remi package index
//!
//! Uses Tantivy to provide fast, typo-tolerant package search with faceted
//! filtering by distribution. Supports both full-text search and prefix-based
//! autocomplete suggestions.

use anyhow::{Context, Result};
use conary_core::db::models::ConvertedPackage;
use serde::Serialize;
use std::collections::HashSet;
use std::path::Path;
use tantivy::collector::TopDocs;
use tantivy::query::{BooleanQuery, Occur, QueryParser, RegexQuery, TermQuery};
use tantivy::schema::{
    self, Facet, FacetOptions, Field, NumericOptions, STORED, STRING, Schema, TextFieldIndexing,
    TextOptions, Value,
};
use tantivy::{Index, IndexReader, IndexWriter, ReloadPolicy, Term, doc};

/// Document to be indexed in the search engine
#[derive(Debug, Clone)]
pub struct PackageSearchDoc {
    pub name: String,
    pub version: String,
    pub release: Option<String>,
    pub distro: String,
    pub architecture: Option<String>,
    pub description: Option<String>,
    /// Search-only projection of exact persisted requirement atoms.
    pub requirement_terms: Option<String>,
    pub size: u64,
    pub converted: bool,
    pub source_kind: Option<String>,
}

/// Search result returned from queries
#[derive(Debug, Clone, Serialize)]
pub struct SearchResult {
    pub name: String,
    pub version: String,
    pub release: Option<String>,
    pub distro: String,
    pub description: Option<String>,
    pub size: u64,
    pub converted: bool,
    pub source_kind: Option<String>,
    pub score: f32,
}

/// Full-text search engine backed by Tantivy
pub struct SearchEngine {
    index: Index,
    reader: IndexReader,
    name_field: Field,
    name_exact_field: Field,
    /// Full package identity key for accurate delete-before-update
    name_distro_field: Field,
    version_field: Field,
    release_field: Field,
    distro_field: Field,
    source_kind_field: Field,
    description_field: Field,
    requirement_terms_field: Field,
    size_field: Field,
    converted_field: Field,
}

impl SearchEngine {
    /// Create or open a search index at the given directory
    pub fn new(index_dir: &Path) -> Result<Self> {
        let schema = Self::build_schema();

        let name_field = schema.get_field("name").unwrap();
        let name_exact_field = schema.get_field("name_exact").unwrap();
        let name_distro_field = schema.get_field("name_distro").unwrap();
        let version_field = schema.get_field("version").unwrap();
        let release_field = schema.get_field("release").unwrap();
        let distro_field = schema.get_field("distro").unwrap();
        let source_kind_field = schema.get_field("source_kind").unwrap();
        let description_field = schema.get_field("description").unwrap();
        let requirement_terms_field = schema.get_field("requirement_terms").unwrap();
        let size_field = schema.get_field("size").unwrap();
        let converted_field = schema.get_field("converted").unwrap();

        // Create or open index
        std::fs::create_dir_all(index_dir).with_context(|| {
            format!("Failed to create index directory: {}", index_dir.display())
        })?;

        let index = if index_dir.join("meta.json").exists() {
            match Index::open_in_dir(index_dir) {
                Ok(idx) => idx,
                Err(e) => {
                    // Schema mismatch or corruption — recreate the index
                    tracing::warn!("Recreating search index due to: {}", e);
                    for entry in std::fs::read_dir(index_dir)? {
                        let entry = entry?;
                        let path = entry.path();
                        if entry.file_type()?.is_dir() {
                            std::fs::remove_dir_all(&path)?;
                        } else {
                            std::fs::remove_file(&path)?;
                        }
                    }
                    Index::create_in_dir(index_dir, schema).with_context(|| {
                        format!("Failed to create index at {}", index_dir.display())
                    })?
                }
            }
        } else {
            Index::create_in_dir(index_dir, schema)
                .with_context(|| format!("Failed to create index at {}", index_dir.display()))?
        };

        let reader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::OnCommitWithDelay)
            .try_into()
            .context("Failed to create index reader")?;

        Ok(Self {
            index,
            reader,
            name_field,
            name_exact_field,
            name_distro_field,
            version_field,
            release_field,
            distro_field,
            source_kind_field,
            description_field,
            requirement_terms_field,
            size_field,
            converted_field,
        })
    }

    fn build_schema() -> Schema {
        let mut schema_builder = Schema::builder();

        // Name: tokenized for partial matching, stored for retrieval
        let text_indexing = TextFieldIndexing::default()
            .set_tokenizer("default")
            .set_index_option(schema::IndexRecordOption::WithFreqsAndPositions);
        let text_options = TextOptions::default()
            .set_indexing_options(text_indexing)
            .set_stored();
        schema_builder.add_text_field("name", text_options);

        // Name exact: for exact match boosting
        schema_builder.add_text_field("name_exact", STRING | STORED);

        // Full identity composite key for delete-before-update.
        schema_builder.add_text_field("name_distro", STRING);

        // Version: stored but not tokenized for search
        schema_builder.add_text_field("version", STRING | STORED);

        // Release and source kind: stored identity/projection fields
        schema_builder.add_text_field("release", STRING | STORED);
        schema_builder.add_text_field("source_kind", STRING | STORED);

        // Distro: faceted field for filtering (must be stored to retrieve in results)
        schema_builder.add_facet_field("distro", FacetOptions::default().set_stored());

        // Description: full-text searchable and stored
        let desc_indexing = TextFieldIndexing::default()
            .set_tokenizer("default")
            .set_index_option(schema::IndexRecordOption::WithFreqsAndPositions);
        let desc_options = TextOptions::default()
            .set_indexing_options(desc_indexing)
            .set_stored();
        schema_builder.add_text_field("description", desc_options);

        // Exact requirement atoms: searchable but not individually stored.
        let deps_indexing = TextFieldIndexing::default()
            .set_tokenizer("default")
            .set_index_option(schema::IndexRecordOption::WithFreqs);
        let deps_options = TextOptions::default().set_indexing_options(deps_indexing);
        schema_builder.add_text_field("requirement_terms", deps_options);

        // Size: numeric, stored and fast for sorting
        schema_builder.add_u64_field("size", NumericOptions::default().set_stored().set_fast());

        // Converted: boolean as u64 (0/1), stored and fast
        schema_builder.add_u64_field(
            "converted",
            NumericOptions::default().set_stored().set_fast(),
        );

        schema_builder.build()
    }

    /// Index a single package document (add or update)
    pub fn index_package(&self, pkg: &PackageSearchDoc) -> Result<()> {
        let mut writer = self
            .index
            .writer(15_000_000) // 15MB heap
            .context("Failed to create index writer")?;

        self.write_package(&mut writer, pkg)?;
        writer.commit().context("Failed to commit index")?;
        self.reader.reload().context("Failed to reload reader")?;
        Ok(())
    }

    /// Write a package document to the index writer (does not commit)
    fn write_package(&self, writer: &mut IndexWriter, pkg: &PackageSearchDoc) -> Result<()> {
        // Delete existing document with the same full package identity.
        let composite_key = search_document_key(
            &pkg.distro,
            &pkg.name,
            &pkg.version,
            pkg.release.as_deref(),
            pkg.architecture.as_deref(),
        );
        let delete_term = Term::from_field_text(self.name_distro_field, &composite_key);
        writer.delete_term(delete_term);

        let distro_facet = Facet::from_path([pkg.distro.as_str()]);
        let converted_val: u64 = if pkg.converted { 1 } else { 0 };

        let mut doc = doc!(
            self.name_field => pkg.name.clone(),
            self.name_exact_field => pkg.name.clone(),
            self.name_distro_field => composite_key,
            self.version_field => pkg.version.clone(),
            self.release_field => pkg.release.clone().unwrap_or_default(),
            self.distro_field => distro_facet,
            self.source_kind_field => pkg.source_kind.clone().unwrap_or_default(),
            self.size_field => pkg.size,
            self.converted_field => converted_val,
        );

        if let Some(ref desc) = pkg.description {
            doc.add_text(self.description_field, desc);
        }

        if let Some(ref terms) = pkg.requirement_terms {
            doc.add_text(self.requirement_terms_field, terms);
        }

        writer.add_document(doc)?;
        Ok(())
    }

    /// Rebuild the entire search index from the database
    pub fn rebuild_from_db(&self, db_path: &Path) -> Result<usize> {
        let conn = crate::server::open_runtime_db(db_path)?;

        let mut writer = self
            .index
            .writer(50_000_000) // 50MB heap for bulk indexing
            .context("Failed to create index writer for rebuild")?;

        // Clear existing index
        writer.delete_all_documents()?;

        // Query the latest version of each package per distro. Conversion
        // status comes only from current, structurally valid conversion rows.
        // Uses a subquery to find the most recently synced row per (name, repo).
        // Persisted repositories carry an exact public profile ID. The public
        // search document uses that profile's declared Remi route; repository
        // names never become serving identity.
        let current_conversions = current_conversion_search_keys(&conn)?;
        let mut stmt = conn.prepare(
            "SELECT rp.name, rp.version, rp.package_release,
                    r.source_profile as profile_id,
                    rp.description,
                    GROUP_CONCAT(
                        rr.capability || ' ' || COALESCE(rr.version_constraint, ''),
                        ' '
                    ) AS requirement_terms,
                    rp.size, rp.architecture,
                    EXISTS(
                        SELECT 1
                        FROM native_package_publications npp
                        WHERE npp.repository_package_id = rp.id
                          AND npp.status = 'public'
                    ) AS is_public_native
             FROM repository_packages rp
             JOIN repositories r ON rp.repository_id = r.id
             LEFT JOIN repository_requirements rr ON rr.repository_package_id = rp.id
             WHERE r.enabled = 1
               AND rp.size > 0
               AND rp.id = (
                   SELECT rp2.id FROM repository_packages rp2
                   WHERE rp2.repository_id = rp.repository_id AND rp2.name = rp.name
                    AND rp2.size > 0
                   ORDER BY rp2.synced_at DESC LIMIT 1
               )
             GROUP BY rp.id
             ORDER BY rp.name",
        )?;

        let mut count = 0;
        let rows = stmt.query_map([], |row| {
            let name: String = row.get(0)?;
            let version: String = row.get(1)?;
            let package_release: String = row.get(2)?;
            let profile_id: String = row.get(3)?;
            let profile = conary_core::repository::supported_profiles::profile_by_public_id(
                &profile_id,
            )
            .ok_or_else(|| {
                rusqlite::Error::FromSqlConversionFailure(
                    3,
                    rusqlite::types::Type::Text,
                    Box::new(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!(
                            "repository package has unsupported persisted profile '{profile_id}'"
                        ),
                    )),
                )
            })?;
            let distro = profile.remi_route_slug().to_string();
            let is_public_native: bool = row.get(8)?;
            if is_public_native {
                return Ok(None);
            }
            let architecture: Option<String> = row.get(7)?;
            let converted = current_conversions.contains(&SearchConversionKey {
                source_profile: profile_id.clone(),
                name: name.clone(),
                version: version.clone(),
                architecture: architecture.clone(),
            }) || current_conversions.contains(&SearchConversionKey {
                source_profile: profile_id,
                name: name.clone(),
                version: version.clone(),
                architecture: None,
            });
            Ok(Some(PackageSearchDoc {
                name,
                version,
                release: (!package_release.is_empty()).then_some(package_release),
                distro,
                architecture,
                description: row.get(4)?,
                requirement_terms: row.get(5)?,
                size: row.get::<_, i64>(6).map(|s| s as u64)?,
                converted,
                source_kind: None,
            }))
        })?;

        for row in rows {
            if let Some(pkg) = row.context("Failed to read package row")? {
                self.write_package(&mut writer, &pkg)?;
                count += 1;
            }
        }

        let mut native_stmt = conn.prepare(
            "SELECT source_profile, name, version, package_release, architecture, total_size
             FROM native_package_publications
             WHERE status = 'public'
             ORDER BY name, version, package_release, architecture",
        )?;
        let native_rows = native_stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, i64>(5)?,
            ))
        })?;

        for row in native_rows {
            let (source_profile, name, version, release, architecture, total_size) =
                row.context("Failed to read native package row")?;
            let profile =
                conary_core::repository::supported_profiles::profile_by_public_id(&source_profile)
                    .with_context(|| {
                        format!(
                            "native package publication has unsupported source profile \
                             '{source_profile}'"
                        )
                    })?;
            let pkg = PackageSearchDoc {
                distro: profile.remi_route_slug().to_string(),
                name,
                version,
                release: Some(release),
                architecture: Some(architecture),
                description: Some("Native CCS release artifact".to_string()),
                requirement_terms: None,
                size: u64::try_from(total_size).context("negative native publication size")?,
                converted: false,
                source_kind: Some("native-ccs".to_string()),
            };
            self.write_package(&mut writer, &pkg)?;
            count += 1;
        }

        writer.commit().context("Failed to commit rebuild")?;
        self.reader.reload().context("Failed to reload reader")?;

        tracing::info!("Search index rebuilt with {} packages", count);
        Ok(count)
    }

    /// Full-text search with optional distro filter
    pub fn search(
        &self,
        query: &str,
        distro: Option<&str>,
        limit: usize,
    ) -> Result<Vec<SearchResult>> {
        let searcher = self.reader.searcher();

        // Build query: search name (boosted) and description
        let mut query_parser =
            QueryParser::for_index(&self.index, vec![self.name_field, self.description_field]);
        query_parser.set_field_boost(self.name_field, 3.0);

        let parsed_query = query_parser
            .parse_query(query)
            .context("Failed to parse search query")?;

        // If distro filter is specified, wrap in a boolean query with facet filter
        let final_query: Box<dyn tantivy::query::Query> = if let Some(distro_name) = distro {
            let facet = Facet::from_path([distro_name]);
            let facet_term = Term::from_facet(self.distro_field, &facet);
            let facet_query = TermQuery::new(facet_term, schema::IndexRecordOption::Basic);

            Box::new(BooleanQuery::new(vec![
                (Occur::Must, parsed_query),
                (Occur::Must, Box::new(facet_query)),
            ]))
        } else {
            parsed_query
        };

        let top_docs = searcher
            .search(&final_query, &TopDocs::with_limit(limit).order_by_score())
            .context("Search failed")?;

        let mut results = Vec::with_capacity(top_docs.len());
        for (score, doc_address) in top_docs {
            let doc: tantivy::TantivyDocument = searcher.doc(doc_address)?;

            let name = doc
                .get_first(self.name_exact_field)
                .and_then(|v| v.as_str())
                .context("search index document has no exact package name")?
                .to_string();

            let version = doc
                .get_first(self.version_field)
                .and_then(|v| v.as_str())
                .context("search index document has no package version")?
                .to_string();

            let release = doc
                .get_first(self.release_field)
                .and_then(|v| v.as_str())
                .filter(|value| !value.is_empty())
                .map(String::from);

            let stored_distro_facet = doc
                .get_first(self.distro_field)
                .and_then(|v| v.as_facet())
                .context("search index document has no distro facet")?;
            let distro_facet = Facet::from_encoded(stored_distro_facet.as_bytes().to_vec())
                .context("search index document has an invalid encoded distro facet")?;
            let distro_path = distro_facet.to_path();
            let distro_val = match distro_path.as_slice() {
                [distro] if !distro.is_empty() => (*distro).to_string(),
                _ => anyhow::bail!(
                    "search index distro facet must contain exactly one non-empty path segment"
                ),
            };

            let description = doc
                .get_first(self.description_field)
                .and_then(|v| v.as_str())
                .map(String::from);

            let size = doc
                .get_first(self.size_field)
                .and_then(|v| v.as_u64())
                .context("search index document has no package size")?;

            let converted_value = doc
                .get_first(self.converted_field)
                .and_then(|v| v.as_u64())
                .context("search index document has no conversion status")?;
            let converted = match converted_value {
                0 => false,
                1 => true,
                value => anyhow::bail!(
                    "search index document has invalid conversion status {value}; expected 0 or 1"
                ),
            };

            let source_kind = doc
                .get_first(self.source_kind_field)
                .and_then(|v| v.as_str())
                .filter(|value| !value.is_empty())
                .map(String::from);

            results.push(SearchResult {
                name,
                version,
                release,
                distro: distro_val,
                description,
                size,
                converted,
                source_kind,
                score,
            });
        }

        Ok(results)
    }

    /// Autocomplete suggestions based on package name prefix
    pub fn suggest(&self, prefix: &str, limit: usize) -> Result<Vec<String>> {
        if prefix.is_empty() {
            return Ok(Vec::new());
        }

        let searcher = self.reader.searcher();

        // Use regex query on the name_exact field for prefix matching
        // Escape special regex characters in the prefix
        let escaped = regex_escape(prefix);
        let pattern = format!("{escaped}.*");

        let regex_query = RegexQuery::from_pattern(&pattern, self.name_exact_field)
            .context("Failed to create prefix query")?;

        let top_docs = searcher
            .search(&regex_query, &TopDocs::with_limit(limit).order_by_score())
            .context("Suggest search failed")?;

        let mut names: Vec<String> = Vec::with_capacity(top_docs.len());
        let mut seen = std::collections::HashSet::new();

        for (_score, doc_address) in top_docs {
            let doc: tantivy::TantivyDocument = searcher.doc(doc_address)?;
            if let Some(name) = doc
                .get_first(self.name_exact_field)
                .and_then(|v| v.as_str())
                && seen.insert(name.to_string())
            {
                names.push(name.to_string());
            }
        }

        Ok(names)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct SearchConversionKey {
    source_profile: String,
    name: String,
    version: String,
    architecture: Option<String>,
}

fn current_conversion_search_keys(
    conn: &rusqlite::Connection,
) -> Result<HashSet<SearchConversionKey>> {
    let mut keys = HashSet::new();
    for converted in ConvertedPackage::list_repository_conversions(conn)? {
        if converted.repository_metadata_is_current(conn)? {
            let artifact = converted.repository_artifact()?;
            converted.scriptlet_summary()?;
            keys.insert(SearchConversionKey {
                source_profile: artifact.source_profile.to_string(),
                name: artifact.package_name.to_string(),
                version: artifact.package_version.to_string(),
                architecture: Some(artifact.package_architecture.to_string()),
            });
        }
    }
    Ok(keys)
}

fn search_document_key(
    distro: &str,
    name: &str,
    version: &str,
    release: Option<&str>,
    architecture: Option<&str>,
) -> String {
    format!(
        "{}\0{}\0{}\0{}\0{}",
        name,
        distro,
        version,
        release.unwrap_or(""),
        architecture.unwrap_or("")
    )
}

/// Escape regex special characters in a string
fn regex_escape(s: &str) -> String {
    let mut escaped = String::with_capacity(s.len() * 2);
    for c in s.chars() {
        match c {
            '.' | '+' | '*' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '\\' | '|' | '^' | '$' => {
                escaped.push('\\');
                escaped.push(c);
            }
            _ => escaped.push(c),
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::native_publish::test_support::seed_native_publication;
    use conary_core::db::models::{
        CONVERSION_VERSION, ConvertedPackage, Repository, RepositoryPackage,
    };
    use tempfile::TempDir;

    fn create_test_engine() -> (TempDir, SearchEngine) {
        let dir = TempDir::new().unwrap();
        let engine = SearchEngine::new(dir.path()).unwrap();
        (dir, engine)
    }

    fn create_test_db() -> (tempfile::NamedTempFile, rusqlite::Connection) {
        let temp_file = tempfile::NamedTempFile::new().unwrap();
        let conn = rusqlite::Connection::open(temp_file.path()).unwrap();
        conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
        conary_core::db::schema::ensure_current(&conn).unwrap();
        (temp_file, conn)
    }

    fn insert_repo_package(conn: &rusqlite::Connection, distro: &str, name: &str, version: &str) {
        let profile = conary_core::repository::supported_profiles::profile_for_remi_route(distro)
            .unwrap_or_else(|| panic!("test route '{distro}' must name a supported Remi profile"));
        let mut repo = Repository::new(format!("{distro}-base"), "https://example.com".to_string());
        repo.source_profile = Some(profile.id().to_string());
        let repo_id = repo.insert(conn).unwrap();

        let mut pkg = RepositoryPackage::new(
            repo_id,
            name.to_string(),
            version.to_string(),
            profile.version_scheme(),
            format!("sha256:{name}-{version}"),
            1024,
            format!("https://example.com/{name}-{version}.rpm"),
        );
        pkg.architecture = Some("x86_64".to_string());
        pkg.description = Some("test package".to_string());
        pkg.insert(conn).unwrap();
    }

    fn insert_stale_conversion(
        conn: &rusqlite::Connection,
        distro: &str,
        package: &str,
        version: &str,
    ) {
        let source_profile =
            conary_core::repository::supported_profiles::profile_for_remi_route(distro)
                .unwrap_or_else(|| panic!("test route '{distro}' must be supported"))
                .id();
        let transport = crate::server::conversion::test_support::test_transport(&[format!(
            "sha256:{package}-{version}-chunk"
        )]);
        let mut converted = ConvertedPackage::new_repository(
            source_profile.to_string(),
            package.to_string(),
            version.to_string(),
            "x86_64".to_string(),
            "rpm".to_string(),
            format!("sha256:{package}-{version}-source"),
            &transport,
            42,
            format!("sha256:{package}-{version}-content"),
            format!("/tmp/{package}-{version}.ccs"),
            conary_core::db::models::EMPTY_REPOSITORY_PROVIDES_DIGEST.to_string(),
        );
        converted.conversion_version = CONVERSION_VERSION - 1;
        converted.insert(conn).unwrap();
    }

    #[test]
    fn test_index_and_search() {
        let (_dir, engine) = create_test_engine();

        let pkg = PackageSearchDoc {
            name: "nginx".to_string(),
            version: "1.24.0".to_string(),
            release: None,
            distro: "fedora".to_string(),
            architecture: Some("x86_64".to_string()),
            description: Some("High performance HTTP server and reverse proxy".to_string()),
            requirement_terms: Some("openssl pcre2 zlib".to_string()),
            size: 1_200_000,
            converted: true,
            source_kind: None,
        };
        engine.index_package(&pkg).unwrap();

        let pkg2 = PackageSearchDoc {
            name: "curl".to_string(),
            version: "8.5.0".to_string(),
            release: None,
            distro: "fedora".to_string(),
            architecture: Some("x86_64".to_string()),
            description: Some("Command line tool for transferring data".to_string()),
            requirement_terms: Some("openssl nghttp2 zlib".to_string()),
            size: 500_000,
            converted: false,
            source_kind: None,
        };
        engine.index_package(&pkg2).unwrap();

        // Search for nginx
        let results = engine.search("nginx", None, 10).unwrap();
        assert!(!results.is_empty());
        assert_eq!(results[0].name, "nginx");
        assert_eq!(results[0].distro, "fedora");
        assert!(results[0].converted);

        // Search for HTTP - should find nginx via description
        let results = engine.search("HTTP server", None, 10).unwrap();
        assert!(!results.is_empty());
        assert_eq!(results[0].name, "nginx");

        // Search with distro filter
        let results = engine.search("nginx", Some("fedora"), 10).unwrap();
        assert!(!results.is_empty());

        let results = engine.search("nginx", Some("arch"), 10).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn search_rebuild_preserves_native_release_identity_and_converted_false() {
        let (_dir, engine) = create_test_engine();
        let (db, conn) = create_test_db();
        seed_native_publication(
            &conn,
            "fedora",
            "hello",
            "1.0.0",
            "1",
            "noarch",
            "/tmp/hello-1.ccs",
        );
        seed_native_publication(
            &conn,
            "fedora",
            "hello",
            "1.0.0",
            "2",
            "noarch",
            "/tmp/hello-2.ccs",
        );

        engine.rebuild_from_db(db.path()).unwrap();
        let results = engine.search("hello", Some("fedora"), 10).unwrap();

        assert_eq!(
            results
                .iter()
                .filter(|result| result.name == "hello")
                .count(),
            2
        );
        assert!(results.iter().all(|result| !result.converted));
    }

    #[test]
    fn test_suggest() {
        let (_dir, engine) = create_test_engine();

        for name in &["nginx", "nginx-module-njs", "nmap", "nodejs", "nano"] {
            let pkg = PackageSearchDoc {
                name: (*name).to_string(),
                version: "1.0.0".to_string(),
                release: None,
                distro: "fedora".to_string(),
                architecture: Some("x86_64".to_string()),
                description: None,
                requirement_terms: None,
                size: 0,
                converted: false,
                source_kind: None,
            };
            engine.index_package(&pkg).unwrap();
        }

        // Prefix "ngi" should match nginx*
        let suggestions = engine.suggest("ngi", 10).unwrap();
        assert!(!suggestions.is_empty());
        assert!(suggestions.iter().all(|s| s.starts_with("ngi")));

        // Prefix "n" should match multiple
        let suggestions = engine.suggest("n", 10).unwrap();
        assert!(suggestions.len() >= 2);

        // Empty prefix returns nothing
        let suggestions = engine.suggest("", 10).unwrap();
        assert!(suggestions.is_empty());
    }

    #[test]
    fn test_update_existing_package() {
        let (_dir, engine) = create_test_engine();

        let pkg = PackageSearchDoc {
            name: "vim".to_string(),
            version: "9.0".to_string(),
            release: None,
            distro: "arch".to_string(),
            architecture: Some("x86_64".to_string()),
            description: Some("Vi Improved".to_string()),
            requirement_terms: None,
            size: 2_000_000,
            converted: false,
            source_kind: None,
        };
        engine.index_package(&pkg).unwrap();

        // Update with the same full identity
        let pkg_updated = PackageSearchDoc {
            name: "vim".to_string(),
            version: "9.0".to_string(),
            release: None,
            distro: "arch".to_string(),
            architecture: Some("x86_64".to_string()),
            description: Some("Vi Improved - text editor".to_string()),
            requirement_terms: None,
            size: 2_100_000,
            converted: true,
            source_kind: None,
        };
        engine.index_package(&pkg_updated).unwrap();

        let results = engine.search("vim", None, 10).unwrap();
        // Should have the updated document for the exact identity.
        assert!(!results.is_empty());
        assert!(results[0].converted);
    }

    #[test]
    fn search_rebuild_marks_stale_rows_unconverted() {
        let (db_file, _) = create_test_db();
        let conn = crate::server::open_runtime_db(db_file.path()).unwrap();
        insert_repo_package(&conn, "fedora", "gtk3", "3.24.0");
        insert_stale_conversion(&conn, "fedora", "gtk3", "3.24.0");
        drop(conn);

        let (_dir, engine) = create_test_engine();
        engine.rebuild_from_db(db_file.path()).unwrap();

        let results = engine.search("gtk3", Some("fedora"), 10).unwrap();

        assert_eq!(results.len(), 1);
        assert!(!results[0].converted);
    }

    #[test]
    fn test_regex_escape() {
        assert_eq!(regex_escape("hello"), "hello");
        assert_eq!(regex_escape("lib++"), "lib\\+\\+");
        assert_eq!(regex_escape("foo.bar"), "foo\\.bar");
        assert_eq!(regex_escape("test[0]"), "test\\[0\\]");
    }
}
