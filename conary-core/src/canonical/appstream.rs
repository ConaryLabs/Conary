// conary-core/src/canonical/appstream.rs

//! AppStream catalog parser for canonical package identity.
//!
//! Parses both AppStream XML (used by Fedora, Arch, etc.) and DEP-11 YAML
//! (used by Debian/Ubuntu) catalog formats, extracting component metadata
//! that maps AppStream IDs to distro package names.

use crate::db::models::{CanonicalPackage, PackageImplementation};
use crate::error::{Error, Result};
use quick_xml::events::Event;
use quick_xml::reader::Reader;
use rusqlite::{params, Connection};
use serde::Deserialize;

/// Capabilities this component provides (cross-distro).
/// Parsed from AppStream `<provides>` element.
#[derive(Debug, Clone, Default)]
pub struct AppStreamProvides {
    /// Shared library sonames (from `<library>`)
    pub libraries: Vec<String>,
    /// Executable binaries (from `<binary>`)
    pub binaries: Vec<String>,
    /// Python 3 modules (from `<python3>`)
    pub python3: Vec<String>,
    /// D-Bus services (from `<dbus>`)
    pub dbus: Vec<String>,
}

impl AppStreamProvides {
    /// Returns true if no capabilities are recorded.
    pub fn is_empty(&self) -> bool {
        self.libraries.is_empty()
            && self.binaries.is_empty()
            && self.python3.is_empty()
            && self.dbus.is_empty()
    }

    /// Total number of capabilities across all types.
    pub fn total_count(&self) -> usize {
        self.libraries.len() + self.binaries.len() + self.python3.len() + self.dbus.len()
    }
}

/// A parsed AppStream component with its identity and metadata.
#[derive(Debug, Clone)]
pub struct AppStreamComponent {
    /// Reverse-DNS identifier (e.g. `org.mozilla.Firefox`)
    pub id: String,
    /// Distro package name (e.g. `firefox`)
    pub pkgname: String,
    /// Human-readable display name
    pub name: String,
    /// Optional short description
    pub summary: Option<String>,
    /// Capabilities this component provides
    pub provides: AppStreamProvides,
}

impl AppStreamComponent {
    /// Returns the canonical name for this component, which is the distro
    /// package name (`pkgname`).
    pub fn canonical_name(&self) -> &str {
        &self.pkgname
    }
}

/// Parse an AppStream XML catalog into a list of components.
///
/// Expects the standard AppStream collection format:
/// ```xml
/// <components version="1.0">
///   <component type="desktop-application">
///     <id>org.mozilla.Firefox</id>
///     <pkgname>firefox</pkgname>
///     <name>Firefox</name>
///     <summary>Web Browser</summary>
///   </component>
/// </components>
/// ```
///
/// Components missing `<id>` or `<pkgname>` are silently skipped.
/// This is a convenience wrapper around [`parse_appstream_xml_enriched`]
/// that discards the origin attribute.
pub fn parse_appstream_xml(xml: &str) -> Result<Vec<AppStreamComponent>> {
    let (_origin, components) = parse_appstream_xml_enriched(xml)?;
    Ok(components)
}

