// apps/remi/src/server/search.rs
//! Full-text search engine for the Remi package index
//!
//! Uses Tantivy to provide fast, typo-tolerant package search with faceted
//! filtering by distribution. Supports both full-text search and prefix-based
//! autocomplete suggestions.

use anyhow::{Context, Result};
use conary_core::db::models::ConvertedPackage;
use conary_core::repository::catalog::CatalogPackageRecordV1;
use parking_lot::RwLock;
use serde::Serialize;
use std::collections::{BTreeSet, HashSet};
use std::path::Path;
use tantivy::collector::TopDocs;
use tantivy::query::{BooleanQuery, Occur, QueryParser, RegexQuery, TermQuery};
use tantivy::schema::{
    self, Facet, FacetOptions, Field, NumericOptions, STORED, STRING, Schema, TextFieldIndexing,
    TextOptions, Value,
};
use tantivy::{Index, IndexReader, IndexWriter, ReloadPolicy, Term, doc};

use super::catalog_authority::CatalogAuthority;
use super::profile_catalog::ProfileCatalog;
use super::public_universe::{PublicUniverseIdentity, PublicUniverseSnapshot};

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

#[derive(Debug)]
pub(crate) enum PublicSearchError {
    Unavailable,
    RevisionMismatch,
    Query(anyhow::Error),
}

/// Full-text search engine backed by Tantivy
pub struct SearchEngine {
    index: Index,
    reader: IndexReader,
    /// Exact signed universe projected by the currently committed index.
    /// Persisted Tantivy bytes have no public authority until this process
    /// successfully rebuilds and binds them.
    authority: RwLock<Option<PublicUniverseIdentity>>,
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
            authority: RwLock::new(None),
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

    /// Index a single package document (add or update) in focused unit tests.
    #[cfg(test)]
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

