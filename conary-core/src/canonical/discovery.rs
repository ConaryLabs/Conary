// conary-core/src/canonical/discovery.rs

//! Multi-strategy auto-discovery for cross-distro canonical package mappings.
//!
//! This module implements several heuristic strategies that discover equivalent
//! packages across different distributions by analyzing names, provides,
//! installed binaries, shared libraries, and name stems.

use std::collections::HashMap;

/// A package from a specific distribution, carrying its metadata for discovery.
#[derive(Debug, Clone)]
pub struct DistroPackage {
    /// The distro-specific package name (e.g., "httpd", "apache2").
    pub name: String,
    /// The distribution identifier (e.g., "fedora", "debian").
    pub distro: String,
    /// Virtual provides / capabilities declared by this package.
    pub provides: Vec<String>,
    /// File paths installed by this package.
    pub files: Vec<String>,
}

/// A discovered cross-distro mapping with its provenance.
#[derive(Debug, Clone)]
pub struct DiscoveredMapping {
    /// The canonical name chosen for this group.
    pub canonical_name: String,
    /// (distro, distro_name) pairs that map to this canonical name.
    pub implementations: Vec<(String, String)>,
    /// The strategy that produced this mapping.
    pub source: String,
}

/// Group packages with identical names across different distros.
///
/// If two or more distros ship a package with the same name, that name
/// becomes the canonical name.
pub fn discover_by_name_match(packages: &[DistroPackage]) -> Vec<DiscoveredMapping> {
    // name -> distro -> package_name (in this strategy they are the same)
    let mut by_name: HashMap<&str, Vec<(&str, &str)>> = HashMap::new();
    for pkg in packages {
        by_name
            .entry(pkg.name.as_str())
            .or_default()
            .push((pkg.distro.as_str(), pkg.name.as_str()));
    }

    let mut results = Vec::new();
    for (name, entries) in &by_name {
        // Need packages from at least 2 different distros.
        let mut distros: Vec<&str> = entries.iter().map(|(d, _)| *d).collect();
        distros.sort_unstable();
        distros.dedup();
        if distros.len() >= 2 {
            results.push(DiscoveredMapping {
                canonical_name: (*name).to_string(),
                implementations: entries
                    .iter()
                    .map(|(d, n)| ((*d).to_string(), (*n).to_string()))
                    .collect(),
                source: "name_match".to_string(),
            });
        }
    }
    results.sort_by(|a, b| a.canonical_name.cmp(&b.canonical_name));
    results
}

/// Group packages that provide the same capability across different distros.
///
/// If packages from different distros declare the same provides entry,
/// that provides value becomes the canonical name.
pub fn discover_by_provides(packages: &[DistroPackage]) -> Vec<DiscoveredMapping> {
    // provides_value -> Vec<(distro, package_name)>
    let mut by_provides: HashMap<&str, Vec<(&str, &str)>> = HashMap::new();
    for pkg in packages {
        for prov in &pkg.provides {
            by_provides
                .entry(prov.as_str())
                .or_default()
                .push((pkg.distro.as_str(), pkg.name.as_str()));
        }
    }

    let mut results = Vec::new();
    for (prov, entries) in &by_provides {
        let mut distros: Vec<&str> = entries.iter().map(|(d, _)| *d).collect();
        distros.sort_unstable();
        distros.dedup();
        if distros.len() >= 2 {
            results.push(DiscoveredMapping {
                canonical_name: (*prov).to_string(),
                implementations: entries
                    .iter()
                    .map(|(d, n)| ((*d).to_string(), (*n).to_string()))
                    .collect(),
                source: "provides".to_string(),
            });
        }
    }
    results.sort_by(|a, b| a.canonical_name.cmp(&b.canonical_name));
    results
}

/// Group packages that install the same binary in `/usr/bin/` or `/usr/sbin/`.
///
/// The binary basename becomes the canonical name.
pub fn discover_by_binary_path(packages: &[DistroPackage]) -> Vec<DiscoveredMapping> {
    // binary_name -> Vec<(distro, package_name)>
    let mut by_binary: HashMap<String, Vec<(&str, &str)>> = HashMap::new();
    for pkg in packages {
        for file in &pkg.files {
            if let Some(bin_name) = extract_binary_name(file) {
                by_binary
                    .entry(bin_name)
                    .or_default()
                    .push((pkg.distro.as_str(), pkg.name.as_str()));
            }
        }
    }

    let mut results = Vec::new();
    for (bin, entries) in &by_binary {
        let mut distros: Vec<&str> = entries.iter().map(|(d, _)| *d).collect();
        distros.sort_unstable();
        distros.dedup();
        if distros.len() >= 2 {
            results.push(DiscoveredMapping {
                canonical_name: bin.clone(),
                implementations: entries
                    .iter()
                    .map(|(d, n)| ((*d).to_string(), (*n).to_string()))
                    .collect(),
                source: "binary_path".to_string(),
            });
        }
    }
    results.sort_by(|a, b| a.canonical_name.cmp(&b.canonical_name));
    results
}

