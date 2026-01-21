// src/capability/resolver.rs

//! Capability-based dependency resolver
//!
//! This module provides resolution of capability requirements to concrete packages.
//! Instead of depending on package names, packages can depend on capabilities like:
//! - `provides(ssl)` - any package providing SSL functionality
//! - `provides(httpd)` - any package providing HTTP server
//! - `cap(network.listen:443)` - any package that can listen on port 443
//!
//! The resolver matches these capability requirements against packages that
//! declare matching capabilities, enabling flexible and semantic dependency resolution.

use crate::capability::{
    CapabilityDeclaration, CapabilityResult, FilesystemCapabilities,
    NetworkCapabilities,
};
use rusqlite::Connection;
use std::collections::HashMap;

/// A capability requirement that needs to be resolved
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityRequirement {
    /// The capability being required
    pub capability: CapabilitySpec,
    /// Whether this is optional
    pub optional: bool,
    /// Reason for the requirement (for diagnostics)
    pub reason: Option<String>,
}

/// Specification of a capability requirement
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CapabilitySpec {
    /// A named capability (from provides table)
    /// e.g., "ssl", "httpd", "database"
    Named(String),

    /// A typed capability
    /// e.g., soname(libssl.so.3), pkgconfig(openssl)
    Typed { kind: String, name: String },

    /// Network capability requirement
    /// e.g., network.listen:443, network.outbound:https
    Network(NetworkCapabilitySpec),

    /// Filesystem capability requirement
    /// e.g., filesystem.read:/etc/ssl, filesystem.write:/var/cache
    Filesystem(FilesystemCapabilitySpec),
}