    /// Rebuild the entire search index from one signed public universe.
    ///
    /// Operational SQLite contributes native publication outcomes and exact
    /// conversion pins only. It never supplies activated source-package
    /// metadata for this projection.
    pub(crate) fn rebuild_from_universe(
        &self,
        db_path: &Path,
        catalog_authority: &CatalogAuthority,
        universe: &PublicUniverseSnapshot,
    ) -> Result<usize> {
        let conn = crate::server::open_runtime_db(db_path)?;

        let mut writer = self
            .index
            .writer(50_000_000) // 50MB heap for bulk indexing
            .context("Failed to create index writer for rebuild")?;

        // Clear existing index
        writer.delete_all_documents()?;

        let mut count = 0;
        let native_keys = current_native_search_keys(&conn)?;
        let source_profiles = universe
            .profiles()
            .map(|selection| selection.source_profile.clone())
            .collect::<BTreeSet<_>>();
        for selection in universe.profiles() {
            let source_profile = &selection.source_profile;
            let profile =
                conary_core::repository::supported_profiles::profile_by_public_id(source_profile)
                    .with_context(|| {
                    format!(
                        "unsupported exact source profile '{source_profile}' for search rebuild"
                    )
                })?;
            let pinned = catalog_authority
                .open_selected_profile(selection)
                .with_context(|| {
                    format!("open public universe catalog for search profile '{source_profile}'")
                })?;
            let current_conversions =
                current_conversion_search_keys(&conn, pinned.profile_revision_sha256())?;
            for package in ProfileCatalog::new(&pinned).downloadable_package_records(1)? {
                require_catalog_profile(&package, source_profile)?;
                let native_key = NativeSearchKey::from_catalog(&package);
                if native_keys.contains(&native_key) {
                    continue;
                }
                let converted =
                    current_conversions.contains(&SearchConversionKey::from_catalog(&package));
                let pkg = PackageSearchDoc {
                    name: package.name,
                    version: package.version,
                    release: (!package.package_release.is_empty())
                        .then_some(package.package_release),
                    distro: profile.remi_route_slug().to_string(),
                    architecture: package.architecture,
                    description: package.description,
                    requirement_terms: catalog_requirement_terms(&package.requirement_groups),
                    size: package.size,
                    converted,
                    source_kind: None,
                };
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
            if !source_profiles.contains(&source_profile) {
                continue;
            }
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

        // A public search holds this lock from its authority comparison through
        // the Tantivy read. Take the matching write lock only for the committed
        // index swap so no request can validate one revision and query another.
        let mut authority = self.authority.write();
        writer.commit().context("Failed to commit rebuild")?;
        if let Err(error) = self.reader.reload().context("Failed to reload reader") {
            *authority = None;
            return Err(error);
        }
        *authority = Some(universe.identity().clone());

        tracing::info!(
            universe = %universe.identity().manifest_sha256,
            sequence = universe.identity().sequence,
            "Search index rebuilt with {count} packages"
        );
        Ok(count)
    }

    pub(crate) fn search_public_universe(
        &self,
        expected: &PublicUniverseIdentity,
        query: &str,
        distro: Option<&str>,
        limit: usize,
    ) -> std::result::Result<Vec<SearchResult>, PublicSearchError> {
        let authority = self.authority.read();
        match authority.as_ref() {
            None => return Err(PublicSearchError::Unavailable),
            Some(actual) if actual != expected => {
                return Err(PublicSearchError::RevisionMismatch);
            }
            Some(_) => {}
        }
        let result = self
            .search(query, distro, limit)
            .map_err(PublicSearchError::Query);
        drop(authority);
        result
    }

    pub(crate) fn suggest_public_universe(
        &self,
        expected: &PublicUniverseIdentity,
        prefix: &str,
        limit: usize,
    ) -> std::result::Result<Vec<String>, PublicSearchError> {
        let authority = self.authority.read();
        match authority.as_ref() {
            None => return Err(PublicSearchError::Unavailable),
            Some(actual) if actual != expected => {
                return Err(PublicSearchError::RevisionMismatch);
            }
            Some(_) => {}
        }
        let result = self
            .suggest(prefix, limit)
            .map_err(PublicSearchError::Query);
        drop(authority);
        result
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
    name: String,
    version: String,
    architecture: Option<String>,
    checksum: String,
}

fn current_conversion_search_keys(
    conn: &rusqlite::Connection,
    profile_revision_sha256: &str,
) -> Result<HashSet<SearchConversionKey>> {
    let mut keys = HashSet::new();
    for converted in
        ConvertedPackage::find_current_conversions(conn, profile_revision_sha256, None)?
    {
        let converted_id = converted
            .id
            .context("persisted repository conversion has no row identity")?;
        ConvertedPackage::require_conversion_pin(conn, converted_id)?;
        let artifact = converted.repository_artifact()?;
        converted.scriptlet_summary()?;
        keys.insert(SearchConversionKey {
            name: artifact.package_name.to_string(),
            version: artifact.package_version.to_string(),
            architecture: Some(artifact.package_architecture.to_string()),
            checksum: converted.original_checksum,
        });
    }
    Ok(keys)
}

impl SearchConversionKey {
    fn from_catalog(package: &CatalogPackageRecordV1) -> Self {
        Self {
            name: package.name.clone(),
            version: package.version.clone(),
            architecture: package.architecture.clone(),
            checksum: package.checksum.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct NativeSearchKey {
    source_profile: String,
    name: String,
    version: String,
    package_release: String,
    architecture: String,
}

impl NativeSearchKey {
    fn from_catalog(package: &CatalogPackageRecordV1) -> Self {
        Self {
            source_profile: package.source_profile.clone(),
            name: package.name.clone(),
            version: package.version.clone(),
            package_release: package.package_release.clone(),
            architecture: package.architecture.clone().unwrap_or_default(),
        }
    }
}

fn current_native_search_keys(conn: &rusqlite::Connection) -> Result<HashSet<NativeSearchKey>> {
    let mut stmt = conn.prepare(
        "SELECT source_profile, name, version, package_release, architecture
         FROM native_package_publications
         WHERE status = 'public'",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(NativeSearchKey {
            source_profile: row.get(0)?,
            name: row.get(1)?,
            version: row.get(2)?,
            package_release: row.get(3)?,
            architecture: row.get(4)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<HashSet<_>>>()?)
}

fn require_catalog_profile(package: &CatalogPackageRecordV1, source_profile: &str) -> Result<()> {
    if package.source_profile != source_profile {
        anyhow::bail!(
            "immutable catalog for '{}' contains package '{}' from profile '{}'",
            source_profile,
            package.name,
            package.source_profile
        );
    }
    Ok(())
}

fn catalog_requirement_terms(
    groups: &[conary_core::repository::catalog::CatalogRequirementGroupV1],
) -> Option<String> {
    let terms = groups
        .iter()
        .flat_map(|group| &group.atoms)
        .flat_map(|atom| {
            std::iter::once(atom.capability.as_str()).chain(atom.version_constraint.as_deref())
        })
        .collect::<Vec<_>>()
        .join(" ");
    (!terms.is_empty()).then_some(terms)
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
    use crate::server::catalog_authority::test_support::{ActiveCatalogFixture, package};
    use crate::server::native_publish::test_support::seed_native_publication;
    use conary_core::db::models::{CONVERSION_VERSION, ConvertedPackage};
    use tempfile::TempDir;

    fn create_test_engine() -> (TempDir, SearchEngine) {
        let dir = TempDir::new().unwrap();
        let engine = SearchEngine::new(dir.path()).unwrap();
        (dir, engine)
    }

    fn insert_stale_conversion(
        conn: &rusqlite::Connection,
        source_profile: &str,
        profile_revision_sha256: &str,
        package: &str,
        version: &str,
    ) {
        let transport = crate::server::conversion::test_support::test_transport(&[format!(
            "sha256:{package}-{version}-chunk"
        )]);
        let mut converted = ConvertedPackage::new_repository(
            source_profile.to_string(),
            profile_revision_sha256.to_string(),
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
        let fixture = ActiveCatalogFixture::new();
        fixture.activate("fedora-44", 1, Vec::new());
        let conn = fixture.connection();
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
        fixture.activate_universe(1);
        let universe = PublicUniverseSnapshot::load(fixture.db_path())
            .unwrap()
            .unwrap();

        engine
            .rebuild_from_universe(fixture.db_path(), fixture.authority(), &universe)
            .unwrap();
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
        let fixture = ActiveCatalogFixture::new();
        let profile_revision = fixture.activate(
            "fedora-44",
            9,
            vec![package(
                "fedora-44",
                "gtk3",
                "3.24.0",
                "1",
                Some("x86_64"),
                1024,
                "gtk3-source",
            )],
        );
        let conn = fixture.connection();
        insert_stale_conversion(&conn, "fedora-44", &profile_revision, "gtk3", "3.24.0");
        drop(conn);
        fixture.activate_universe(1);
        let universe = PublicUniverseSnapshot::load(fixture.db_path())
            .unwrap()
            .unwrap();

        let (_dir, engine) = create_test_engine();
        engine
            .rebuild_from_universe(fixture.db_path(), fixture.authority(), &universe)
            .unwrap();

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
