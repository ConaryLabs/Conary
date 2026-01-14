// src/db/models/provide_entry.rs

//! ProvideEntry model - capabilities that packages offer
//!
//! This tracks what each package "provides" - the capabilities it offers
//! that can satisfy dependencies. This enables self-contained dependency
//! resolution without querying the host package manager.

use crate::error::Result;
use rusqlite::{Connection, OptionalExtension, Row, params};

/// A capability that a package provides
#[derive(Debug, Clone)]
pub struct ProvideEntry {
    pub id: Option<i64>,
    pub trove_id: i64,
    /// The capability name (package name, virtual provide, library, or file path)
    pub capability: String,
    /// Optional version of this capability
    pub version: Option<String>,
    /// The kind of capability (package, python, soname, pkgconfig, etc.)
    pub kind: String,
}

impl ProvideEntry {
    /// Create a new ProvideEntry
    pub fn new(trove_id: i64, capability: String, version: Option<String>) -> Self {
        Self {
            id: None,
            trove_id,
            capability,
            version,
            kind: "package".to_string(),
        }
    }

    /// Create a new typed ProvideEntry
    pub fn new_typed(trove_id: i64, kind: &str, capability: String, version: Option<String>) -> Self {
        Self {
            id: None,
            trove_id,
            capability,
            version,
            kind: kind.to_string(),
        }
    }

    /// Insert this provide into the database
    pub fn insert(&mut self, conn: &Connection) -> Result<i64> {
        conn.execute(
            "INSERT INTO provides (trove_id, capability, version, kind)
             VALUES (?1, ?2, ?3, ?4)",
            params![&self.trove_id, &self.capability, &self.version, &self.kind],
        )?;

        let id = conn.last_insert_rowid();
        self.id = Some(id);
        Ok(id)
    }

    /// Insert or ignore if already exists (for idempotent imports)
    pub fn insert_or_ignore(&mut self, conn: &Connection) -> Result<i64> {
        conn.execute(
            "INSERT OR IGNORE INTO provides (trove_id, capability, version, kind)
             VALUES (?1, ?2, ?3, ?4)",
            params![&self.trove_id, &self.capability, &self.version, &self.kind],
        )?;

        // Get the ID (either new or existing)
        let id = conn.query_row(
            "SELECT id FROM provides WHERE trove_id = ?1 AND capability = ?2",
            params![&self.trove_id, &self.capability],
            |row| row.get(0),
        )?;

        self.id = Some(id);
        Ok(id)
    }

