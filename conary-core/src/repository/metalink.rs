// conary-core/src/repository/metalink.rs

//! Metalink parser for mirror discovery
//!
//! Supports two formats:
//! - Metalink XML (RFC 5854) - used by Fedora's mirror infrastructure
//! - Metalink HTTP headers (RFC 6249) - Link headers with rel=duplicate

use quick_xml::Reader;
use quick_xml::events::Event;
use std::collections::HashMap;

/// A mirror discovered from a Metalink document
#[derive(Debug, Clone)]
pub struct MetalinkMirror {
    /// Mirror URL
    pub url: String,
    /// Priority/preference (lower number = higher priority)
    pub priority: u32,
    /// ISO 3166-1 alpha-2 country code for geo selection
    pub location: Option<String>,
    /// Protocol (https, http, ftp)
    pub protocol: String,
}

/// A file described in a Metalink document
#[derive(Debug, Clone)]
pub struct MetalinkFile {
    /// Filename
    pub name: String,
    /// File size in bytes
    pub size: Option<u64>,
    /// Available mirrors sorted by priority
    pub mirrors: Vec<MetalinkMirror>,
    /// Hash verification: algorithm -> hex hash
    pub hashes: HashMap<String, String>,
}

/// Parse a Metalink XML document (RFC 5854)
///
/// Fedora's metalink format uses `<url>` elements inside `<resources>` with
/// `preference` attributes (higher = better). We convert to `priority` where
/// lower = better by computing `200 - preference`.
pub fn parse_metalink_xml(xml: &str) -> Result<Vec<MetalinkFile>, String> {
    let mut reader = Reader::from_str(xml);
    reader.trim_text(true);

    let mut buf = Vec::new();
    let mut files = Vec::new();

    // Current parsing state
    let mut current_file: Option<MetalinkFile> = None;
    let mut in_verification = false;
    let mut in_resources = false;
    let mut current_hash_type: Option<String> = None;
    let mut current_text = String::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let qname = e.name();
                let local_name = qname.as_ref();
                match local_name {
                    b"file" => {
                        let name = extract_attr(&e, b"name").unwrap_or_default();
                        current_file = Some(MetalinkFile {
                            name,
                            size: None,
                            mirrors: Vec::new(),
                            hashes: HashMap::new(),
                        });
                    }
                    b"verification" => {
                        in_verification = true;
                    }
                    b"resources" => {
                        in_resources = true;
                    }
                    b"hash" if in_verification => {
                        current_hash_type = extract_attr(&e, b"type");
                        current_text.clear();
                    }
                    b"url" if in_resources => {
                        if let Some(ref mut file) = current_file {
                            let protocol =
                                extract_attr(&e, b"protocol").unwrap_or_else(|| "https".into());
                            let location = extract_attr(&e, b"location");

                            // Preference: higher = better. Convert to priority: lower = better.
                            let preference: u32 = extract_attr(&e, b"preference")
                                .and_then(|v| v.parse().ok())
                                .unwrap_or(50);
                            let priority = 200u32.saturating_sub(preference);

                            // Read the URL text content
                            let mut url_text = String::new();
                            if let Ok(Event::Text(t)) = reader.read_event_into(&mut buf) {
                                url_text = t.unescape().unwrap_or_default().trim().to_string();
                            }

                            if !url_text.is_empty() {
                                file.mirrors.push(MetalinkMirror {
                                    url: url_text,
                                    priority,
                                    location,
                                    protocol,
                                });
                            }
                        }
                    }
                    b"size" => {
                        current_text.clear();
                    }
                    _ => {}
                }
            }
            Ok(Event::End(e)) => {
                let qname = e.name();
                let local_name = qname.as_ref();
                match local_name {
                    b"file" => {
                        if let Some(mut file) = current_file.take() {
                            // Sort mirrors by priority (lowest first = highest preference)
                            file.mirrors.sort_by_key(|m| m.priority);
                            files.push(file);
                        }
                    }
                    b"verification" => {
                        in_verification = false;
                    }
                    b"resources" => {
                        in_resources = false;
                    }
                    b"hash" if in_verification => {
                        if let Some(hash_type) = current_hash_type.take()
                            && let Some(ref mut file) = current_file
                        {
                            let hash_value = current_text.trim().to_string();
                            if !hash_value.is_empty() {
                                file.hashes.insert(hash_type, hash_value);
                            }
                        }
                    }
                    b"size" => {
                        if let Some(ref mut file) = current_file {
                            file.size = current_text.trim().parse().ok();
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::Text(t)) => {
                if let Ok(text) = t.unescape() {
                    current_text.push_str(&text);
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(format!("XML parse error: {}", e)),
            _ => {}
        }
        buf.clear();
    }

    Ok(files)
}

/// Parse Metalink HTTP headers (RFC 6249 - Link headers with rel=duplicate)
///
/// Parses headers like:
/// ```text
/// Link: <https://mirror1.example.com/file>; rel=duplicate; pri=1; geo=us
/// Link: <https://mirror2.example.com/file>; rel=duplicate; pri=2; geo=de
/// ```
pub fn parse_metalink_headers(headers: &[(String, String)]) -> Vec<MetalinkMirror> {
    let mut mirrors = Vec::new();

    for (name, value) in headers {
        if !name.eq_ignore_ascii_case("link") {
            continue;
        }

        // A Link header can contain multiple entries separated by commas,
        // but commas inside angle brackets are part of the URL
        for link_entry in split_link_header(value) {
            if let Some(mirror) = parse_link_entry(&link_entry) {
                mirrors.push(mirror);
            }
        }
    }

    mirrors.sort_by_key(|m| m.priority);
    mirrors
}

/// Extract base mirror URLs from metalink files
///
/// Given mirrors like `"https://mirror1.example.com/repo/repodata/repomd.xml"`
/// returns `"https://mirror1.example.com/repo/"` (strips the file path suffix).
/// This assumes the metalink describes a file within a repository structure
/// and the base URL is the repository root.
pub fn extract_base_urls(files: &[MetalinkFile]) -> Vec<MetalinkMirror> {
    let mut base_mirrors = Vec::new();

    for file in files {
        for mirror in &file.mirrors {
            // Find the last component that matches the file name and strip it
            // plus any repo-internal path (e.g., "repodata/repomd.xml")
            if let Some(base_url) = strip_file_path(&mirror.url, &file.name) {
                base_mirrors.push(MetalinkMirror {
                    url: base_url,
                    priority: mirror.priority,
                    location: mirror.location.clone(),
                    protocol: mirror.protocol.clone(),
                });
            }
        }
    }

    // Deduplicate by URL, keeping lowest priority
    base_mirrors.sort_by_key(|m| m.priority);
    let mut seen = std::collections::HashSet::new();
    base_mirrors.retain(|m| seen.insert(m.url.clone()));

    base_mirrors
}

/// Strip the file path from a mirror URL to get the base repository URL
fn strip_file_path(url: &str, filename: &str) -> Option<String> {
    // Find the filename in the URL path and strip everything from that point
    if let Some(pos) = url.rfind(filename) {
        // Walk backwards to find the repository root
        // For Fedora: ".../Everything/x86_64/os/repodata/repomd.xml"
        // We want to strip "repodata/repomd.xml" and keep the rest
        let path_before = &url[..pos];
        let base = path_before.trim_end_matches('/');
        // Also strip "repodata" if present (Fedora convention)
        let base = base.strip_suffix("/repodata").unwrap_or(base);
        Some(format!("{}/", base))
    } else {
        // Filename not found in URL - try stripping from the last path component
        url.rfind('/')
            .map(|last_slash| format!("{}/", &url[..last_slash]))
    }
}

/// Extract an attribute value from an XML element
fn extract_attr(e: &quick_xml::events::BytesStart, attr_name: &[u8]) -> Option<String> {
    for attr in e.attributes().flatten() {
        if attr.key.as_ref() == attr_name {
            return Some(String::from_utf8_lossy(attr.value.as_ref()).to_string());
        }
    }
    None
}

/// Split a Link header value by commas that are outside angle brackets
fn split_link_header(value: &str) -> Vec<String> {
    let mut entries = Vec::new();
    let mut current = String::new();
    let mut in_angle = false;

    for ch in value.chars() {
        match ch {
            '<' => {
                in_angle = true;
                current.push(ch);
            }
            '>' => {
                in_angle = false;
                current.push(ch);
            }
            ',' if !in_angle => {
                let trimmed = current.trim().to_string();
                if !trimmed.is_empty() {
                    entries.push(trimmed);
                }
                current.clear();
            }
            _ => {
                current.push(ch);
            }
        }
    }

    let trimmed = current.trim().to_string();
    if !trimmed.is_empty() {
        entries.push(trimmed);
    }

    entries
}

/// Parse a single Link header entry
fn parse_link_entry(entry: &str) -> Option<MetalinkMirror> {
    // Extract URL from angle brackets
    let url_start = entry.find('<')?;
    let url_end = entry.find('>')?;
    if url_end <= url_start {
        return None;
    }
    let url = entry[url_start + 1..url_end].trim().to_string();

    // Parse parameters after the URL
    let params_str = &entry[url_end + 1..];

    // Check for rel=duplicate
    let mut is_duplicate = false;
    let mut priority = 100u32;
    let mut location = None;

    for param in params_str.split(';') {
        let param = param.trim();
        if let Some((key, value)) = param.split_once('=') {
            let key = key.trim().to_lowercase();
            let value = value.trim().trim_matches('"');
            match key.as_str() {
                "rel" if value == "duplicate" => {
                    is_duplicate = true;
                }
                "pri" => {
                    if let Ok(p) = value.parse::<u32>() {
                        priority = p;
                    }
                }
                "geo" => {
                    location = Some(value.to_uppercase());
                }
                _ => {}
            }
        }
    }

    if !is_duplicate {
        return None;
    }

    // Determine protocol from URL
    let protocol = if url.starts_with("https://") {
        "https"
    } else if url.starts_with("http://") {
        "http"
    } else if url.starts_with("ftp://") {
        "ftp"
    } else {
        "unknown"
    }
    .to_string();

    Some(MetalinkMirror {
        url,
        priority,
        location,
        protocol,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const FEDORA_METALINK: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<metalink xmlns="urn:ietf:params:xml:ns:metalink">
  <file name="repomd.xml">
    <size>4567</size>
    <verification>
      <hash type="sha256">abc123def456789012345678901234567890123456789012345678901234</hash>
      <hash type="sha512">fedcba0987654321fedcba0987654321fedcba0987654321fedcba0987654321</hash>
    </verification>
    <resources>
      <url protocol="https" type="https" location="US" preference="100">
        https://mirror1.example.com/linux/releases/43/Everything/x86_64/os/repodata/repomd.xml
      </url>
      <url protocol="https" type="https" location="DE" preference="90">
        https://mirror2.example.com/linux/releases/43/Everything/x86_64/os/repodata/repomd.xml
      </url>
      <url protocol="http" type="http" location="JP" preference="80">
        http://mirror3.example.com/linux/releases/43/Everything/x86_64/os/repodata/repomd.xml
      </url>
      <url protocol="ftp" type="ftp" location="FR" preference="50">
        ftp://mirror4.example.com/linux/releases/43/Everything/x86_64/os/repodata/repomd.xml
      </url>
    </resources>
  </file>
</metalink>"#;

    #[test]
    fn test_parse_metalink_xml_basic() {
        let files = parse_metalink_xml(FEDORA_METALINK).unwrap();
        assert_eq!(files.len(), 1);

        let file = &files[0];
        assert_eq!(file.name, "repomd.xml");
        assert_eq!(file.size, Some(4567));
        assert_eq!(file.mirrors.len(), 4);
    }

    #[test]
    fn test_parse_metalink_xml_hashes() {
        let files = parse_metalink_xml(FEDORA_METALINK).unwrap();
        let file = &files[0];

        assert_eq!(file.hashes.len(), 2);
        assert_eq!(
            file.hashes.get("sha256").unwrap(),
            "abc123def456789012345678901234567890123456789012345678901234"
        );
        assert_eq!(
            file.hashes.get("sha512").unwrap(),
            "fedcba0987654321fedcba0987654321fedcba0987654321fedcba0987654321"
        );
    }

    #[test]
    fn test_parse_metalink_xml_mirror_priority() {
        let files = parse_metalink_xml(FEDORA_METALINK).unwrap();
        let file = &files[0];

        // Mirrors should be sorted by priority (lowest first)
        // preference 100 -> priority 100 (200-100)
        // preference 90  -> priority 110 (200-90)
        // preference 80  -> priority 120 (200-80)
        // preference 50  -> priority 150 (200-50)

        assert_eq!(file.mirrors[0].priority, 100);
        assert_eq!(file.mirrors[0].location, Some("US".to_string()));
        assert_eq!(file.mirrors[0].protocol, "https");

        assert_eq!(file.mirrors[1].priority, 110);
        assert_eq!(file.mirrors[1].location, Some("DE".to_string()));

        assert_eq!(file.mirrors[2].priority, 120);
        assert_eq!(file.mirrors[2].location, Some("JP".to_string()));
        assert_eq!(file.mirrors[2].protocol, "http");

        assert_eq!(file.mirrors[3].priority, 150);
        assert_eq!(file.mirrors[3].protocol, "ftp");
    }

    #[test]
    fn test_parse_metalink_xml_mirror_urls() {
        let files = parse_metalink_xml(FEDORA_METALINK).unwrap();
        let file = &files[0];

        assert!(file.mirrors[0].url.contains("mirror1.example.com"));
        assert!(file.mirrors[1].url.contains("mirror2.example.com"));
        assert!(file.mirrors[2].url.contains("mirror3.example.com"));
        assert!(file.mirrors[3].url.contains("mirror4.example.com"));
    }

    #[test]
    fn test_parse_metalink_xml_empty() {
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<metalink xmlns="urn:ietf:params:xml:ns:metalink">
</metalink>"#;
        let files = parse_metalink_xml(xml).unwrap();
        assert!(files.is_empty());
    }

    #[test]
    fn test_parse_metalink_xml_no_size() {
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<metalink xmlns="urn:ietf:params:xml:ns:metalink">
  <file name="test.rpm">
    <resources>
      <url protocol="https" preference="100">
        https://example.com/test.rpm
      </url>
    </resources>
  </file>
</metalink>"#;

        let files = parse_metalink_xml(xml).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].name, "test.rpm");
        assert_eq!(files[0].size, None);
        assert_eq!(files[0].mirrors.len(), 1);
    }

    #[test]
    fn test_parse_metalink_xml_multiple_files() {
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<metalink xmlns="urn:ietf:params:xml:ns:metalink">
  <file name="primary.xml.gz">
    <size>1000</size>
    <resources>
      <url protocol="https" preference="100">https://mirror.example.com/primary.xml.gz</url>
    </resources>
  </file>
  <file name="filelists.xml.gz">
    <size>2000</size>
    <resources>
      <url protocol="https" preference="100">https://mirror.example.com/filelists.xml.gz</url>
    </resources>
  </file>
</metalink>"#;

        let files = parse_metalink_xml(xml).unwrap();
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].name, "primary.xml.gz");
        assert_eq!(files[0].size, Some(1000));
        assert_eq!(files[1].name, "filelists.xml.gz");
        assert_eq!(files[1].size, Some(2000));
    }

    #[test]
    fn test_parse_metalink_xml_invalid() {
        // quick-xml is lenient with some malformed XML, so test with content
        // that has no metalink structure - should parse but return no files
        let result = parse_metalink_xml("<root><child/></root>");
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    #[test]
    fn test_parse_metalink_headers_basic() {
        let headers = vec![
            (
                "Link".to_string(),
                "<https://mirror1.example.com/file.rpm>; rel=duplicate; pri=1; geo=us".to_string(),
            ),
            (
                "Link".to_string(),
                "<https://mirror2.example.com/file.rpm>; rel=duplicate; pri=2; geo=de".to_string(),
            ),
        ];

        let mirrors = parse_metalink_headers(&headers);
        assert_eq!(mirrors.len(), 2);

        assert_eq!(mirrors[0].url, "https://mirror1.example.com/file.rpm");
        assert_eq!(mirrors[0].priority, 1);
        assert_eq!(mirrors[0].location, Some("US".to_string()));
        assert_eq!(mirrors[0].protocol, "https");

        assert_eq!(mirrors[1].url, "https://mirror2.example.com/file.rpm");
        assert_eq!(mirrors[1].priority, 2);
        assert_eq!(mirrors[1].location, Some("DE".to_string()));
    }

    #[test]
    fn test_parse_metalink_headers_multiple_in_one() {
        let headers = vec![(
            "Link".to_string(),
            "<https://mirror1.example.com/file>; rel=duplicate; pri=1, \
             <https://mirror2.example.com/file>; rel=duplicate; pri=2"
                .to_string(),
        )];

        let mirrors = parse_metalink_headers(&headers);
        assert_eq!(mirrors.len(), 2);
        assert_eq!(mirrors[0].priority, 1);
        assert_eq!(mirrors[1].priority, 2);
    }

    #[test]
    fn test_parse_metalink_headers_non_duplicate_ignored() {
        let headers = vec![
            (
                "Link".to_string(),
                "<https://example.com/other>; rel=describedby".to_string(),
            ),
            (
                "Link".to_string(),
                "<https://mirror.example.com/file>; rel=duplicate; pri=1".to_string(),
            ),
        ];

        let mirrors = parse_metalink_headers(&headers);
        assert_eq!(mirrors.len(), 1);
        assert_eq!(mirrors[0].url, "https://mirror.example.com/file");
    }

    #[test]
    fn test_parse_metalink_headers_non_link_ignored() {
        let headers = vec![
            (
                "Content-Type".to_string(),
                "application/octet-stream".to_string(),
            ),
            (
                "Link".to_string(),
                "<https://mirror.example.com/file>; rel=duplicate; pri=1".to_string(),
            ),
        ];

        let mirrors = parse_metalink_headers(&headers);
        assert_eq!(mirrors.len(), 1);
    }

    #[test]
    fn test_parse_metalink_headers_empty() {
        let mirrors = parse_metalink_headers(&[]);
        assert!(mirrors.is_empty());
    }

    #[test]
    fn test_parse_metalink_headers_http_protocol() {
        let headers = vec![(
            "Link".to_string(),
            "<http://mirror.example.com/file>; rel=duplicate; pri=1".to_string(),
        )];

        let mirrors = parse_metalink_headers(&headers);
        assert_eq!(mirrors[0].protocol, "http");
    }

    #[test]
    fn test_parse_metalink_headers_default_priority() {
        let headers = vec![(
            "Link".to_string(),
            "<https://mirror.example.com/file>; rel=duplicate".to_string(),
        )];

        let mirrors = parse_metalink_headers(&headers);
        assert_eq!(mirrors[0].priority, 100);
    }

    #[test]
    fn test_extract_base_urls() {
        let files = parse_metalink_xml(FEDORA_METALINK).unwrap();
        let base_urls = extract_base_urls(&files);

        // Should strip "repodata/repomd.xml" from each mirror URL
        assert_eq!(base_urls.len(), 4);

        assert!(
            base_urls[0]
                .url
                .ends_with("/linux/releases/43/Everything/x86_64/os/")
        );
        assert!(base_urls[0].url.contains("mirror1.example.com"));

        // Should be sorted by priority
        assert!(base_urls[0].priority <= base_urls[1].priority);
    }

    #[test]
    fn test_extract_base_urls_deduplication() {
        let files = vec![
            MetalinkFile {
                name: "repomd.xml".to_string(),
                size: None,
                mirrors: vec![MetalinkMirror {
                    url: "https://mirror.example.com/repo/repodata/repomd.xml".to_string(),
                    priority: 10,
                    location: None,
                    protocol: "https".to_string(),
                }],
                hashes: HashMap::new(),
            },
            MetalinkFile {
                name: "repomd.xml".to_string(),
                size: None,
                mirrors: vec![MetalinkMirror {
                    url: "https://mirror.example.com/repo/repodata/repomd.xml".to_string(),
                    priority: 20,
                    location: None,
                    protocol: "https".to_string(),
                }],
                hashes: HashMap::new(),
            },
        ];

        let base_urls = extract_base_urls(&files);
        // Same base URL should be deduplicated, keeping lowest priority
        assert_eq!(base_urls.len(), 1);
        assert_eq!(base_urls[0].priority, 10);
    }

    #[test]
    fn test_strip_file_path_fedora_style() {
        let url =
            "https://mirror.example.com/linux/releases/43/Everything/x86_64/os/repodata/repomd.xml";
        let base = strip_file_path(url, "repomd.xml").unwrap();
        assert_eq!(
            base,
            "https://mirror.example.com/linux/releases/43/Everything/x86_64/os/"
        );
    }

    #[test]
    fn test_strip_file_path_simple() {
        let url = "https://mirror.example.com/repo/file.rpm";
        let base = strip_file_path(url, "file.rpm").unwrap();
        assert_eq!(base, "https://mirror.example.com/repo/");
    }

    #[test]
    fn test_split_link_header() {
        let value =
            "<https://a.example.com/f>; rel=duplicate, <https://b.example.com/f>; rel=duplicate";
        let entries = split_link_header(value);
        assert_eq!(entries.len(), 2);
        assert!(entries[0].contains("a.example.com"));
        assert!(entries[1].contains("b.example.com"));
    }

    #[test]
    fn test_split_link_header_single() {
        let value = "<https://a.example.com/f>; rel=duplicate";
        let entries = split_link_header(value);
        assert_eq!(entries.len(), 1);
    }
}
