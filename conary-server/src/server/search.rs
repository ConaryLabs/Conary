// conary-server/src/server/search.rs
//! Full-text search engine for the Remi package index
//!
//! Uses Tantivy to provide fast, typo-tolerant package search with faceted
//! filtering by distribution. Supports both full-text search and prefix-based
//! autocomplete suggestions.

use anyhow::{Context, Result};
use serde::Serialize;
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
    pub distro: String,
    pub description: Option<String>,
    pub dependencies: Option<String>,
    pub size: u64,
    pub converted: bool,
}

/// Search result returned from queries
#[derive(Debug, Clone, Serialize)]
pub struct SearchResult {
    pub name: String,
    pub version: String,
    pub distro: String,
    pub description: Option<String>,
    pub size: u64,
    pub converted: bool,
    pub score: f32,
}

/// Full-text search engine backed by Tantivy
pub struct SearchEngine {
    index: Index,
    reader: IndexReader,
    name_field: Field,
    name_exact_field: Field,
    /// Composite key "name\0distro" for accurate delete-before-update
    name_distro_field: Field,
    version_field: Field,
    distro_field: Field,
    description_field: Field,
    dependencies_field: Field,
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
        let distro_field = schema.get_field("distro").unwrap();
        let description_field = schema.get_field("description").unwrap();
        let dependencies_field = schema.get_field("dependencies").unwrap();
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
                        std::fs::remove_file(entry.path()).ok();
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
            distro_field,
            description_field,
            dependencies_field,
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

        // Name+distro composite key for delete-before-update (not stored, not searchable)
        schema_builder.add_text_field("name_distro", STRING);

        // Version: stored but not tokenized for search
        schema_builder.add_text_field("version", STRING | STORED);

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

        // Dependencies: searchable but not individually stored
        let deps_indexing = TextFieldIndexing::default()
            .set_tokenizer("default")
            .set_index_option(schema::IndexRecordOption::WithFreqs);
        let deps_options = TextOptions::default().set_indexing_options(deps_indexing);
        schema_builder.add_text_field("dependencies", deps_options);

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
        // Delete existing document with same name+distro (composite key)
        let composite_key = format!("{}\0{}", pkg.name, pkg.distro);
        let delete_term = Term::from_field_text(self.name_distro_field, &composite_key);
        writer.delete_term(delete_term);

        let distro_facet = Facet::from(&format!("/{}", pkg.distro));
        let converted_val: u64 = if pkg.converted { 1 } else { 0 };

        let mut doc = doc!(
            self.name_field => pkg.name.clone(),
            self.name_exact_field => pkg.name.clone(),
            self.name_distro_field => composite_key,
            self.version_field => pkg.version.clone(),
            self.distro_field => distro_facet,
            self.size_field => pkg.size,
            self.converted_field => converted_val,
        );

        if let Some(ref desc) = pkg.description {
            doc.add_text(self.description_field, desc);
        }

        if let Some(ref deps) = pkg.dependencies {
            doc.add_text(self.dependencies_field, deps);
        }