    /// Find a provide by capability name
    ///
    /// Returns the first trove that provides this capability
    pub fn find_by_capability(conn: &Connection, capability: &str) -> Result<Option<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, trove_id, capability, version, kind
             FROM provides WHERE capability = ?1 LIMIT 1",
        )?;

        let provide = stmt.query_row([capability], Self::from_row).optional()?;
        Ok(provide)
    }

    /// Find a provide by kind and capability name
    pub fn find_typed(conn: &Connection, kind: &str, capability: &str) -> Result<Option<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, trove_id, capability, version, kind
             FROM provides WHERE kind = ?1 AND capability = ?2 LIMIT 1",
        )?;

        let provide = stmt.query_row([kind, capability], Self::from_row).optional()?;
        Ok(provide)
    }

    /// Find all troves that provide a capability
    pub fn find_all_by_capability(conn: &Connection, capability: &str) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, trove_id, capability, version, kind
             FROM provides WHERE capability = ?1",
        )?;

        let provides = stmt
            .query_map([capability], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(provides)
    }

    /// Find all typed provides (by kind and capability)
    pub fn find_all_typed(conn: &Connection, kind: &str, capability: &str) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, trove_id, capability, version, kind
             FROM provides WHERE kind = ?1 AND capability = ?2",
        )?;

        let provides = stmt
            .query_map([kind, capability], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(provides)
    }

    /// Find all provides for a trove
    pub fn find_by_trove(conn: &Connection, trove_id: i64) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, trove_id, capability, version, kind
             FROM provides WHERE trove_id = ?1",
        )?;

        let provides = stmt
            .query_map([trove_id], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(provides)
    }

    /// Find all provides of a specific kind for a trove
    pub fn find_by_trove_and_kind(conn: &Connection, trove_id: i64, kind: &str) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, trove_id, capability, version, kind
             FROM provides WHERE trove_id = ?1 AND kind = ?2",
        )?;

        let provides = stmt
            .query_map(params![trove_id, kind], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(provides)
    }

    /// Check if a capability is provided by any installed package
    pub fn is_capability_satisfied(conn: &Connection, capability: &str) -> Result<bool> {
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM provides WHERE capability = ?1",
            [capability],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    /// Find provider with version constraint check
    ///
    /// Returns the trove name and version if the capability is satisfied
    pub fn find_satisfying_provider(
        conn: &Connection,
        capability: &str,
    ) -> Result<Option<(String, String)>> {
        // First try exact match
        let result = conn
            .query_row(
                "SELECT t.name, t.version
                 FROM provides p
                 JOIN troves t ON p.trove_id = t.id
                 WHERE p.capability = ?1
                 LIMIT 1",
                [capability],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?;

        if result.is_some() {
            return Ok(result);
        }

        // Try prefix match for capabilities with version suffixes
        // e.g., "perl(Text::CharWidth)" should match "perl(Text::CharWidth) = 0.04"
        let prefix_pattern = format!("{} %", capability);
        let result = conn
            .query_row(
                "SELECT t.name, t.version
                 FROM provides p
                 JOIN troves t ON p.trove_id = t.id
                 WHERE p.capability LIKE ?1
                 LIMIT 1",
                [&prefix_pattern],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?;

        if result.is_some() {
            return Ok(result);
        }

        // Try case-insensitive prefix match for cross-distro compatibility
        // e.g., perl(Text::Charwidth) should match perl(Text::CharWidth) = 0.04
        let lower_cap = capability.to_lowercase();
        let result = conn
            .query_row(
                "SELECT t.name, t.version
                 FROM provides p
                 JOIN troves t ON p.trove_id = t.id
                 WHERE LOWER(p.capability) LIKE ?1 || ' %' OR LOWER(p.capability) = ?1
                 LIMIT 1",
                [&lower_cap],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?;

        Ok(result)
    }

    /// Search for capabilities matching a pattern (using SQL LIKE)
    pub fn search_capability(conn: &Connection, pattern: &str) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, trove_id, capability, version, kind
             FROM provides WHERE capability LIKE ?1",
        )?;

        let provides = stmt
            .query_map([pattern], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(provides)
    }

    /// Search for typed capabilities matching a kind and pattern
    pub fn search_typed(conn: &Connection, kind: &str, pattern: &str) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, trove_id, capability, version, kind
             FROM provides WHERE kind = ?1 AND capability LIKE ?2",
        )?;

        let provides = stmt
            .query_map([kind, pattern], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(provides)
    }

    /// Delete all provides for a trove
    pub fn delete_by_trove(conn: &Connection, trove_id: i64) -> Result<()> {
        conn.execute("DELETE FROM provides WHERE trove_id = ?1", [trove_id])?;
        Ok(())
    }

    /// Convert a database row to a ProvideEntry
    fn from_row(row: &Row) -> rusqlite::Result<Self> {
        Ok(Self {
            id: Some(row.get(0)?),
            trove_id: row.get(1)?,
            capability: row.get(2)?,
            version: row.get(3)?,
            kind: row.get::<_, Option<String>>(4)?.unwrap_or_else(|| "package".to_string()),
        })
    }

    /// Format this provide as a typed string (e.g., "python(requests)")
    pub fn to_typed_string(&self) -> String {
        if self.kind == "package" || self.kind.is_empty() {
            self.capability.clone()
        } else {
            format!("{}({})", self.kind, self.capability)
        }
    }

    /// Find a satisfying provider, trying common cross-distro variations
    ///
    /// This extends `find_satisfying_provider` by also trying common variations
    /// of the capability name for cross-distro compatibility.
    pub fn find_satisfying_provider_fuzzy(
        conn: &Connection,
        capability: &str,
    ) -> Result<Option<(String, String)>> {
        // First try exact match
        if let Some(result) = Self::find_satisfying_provider(conn, capability)? {
            return Ok(Some(result));
        }

        // Try cross-distro variations
        for variation in generate_capability_variations(capability) {
            if let Some(result) = Self::find_satisfying_provider(conn, &variation)? {
                return Ok(Some(result));
            }
        }

        Ok(None)
    }

    /// Check if a capability is satisfied (with fuzzy cross-distro matching)
    pub fn is_capability_satisfied_fuzzy(conn: &Connection, capability: &str) -> Result<bool> {
        // First try exact match
        if Self::is_capability_satisfied(conn, capability)? {
            return Ok(true);
        }

        // Try cross-distro variations
        for variation in generate_capability_variations(capability) {
            if Self::is_capability_satisfied(conn, &variation)? {
                return Ok(true);
            }
        }

        Ok(false)
    }

    /// Check if a name looks like a virtual provide (capability) rather than a package name
    ///
    /// Virtual provides have patterns like:
    /// - perl(Cwd) - Perl module
    /// - python3dist(setuptools) - Python package
    /// - config(package) - Configuration capability
    /// - pkgconfig(foo) - pkg-config module
    /// - lib*.so.* - Shared library
    /// - /usr/bin/foo - File path
    pub fn is_virtual_provide(name: &str) -> bool {
        name.contains('(')  // perl(Foo), python3dist(bar), etc.
            || name.starts_with("lib") && name.contains(".so")  // libfoo.so.1
            || name.starts_with('/')  // File path dependencies
    }
}

/// Generate common variations of a capability name for cross-distro matching
///
/// For example:
/// - perl(Text::CharWidth) might also be: perl-Text-CharWidth
/// - libc.so.6 might also be: glibc, libc6
pub fn generate_capability_variations(capability: &str) -> Vec<String> {
    let mut variations = Vec::new();

    // Perl module variations: perl(Foo::Bar) <-> perl-Foo-Bar
    if capability.starts_with("perl(") && capability.ends_with(')') {
        let module = &capability[5..capability.len()-1];
        // perl(Foo::Bar) -> perl-Foo-Bar
        variations.push(format!("perl-{}", module.replace("::", "-")));
        // Also try lowercase
        variations.push(format!("perl-{}", module.replace("::", "-").to_lowercase()));
    } else if let Some(rest) = capability.strip_prefix("perl-") {
        // perl-Foo-Bar -> perl(Foo::Bar)
        let module = rest.replace('-', "::");
        variations.push(format!("perl({})", module));
    }

    // Python module variations
    if let Some(module) = capability.strip_prefix("python3-") {
        variations.push(format!("python3dist({})", module));
        variations.push(format!("python({})", module));
    } else if capability.starts_with("python3dist(") && capability.ends_with(')') {
        let module = &capability[12..capability.len()-1];
        variations.push(format!("python3-{}", module));
    }

    // Library variations
    if capability.ends_with(".so") || capability.contains(".so.") {
        // libc.so.6 -> glibc, libc6
        if capability.starts_with("libc.so") {
            variations.push("glibc".to_string());
            variations.push("libc6".to_string());
        }
        // Extract library name: libfoo.so.1 -> libfoo, foo
        if let Some(base) = capability.split(".so").next() {
            variations.push(base.to_string());
            if let Some(name) = base.strip_prefix("lib") {
                variations.push(name.to_string());
            }
        }
    }

    // Debian :any suffix (architecture-independent)
    // perl:any -> perl
    if let Some(base) = capability.strip_suffix(":any") {
        variations.push(base.to_string());
    }

    // Debian perl library naming: libfoo-bar-perl -> perl-Foo-Bar, perl(Foo::Bar)
    if capability.starts_with("lib") && capability.ends_with("-perl") {
        // libtext-charwidth-perl -> text-charwidth -> Text::CharWidth
        let middle = &capability[3..capability.len()-5]; // strip "lib" and "-perl"
        // Convert to title case with :: separators
        let module_name: String = middle
            .split('-')
            .map(|part| {
                let mut chars = part.chars();
                match chars.next() {
                    Some(first) => first.to_uppercase().chain(chars).collect(),
                    None => String::new(),
                }
            })
            .collect::<Vec<_>>()
            .join("::");
        variations.push(format!("perl({})", module_name));
        variations.push(format!("perl-{}", middle.split('-').map(|p| {
            let mut c = p.chars();
            match c.next() {
                Some(f) => f.to_uppercase().chain(c).collect(),
                None => String::new(),
            }
        }).collect::<Vec<_>>().join("-")));
    }

    // Package name might be used directly
    // Try stripping version suffixes: foo-1.0 -> foo
    if let Some(pos) = capability.rfind('-') {
        let potential_name = &capability[..pos];
        if !potential_name.is_empty() && capability[pos+1..].chars().next().is_some_and(|c| c.is_ascii_digit()) {
            variations.push(potential_name.to_string());
        }
    }

    variations
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn setup_test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "
            CREATE TABLE troves (
                id INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                version TEXT NOT NULL
            );
            CREATE TABLE provides (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                trove_id INTEGER NOT NULL REFERENCES troves(id),
                capability TEXT NOT NULL,
                version TEXT,
                kind TEXT DEFAULT 'package',
                UNIQUE(trove_id, capability)
            );
            CREATE INDEX idx_provides_capability ON provides(capability);
            CREATE INDEX idx_provides_kind ON provides(kind);

            INSERT INTO troves (id, name, version) VALUES (1, 'perl-Text-CharWidth', '0.04');
            ",
        )
        .unwrap();
        conn
    }

    #[test]
    fn test_insert_and_find() {
        let conn = setup_test_db();

        let mut provide = ProvideEntry::new_typed(1, "perl", "Text::CharWidth".to_string(), Some("0.04".to_string()));
        provide.insert(&conn).unwrap();

        let found = ProvideEntry::find_typed(&conn, "perl", "Text::CharWidth").unwrap();
        assert!(found.is_some());
        let found = found.unwrap();
        assert_eq!(found.capability, "Text::CharWidth");
        assert_eq!(found.kind, "perl");
        assert_eq!(found.version, Some("0.04".to_string()));
    }

    #[test]
    fn test_is_capability_satisfied() {
        let conn = setup_test_db();

        let mut provide = ProvideEntry::new_typed(1, "soname", "libc.so.6".to_string(), None);
        provide.insert(&conn).unwrap();

        // Direct capability check still works
        assert!(ProvideEntry::is_capability_satisfied(&conn, "libc.so.6").unwrap());
        assert!(!ProvideEntry::is_capability_satisfied(&conn, "libfoo.so.1").unwrap());
    }

    #[test]
    fn test_search_capability() {
        let conn = setup_test_db();

        let mut p1 = ProvideEntry::new_typed(1, "perl", "Text::CharWidth".to_string(), None);
        let mut p2 = ProvideEntry::new_typed(1, "perl", "Text::Wrap".to_string(), None);
        p1.insert(&conn).unwrap();
        p2.insert(&conn).unwrap();

        let results = ProvideEntry::search_typed(&conn, "perl", "Text::%").unwrap();
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_to_typed_string() {
        let provide = ProvideEntry::new_typed(1, "python", "requests".to_string(), Some("2.28".to_string()));
        assert_eq!(provide.to_typed_string(), "python(requests)");

        let provide = ProvideEntry::new(1, "nginx".to_string(), Some("1.24".to_string()));
        assert_eq!(provide.to_typed_string(), "nginx");
    }
}