/// Extract the binary name from a path under `/usr/bin/` or `/usr/sbin/`.
fn extract_binary_name(path: &str) -> Option<String> {
    for prefix in &["/usr/bin/", "/usr/sbin/", "/bin/", "/sbin/"] {
        if let Some(rest) = path.strip_prefix(prefix) {
            // Only top-level binaries, not nested paths.
            if !rest.is_empty() && !rest.contains('/') {
                return Some(rest.to_string());
            }
        }
    }
    None
}

/// Group packages that provide the same `.so` shared library.
///
/// The soname (without version suffix) becomes the canonical name.
pub fn discover_by_soname(packages: &[DistroPackage]) -> Vec<DiscoveredMapping> {
    // soname -> Vec<(distro, package_name)>
    let mut by_soname: HashMap<String, Vec<(&str, &str)>> = HashMap::new();
    for pkg in packages {
        for file in &pkg.files {
            if let Some(soname) = extract_soname(file) {
                by_soname
                    .entry(soname)
                    .or_default()
                    .push((pkg.distro.as_str(), pkg.name.as_str()));
            }
        }
    }

    let mut results = Vec::new();
    for (soname, entries) in &by_soname {
        let mut distros: Vec<&str> = entries.iter().map(|(d, _)| *d).collect();
        distros.sort_unstable();
        distros.dedup();
        if distros.len() >= 2 {
            results.push(DiscoveredMapping {
                canonical_name: soname.clone(),
                implementations: entries
                    .iter()
                    .map(|(d, n)| ((*d).to_string(), (*n).to_string()))
                    .collect(),
                source: "soname".to_string(),
            });
        }
    }
    results.sort_by(|a, b| a.canonical_name.cmp(&b.canonical_name));
    results
}

/// Extract a normalized soname from a library path.
///
/// For example, `/usr/lib64/libcurl.so.4.8.0` yields `libcurl.so`.
fn extract_soname(path: &str) -> Option<String> {
    let filename = path.rsplit('/').next()?;
    // Must contain ".so" to be a shared library.
    let so_pos = filename.find(".so")?;
    // Return the name up to and including ".so".
    Some(filename[..so_pos + 3].to_string())
}

/// Strip common distro-specific affixes from a package name.
///
/// Removes `lib` prefix and `-dev`, `-devel`, `-libs`, `-common`, `-doc`,
/// `-tools` suffixes to produce a normalized stem.
pub fn strip_distro_affixes(name: &str) -> String {
    let mut s = name.to_string();

    // Strip common suffixes (order matters: longest first).
    let suffixes = ["-devel", "-common", "-tools", "-libs", "-dev", "-doc"];
    for suffix in &suffixes {
        if let Some(stripped) = s.strip_suffix(suffix) {
            s = stripped.to_string();
            break;
        }
    }

    // Strip lib prefix.
    if let Some(stripped) = s.strip_prefix("lib")
        && !stripped.is_empty()
    {
        s = stripped.to_string();
    }

    s
}

/// Group packages whose stripped stems match across different distros.
///
/// This is a fuzzy strategy: after stripping common affixes, packages that
/// reduce to the same stem are grouped together.
pub fn discover_by_stem(packages: &[DistroPackage]) -> Vec<DiscoveredMapping> {
    // stem -> Vec<(distro, package_name)>
    let mut by_stem: HashMap<String, Vec<(&str, &str)>> = HashMap::new();
    for pkg in packages {
        let stem = strip_distro_affixes(&pkg.name);
        by_stem
            .entry(stem)
            .or_default()
            .push((pkg.distro.as_str(), pkg.name.as_str()));
    }

    let mut results = Vec::new();
    for (stem, entries) in &by_stem {
        let mut distros: Vec<&str> = entries.iter().map(|(d, _)| *d).collect();
        distros.sort_unstable();
        distros.dedup();
        if distros.len() >= 2 {
            results.push(DiscoveredMapping {
                canonical_name: stem.clone(),
                implementations: entries
                    .iter()
                    .map(|(d, n)| ((*d).to_string(), (*n).to_string()))
                    .collect(),
                source: "stem".to_string(),
            });
        }
    }
    results.sort_by(|a, b| a.canonical_name.cmp(&b.canonical_name));
    results
}

