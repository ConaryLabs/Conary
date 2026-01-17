// src/recipe/pkgbuild.rs

//! PKGBUILD to Recipe converter
//!
//! Converts Arch Linux PKGBUILD files to Conary Recipe format.
//!
//! # PKGBUILD Format
//!
//! PKGBUILDs are Bash scripts with specific variables and functions:
//!
//! ```bash
//! pkgname=nano
//! pkgver=8.5
//! pkgrel=2
//! pkgdesc="A small and friendly text editor"
//! url="https://www.nano-editor.org"
//! license=('GPL')
//! depends=('ncurses')
//! makedepends=('gcc')
//! source=("https://nano-editor.org/dist/v${pkgver%.*}/nano-$pkgver.tar.xz")
//! sha256sums=('abc123...')
//!
//! build() {
//!     cd "$pkgname-$pkgver"
//!     ./configure --prefix=/usr
//!     make
//! }
//!
//! package() {
//!     cd "$pkgname-$pkgver"
//!     make DESTDIR="$pkgdir" install
//! }
//! ```
//!
//! # Limitations
//!
//! - Only basic variable extraction is supported
//! - Complex Bash expressions are simplified
//! - Split packages (pkgname=(...)) are not supported
//! - VCS packages (-git, -svn) need manual adjustment

use crate::recipe::format::{BuildSection, PatchInfo, PatchSection, Recipe, PackageSection, SourceSection};
use std::collections::HashMap;
use regex::Regex;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum PkgbuildError {
    #[error("Missing required variable: {0}")]
    MissingVariable(String),

    #[error("Invalid PKGBUILD: {0}")]
    Invalid(String),

    #[error("Unsupported feature: {0}")]
    Unsupported(String),

    #[error("Parse error: {0}")]
    ParseError(String),
}

/// Result of PKGBUILD conversion
pub struct ConversionResult {
    /// The converted recipe
    pub recipe: Recipe,
    /// Warnings about conversion issues
    pub warnings: Vec<String>,
}

