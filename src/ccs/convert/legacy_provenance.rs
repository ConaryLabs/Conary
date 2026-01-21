// src/ccs/convert/legacy_provenance.rs

//! Legacy package provenance extraction
//!
//! Extracts provenance information from RPM, DEB, and Arch packages
//! during conversion to CCS format. This preserves the original package's
//! lineage information for audit and verification purposes.

use crate::packages::arch::ArchPackage;
use crate::packages::deb::DebPackage;
use crate::packages::rpm::RpmPackage;
use crate::packages::traits::PackageFormat;
use crate::provenance::{
    BuildProvenance, ContentProvenance, HostAttestation, Provenance, Signature, SignatureProvenance,
    SignatureScope, SourceProvenance,
};
use chrono::{DateTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};

/// Provenance information extracted from a legacy package
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LegacyProvenance {
    /// Original package format (rpm, deb, arch)
    pub format: String,

    /// Original package checksum
    pub original_checksum: String,

    // Source layer
    /// Upstream URL (homepage/url field)
    pub upstream_url: Option<String>,

    /// Source RPM name (RPM only)
    pub source_rpm: Option<String>,

    // Build layer
    /// Build host
    pub build_host: Option<String>,

    /// Build date/timestamp
    pub build_date: Option<String>,

    /// Packager/maintainer identity
    pub packager: Option<String>,

    /// Vendor (RPM only)
    pub vendor: Option<String>,

    // License information
    /// License(s) declared in the package
    pub licenses: Vec<String>,

    // Debian-specific
    /// Section (DEB only)
    pub section: Option<String>,

    /// Priority (DEB only)
    pub priority: Option<String>,

    // Arch-specific
    /// Groups (Arch only)
    pub groups: Vec<String>,

    // Signature information (if extractable)
    /// Whether the original package was signed
    pub was_signed: bool,

    /// Original signature data (base64 encoded if available)
    pub original_signature: Option<String>,

    /// Signature key ID (if extractable)
    pub signature_key_id: Option<String>,
}

impl LegacyProvenance {
    /// Create empty provenance for a format
    pub fn new(format: &str, checksum: &str) -> Self {
        Self {
            format: format.to_string(),
            original_checksum: checksum.to_string(),
            ..Default::default()
        }
    }

    /// Extract provenance from an RPM package
    pub fn from_rpm(pkg: &RpmPackage, checksum: &str) -> Self {
        Self::from_rpm_with_path(pkg, checksum, None)
    }

    /// Extract provenance from an RPM package with path for signature extraction
    pub fn from_rpm_with_path(pkg: &RpmPackage, checksum: &str, package_path: Option<&str>) -> Self {
        let mut prov = Self::new("rpm", checksum);

        // Source layer
        prov.upstream_url = pkg.url().map(String::from);
        prov.source_rpm = pkg.source_rpm().map(String::from);

        // Build layer
        prov.build_host = pkg.build_host().map(String::from);
        prov.vendor = pkg.vendor().map(String::from);
        prov.packager = None; // RPM uses vendor instead

        // License
        if let Some(license) = pkg.license() {
            prov.licenses = parse_license_string(license);
        }

        // Extract signature if we have the package path
        if let Some(path) = package_path {
            if let Some(sig) = extract_rpm_signature(path) {
                prov.was_signed = true;
                prov.signature_key_id = Some(sig.key_id);
                prov.original_signature = if sig.signature_data.is_empty() {
                    None
                } else {
                    Some(sig.signature_data)
                };
            }
        }

        prov
    }

    /// Extract provenance from a DEB package
    pub fn from_deb(pkg: &DebPackage, checksum: &str) -> Self {
        Self::from_deb_with_path(pkg, checksum, None)
    }

    /// Extract provenance from a DEB package with path for signature extraction
    pub fn from_deb_with_path(pkg: &DebPackage, checksum: &str, package_path: Option<&str>) -> Self {
        let mut prov = Self::new("deb", checksum);

        // Source layer
        prov.upstream_url = pkg.homepage().map(String::from);

        // Build layer
        prov.packager = pkg.maintainer().map(String::from);

        // Debian-specific
        prov.section = pkg.section().map(String::from);
        prov.priority = pkg.priority().map(String::from);

        // Extract signature if we have the package path
        if let Some(path) = package_path {
            if let Some(sig) = extract_deb_signature(path) {
                prov.was_signed = true;
                prov.signature_key_id = Some(sig.key_id);
                prov.original_signature = if sig.signature_data.is_empty() {
                    None
                } else {
                    Some(sig.signature_data)
                };
            }
        }

        prov
    }