/// Run all discovery strategies and merge/deduplicate results.
///
/// Priority order (earlier strategies win for the same canonical name):
/// 1. Name match (highest confidence)
/// 2. Provides
/// 3. Binary path
/// 4. Soname
/// 5. Stem (lowest confidence)
pub fn run_discovery(packages: &[DistroPackage]) -> Vec<DiscoveredMapping> {
    let strategies: Vec<Vec<DiscoveredMapping>> = vec![
        discover_by_name_match(packages),
        discover_by_provides(packages),
        discover_by_binary_path(packages),
        discover_by_soname(packages),
        discover_by_stem(packages),
    ];

    // Deduplicate: keep the first occurrence of each canonical name
    // (from the highest-priority strategy).
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut merged = Vec::new();

    for strategy_results in strategies {
        for mapping in strategy_results {
            if let std::collections::hash_map::Entry::Vacant(e) =
                seen.entry(mapping.canonical_name.clone())
            {
                e.insert(merged.len());
                merged.push(mapping);
            }
        }
    }

    merged.sort_by(|a, b| a.canonical_name.cmp(&b.canonical_name));
    merged
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_pkg(name: &str, distro: &str, provides: &[&str], files: &[&str]) -> DistroPackage {
        DistroPackage {
            name: name.to_string(),
            distro: distro.to_string(),
            provides: provides.iter().map(|s| (*s).to_string()).collect(),
            files: files.iter().map(|s| (*s).to_string()).collect(),
        }
    }

    #[test]
    fn test_name_match_strategy() {
        let packages = vec![
            make_pkg("curl", "fedora", &[], &[]),
            make_pkg("curl", "debian", &[], &[]),
            make_pkg("wget", "fedora", &[], &[]),
        ];
        let results = discover_by_name_match(&packages);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].canonical_name, "curl");
        assert_eq!(results[0].source, "name_match");
        assert_eq!(results[0].implementations.len(), 2);
    }

    #[test]
    fn test_provides_strategy() {
        let packages = vec![
            make_pkg("httpd", "fedora", &["webserver"], &[]),
            make_pkg("apache2", "debian", &["webserver"], &[]),
            make_pkg("nginx", "fedora", &["webserver"], &[]),
        ];
        let results = discover_by_provides(&packages);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].canonical_name, "webserver");
        assert_eq!(results[0].source, "provides");
        assert_eq!(results[0].implementations.len(), 3);
    }

    #[test]
    fn test_binary_path_strategy() {
        let packages = vec![
            make_pkg("httpd", "fedora", &[], &["/usr/sbin/httpd"]),
            make_pkg("apache2", "debian", &[], &["/usr/sbin/httpd"]),
        ];
        let results = discover_by_binary_path(&packages);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].canonical_name, "httpd");
        assert_eq!(results[0].source, "binary_path");
    }

    #[test]
    fn test_stem_match_strategy() {
        // libcurl-devel (fedora) and libcurl-dev (debian) should both stem to "curl"
        let packages = vec![
            make_pkg("libcurl-devel", "fedora", &[], &[]),
            make_pkg("libcurl-dev", "debian", &[], &[]),
        ];
        let results = discover_by_stem(&packages);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].canonical_name, "curl");
        assert_eq!(results[0].source, "stem");

        // Verify the affix stripping itself.
        assert_eq!(strip_distro_affixes("libcurl-devel"), "curl");
        assert_eq!(strip_distro_affixes("libcurl-dev"), "curl");
        assert_eq!(strip_distro_affixes("openssl-libs"), "openssl");
        assert_eq!(strip_distro_affixes("zlib-doc"), "zlib");
    }

    #[test]
    fn test_soname_strategy() {
        let packages = vec![
            make_pkg(
                "libcurl",
                "fedora",
                &[],
                &["/usr/lib64/libcurl.so.4.8.0"],
            ),
            make_pkg(
                "libcurl4",
                "debian",
                &[],
                &["/usr/lib/x86_64-linux-gnu/libcurl.so.4.8.0"],
            ),
        ];
        let results = discover_by_soname(&packages);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].canonical_name, "libcurl.so");
        assert_eq!(results[0].source, "soname");
    }

    #[test]
    fn test_full_discovery_pipeline() {
        let packages = vec![
            // "curl" appears in both distros by name -- name_match wins.
            make_pkg("curl", "fedora", &["http-client"], &["/usr/bin/curl"]),
            make_pkg("curl", "debian", &["http-client"], &["/usr/bin/curl"]),
            // "httpd" vs "apache2" -- only discoverable via provides or binary.
            make_pkg(
                "httpd",
                "fedora",
                &["webserver"],
                &["/usr/sbin/httpd"],
            ),
            make_pkg(
                "apache2",
                "debian",
                &["webserver"],
                &["/usr/sbin/httpd"],
            ),
        ];

        let results = run_discovery(&packages);

        // "curl" should be found by name_match (highest priority).
        let curl = results.iter().find(|m| m.canonical_name == "curl");
        assert!(curl.is_some());
        assert_eq!(curl.unwrap().source, "name_match");

        // "webserver" from provides and "httpd" from binary_path should both appear
        // (they have different canonical names, so no dedup conflict).
        let webserver = results.iter().find(|m| m.canonical_name == "webserver");
        assert!(webserver.is_some());
        assert_eq!(webserver.unwrap().source, "provides");

        let httpd_bin = results.iter().find(|m| {
            m.canonical_name == "httpd" && m.source == "binary_path"
        });
        assert!(httpd_bin.is_some());

        // "http-client" from provides should appear (curl provides it from 2 distros).
        let http_client = results.iter().find(|m| m.canonical_name == "http-client");
        assert!(http_client.is_some());
        assert_eq!(http_client.unwrap().source, "provides");

        // Verify no duplicate canonical names.
        let mut names: Vec<&str> = results.iter().map(|m| m.canonical_name.as_str()).collect();
        let original_len = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), original_len, "No duplicate canonical names");
    }
}
