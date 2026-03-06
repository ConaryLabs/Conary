// conary-core/src/canonical/rules.rs

//! YAML-based canonical mapping rules engine (Repology-compatible format).
//!
//! Rules map distro-specific package names to canonical names. Each rule can match
//! by exact name, regex pattern, and/or repository. Rules are evaluated in order;
//! the first match wins.

use std::path::Path;

use regex::Regex;
use serde::Deserialize;

use crate::{Error, Result};

/// A single canonical mapping rule (Repology-compatible format).
#[derive(Debug, Clone, Deserialize)]
pub struct Rule {
    /// Canonical name to assign when this rule matches.
    #[serde(default)]
    pub setname: String,

    /// Exact distro package name to match.
    #[serde(default)]
    pub name: String,

    /// Regex pattern for name matching (alternative to exact `name`).
    #[serde(default)]
    pub namepat: Option<String>,

    /// Repository to match (e.g., "fedora_43").
    #[serde(default)]
    pub repo: Option<String>,

    /// Kind classification (e.g., "group").
    #[serde(default)]
    pub kind: Option<String>,

    /// Category classification.
    #[serde(default)]
    pub category: Option<String>,
}

/// Parsed YAML document containing a list of rules.
#[derive(Debug, Clone, Deserialize)]
struct RuleFile {
    rules: Vec<Rule>,
}

/// Parse a YAML string into a list of rules.
///
/// The YAML must contain a top-level `rules` key with a list of rule objects.
pub fn parse_rules(yaml: &str) -> Result<Vec<Rule>> {
    let file: RuleFile =
        serde_yaml::from_str(yaml).map_err(|e| Error::ParseError(format!("YAML parse error: {e}")))?;
    Ok(file.rules)
}

/// A compiled rule with pre-compiled regex for efficient matching.
#[derive(Debug, Clone)]
struct CompiledRule {
    rule: Rule,
    name_regex: Option<Regex>,
}

/// Engine that holds compiled rules and resolves package names to canonical names.
#[derive(Debug, Clone)]
pub struct RulesEngine {
    compiled: Vec<CompiledRule>,
}

impl RulesEngine {
    /// Create a new rules engine from a list of rules.
    ///
    /// Regex patterns in `namepat` fields are compiled at construction time.
    pub fn new(rules: Vec<Rule>) -> Result<Self> {
        let mut compiled = Vec::with_capacity(rules.len());
        for rule in rules {
            let name_regex = if let Some(ref pat) = rule.namepat {
                let anchored = anchor_regex(pat);
                let re = Regex::new(&anchored)
                    .map_err(|e| Error::ParseError(format!("Invalid regex '{pat}': {e}")))?;
                Some(re)
            } else {
                None
            };
            compiled.push(CompiledRule { rule, name_regex });
        }
        Ok(Self { compiled })
    }

    /// Load all `.yaml` / `.yml` files from a directory, sorted by filename.
    ///
    /// Rules from files sorted earlier alphabetically take precedence.
    pub fn load_from_dir(dir: &Path) -> Result<Self> {
        let mut entries: Vec<_> = std::fs::read_dir(dir)
            .map_err(|e| Error::IoError(format!("Cannot read directory {}: {e}", dir.display())))?
            .filter_map(|entry| {
                let entry = entry.ok()?;
                let path = entry.path();
                let ext = path.extension()?.to_str()?;
                if ext == "yaml" || ext == "yml" {
                    Some(path)
                } else {
                    None
                }
            })
            .collect();
        entries.sort();

        let mut all_rules = Vec::new();
        for path in entries {
            let content = std::fs::read_to_string(path.as_path()).map_err(|e| {
                Error::IoError(format!("Cannot read {}: {e}", path.display()))
            })?;
            let mut rules = parse_rules(&content)?;
            all_rules.append(&mut rules);
        }

        Self::new(all_rules)
    }

    /// Resolve a distro package name (and optional repo) to a canonical name.
    ///
    /// Rules are evaluated in order; the first match wins. A rule matches if:
    /// - Its `name` matches exactly (or `name` is empty and `namepat` is used)
    /// - Its `namepat` regex matches the package name (if present)
    /// - Its `repo` matches the given repo (if the rule specifies a repo)
    pub fn resolve(&self, name: &str, repo: Option<&str>) -> Option<String> {
        for compiled in &self.compiled {
            let rule = &compiled.rule;

            // Skip rules that are kind-only (no name/namepat to match against).
            if rule.name.is_empty() && rule.namepat.is_none() {
                continue;
            }

            // Check repo constraint.
            if let Some(ref rule_repo) = rule.repo {
                match repo {
                    Some(r) if r == rule_repo => {}
                    _ => continue,
                }
            }

            // Check name match (exact or regex).
            let name_matches = if let Some(ref re) = compiled.name_regex {
                re.is_match(name)
            } else if !rule.name.is_empty() {
                rule.name == name
            } else {
                false
            };

            if name_matches && !rule.setname.is_empty() {
                // Expand regex capture groups ($1, $2, ...) in setname
                if let Some(ref re) = compiled.name_regex
                    && let Some(caps) = re.captures(name)
                {
                    let mut result = rule.setname.clone();
                    for i in 1..caps.len() {
                        if let Some(m) = caps.get(i) {
                            result = result.replace(&format!("${i}"), m.as_str());
                        }
                    }
                    return Some(result);
                }
                return Some(rule.setname.clone());
            }
        }
        None
    }