    /// Extract provenance from an Arch package
    pub fn from_arch(pkg: &ArchPackage, checksum: &str) -> Self {
        let mut prov = Self::new("arch", checksum);

        // Source layer
        prov.upstream_url = pkg.url().map(String::from);

        // Build layer
        prov.packager = pkg.packager().map(String::from);
        prov.build_date = pkg.build_date().map(String::from);

        // License
        prov.licenses = pkg.licenses().to_vec();

        // Arch-specific
        prov.groups = pkg.groups().to_vec();

        // Note: Arch package signatures are stored separately in .sig files
        // not within the package itself
        prov.was_signed = false;

        prov
    }

    /// Convert to a full Provenance structure for storage
    pub fn to_provenance(&self) -> Provenance {
        // Build source layer
        let mut source = SourceProvenance::default();
        source.upstream_url = self.upstream_url.clone();

        // If we have a source RPM, record it as a reference
        if let Some(ref srpm) = self.source_rpm {
            // Source RPM is like a reference to the source package
            source.upstream_url = source.upstream_url.or_else(|| {
                Some(format!("srpm://{}", srpm))
            });
        }

        // Build build layer
        let mut build = BuildProvenance::default();

        // Set host attestation if we have build host info
        if let Some(ref host) = self.build_host {
            build.host_attestation = Some(HostAttestation {
                hostname: Some(host.clone()),
                arch: String::new(), // Unknown from package metadata
                kernel: String::new(),
                distro: None,
                tpm_quote: None,
                secure_boot: None,
            });
        }

        // Parse and set build date
        if let Some(ref date_str) = self.build_date {
            if let Some(dt) = parse_build_date(date_str) {
                build.build_start = Some(dt);
                build.build_end = Some(dt);
            }
        }

        // Build signature layer
        let mut signatures = SignatureProvenance::default();
        if self.was_signed {
            // We know it was signed but may not have the actual signature
            // This at least records the provenance that it was signed
            if let Some(ref key_id) = self.signature_key_id {
                signatures.builder_sig = Some(Signature {
                    key_id: key_id.clone(),
                    signature: self.original_signature.clone().unwrap_or_default(),
                    scope: SignatureScope::Build,
                    timestamp: Utc::now(),
                    algorithm: None,
                    metadata: Some(format!("Extracted from {} package", self.format)),
                });
            }
        }

        // Content layer is populated separately during conversion
        let content = ContentProvenance::default();

        Provenance::new(source, build, signatures, content)
    }

    /// Serialize to JSON for storage
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    /// Deserialize from JSON
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    /// Check if this provenance has meaningful information
    pub fn has_content(&self) -> bool {
        self.upstream_url.is_some()
            || self.source_rpm.is_some()
            || self.build_host.is_some()
            || self.packager.is_some()
            || !self.licenses.is_empty()
            || self.was_signed
    }

    /// Get a summary string for display
    pub fn summary(&self) -> String {
        let mut parts = vec![format!("format={}", self.format)];

        if let Some(ref url) = self.upstream_url {
            parts.push(format!("url={}", url));
        }

        if let Some(ref packager) = self.packager {
            parts.push(format!("packager={}", packager));
        }

        if !self.licenses.is_empty() {
            parts.push(format!("licenses={}", self.licenses.join(", ")));
        }

        if self.was_signed {
            parts.push("signed=true".to_string());
        }

        parts.join("; ")
    }

    /// Extract provenance from a package file by re-opening it
    ///
    /// This is useful when you only have the path and format, but not
    /// the parsed package object. The package is re-parsed to extract
    /// provenance metadata.
    ///
    /// # Arguments
    /// * `format` - Package format ("rpm", "deb", "arch")
    /// * `checksum` - Checksum of the original package
    /// * `path` - Path to the package file
    ///
    /// # Returns
    /// Extracted provenance, or a basic provenance if parsing fails
    pub fn extract_from_path(
        format: &str,
        checksum: &str,
        path: &std::path::Path,
    ) -> Self {
        let path_str = path.to_string_lossy();

        match format {
            "rpm" => {
                match RpmPackage::parse(&path_str) {
                    Ok(pkg) => Self::from_rpm_with_path(&pkg, checksum, Some(&path_str)),
                    Err(e) => {
                        tracing::warn!("Failed to parse RPM for provenance: {}", e);
                        Self::new("rpm", checksum)
                    }
                }
            }
            "deb" => {
                match DebPackage::parse(&path_str) {
                    Ok(pkg) => Self::from_deb_with_path(&pkg, checksum, Some(&path_str)),
                    Err(e) => {
                        tracing::warn!("Failed to parse DEB for provenance: {}", e);
                        Self::new("deb", checksum)
                    }
                }
            }
            "arch" => {
                match ArchPackage::parse(&path_str) {
                    Ok(pkg) => Self::from_arch(&pkg, checksum),
                    Err(e) => {
                        tracing::warn!("Failed to parse Arch package for provenance: {}", e);
                        Self::new("arch", checksum)
                    }
                }
            }
            _ => {
                tracing::warn!("Unknown package format for provenance: {}", format);
                Self::new(format, checksum)
            }
        }
    }
}