        writer.add_document(doc)?;
        Ok(())
    }

    /// Rebuild the entire search index from the database
    pub fn rebuild_from_db(&self, db_path: &Path) -> Result<usize> {
        let conn = conary_core::db::open(db_path)?;

        let mut writer = self
            .index
            .writer(50_000_000) // 50MB heap for bulk indexing
            .context("Failed to create index writer for rebuild")?;

        // Clear existing index
        writer.delete_all_documents()?;

        // Query latest version of each package per distro, joined with conversion status.
        // Uses a subquery to find the most recently synced row per (name, repo).
        // COALESCE(r.default_strategy_distro, r.name) matches the distro slug
        // stored in converted_packages (which may differ from the repo name).
        let mut stmt = conn.prepare(
            "SELECT rp.name, rp.version,
                    COALESCE(r.default_strategy_distro, r.name) as distro,
                    rp.description, rp.dependencies, rp.size,
                    CASE WHEN cp.id IS NOT NULL THEN 1 ELSE 0 END as is_converted
             FROM repository_packages rp
             JOIN repositories r ON rp.repository_id = r.id
             LEFT JOIN converted_packages cp
                 ON cp.package_name = rp.name
                    AND cp.distro = COALESCE(r.default_strategy_distro, r.name)
             WHERE r.enabled = 1
               AND rp.id = (
                   SELECT rp2.id FROM repository_packages rp2
                   WHERE rp2.repository_id = rp.repository_id AND rp2.name = rp.name
                   ORDER BY rp2.synced_at DESC LIMIT 1
               )
             ORDER BY rp.name",
        )?;

        let mut count = 0;
        let rows = stmt.query_map([], |row| {
            Ok(PackageSearchDoc {
                name: row.get(0)?,
                version: row.get(1)?,
                distro: row.get(2)?,
                description: row.get(3)?,
                dependencies: row.get(4)?,
                size: row.get::<_, i64>(5).map(|s| s as u64)?,
                converted: row.get::<_, i64>(6).map(|c| c != 0)?,
            })
        })?;

        for row in rows {
            let pkg = row.context("Failed to read package row")?;
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
            let facet = Facet::from(&format!("/{}", distro_name));
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
            .search(&final_query, &TopDocs::with_limit(limit))
            .context("Search failed")?;

        let mut results = Vec::with_capacity(top_docs.len());
        for (score, doc_address) in top_docs {
            let doc: tantivy::TantivyDocument = searcher.doc(doc_address)?;

            let name = doc
                .get_first(self.name_exact_field)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            let version = doc
                .get_first(self.version_field)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            let distro_val = doc
                .get_first(self.distro_field)
                .and_then(|v| v.as_facet())
                .map(|path| {
                    path.strip_prefix('/').unwrap_or(path).to_string()
                })
                .unwrap_or_default();

            let description = doc
                .get_first(self.description_field)
                .and_then(|v| v.as_str())
                .map(String::from);

            let size = doc
                .get_first(self.size_field)
                .and_then(|v| v.as_u64())
                .unwrap_or(0);

            let converted = doc
                .get_first(self.converted_field)
                .and_then(|v| v.as_u64())
                .unwrap_or(0)
                != 0;

            results.push(SearchResult {
                name,
                version,
                distro: distro_val,
                description,
                size,
                converted,
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
            .search(&regex_query, &TopDocs::with_limit(limit))
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
    use tempfile::TempDir;

    fn create_test_engine() -> (TempDir, SearchEngine) {
        let dir = TempDir::new().unwrap();
        let engine = SearchEngine::new(dir.path()).unwrap();
        (dir, engine)
    }

    #[test]
    fn test_index_and_search() {
        let (_dir, engine) = create_test_engine();

        let pkg = PackageSearchDoc {
            name: "nginx".to_string(),
            version: "1.24.0".to_string(),
            distro: "fedora".to_string(),
            description: Some("High performance HTTP server and reverse proxy".to_string()),
            dependencies: Some("openssl pcre2 zlib".to_string()),
            size: 1_200_000,
            converted: true,
        };
        engine.index_package(&pkg).unwrap();

        let pkg2 = PackageSearchDoc {
            name: "curl".to_string(),
            version: "8.5.0".to_string(),
            distro: "fedora".to_string(),
            description: Some("Command line tool for transferring data".to_string()),
            dependencies: Some("openssl nghttp2 zlib".to_string()),
            size: 500_000,
            converted: false,
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
    fn test_suggest() {
        let (_dir, engine) = create_test_engine();

        for name in &["nginx", "nginx-module-njs", "nmap", "nodejs", "nano"] {
            let pkg = PackageSearchDoc {
                name: (*name).to_string(),
                version: "1.0.0".to_string(),
                distro: "fedora".to_string(),
                description: None,
                dependencies: None,
                size: 0,
                converted: false,
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
            distro: "arch".to_string(),
            description: Some("Vi Improved".to_string()),
            dependencies: None,
            size: 2_000_000,
            converted: false,
        };
        engine.index_package(&pkg).unwrap();

        // Update with new version
        let pkg_updated = PackageSearchDoc {
            name: "vim".to_string(),
            version: "9.1".to_string(),
            distro: "arch".to_string(),
            description: Some("Vi Improved - text editor".to_string()),
            dependencies: None,
            size: 2_100_000,
            converted: true,
        };
        engine.index_package(&pkg_updated).unwrap();

        let results = engine.search("vim", None, 10).unwrap();
        // Should have the updated version (old one deleted by name_exact match)
        assert!(!results.is_empty());
        assert!(results[0].converted);
    }

    #[test]
    fn test_regex_escape() {
        assert_eq!(regex_escape("hello"), "hello");
        assert_eq!(regex_escape("lib++"), "lib\\+\\+");
        assert_eq!(regex_escape("foo.bar"), "foo\\.bar");
        assert_eq!(regex_escape("test[0]"), "test\\[0\\]");
    }
}