/// Parse an AppStream XML catalog, capturing the `origin` attribute and
/// `<provides>` children for each component.
///
/// Returns `(origin, components)` where `origin` is the value of the
/// `origin` attribute on the root `<components>` element (if present).
///
/// Inside each `<component>`, the `<provides>` block is parsed for:
/// - `<library>` -- shared library sonames
/// - `<binary>` -- executable binaries
/// - `<python3>` -- Python 3 modules
/// - `<dbus>` -- D-Bus service names
pub fn parse_appstream_xml_enriched(
    xml: &str,
) -> Result<(Option<String>, Vec<AppStreamComponent>)> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text_end = true;

    let mut origin: Option<String> = None;
    let mut components = Vec::new();
    let mut buf = Vec::new();

    // State tracking for the current component being parsed
    let mut in_component = false;
    let mut in_provides = false;
    let mut current_tag: Option<String> = None;
    let mut id = None;
    let mut pkgname = None;
    let mut name = None;
    let mut summary = None;
    let mut provides = AppStreamProvides::default();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Eof) => break,
            Err(e) => {
                return Err(Error::ParseError(format!(
                    "AppStream XML parse error at position {}: {e}",
                    reader.buffer_position()
                )));
            }
            Ok(Event::Start(e)) => {
                let tag = e.name();
                match tag.as_ref() {
                    b"components" => {
                        // Extract the origin attribute from the root element
                        for attr in e.attributes().flatten() {
                            if attr.key.as_ref() == b"origin" {
                                origin = Some(
                                    String::from_utf8_lossy(attr.value.as_ref()).into_owned(),
                                );
                            }
                        }
                    }
                    b"component" => {
                        in_component = true;
                        id = None;
                        pkgname = None;
                        name = None;
                        summary = None;
                        provides = AppStreamProvides::default();
                    }
                    b"provides" if in_component => {
                        in_provides = true;
                    }
                    b"library" | b"binary" | b"python3" | b"dbus"
                        if in_component && in_provides =>
                    {
                        current_tag = Some(String::from_utf8_lossy(tag.as_ref()).into_owned());
                    }
                    b"id" | b"pkgname" | b"name" | b"summary"
                        if in_component && !in_provides =>
                    {
                        current_tag = Some(String::from_utf8_lossy(tag.as_ref()).into_owned());
                    }
                    _ => {}
                }
            }
            Ok(Event::Text(e)) => {
                if in_component
                    && let Some(ref tag) = current_tag
                {
                    let text = e
                        .decode()
                        .map_err(|err| {
                            Error::ParseError(format!(
                                "AppStream XML text decode error: {err}"
                            ))
                        })?
                        .into_owned();
                    if in_provides {
                        match tag.as_str() {
                            "library" => provides.libraries.push(text),
                            "binary" => provides.binaries.push(text),
                            "python3" => provides.python3.push(text),
                            "dbus" => provides.dbus.push(text),
                            _ => {}
                        }
                    } else {
                        match tag.as_str() {
                            "id" => id = Some(text),
                            "pkgname" => pkgname = Some(text),
                            "name" => name = Some(text),
                            "summary" => summary = Some(text),
                            _ => {}
                        }
                    }
                }
            }
            Ok(Event::End(e)) => match e.name().as_ref() {
                b"component" => {
                    if let (Some(id_val), Some(pkg_val)) = (id.take(), pkgname.take()) {
                        components.push(AppStreamComponent {
                            id: id_val,
                            pkgname: pkg_val,
                            name: name.take().unwrap_or_default(),
                            summary: summary.take(),
                            provides: std::mem::take(&mut provides),
                        });
                    }
                    in_component = false;
                    in_provides = false;
                    current_tag = None;
                }
                b"provides" => {
                    in_provides = false;
                    current_tag = None;
                }
                b"library" | b"binary" | b"python3" | b"dbus" if in_provides => {
                    current_tag = None;
                }
                b"id" | b"pkgname" | b"name" | b"summary" if !in_provides => {
                    current_tag = None;
                }
                _ => {}
            },
            _ => {}
        }
        buf.clear();
    }

    Ok((origin, components))
}

/// Persist cross-distro provides from AppStream into the `appstream_provides` table.
///
/// Inserts one row per capability, using `INSERT OR IGNORE` to skip duplicates.
/// Returns the number of rows inserted (including ignored duplicates in the count).
pub fn persist_appstream_provides(
    conn: &Connection,
    canonical_id: i64,
    provides: &AppStreamProvides,
) -> Result<usize> {
    let mut count = 0;
    let mut stmt = conn.prepare(
        "INSERT OR IGNORE INTO appstream_provides (canonical_id, provide_type, capability)
         VALUES (?1, ?2, ?3)",
    )?;
    for lib in &provides.libraries {
        stmt.execute(params![canonical_id, "library", lib])?;
        count += 1;
    }
    for bin in &provides.binaries {
        stmt.execute(params![canonical_id, "binary", bin])?;
        count += 1;
    }
    for py in &provides.python3 {
        stmt.execute(params![canonical_id, "python3", py])?;
        count += 1;
    }
    for dbus_svc in &provides.dbus {
        stmt.execute(params![canonical_id, "dbus", dbus_svc])?;
        count += 1;
    }
    Ok(count)
}

/// Internal serde model for DEP-11 YAML documents.
#[derive(Deserialize)]
struct Dep11Doc {
    #[serde(rename = "ID")]
    id: Option<String>,
    #[serde(rename = "Package")]
    package: Option<String>,
    #[serde(rename = "Name")]
    name: Option<Dep11Localized>,
    #[serde(rename = "Summary")]
    summary: Option<Dep11Localized>,
}