/// Convert a PKGBUILD to a Recipe
///
/// # Arguments
/// * `content` - The PKGBUILD file content
///
/// # Returns
/// A ConversionResult containing the recipe and any warnings
pub fn convert_pkgbuild(content: &str) -> Result<ConversionResult, PkgbuildError> {
    let mut warnings = Vec::new();
    let vars = extract_variables(content)?;
    let functions = extract_functions(content);

    // Required variables
    let pkgname = vars.get("pkgname")
        .ok_or_else(|| PkgbuildError::MissingVariable("pkgname".to_string()))?
        .clone();

    let pkgver = vars.get("pkgver")
        .ok_or_else(|| PkgbuildError::MissingVariable("pkgver".to_string()))?
        .clone();

    let pkgrel = vars.get("pkgrel")
        .cloned()
        .unwrap_or_else(|| "1".to_string());

    // Check for split packages
    if pkgname.starts_with('(') {
        return Err(PkgbuildError::Unsupported("Split packages (pkgname=(...)) are not supported".to_string()));
    }

    // Optional variables
    let pkgdesc = vars.get("pkgdesc").cloned();
    let url = vars.get("url").cloned();
    let license = vars.get("license").cloned();

    // Extract source URL and checksum
    let sources = extract_array(content, "source")
        .ok_or_else(|| PkgbuildError::MissingVariable("source".to_string()))?;
    let checksums = extract_array(content, "sha256sums")
        .or_else(|| extract_array(content, "sha512sums"))
        .or_else(|| extract_array(content, "b2sums"))
        .or_else(|| extract_array(content, "md5sums"));

    if sources.is_empty() {
        return Err(PkgbuildError::MissingVariable("source".to_string()));
    }

    let source_url = convert_pkgbuild_url(&sources[0], &pkgname, &pkgver);
    let checksum = checksums.as_ref()
        .and_then(|c| c.first())
        .map(|s| format!("sha256:{}", s))
        .unwrap_or_else(|| {
            warnings.push("No checksum found, using SKIP".to_string());
            "SKIP".to_string()
        });

    // Handle additional sources
    let additional_sources: Vec<_> = sources.iter().skip(1)
        .enumerate()
        .map(|(i, url)| {
            let cs = checksums.as_ref()
                .and_then(|c| c.get(i + 1))
                .map(|s| format!("sha256:{}", s))
                .unwrap_or_else(|| "SKIP".to_string());
            crate::recipe::format::AdditionalSource {
                url: convert_pkgbuild_url(url, &pkgname, &pkgver),
                checksum: cs,
                extract_to: None,
            }
        })
        .collect();

    // Extract dependencies
    let depends = extract_array(content, "depends").unwrap_or_default();
    let makedepends = extract_array(content, "makedepends").unwrap_or_default();

    let mut build_requires: Vec<String> = makedepends.iter()
        .map(|d| d.split(|c| c == '>' || c == '<' || c == '=').next().unwrap_or(d).to_string())
        .collect();

    // Add runtime deps to build requires too (they're often needed)
    for dep in &depends {
        let clean_dep = dep.split(|c| c == '>' || c == '<' || c == '=').next().unwrap_or(dep).to_string();
        if !build_requires.contains(&clean_dep) {
            build_requires.push(clean_dep);
        }
    }

    // Convert build functions to commands
    let build_cmd = functions.get("build").map(|f| convert_function_body(f, &pkgname, &pkgver));
    let package_cmd = functions.get("package").map(|f| convert_function_body(f, &pkgname, &pkgver));
    let prepare_cmd = functions.get("prepare").map(|f| convert_function_body(f, &pkgname, &pkgver));
    let check_cmd = functions.get("check").map(|f| convert_function_body(f, &pkgname, &pkgver));

    // Try to split build into configure and make
    let (configure, make) = if let Some(ref build) = build_cmd {
        split_build_commands(build)
    } else {
        (None, None)
    };

    // Convert package function to install
    let install = package_cmd.map(|cmd| {
        // Replace $pkgdir with %(destdir)s
        cmd.replace("$pkgdir", "%(destdir)s")
           .replace("${pkgdir}", "%(destdir)s")
    });

    // Detect patches from source array
    let patches: Vec<PatchInfo> = sources.iter()
        .enumerate()
        .filter(|(_, s)| s.ends_with(".patch") || s.ends_with(".diff"))
        .map(|(i, s)| PatchInfo {
            file: convert_pkgbuild_url(s, &pkgname, &pkgver),
            checksum: checksums.as_ref()
                .and_then(|c| c.get(i))
                .map(|cs| format!("sha256:{}", cs)),
            strip: 1,
            condition: None,
        })
        .collect();

    let patch_section = if patches.is_empty() {
        None
    } else {
        Some(PatchSection { files: patches })
    };

    // Build the recipe
    let recipe = Recipe {
        package: PackageSection {
            name: pkgname.clone(),
            version: pkgver.clone(),
            release: pkgrel,
            summary: pkgdesc.clone(),
            description: pkgdesc,
            license,
            homepage: url,
        },
        source: SourceSection {
            archive: source_url,
            checksum,
            signature: None,
            additional: additional_sources,
            extract_dir: None,
        },
        build: BuildSection {
            requires: build_requires,
            makedepends: Vec::new(), // PKGBUILD makedepends handled separately
            configure,
            make,
            install,
            check: check_cmd,
            setup: prepare_cmd,
            post_install: None,
            environment: HashMap::new(),
            workdir: Some(format!("{}-{}", pkgname, pkgver)),
            script_file: None,
            jobs: None,
        },
        cross: None, // PKGBUILD doesn't support cross-compilation
        patches: patch_section,
        components: None,
        variables: HashMap::new(),
    };

    // Add warnings for unsupported features
    if content.contains("pkgbase=") {
        warnings.push("pkgbase detected - split package support is limited".to_string());
    }
    if content.contains("-git") || content.contains("-svn") || content.contains("-hg") {
        warnings.push("VCS package detected - source URL may need manual adjustment".to_string());
    }
    if functions.contains_key("pkgver") {
        warnings.push("Dynamic pkgver() function detected - version may need manual update".to_string());
    }

    Ok(ConversionResult { recipe, warnings })
}

