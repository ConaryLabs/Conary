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
use serde::Deserialize;

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
pub fn parse_appstream_xml(xml: &str) -> Result<Vec<AppStreamComponent>> {
    let mut reader = Reader::from_str(xml);
    reader.trim_text(true);

    let mut components = Vec::new();
    let mut buf = Vec::new();

    // State tracking for the current component being parsed
    let mut in_component = false;
    let mut current_tag: Option<String> = None;
    let mut id = None;
    let mut pkgname = None;
    let mut name = None;
    let mut summary = None;

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
                    b"component" => {
                        in_component = true;
                        id = None;
                        pkgname = None;
                        name = None;
                        summary = None;
                    }
                    b"id" | b"pkgname" | b"name" | b"summary" if in_component => {
                        current_tag =
                            Some(String::from_utf8_lossy(tag.as_ref()).into_owned());
                    }
                    _ => {}
                }
            }
            Ok(Event::Text(e)) => {
                if in_component
                    && let Some(ref tag) = current_tag
                {
                    let text = e
                        .unescape()
                        .map_err(|err| {
                            Error::ParseError(format!(
                                "AppStream XML text decode error: {err}"
                            ))
                        })?
                        .into_owned();
                    match tag.as_str() {
                        "id" => id = Some(text),
                        "pkgname" => pkgname = Some(text),
                        "name" => name = Some(text),
                        "summary" => summary = Some(text),
                        _ => {}
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
                        });
                    }
                    in_component = false;
                    current_tag = None;
                }
                b"id" | b"pkgname" | b"name" | b"summary" => {
                    current_tag = None;
                }
                _ => {}
            },
            _ => {}
        }
        buf.clear();
    }

    Ok(components)
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
            let _header: serde_yaml::Value =
                serde::Deserialize::deserialize(document).map_err(|e| {
                    Error::ParseError(format!("DEP-11 YAML header parse error: {e}"))
                })?;
            continue;
        }

        let doc: Dep11Doc = serde::Deserialize::deserialize(document)
            .map_err(|e| Error::ParseError(format!("DEP-11 YAML parse error: {e}")))?;

        if let (Some(id), Some(package)) = (doc.id, doc.package) {
            let name_str = doc
                .name
                .and_then(|n| n.c)
                .unwrap_or_default();
            let summary_str = doc.summary.and_then(|s| s.c);

            components.push(AppStreamComponent {
                id,
                pkgname: package,
                name: name_str,
                summary: summary_str,
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
    let mut count = 0;

    for comp in components {
        let mut canonical = CanonicalPackage::new(
            comp.pkgname.clone(),
            "package".to_string(),
        );
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
            },
            AppStreamComponent {
                id: "org.gnome.Nautilus".to_string(),
                pkgname: "nautilus".to_string(),
                name: "Files".to_string(),
                summary: None,
            },
        ];

        let count = ingest_appstream(&conn, &components, "fedora").unwrap();
        assert_eq!(count, 2);

        // Verify canonical packages were created
        let pkg = CanonicalPackage::find_by_name(&conn, "firefox").unwrap().unwrap();
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
}