    /// Get the kind for a canonical name (e.g., "group").
    ///
    /// Searches rules where `setname` matches the given canonical name
    /// and returns the first `kind` found.
    pub fn get_kind(&self, canonical_name: &str) -> Option<String> {
        for compiled in &self.compiled {
            let rule = &compiled.rule;
            if rule.setname == canonical_name
                && let Some(ref kind) = rule.kind
            {
                return Some(kind.clone());
            }
        }
        None
    }

    /// Return the total number of rules loaded.
    pub fn rule_count(&self) -> usize {
        self.compiled.len()
    }

    /// Iterate over the raw rules.
    pub fn rules(&self) -> impl Iterator<Item = &Rule> {
        self.compiled.iter().map(|c| &c.rule)
    }
}

/// Ensure a regex pattern is anchored at both ends.
///
/// Unanchored patterns could match substrings, leading to unexpected
/// canonical name mappings. This adds `^` and `$` if not already present.
fn anchor_regex(pat: &str) -> String {
    let needs_start = !pat.starts_with('^');
    let needs_end = !pat.ends_with('$');
    match (needs_start, needs_end) {
        (true, true) => format!("^(?:{pat})$"),
        (true, false) => format!("^(?:{pat})"),
        (false, true) => format!("(?:{pat})$"),
        (false, false) => pat.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_rename_rule() {
        let yaml = r#"
rules:
  - name: httpd
    setname: apache-httpd
"#;
        let rules = parse_rules(yaml).unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].name, "httpd");
        assert_eq!(rules[0].setname, "apache-httpd");
        assert!(rules[0].namepat.is_none());
        assert!(rules[0].repo.is_none());
    }

    #[test]
    fn test_parse_group_rule() {
        let yaml = r#"
rules:
  - setname: xorg
    kind: group
    category: x11
"#;
        let rules = parse_rules(yaml).unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].setname, "xorg");
        assert_eq!(rules[0].kind.as_deref(), Some("group"));
        assert_eq!(rules[0].category.as_deref(), Some("x11"));
    }

    #[test]
    fn test_parse_wildcard_rule() {
        let yaml = r#"
rules:
  - namepat: "^python3?-(.+)$"
    setname: "python:$1"
"#;
        let rules = parse_rules(yaml).unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].namepat.as_deref(), Some("^python3?-(.+)$"));
        // The regex should compile without error.
        let engine = RulesEngine::new(rules).unwrap();
        assert_eq!(engine.rule_count(), 1);
    }

    #[test]
    fn test_apply_rules() {
        let yaml = r#"
rules:
  - name: httpd
    setname: apache-httpd
  - name: apache2
    setname: apache-httpd
  - namepat: "^lib(.+)-dev$"
    setname: "$1"
"#;
        let engine = RulesEngine::new(parse_rules(yaml).unwrap()).unwrap();

        assert_eq!(
            engine.resolve("httpd", None),
            Some("apache-httpd".to_string())
        );
        assert_eq!(
            engine.resolve("apache2", None),
            Some("apache-httpd".to_string())
        );
        // curl has no matching rule.
        assert_eq!(engine.resolve("curl", None), None);
        // Regex capture group expansion: libssl-dev -> "ssl"
        assert_eq!(
            engine.resolve("libssl-dev", None),
            Some("ssl".to_string())
        );
    }

    #[test]
    fn test_load_rules_from_dir() {
        let dir = tempfile::tempdir().unwrap();

        // Write two YAML files; they should be loaded in alphabetical order.
        std::fs::write(
            dir.path().join("01-rename.yaml"),
            r#"
rules:
  - name: vim-enhanced
    setname: vim
"#,
        )
        .unwrap();

        std::fs::write(
            dir.path().join("02-groups.yml"),
            r#"
rules:
  - name: nginx
    setname: nginx
    kind: group
"#,
        )
        .unwrap();

        // Also write a non-YAML file that should be ignored.
        std::fs::write(dir.path().join("README.txt"), "not a rule file").unwrap();

        let engine = RulesEngine::load_from_dir(dir.path()).unwrap();
        assert_eq!(engine.rule_count(), 2);
        assert_eq!(
            engine.resolve("vim-enhanced", None),
            Some("vim".to_string())
        );
        assert_eq!(
            engine.resolve("nginx", None),
            Some("nginx".to_string())
        );
        assert_eq!(engine.get_kind("nginx"), Some("group".to_string()));
    }
}