/// Network capability specification
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkCapabilitySpec {
    /// Type: listen, outbound
    pub cap_type: NetworkCapType,
    /// Port or service name
    pub port: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkCapType {
    Listen,
    Outbound,
}

/// Filesystem capability specification
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilesystemCapabilitySpec {
    /// Type: read, write, execute
    pub cap_type: FilesystemCapType,
    /// Path pattern
    pub path: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilesystemCapType {
    Read,
    Write,
    Execute,
}

/// Result of resolving a capability requirement
#[derive(Debug, Clone)]
pub struct ResolvedCapability {
    /// The original requirement
    pub requirement: CapabilityRequirement,
    /// Packages that satisfy this requirement (in priority order)
    pub providers: Vec<CapabilityProvider>,
    /// Whether resolution succeeded
    pub resolved: bool,
}

/// A package that provides a capability
#[derive(Debug, Clone)]
pub struct CapabilityProvider {
    /// Package name
    pub package_name: String,
    /// Package version (if known)
    pub version: Option<String>,
    /// Trove ID in database
    pub trove_id: Option<i64>,
    /// How well this provider matches (0-100)
    pub match_score: u32,
    /// Why this provider was selected
    pub match_reason: String,
}

/// Capability resolver that matches requirements to providers
pub struct CapabilityResolver<'a> {
    conn: &'a Connection,
    /// Cache of loaded capability declarations (reserved for future caching optimization)
    #[allow(dead_code)]
    cap_cache: HashMap<i64, CapabilityDeclaration>,
    /// Preference weights for different match types
    preferences: ResolverPreferences,
}

/// Preferences for capability resolution
#[derive(Debug, Clone)]
pub struct ResolverPreferences {
    /// Prefer packages with declared capabilities over inferred
    pub prefer_declared: bool,
    /// Prefer packages already installed
    pub prefer_installed: bool,
    /// Minimum match score to consider (0-100)
    pub min_match_score: u32,
}

impl Default for ResolverPreferences {
    fn default() -> Self {
        Self {
            prefer_declared: true,
            prefer_installed: true,
            min_match_score: 50,
        }
    }
}

impl<'a> CapabilityResolver<'a> {
    /// Create a new capability resolver
    pub fn new(conn: &'a Connection) -> Self {
        Self {
            conn,
            cap_cache: HashMap::new(),
            preferences: ResolverPreferences::default(),
        }
    }

    /// Create with custom preferences
    pub fn with_preferences(conn: &'a Connection, preferences: ResolverPreferences) -> Self {
        Self {
            conn,
            cap_cache: HashMap::new(),
            preferences,
        }
    }

    /// Resolve a single capability requirement
    pub fn resolve(&mut self, requirement: &CapabilityRequirement) -> CapabilityResult<ResolvedCapability> {
        let providers = match &requirement.capability {
            CapabilitySpec::Named(name) => self.resolve_named(name)?,
            CapabilitySpec::Typed { kind, name } => self.resolve_typed(kind, name)?,
            CapabilitySpec::Network(spec) => self.resolve_network(spec)?,
            CapabilitySpec::Filesystem(spec) => self.resolve_filesystem(spec)?,
        };

        let resolved = !providers.is_empty() || requirement.optional;

        Ok(ResolvedCapability {
            requirement: requirement.clone(),
            providers,
            resolved,
        })
    }

    /// Resolve multiple requirements at once
    pub fn resolve_all(
        &mut self,
        requirements: &[CapabilityRequirement],
    ) -> CapabilityResult<Vec<ResolvedCapability>> {
        requirements.iter().map(|r| self.resolve(r)).collect()
    }

    /// Resolve a named capability (from provides table)
    fn resolve_named(&self, name: &str) -> CapabilityResult<Vec<CapabilityProvider>> {
        let mut providers = Vec::new();

        // Query provides table for matching capabilities
        let mut stmt = self.conn.prepare(
            "SELECT p.trove_id, p.capability, p.version, p.kind, t.name, t.version as pkg_version
             FROM provides p
             JOIN troves t ON p.trove_id = t.id
             WHERE p.capability = ?1"
        )?;

        let rows = stmt.query_map([name], |row| {
            Ok((
                row.get::<_, i64>(0)?,      // trove_id
                row.get::<_, String>(1)?,   // capability
                row.get::<_, Option<String>>(2)?, // version
                row.get::<_, Option<String>>(3)?, // kind
                row.get::<_, String>(4)?,   // package name
                row.get::<_, String>(5)?,   // package version
            ))
        })?;

        for row in rows {
            let (trove_id, _cap, _cap_version, kind, pkg_name, pkg_version) = row?;

            let match_score = if self.preferences.prefer_declared {
                // Higher score for explicit provides
                90
            } else {
                80
            };

            providers.push(CapabilityProvider {
                package_name: pkg_name,
                version: Some(pkg_version),
                trove_id: Some(trove_id),
                match_score,
                match_reason: format!("Provides {} (kind: {:?})", name, kind),
            });
        }

        // Sort by match score descending
        providers.sort_by(|a, b| b.match_score.cmp(&a.match_score));

        Ok(providers)
    }

    /// Resolve a typed capability (soname, pkgconfig, etc.)
    fn resolve_typed(&self, kind: &str, name: &str) -> CapabilityResult<Vec<CapabilityProvider>> {
        let mut providers = Vec::new();

        // Query provides table for matching kind and capability
        let mut stmt = self.conn.prepare(
            "SELECT p.trove_id, t.name, t.version
             FROM provides p
             JOIN troves t ON p.trove_id = t.id
             WHERE p.kind = ?1 AND p.capability = ?2"
        )?;

        let rows = stmt.query_map([kind, name], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;

        for row in rows {
            let (trove_id, pkg_name, pkg_version) = row?;

            providers.push(CapabilityProvider {
                package_name: pkg_name,
                version: Some(pkg_version),
                trove_id: Some(trove_id),
                match_score: 95, // High score for exact typed match
                match_reason: format!("Provides {}({})", kind, name),
            });
        }

        providers.sort_by(|a, b| b.match_score.cmp(&a.match_score));
        Ok(providers)
    }

    /// Resolve a network capability requirement
    fn resolve_network(&mut self, spec: &NetworkCapabilitySpec) -> CapabilityResult<Vec<CapabilityProvider>> {
        let mut providers = Vec::new();

        // Query all packages with capability declarations
        let mut stmt = self.conn.prepare(
            "SELECT c.trove_id, c.declaration_json, t.name, t.version
             FROM capabilities c
             JOIN troves t ON c.trove_id = t.id"
        )?;

        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?;

        for row in rows {
            let (trove_id, decl_json, pkg_name, pkg_version) = row?;

            if let Ok(decl) = serde_json::from_str::<CapabilityDeclaration>(&decl_json) {
                if self.network_matches(&decl.network, spec) {
                    let match_reason = match spec.cap_type {
                        NetworkCapType::Listen => format!("Can listen on port {}", spec.port),
                        NetworkCapType::Outbound => format!("Can connect to port {}", spec.port),
                    };

                    providers.push(CapabilityProvider {
                        package_name: pkg_name,
                        version: Some(pkg_version),
                        trove_id: Some(trove_id),
                        match_score: 85,
                        match_reason,
                    });
                }
            }
        }

        providers.sort_by(|a, b| b.match_score.cmp(&a.match_score));
        Ok(providers)
    }

    /// Check if network capabilities match the requirement
    fn network_matches(&self, caps: &NetworkCapabilities, spec: &NetworkCapabilitySpec) -> bool {
        let ports = match spec.cap_type {
            NetworkCapType::Listen => &caps.listen,
            NetworkCapType::Outbound => &caps.outbound,
        };

        // Check for exact match or "any"
        ports.iter().any(|p| {
            p == &spec.port || p == "any" || self.port_in_range(p, &spec.port)
        })
    }

    /// Check if a port is within a range specification
    fn port_in_range(&self, range_spec: &str, port: &str) -> bool {
        if let Some((start, end)) = range_spec.split_once('-') {
            if let (Ok(s), Ok(e), Ok(p)) = (
                start.parse::<u16>(),
                end.parse::<u16>(),
                port.parse::<u16>(),
            ) {
                return p >= s && p <= e;
            }
        }
        false
    }

    /// Resolve a filesystem capability requirement
    fn resolve_filesystem(&mut self, spec: &FilesystemCapabilitySpec) -> CapabilityResult<Vec<CapabilityProvider>> {
        let mut providers = Vec::new();

        // Query all packages with capability declarations
        let mut stmt = self.conn.prepare(
            "SELECT c.trove_id, c.declaration_json, t.name, t.version
             FROM capabilities c
             JOIN troves t ON c.trove_id = t.id"
        )?;

        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?;

        for row in rows {
            let (trove_id, decl_json, pkg_name, pkg_version) = row?;

            if let Ok(decl) = serde_json::from_str::<CapabilityDeclaration>(&decl_json) {
                if self.filesystem_matches(&decl.filesystem, spec) {
                    let match_reason = match spec.cap_type {
                        FilesystemCapType::Read => format!("Can read {}", spec.path),
                        FilesystemCapType::Write => format!("Can write {}", spec.path),
                        FilesystemCapType::Execute => format!("Can execute in {}", spec.path),
                    };

                    providers.push(CapabilityProvider {
                        package_name: pkg_name,
                        version: Some(pkg_version),
                        trove_id: Some(trove_id),
                        match_score: 80,
                        match_reason,
                    });
                }
            }
        }

        providers.sort_by(|a, b| b.match_score.cmp(&a.match_score));
        Ok(providers)
    }

    /// Check if filesystem capabilities match the requirement
    fn filesystem_matches(&self, caps: &FilesystemCapabilities, spec: &FilesystemCapabilitySpec) -> bool {
        let paths = match spec.cap_type {
            FilesystemCapType::Read => &caps.read,
            FilesystemCapType::Write => &caps.write,
            FilesystemCapType::Execute => &caps.execute,
        };

        paths.iter().any(|p| self.path_matches(p, &spec.path))
    }

    /// Check if a path pattern matches a required path
    fn path_matches(&self, pattern: &str, required: &str) -> bool {
        // Exact match
        if pattern == required {
            return true;
        }

        // Prefix match (e.g., /var/cache matches /var/cache/nginx)
        if required.starts_with(pattern) && required[pattern.len()..].starts_with('/') {
            return true;
        }

        // Glob-style matching (e.g., /var/cache/* matches /var/cache/nginx)
        if pattern.ends_with("/*") {
            let prefix = &pattern[..pattern.len() - 2];
            if required.starts_with(prefix) {
                return true;
            }
        }

        false
    }

    /// Get packages providing a specific capability by name
    pub fn find_providers(&self, capability: &str) -> CapabilityResult<Vec<String>> {
        let providers = self.resolve_named(capability)?;
        Ok(providers.into_iter().map(|p| p.package_name).collect())
    }

    /// Check if a specific package satisfies a capability requirement
    pub fn package_satisfies(
        &mut self,
        package_name: &str,
        requirement: &CapabilityRequirement,
    ) -> CapabilityResult<bool> {
        let resolved = self.resolve(requirement)?;
        Ok(resolved
            .providers
            .iter()
            .any(|p| p.package_name == package_name))
    }
}

/// Parse a capability requirement string into a CapabilitySpec
///
/// Formats:
/// - `ssl` → Named("ssl")
/// - `soname(libssl.so.3)` → Typed { kind: "soname", name: "libssl.so.3" }
/// - `network.listen:443` → Network(...)
/// - `filesystem.read:/etc/ssl` → Filesystem(...)
pub fn parse_capability_spec(spec: &str) -> Result<CapabilitySpec, String> {
    // Check for typed capability: kind(name)
    if let Some(paren_pos) = spec.find('(') {
        if spec.ends_with(')') {
            let kind = &spec[..paren_pos];
            let name = &spec[paren_pos + 1..spec.len() - 1];
            return Ok(CapabilitySpec::Typed {
                kind: kind.to_string(),
                name: name.to_string(),
            });
        }
    }

    // Check for network capability: network.listen:port or network.outbound:port
    if let Some(rest) = spec.strip_prefix("network.") {
        if let Some((cap_type, port)) = rest.split_once(':') {
            let cap_type = match cap_type {
                "listen" => NetworkCapType::Listen,
                "outbound" => NetworkCapType::Outbound,
                _ => return Err(format!("Unknown network capability type: {}", cap_type)),
            };
            return Ok(CapabilitySpec::Network(NetworkCapabilitySpec {
                cap_type,
                port: port.to_string(),
            }));
        }
    }

    // Check for filesystem capability: filesystem.read:/path
    if let Some(rest) = spec.strip_prefix("filesystem.") {
        if let Some((cap_type, path)) = rest.split_once(':') {
            let cap_type = match cap_type {
                "read" => FilesystemCapType::Read,
                "write" => FilesystemCapType::Write,
                "execute" => FilesystemCapType::Execute,
                _ => return Err(format!("Unknown filesystem capability type: {}", cap_type)),
            };
            return Ok(CapabilitySpec::Filesystem(FilesystemCapabilitySpec {
                cap_type,
                path: path.to_string(),
            }));
        }
    }

    // Default: named capability
    Ok(CapabilitySpec::Named(spec.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_named_capability() {
        let spec = parse_capability_spec("ssl").unwrap();
        assert_eq!(spec, CapabilitySpec::Named("ssl".to_string()));
    }

    #[test]
    fn test_parse_typed_capability() {
        let spec = parse_capability_spec("soname(libssl.so.3)").unwrap();
        assert_eq!(
            spec,
            CapabilitySpec::Typed {
                kind: "soname".to_string(),
                name: "libssl.so.3".to_string()
            }
        );

        let spec = parse_capability_spec("pkgconfig(openssl)").unwrap();
        assert_eq!(
            spec,
            CapabilitySpec::Typed {
                kind: "pkgconfig".to_string(),
                name: "openssl".to_string()
            }
        );
    }

    #[test]
    fn test_parse_network_capability() {
        let spec = parse_capability_spec("network.listen:443").unwrap();
        assert_eq!(
            spec,
            CapabilitySpec::Network(NetworkCapabilitySpec {
                cap_type: NetworkCapType::Listen,
                port: "443".to_string()
            })
        );

        let spec = parse_capability_spec("network.outbound:https").unwrap();
        assert_eq!(
            spec,
            CapabilitySpec::Network(NetworkCapabilitySpec {
                cap_type: NetworkCapType::Outbound,
                port: "https".to_string()
            })
        );
    }

    #[test]
    fn test_parse_filesystem_capability() {
        let spec = parse_capability_spec("filesystem.read:/etc/ssl").unwrap();
        assert_eq!(
            spec,
            CapabilitySpec::Filesystem(FilesystemCapabilitySpec {
                cap_type: FilesystemCapType::Read,
                path: "/etc/ssl".to_string()
            })
        );

        let spec = parse_capability_spec("filesystem.write:/var/cache").unwrap();
        assert_eq!(
            spec,
            CapabilitySpec::Filesystem(FilesystemCapabilitySpec {
                cap_type: FilesystemCapType::Write,
                path: "/var/cache".to_string()
            })
        );
    }

    #[test]
    fn test_capability_requirement_creation() {
        let req = CapabilityRequirement {
            capability: CapabilitySpec::Named("ssl".to_string()),
            optional: false,
            reason: Some("Needed for HTTPS".to_string()),
        };

        assert!(!req.optional);
        assert_eq!(req.reason, Some("Needed for HTTPS".to_string()));
    }

    #[test]
    fn test_resolver_preferences_default() {
        let prefs = ResolverPreferences::default();
        assert!(prefs.prefer_declared);
        assert!(prefs.prefer_installed);
        assert_eq!(prefs.min_match_score, 50);
    }

    #[test]
    fn test_path_matching() {
        // Create a minimal resolver for testing path matching
        // (We can't easily test database operations without a real DB)
        let patterns_and_paths = vec![
            ("/var/cache", "/var/cache", true),
            ("/var/cache", "/var/cache/nginx", true),
            ("/var/cache/*", "/var/cache/nginx", true),
            ("/var/cache", "/var/log", false),
            ("/etc", "/etc/nginx/nginx.conf", true),
        ];

        for (pattern, path, expected) in patterns_and_paths {
            let matches = path_matches_pattern(pattern, path);
            assert_eq!(
                matches, expected,
                "pattern={}, path={}, expected={}",
                pattern, path, expected
            );
        }
    }

    // Helper for testing without resolver instance
    fn path_matches_pattern(pattern: &str, required: &str) -> bool {
        if pattern == required {
            return true;
        }
        if required.starts_with(pattern) && required[pattern.len()..].starts_with('/') {
            return true;
        }
        if pattern.ends_with("/*") {
            let prefix = &pattern[..pattern.len() - 2];
            if required.starts_with(prefix) {
                return true;
            }
        }
        false
    }

    #[test]
    fn test_port_range_matching() {
        // Test port range parsing
        fn port_in_range(range_spec: &str, port: &str) -> bool {
            if let Some((start, end)) = range_spec.split_once('-') {
                if let (Ok(s), Ok(e), Ok(p)) = (
                    start.parse::<u16>(),
                    end.parse::<u16>(),
                    port.parse::<u16>(),
                ) {
                    return p >= s && p <= e;
                }
            }
            false
        }

        assert!(port_in_range("8080-8090", "8085"));
        assert!(port_in_range("8080-8090", "8080"));
        assert!(port_in_range("8080-8090", "8090"));
        assert!(!port_in_range("8080-8090", "8091"));
        assert!(!port_in_range("8080-8090", "80"));
    }
}