/// Localized string map (we only extract the `C` / default locale).
#[derive(Deserialize)]
struct Dep11Localized {
    #[serde(rename = "C")]
    c: Option<String>,
}

/// Parse a DEP-11 YAML catalog (multi-document) into a list of components.
///
/// DEP-11 is the YAML-based AppStream format used by Debian and Ubuntu.
/// The first YAML document is a header (`File: DEP-11`) and is skipped.
/// Each subsequent document represents one component:
///
/// ```yaml
/// ---
/// Type: desktop-application
/// ID: org.mozilla.Firefox
/// Package: firefox
/// Name:
///   C: Firefox
/// Summary:
///   C: Web Browser
/// ```
///
/// Components missing `ID` or `Package` are silently skipped.
pub fn parse_appstream_yaml(yaml: &str) -> Result<Vec<AppStreamComponent>> {
    let mut components = Vec::new();
    let mut first = true;

    for document in serde_yaml::Deserializer::from_str(yaml) {
        // Skip the header document
        if first {
            first = false;
            // Consume the header so the deserializer advances
            let _header: serde_yaml::Value = serde::Deserialize::deserialize(document)
                .map_err(|e| Error::ParseError(format!("DEP-11 YAML header parse error: {e}")))?;
            continue;
        }

        let doc: Dep11Doc = serde::Deserialize::deserialize(document)
            .map_err(|e| Error::ParseError(format!("DEP-11 YAML parse error: {e}")))?;

        if let (Some(id), Some(package)) = (doc.id, doc.package) {
            let name_str = doc.name.and_then(|n| n.c).unwrap_or_default();
            let summary_str = doc.summary.and_then(|s| s.c);

            components.push(AppStreamComponent {
                id,
                pkgname: package,
                name: name_str,
                summary: summary_str,
                provides: AppStreamProvides::default(),
            });
        }
    }

    Ok(components)
}

/// Ingest parsed AppStream components into the canonical package database.
///
/// For each component, creates or finds a `CanonicalPackage` (using `pkgname`
/// as the canonical name and setting the `appstream_id`), then creates a
/// `PackageImplementation` linking it to the specified distro.
///
/// Returns the number of components successfully ingested.
pub fn ingest_appstream(
    conn: &rusqlite::Connection,
    components: &[AppStreamComponent],
    distro: &str,
) -> Result<usize> {
    // Wrap in a transaction for atomicity and performance (avoids per-statement autocommit).
    // On error, the transaction auto-rolls-back when dropped.
    let tx = conn.unchecked_transaction()?;
    let count = ingest_appstream_inner(&tx, components, distro)?;
    tx.commit()?;
    Ok(count)
}

fn ingest_appstream_inner(
    conn: &rusqlite::Connection,
    components: &[AppStreamComponent],
    distro: &str,
) -> Result<usize> {
    let mut count = 0;

    for comp in components {
        let mut canonical = CanonicalPackage::new(comp.pkgname.clone(), "package".to_string());
        canonical.appstream_id = Some(comp.id.clone());
        canonical.description = comp.summary.clone();

        let canonical_id = canonical.insert_or_ignore(conn)?;

        if let Some(can_id) = canonical_id {
            // Update appstream_id if the package already existed without one
            conn.execute(
                "UPDATE canonical_packages SET appstream_id = ?1 WHERE id = ?2 AND appstream_id IS NULL",
                rusqlite::params![&comp.id, can_id],
            )?;

            let mut impl_entry = PackageImplementation::new(
                can_id,
                distro.to_string(),
                comp.pkgname.clone(),
                "appstream".to_string(),
            );
            impl_entry.insert_or_ignore(conn)?;
            count += 1;
        }
    }

    Ok(count)
}