/// Extract simple variable assignments from PKGBUILD
fn extract_variables(content: &str) -> Result<HashMap<String, String>, PkgbuildError> {
    let mut vars = HashMap::new();

    // Match: varname=value or varname="value" or varname='value'
    let re = Regex::new(r#"^([a-zA-Z_][a-zA-Z0-9_]*)=["']?([^"'\n]*)["']?\s*$"#)
        .map_err(|e| PkgbuildError::ParseError(e.to_string()))?;

    for line in content.lines() {
        let line = line.trim();
        if let Some(caps) = re.captures(line) {
            let name = caps.get(1).unwrap().as_str().to_string();
            let value = caps.get(2).unwrap().as_str().to_string();
            vars.insert(name, value);
        }
    }

    Ok(vars)
}

/// Extract array values from PKGBUILD
fn extract_array(content: &str, name: &str) -> Option<Vec<String>> {
    // Match: name=('value1' 'value2' ...) or name=("value1" "value2" ...)
    let pattern = format!(r#"{}=\(([^)]*)\)"#, regex::escape(name));
    let re = Regex::new(&pattern).ok()?;

    if let Some(caps) = re.captures(content) {
        let array_content = caps.get(1)?.as_str();
        // Extract quoted values
        let value_re = Regex::new(r#"["']([^"']+)["']"#).ok()?;
        let values: Vec<String> = value_re.captures_iter(array_content)
            .filter_map(|c| c.get(1))
            .map(|m| m.as_str().to_string())
            .collect();

        if values.is_empty() {
            // Try unquoted values
            let values: Vec<String> = array_content.split_whitespace()
                .map(|s| s.trim_matches(|c| c == '"' || c == '\'').to_string())
                .filter(|s| !s.is_empty())
                .collect();
            if !values.is_empty() {
                return Some(values);
            }
        }
        return Some(values);
    }

    None
}

/// Extract function bodies from PKGBUILD
fn extract_functions(content: &str) -> HashMap<String, String> {
    let mut functions = HashMap::new();

    // Simple function extraction - looks for function_name() { ... }
    let fn_re = Regex::new(r#"(?m)^(\w+)\(\)\s*\{"#).unwrap();

    for caps in fn_re.captures_iter(content) {
        let fn_name = caps.get(1).unwrap().as_str();
        let start = caps.get(0).unwrap().end();

        // Find matching closing brace (simple nesting)
        let rest = &content[start..];
        let mut depth = 1;
        let mut end = 0;
        for (i, c) in rest.chars().enumerate() {
            match c {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = i;
                        break;
                    }
                }
                _ => {}
            }
        }

        if end > 0 {
            let body = rest[..end].trim().to_string();
            functions.insert(fn_name.to_string(), body);
        }
    }

    functions
}

/// Convert PKGBUILD URL format to Recipe format
fn convert_pkgbuild_url(url: &str, pkgname: &str, pkgver: &str) -> String {
    // Replace $pkgname, ${pkgname}, $pkgver, ${pkgver}
    let url = url.replace("$pkgname", "%(name)s")
        .replace("${pkgname}", "%(name)s")
        .replace("$pkgver", "%(version)s")
        .replace("${pkgver}", "%(version)s");

    // Handle ${pkgver%.*} (version without last component)
    // This is common but hard to replicate exactly, so we just use full version
    let url = url.replace("${pkgver%.*}", &pkgver.rsplit('.').skip(1).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("."));

    // Replace actual values with placeholders where they appear literally
    let url = url.replace(pkgname, "%(name)s");
    let url = url.replace(pkgver, "%(version)s");

    url
}

/// Convert function body to shell commands for recipe
fn convert_function_body(body: &str, pkgname: &str, pkgver: &str) -> String {
    body.replace("$pkgname", &pkgname)
        .replace("${pkgname}", &pkgname)
        .replace("$pkgver", &pkgver)
        .replace("${pkgver}", &pkgver)
        .replace("$srcdir", ".")
        .replace("${srcdir}", ".")
}

/// Try to split build commands into configure and make steps
fn split_build_commands(build: &str) -> (Option<String>, Option<String>) {
    let lines: Vec<&str> = build.lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty() && !l.starts_with('#') && !l.starts_with("cd "))
        .collect();

    // Look for configure command
    let configure_idx = lines.iter().position(|l|
        l.contains("./configure") ||
        l.contains("cmake") ||
        l.contains("meson")
    );

    // Look for make command
    let make_idx = lines.iter().position(|l|
        l.starts_with("make") ||
        l.starts_with("ninja") ||
        l.contains("cmake --build")
    );

    match (configure_idx, make_idx) {
        (Some(c), Some(m)) if c < m => {
            (Some(lines[c].to_string()), Some(lines[m..].join("\n")))
        }
        (Some(c), None) => {
            (Some(lines[c].to_string()), None)
        }
        (None, Some(m)) => {
            (None, Some(lines[m..].join("\n")))
        }
        _ => {
            // Can't split, return everything as make
            (None, Some(build.to_string()))
        }
    }
}

/// Convert a PKGBUILD file to Recipe format and return as TOML string
pub fn pkgbuild_to_toml(pkgbuild_content: &str) -> Result<String, PkgbuildError> {
    let result = convert_pkgbuild(pkgbuild_content)?;
    toml::to_string_pretty(&result.recipe)
        .map_err(|e| PkgbuildError::ParseError(format!("Failed to serialize recipe: {}", e)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_variables() {
        let content = r#"
pkgname=nano
pkgver=8.5
pkgrel=2
pkgdesc="A small text editor"
url='https://nano-editor.org'
"#;
        let vars = extract_variables(content).unwrap();
        assert_eq!(vars.get("pkgname"), Some(&"nano".to_string()));
        assert_eq!(vars.get("pkgver"), Some(&"8.5".to_string()));
        assert_eq!(vars.get("pkgrel"), Some(&"2".to_string()));
    }

    #[test]
    fn test_extract_array() {
        let content = r#"
depends=('ncurses' 'file')
makedepends=("gcc")
"#;
        let depends = extract_array(content, "depends").unwrap();
        assert_eq!(depends, vec!["ncurses", "file"]);

        let makedepends = extract_array(content, "makedepends").unwrap();
        assert_eq!(makedepends, vec!["gcc"]);
    }

    #[test]
    fn test_convert_pkgbuild_url() {
        let url = "https://example.com/${pkgname}-${pkgver}.tar.gz";
        let converted = convert_pkgbuild_url(url, "nano", "8.5");
        assert!(converted.contains("%(name)s") || converted.contains("%(version)s"));
    }

    #[test]
    fn test_convert_simple_pkgbuild() {
        let pkgbuild = r#"
pkgname=hello
pkgver=1.0
pkgrel=1
pkgdesc="Hello World"
url="https://example.com"
license=('GPL')
depends=('glibc')
source=("https://example.com/${pkgname}-${pkgver}.tar.gz")
sha256sums=('abc123')

build() {
    cd "$pkgname-$pkgver"
    ./configure --prefix=/usr
    make
}

package() {
    cd "$pkgname-$pkgver"
    make DESTDIR="$pkgdir" install
}
"#;
        let result = convert_pkgbuild(pkgbuild).unwrap();
        assert_eq!(result.recipe.package.name, "hello");
        assert_eq!(result.recipe.package.version, "1.0");
        assert!(result.recipe.build.install.is_some());
    }
}