/// Parse a license string into multiple licenses
///
/// Handles common patterns:
/// - "MIT" -> ["MIT"]
/// - "GPL-2.0 or MIT" -> ["GPL-2.0", "MIT"]
/// - "GPL-2.0 AND MIT" -> ["GPL-2.0", "MIT"]
/// - "(GPL-2.0 OR MIT)" -> ["GPL-2.0", "MIT"]
fn parse_license_string(license: &str) -> Vec<String> {
    // Remove parentheses
    let license = license.trim_matches(|c| c == '(' || c == ')');

    // Split on common separators
    let separators = [" or ", " OR ", " and ", " AND ", ", ", "/"];

    let mut result = vec![license.to_string()];

    for sep in separators {
        let mut new_result = Vec::new();
        for part in &result {
            if part.contains(sep) {
                new_result.extend(part.split(sep).map(|s| s.trim().to_string()));
            } else {
                new_result.push(part.clone());
            }
        }
        result = new_result;
    }

    // Clean up and deduplicate
    result
        .into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Parse various build date formats
fn parse_build_date(date_str: &str) -> Option<DateTime<Utc>> {
    // Try Unix timestamp first (Arch uses this)
    if let Ok(ts) = date_str.parse::<i64>() {
        return Utc.timestamp_opt(ts, 0).single();
    }

    // Try RFC 2822 format (common in RPM)
    if let Ok(dt) = DateTime::parse_from_rfc2822(date_str) {
        return Some(dt.with_timezone(&Utc));
    }

    // Try ISO 8601
    if let Ok(dt) = DateTime::parse_from_rfc3339(date_str) {
        return Some(dt.with_timezone(&Utc));
    }

    // Try common formats
    let formats = [
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%d",
        "%a %b %d %H:%M:%S %Y",
        "%a %b %d %H:%M:%S UTC %Y",
    ];

    for fmt in formats {
        if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(date_str, fmt) {
            return Some(DateTime::from_naive_utc_and_offset(dt, Utc));
        }
    }

    None
}

/// Signature information extracted from a package
#[derive(Debug, Clone)]
pub struct ExtractedSignature {
    /// Key ID used to sign (may be truncated fingerprint)
    pub key_id: String,
    /// Signature type (PGP, GPG, RSA)
    pub sig_type: String,
    /// Raw signature data (base64 encoded)
    pub signature_data: String,
}

/// Extract signature information from RPM package
///
/// RPM packages can contain multiple signature types:
/// - RSA/DSA header signatures (most common in modern RPMs)
/// - PGP signatures (legacy)
/// - GPG signatures
///
/// Returns signature info if a signature is found.
pub fn extract_rpm_signature(path: &str) -> Option<ExtractedSignature> {
    use base64::{engine::general_purpose::STANDARD, Engine};
    use rpm::IndexSignatureTag;
    use std::fs::File;
    use std::io::BufReader;

    let file = File::open(path).ok()?;
    let mut reader = BufReader::new(file);
    let pkg = rpm::Package::parse(&mut reader).ok()?;

    // Try to get RSA signature (most common in modern RPMs)
    if let Ok(sig_data) = pkg
        .metadata
        .signature
        .get_entry_data_as_binary(IndexSignatureTag::RPMSIGTAG_RSA)
    {
        let key_id = extract_pgp_key_id(sig_data);
        let sig_b64 = STANDARD.encode(sig_data);

        return Some(ExtractedSignature {
            key_id: key_id.unwrap_or_else(|| "unknown".to_string()),
            sig_type: "RSA".to_string(),
            signature_data: sig_b64,
        });
    }

    // Try DSA signature
    if let Ok(sig_data) = pkg
        .metadata
        .signature
        .get_entry_data_as_binary(IndexSignatureTag::RPMSIGTAG_DSA)
    {
        let key_id = extract_pgp_key_id(sig_data);
        let sig_b64 = STANDARD.encode(sig_data);

        return Some(ExtractedSignature {
            key_id: key_id.unwrap_or_else(|| "unknown".to_string()),
            sig_type: "DSA".to_string(),
            signature_data: sig_b64,
        });
    }

    // Try legacy PGP signature (RPM v3 style)
    if let Ok(sig_data) = pkg
        .metadata
        .signature
        .get_entry_data_as_binary(IndexSignatureTag::RPMSIGTAG_PGP)
    {
        let key_id = extract_pgp_key_id(sig_data);
        let sig_b64 = STANDARD.encode(sig_data);

        return Some(ExtractedSignature {
            key_id: key_id.unwrap_or_else(|| "unknown".to_string()),
            sig_type: "PGP".to_string(),
            signature_data: sig_b64,
        });
    }

    None
}

/// Extract key ID from PGP signature packet
///
/// PGP signature packets contain the issuer key ID in the unhashed
/// subpacket area. The key ID is typically the last 8 bytes of the
/// key fingerprint.
fn extract_pgp_key_id(sig_data: &[u8]) -> Option<String> {
    // PGP signature format (simplified):
    // - Version byte (3 or 4)
    // - Signature type
    // - For v4: hashed subpacket length, hashed subpackets,
    //           unhashed subpacket length, unhashed subpackets
    //
    // The issuer key ID is in subpacket type 16

    if sig_data.len() < 10 {
        return None;
    }

    // Check version
    let version = sig_data[0];
    if version != 3 && version != 4 {
        // Not a recognized PGP signature version
        // Fall back to hex encoding the last 8 bytes
        if sig_data.len() >= 8 {
            let key_bytes = &sig_data[sig_data.len() - 8..];
            return Some(hex::encode(key_bytes).to_uppercase());
        }
        return None;
    }

    // For v4 signatures, search for subpacket type 16 (issuer)
    if version == 4 && sig_data.len() > 6 {
        // Skip: version(1) + sig_type(1) + pub_algo(1) + hash_algo(1) = 4 bytes
        // Then hashed subpacket length (2 bytes)
        let hashed_len = u16::from_be_bytes([sig_data[4], sig_data[5]]) as usize;

        if sig_data.len() > 6 + hashed_len + 2 {
            // Skip hashed subpackets, get unhashed length
            let unhashed_start = 6 + hashed_len;
            let unhashed_len =
                u16::from_be_bytes([sig_data[unhashed_start], sig_data[unhashed_start + 1]])
                    as usize;

            // Search unhashed subpackets for issuer (type 16)
            let mut offset = unhashed_start + 2;
            let end = offset + unhashed_len;

            while offset < end && offset < sig_data.len() {
                // Subpacket length (1 or 2 or 5 bytes)
                let subpkt_len = sig_data[offset] as usize;
                if subpkt_len == 0 || offset + 1 >= sig_data.len() {
                    break;
                }

                let subpkt_type = sig_data[offset + 1];

                // Type 16 is issuer key ID (8 bytes)
                if subpkt_type == 16 && subpkt_len == 9 && offset + 10 <= sig_data.len() {
                    let key_id = &sig_data[offset + 2..offset + 10];
                    return Some(hex::encode(key_id).to_uppercase());
                }

                offset += 1 + subpkt_len;
            }
        }
    }

    // Fallback: return last 8 bytes as key ID hint
    if sig_data.len() >= 8 {
        let key_bytes = &sig_data[sig_data.len() - 8..];
        return Some(hex::encode(key_bytes).to_uppercase());
    }

    None
}

/// Extract signature from DEB package
///
/// Debian packages may have detached signatures in _gpgorigin file,
/// but this is not commonly used. Most Debian package verification
/// is done via Release file signatures in the repository.
pub fn extract_deb_signature(path: &str) -> Option<ExtractedSignature> {
    use base64::{engine::general_purpose::STANDARD, Engine};
    use std::fs::File;
    use std::io::Read;

    // Try to extract _gpgorigin from the AR archive
    let file = File::open(path).ok()?;
    let mut archive = ar::Archive::new(file);

    while let Some(entry) = archive.next_entry() {
        let mut entry = entry.ok()?;
        let name = String::from_utf8_lossy(entry.header().identifier()).to_string();

        if name.starts_with("_gpgorigin") {
            let mut sig_data = Vec::new();
            entry.read_to_end(&mut sig_data).ok()?;

            // ASCII-armored PGP signature
            let sig_str = String::from_utf8_lossy(&sig_data);

            // Extract key ID from armored signature
            let key_id = extract_armor_key_id(&sig_str);
            let sig_b64 = STANDARD.encode(&sig_data);

            return Some(ExtractedSignature {
                key_id: key_id.unwrap_or_else(|| "unknown".to_string()),
                sig_type: "PGP-ARMOR".to_string(),
                signature_data: sig_b64,
            });
        }
    }

    None
}

/// Extract key ID from ASCII-armored PGP signature
fn extract_armor_key_id(armored: &str) -> Option<String> {
    // Look for "Key ID" or similar in the armor headers
    for line in armored.lines() {
        if line.contains("Key ID") || line.contains("KeyID") {
            // Extract hex key ID from the line
            let hex_chars: String = line.chars().filter(|c| c.is_ascii_hexdigit()).collect();
            if hex_chars.len() >= 8 {
                return Some(hex_chars[..16.min(hex_chars.len())].to_uppercase());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_license_string_simple() {
        let licenses = parse_license_string("MIT");
        assert_eq!(licenses, vec!["MIT"]);
    }

    #[test]
    fn test_parse_license_string_or() {
        let licenses = parse_license_string("GPL-2.0 or MIT");
        assert_eq!(licenses, vec!["GPL-2.0", "MIT"]);
    }

    #[test]
    fn test_parse_license_string_and() {
        let licenses = parse_license_string("GPL-2.0 AND Apache-2.0");
        assert_eq!(licenses, vec!["GPL-2.0", "Apache-2.0"]);
    }

    #[test]
    fn test_parse_license_string_complex() {
        let licenses = parse_license_string("(GPL-2.0 OR MIT)");
        assert_eq!(licenses, vec!["GPL-2.0", "MIT"]);
    }

    #[test]
    fn test_parse_build_date_unix() {
        let dt = parse_build_date("1700000000");
        assert!(dt.is_some());
        assert_eq!(dt.unwrap().timestamp(), 1700000000);
    }

    #[test]
    fn test_parse_build_date_iso() {
        let dt = parse_build_date("2024-01-15T10:30:00Z");
        assert!(dt.is_some());
    }

    #[test]
    fn test_legacy_provenance_new() {
        let prov = LegacyProvenance::new("rpm", "sha256:abc123");
        assert_eq!(prov.format, "rpm");
        assert_eq!(prov.original_checksum, "sha256:abc123");
        assert!(!prov.has_content());
    }

    #[test]
    fn test_legacy_provenance_has_content() {
        let mut prov = LegacyProvenance::new("deb", "sha256:def456");
        assert!(!prov.has_content());

        prov.upstream_url = Some("https://example.com".to_string());
        assert!(prov.has_content());
    }

    #[test]
    fn test_legacy_provenance_summary() {
        let mut prov = LegacyProvenance::new("arch", "sha256:ghi789");
        prov.upstream_url = Some("https://example.com".to_string());
        prov.packager = Some("Test User".to_string());
        prov.licenses = vec!["MIT".to_string()];

        let summary = prov.summary();
        assert!(summary.contains("format=arch"));
        assert!(summary.contains("url=https://example.com"));
        assert!(summary.contains("packager=Test User"));
        assert!(summary.contains("licenses=MIT"));
    }

    #[test]
    fn test_legacy_provenance_json_roundtrip() {
        let mut prov = LegacyProvenance::new("rpm", "sha256:test");
        prov.upstream_url = Some("https://test.com".to_string());
        prov.licenses = vec!["Apache-2.0".to_string(), "MIT".to_string()];

        let json = prov.to_json().unwrap();
        let restored = LegacyProvenance::from_json(&json).unwrap();

        assert_eq!(restored.format, prov.format);
        assert_eq!(restored.upstream_url, prov.upstream_url);
        assert_eq!(restored.licenses, prov.licenses);
    }

    #[test]
    fn test_to_provenance() {
        let mut prov = LegacyProvenance::new("rpm", "sha256:test");
        prov.upstream_url = Some("https://nginx.org".to_string());
        prov.build_host = Some("builder.example.com".to_string());
        prov.build_date = Some("1700000000".to_string());

        let full_prov = prov.to_provenance();

        assert_eq!(
            full_prov.source.upstream_url,
            Some("https://nginx.org".to_string())
        );
        assert!(full_prov.build.host_attestation.is_some());
        assert!(full_prov.build.build_start.is_some());
    }
}