/// Write parsed AppStream components to the appstream_cache table.
/// `pkgname` is always present (components without it are dropped at parse time).
pub fn cache_components_to_db(
    conn: &rusqlite::Connection,
    components: &[AppStreamComponent],
    distro: &str,
) -> Result<usize> {
    let tx = conn.unchecked_transaction()?;
    let now = chrono::Utc::now().to_rfc3339();
    let mut count = 0;

    for component in components {
        let entry = crate::db::models::AppstreamCacheEntry {
            appstream_id: component.id.clone(),
            pkgname: component.pkgname.clone(),
            display_name: Some(component.name.clone()),
            summary: component.summary.clone(),
            distro: distro.to_string(),
            fetched_at: now.clone(),
        };
        crate::db::models::AppstreamCacheEntry::insert_or_replace(&tx, &entry)?;
        count += 1;
    }

    tx.commit()?;
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_appstream_xml() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<components version="1.0">
  <component type="desktop-application">
    <id>org.mozilla.Firefox</id>
    <pkgname>firefox</pkgname>
    <name>Firefox</name>
    <summary>Web Browser</summary>
  </component>
  <component type="desktop-application">
    <id>org.gnome.Nautilus</id>
    <pkgname>nautilus</pkgname>
    <name>Files</name>
    <summary>File manager</summary>
  </component>
</components>"#;
        let components = parse_appstream_xml(xml).unwrap();
        assert_eq!(components.len(), 2);
        assert_eq!(components[0].id, "org.mozilla.Firefox");
        assert_eq!(components[0].pkgname, "firefox");
        assert_eq!(components[1].id, "org.gnome.Nautilus");
    }

    #[test]
    fn test_component_to_canonical_name() {
        let comp = AppStreamComponent {
            id: "org.mozilla.Firefox".to_string(),
            pkgname: "firefox".to_string(),
            name: "Firefox".to_string(),
            summary: Some("Web Browser".to_string()),
            provides: AppStreamProvides::default(),
        };
        assert_eq!(comp.canonical_name(), "firefox");
    }

    #[test]
    fn test_parse_appstream_yaml() {
        let yaml = "---\nFile: DEP-11\nVersion: '1.0'\n---\nType: desktop-application\nID: org.mozilla.Firefox\nPackage: firefox\nName:\n  C: Firefox\nSummary:\n  C: Web Browser\n---\nType: desktop-application\nID: org.gnome.Nautilus\nPackage: nautilus\nName:\n  C: Files\n";
        let components = parse_appstream_yaml(yaml).unwrap();
        assert_eq!(components.len(), 2);
    }

    #[test]
    fn test_xml_skips_components_without_pkgname() {
        let xml = r#"<components version="1.0">
  <component type="desktop-application">
    <id>org.example.NoPkg</id>
    <name>No Package</name>
  </component>
  <component type="desktop-application">
    <id>org.example.HasPkg</id>
    <pkgname>haspkg</pkgname>
    <name>Has Package</name>
  </component>
</components>"#;
        let components = parse_appstream_xml(xml).unwrap();
        assert_eq!(components.len(), 1);
        assert_eq!(components[0].pkgname, "haspkg");
    }

    #[test]
    fn test_yaml_skips_components_without_package() {
        let yaml = "---\nFile: DEP-11\nVersion: '1.0'\n---\nType: desktop-application\nID: org.example.NoPkg\nName:\n  C: No Package\n---\nType: desktop-application\nID: org.example.HasPkg\nPackage: haspkg\nName:\n  C: Has Package\n";
        let components = parse_appstream_yaml(yaml).unwrap();
        assert_eq!(components.len(), 1);
        assert_eq!(components[0].pkgname, "haspkg");
    }

    #[test]
    fn test_ingest_appstream() {
        use crate::db::schema;
        use tempfile::NamedTempFile;

        let temp_file = NamedTempFile::new().unwrap();
        let conn = rusqlite::Connection::open(temp_file.path()).unwrap();
        conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
        schema::migrate(&conn).unwrap();

        let components = vec![
            AppStreamComponent {
                id: "org.mozilla.Firefox".to_string(),
                pkgname: "firefox".to_string(),
                name: "Firefox".to_string(),
                summary: Some("Web Browser".to_string()),
                provides: AppStreamProvides::default(),
            },
            AppStreamComponent {
                id: "org.gnome.Nautilus".to_string(),
                pkgname: "nautilus".to_string(),
                name: "Files".to_string(),
                summary: None,
                provides: AppStreamProvides::default(),
            },
        ];

        let count = ingest_appstream(&conn, &components, "fedora").unwrap();
        assert_eq!(count, 2);

        // Verify canonical packages were created
        let pkg = CanonicalPackage::find_by_name(&conn, "firefox")
            .unwrap()
            .unwrap();
        assert_eq!(pkg.appstream_id, Some("org.mozilla.Firefox".to_string()));

        // Verify implementations were created
        let impls = PackageImplementation::find_by_canonical(&conn, pkg.id.unwrap()).unwrap();
        assert_eq!(impls.len(), 1);
        assert_eq!(impls[0].distro, "fedora");
        assert_eq!(impls[0].source, "appstream");

        // Ingesting again should not duplicate (insert_or_ignore)
        let count2 = ingest_appstream(&conn, &components, "fedora").unwrap();
        assert_eq!(count2, 2);
        let impls2 = PackageImplementation::find_by_canonical(&conn, pkg.id.unwrap()).unwrap();
        assert_eq!(impls2.len(), 1);
    }

    #[test]
    fn test_cache_appstream_components() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::schema::migrate(&conn).unwrap();

        let components = vec![AppStreamComponent {
            id: "org.mozilla.firefox".into(),
            pkgname: "firefox".into(),
            name: "Firefox".into(),
            summary: Some("Web Browser".into()),
            provides: AppStreamProvides::default(),
        }];

        let count = cache_components_to_db(&conn, &components, "fedora").unwrap();
        assert_eq!(count, 1);

        let entries = crate::db::models::AppstreamCacheEntry::find_all(&conn).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].pkgname, "firefox");
    }

    #[test]
    fn test_parse_appstream_xml_captures_provides() {
        let xml = r#"
    <components version="1.0" origin="fedora-41-main">
      <component type="generic">
        <id>org.openssl.openssl</id>
        <pkgname>openssl-libs</pkgname>
        <name>OpenSSL</name>
        <provides>
          <library>libssl.so.3</library>
          <library>libcrypto.so.3</library>
          <binary>openssl</binary>
          <python3>ssl</python3>
          <dbus type="system">org.freedesktop.secrets</dbus>
        </provides>
      </component>
    </components>"#;

        let (origin, components) = parse_appstream_xml_enriched(xml).unwrap();
        assert_eq!(origin.as_deref(), Some("fedora-41-main"));
        assert_eq!(components.len(), 1);

        let comp = &components[0];
        assert_eq!(comp.pkgname, "openssl-libs");
        assert_eq!(
            comp.provides.libraries,
            vec!["libssl.so.3", "libcrypto.so.3"]
        );
        assert_eq!(comp.provides.binaries, vec!["openssl"]);
        assert_eq!(comp.provides.python3, vec!["ssl"]);
        assert_eq!(comp.provides.dbus, vec!["org.freedesktop.secrets"]);
        assert_eq!(comp.provides.total_count(), 5);
    }

    #[test]
    fn test_parse_appstream_xml_no_provides() {
        let xml = r#"
    <components version="1.0">
      <component type="desktop-application">
        <id>org.mozilla.Firefox</id>
        <pkgname>firefox</pkgname>
        <name>Firefox</name>
        <summary>Web Browser</summary>
      </component>
    </components>"#;

        let (origin, components) = parse_appstream_xml_enriched(xml).unwrap();
        assert!(origin.is_none());
        assert_eq!(components.len(), 1);
        assert!(components[0].provides.is_empty());
    }

    #[test]
    fn test_existing_parse_appstream_xml_still_works() {
        let xml = r#"
    <components version="1.0">
      <component type="desktop-application">
        <id>org.mozilla.Firefox</id>
        <pkgname>firefox</pkgname>
        <name>Firefox</name>
        <summary>Web Browser</summary>
      </component>
    </components>"#;

        let components = parse_appstream_xml(xml).unwrap();
        assert_eq!(components.len(), 1);
        assert_eq!(components[0].pkgname, "firefox");
    }

    #[test]
    fn test_persist_appstream_provides() {
        use crate::db::testing::create_test_db;

        let (_temp, conn) = create_test_db();

        conn.execute(
            "INSERT INTO canonical_packages (name, kind) VALUES ('openssl', 'package')",
            [],
        )
        .unwrap();
        let canonical_id = conn.last_insert_rowid();

        let provides = AppStreamProvides {
            libraries: vec!["libssl.so.3".to_string(), "libcrypto.so.3".to_string()],
            binaries: vec!["openssl".to_string()],
            python3: vec![],
            dbus: vec![],
        };

        let count = persist_appstream_provides(&conn, canonical_id, &provides).unwrap();
        assert_eq!(count, 3);

        // Verify they're in the DB
        let db_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM appstream_provides WHERE canonical_id = ?1",
                [canonical_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(db_count, 3);
    }
}
