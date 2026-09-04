// conary-core/src/ccs/convert/native_provenance.rs

//! Native package provenance extraction
//!
//! Extracts provenance information from RPM, DEB, and Arch packages
//! during conversion to CCS format. This preserves the original package's
//! lineage information for audit and verification purposes.

use crate::packages::arch::ArchPackage;
use crate::packages::deb::DebPackage;
use crate::packages::eopkg::EopkgPackage;
use crate::packages::rpm::RpmPackage;
use crate::packages::traits::PackageFormat;
use crate::provenance::{
    BuildProvenance, ContentProvenance, HostAttestation, Provenance, Signature,
    SignatureProvenance, SignatureScope, SourceProvenance,
};
use anyhow::{Context, bail};
use chrono::{DateTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};

mod signature;
use signature::{ExtractedSignature, extract_deb_signature, extract_rpm_signature};

/// Provenance information extracted from a native package
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct NativeProvenance {
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

    /// Exact signature payload or a typed signature-inspection state.
    #[serde(default)]
    pub signature: NativeSignatureEvidence,
}

/// Native signature evidence. Presence is not a cryptographic signature.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum NativeSignatureEvidence {
    /// The artifact was not inspected for an embedded signature.
    #[default]
    NotInspected,
    /// No native signature was observed in the package artifact.
    NotObserved,
    /// A signature entry was observed, but no exact payload was available.
    Observed {
        signature_type: Option<String>,
        key_id: Option<String>,
    },
    /// Exact base64-encoded signature bytes were extracted from the artifact.
    Payload {
        signature_type: String,
        key_id: Option<String>,
        signature_base64: String,
    },
}

impl NativeProvenance {
    /// Create empty provenance for a format
    pub fn new(format: &str, checksum: &str) -> Self {
        Self {
            format: format.to_string(),
            original_checksum: checksum.to_string(),
            ..Default::default()
        }
    }

    fn apply_signature(&mut self, sig: Option<ExtractedSignature>) {
        self.signature = match sig {
            Some(sig) if sig.signature_data.is_empty() => NativeSignatureEvidence::Observed {
                signature_type: Some(sig.sig_type),
                key_id: sig.key_id,
            },
            Some(sig) => NativeSignatureEvidence::Payload {
                signature_type: sig.sig_type,
                key_id: sig.key_id,
                signature_base64: sig.signature_data,
            },
            None => NativeSignatureEvidence::NotObserved,
        };
    }

    fn rpm_metadata(pkg: &RpmPackage, checksum: &str) -> Self {
        let mut provenance = Self::new("rpm", checksum);
        provenance.upstream_url = pkg.url().map(String::from);
        provenance.source_rpm = pkg.source_rpm().map(String::from);
        provenance.build_host = pkg.build_host().map(String::from);
        provenance.vendor = pkg.vendor().map(String::from);
        provenance.packager = None;
        if let Some(license) = pkg.license() {
            provenance.licenses = parse_license_string(license);
        }
        provenance
    }

    /// Extract provenance from an RPM package with path for signature extraction
    pub fn from_rpm_with_path(
        pkg: &RpmPackage,
        checksum: &str,
        package_path: &str,
    ) -> anyhow::Result<Self> {
        let mut provenance = Self::rpm_metadata(pkg, checksum);
        provenance.apply_signature(extract_rpm_signature(package_path)?);
        Ok(provenance)
    }

    fn deb_metadata(pkg: &DebPackage, checksum: &str) -> Self {
        let mut provenance = Self::new("deb", checksum);
        provenance.upstream_url = pkg.homepage().map(String::from);
        provenance.packager = pkg.maintainer().map(String::from);
        provenance.section = pkg.section().map(String::from);
        provenance.priority = pkg.priority().map(String::from);
        provenance
    }

    /// Extract provenance from a DEB package with path for signature extraction
    pub fn from_deb_with_path(
        pkg: &DebPackage,
        checksum: &str,
        package_path: &str,
    ) -> anyhow::Result<Self> {
        let mut provenance = Self::deb_metadata(pkg, checksum);
        provenance.apply_signature(extract_deb_signature(package_path)?);
        Ok(provenance)
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
        prov.signature = NativeSignatureEvidence::NotObserved;

        prov
    }

    /// Convert to a full Provenance structure for storage
    pub fn to_provenance(&self) -> Provenance {
        // Build source layer
        let mut source = SourceProvenance {
            upstream_url: self.upstream_url.clone(),
            ..Default::default()
        };

        // If we have a source RPM, record it as a reference
        if let Some(ref srpm) = self.source_rpm {
            // Source RPM is like a reference to the source package
            source.upstream_url = source
                .upstream_url
                .or_else(|| Some(format!("srpm://{}", srpm)));
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
        if let Some(ref date_str) = self.build_date
            && let Some(dt) = parse_build_date(date_str)
        {
            build.build_start = Some(dt);
            build.build_end = Some(dt);
        }

        // Build signature layer
        let mut signatures = SignatureProvenance::default();
        if let NativeSignatureEvidence::Payload {
            signature_type,
            key_id: Some(key_id),
            signature_base64,
        } = &self.signature
            && !signature_base64.is_empty()
        {
            signatures.builder_sig = Some(Signature {
                key_id: key_id.clone(),
                signature: signature_base64.clone(),
                scope: SignatureScope::Build,
                timestamp: Utc::now(),
                algorithm: Some(signature_type.clone()),
                metadata: Some(format!("Extracted from {} package", self.format)),
            });
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
            || matches!(
                self.signature,
                NativeSignatureEvidence::Observed { .. } | NativeSignatureEvidence::Payload { .. }
            )
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

        if matches!(
            self.signature,
            NativeSignatureEvidence::Observed { .. } | NativeSignatureEvidence::Payload { .. }
        ) {
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
    /// * `format` - Package format ("rpm", "deb", "arch", "eopkg")
    /// * `checksum` - Checksum of the original package
    /// * `path` - Path to the package file
    ///
    /// # Returns
    /// Exact extracted provenance. Package, archive, and signature parse
    /// failures are returned to the conversion boundary.
    pub fn extract_from_path(
        format: &str,
        checksum: &str,
        path: &std::path::Path,
    ) -> anyhow::Result<Self> {
        let path_str = path
            .to_str()
            .context("native package path is not valid UTF-8")?;

        match format {
            "rpm" => {
                let package = RpmPackage::parse(path_str)
                    .with_context(|| format!("failed to parse RPM provenance from {path:?}"))?;
                Self::from_rpm_with_path(&package, checksum, path_str)
            }
            "deb" => {
                let package = DebPackage::parse(path_str)
                    .with_context(|| format!("failed to parse DEB provenance from {path:?}"))?;
                Self::from_deb_with_path(&package, checksum, path_str)
            }
            "arch" => {
                let package = ArchPackage::parse(path_str)
                    .with_context(|| format!("failed to parse Arch provenance from {path:?}"))?;
                Ok(Self::from_arch(&package, checksum))
            }
            "eopkg" => {
                EopkgPackage::parse(path_str)
                    .with_context(|| format!("failed to parse eopkg provenance from {path:?}"))?;
                let mut provenance = Self::new("eopkg", checksum);
                provenance.signature = NativeSignatureEvidence::NotObserved;
                Ok(provenance)
            }
            _ => bail!("unsupported native provenance format {format:?}"),
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

#[cfg(test)]
mod tests;
