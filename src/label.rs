// src/label.rs

//! Conary-style label system for tracking package provenance
//!
//! Labels identify where a package came from using the format:
//! `repository@namespace:tag`
//!
//! Examples:
//! - `conary.example.com@rpl:2` - rPath Linux 2 from conary.example.com
//! - `fedora.conary.io@fc:41` - Fedora 41 packages
//! - `local@devel:main` - Local development branch
//!
//! # Label Components
//!
//! - **Repository**: The hostname or identifier of the package source
//! - **Namespace**: A grouping within the repository (e.g., project name)
//! - **Tag**: The branch or version identifier
//!
//! # Label Path
//!
//! A label path is an ordered list of labels that defines the search order
//! when resolving package dependencies. Higher priority labels are searched first.

use std::fmt;
use std::str::FromStr;

/// A Conary-style label identifying package provenance
///
/// Format: `repository@namespace:tag`
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Label {
    /// Repository hostname or identifier
    pub repository: String,
    /// Namespace within the repository
    pub namespace: String,
    /// Branch or version tag
    pub tag: String,
}

impl Label {
    /// Create a new label
    pub fn new(repository: impl Into<String>, namespace: impl Into<String>, tag: impl Into<String>) -> Self {
        Self {
            repository: repository.into(),
            namespace: namespace.into(),
            tag: tag.into(),
        }
    }

    /// Parse a label from string format `repository@namespace:tag`
    pub fn parse(s: &str) -> Result<Self, LabelParseError> {
        // Find the @ separator
        let at_pos = s.find('@').ok_or_else(|| LabelParseError::MissingAt(s.to_string()))?;

        // Find the : separator after @
        let colon_pos = s[at_pos..].find(':')
            .map(|p| at_pos + p)
            .ok_or_else(|| LabelParseError::MissingColon(s.to_string()))?;

        let repository = &s[..at_pos];
        let namespace = &s[at_pos + 1..colon_pos];
        let tag = &s[colon_pos + 1..];

        // Validate components are not empty
        if repository.is_empty() {
            return Err(LabelParseError::EmptyRepository(s.to_string()));
        }
        if namespace.is_empty() {
            return Err(LabelParseError::EmptyNamespace(s.to_string()));
        }
        if tag.is_empty() {
            return Err(LabelParseError::EmptyTag(s.to_string()));
        }

        // Validate characters (alphanumeric, dots, hyphens, underscores)
        let valid_chars = |c: char| c.is_alphanumeric() || c == '.' || c == '-' || c == '_';

        if !repository.chars().all(valid_chars) {
            return Err(LabelParseError::InvalidRepository(repository.to_string()));
        }
        if !namespace.chars().all(valid_chars) {
            return Err(LabelParseError::InvalidNamespace(namespace.to_string()));
        }
        if !tag.chars().all(valid_chars) {
            return Err(LabelParseError::InvalidTag(tag.to_string()));
        }

        Ok(Self {
            repository: repository.to_string(),
            namespace: namespace.to_string(),
            tag: tag.to_string(),
        })
    }

    /// Check if this label matches another (considering wildcards)
    ///
    /// A `*` in any component matches any value.
    pub fn matches(&self, other: &Label) -> bool {
        (self.repository == "*" || other.repository == "*" || self.repository == other.repository)
            && (self.namespace == "*" || other.namespace == "*" || self.namespace == other.namespace)
            && (self.tag == "*" || other.tag == "*" || self.tag == other.tag)
    }

    /// Get the parent label (same repository and namespace, different tag)
    ///
    /// Returns None if the tag doesn't contain a version separator.
    /// For example, `repo@ns:2.1` -> `repo@ns:2`
    pub fn parent(&self) -> Option<Self> {
        // Try to find a version separator (. or -)
        if let Some(pos) = self.tag.rfind(['.', '-']) {
            let parent_tag = &self.tag[..pos];
            if !parent_tag.is_empty() {
                return Some(Self {
                    repository: self.repository.clone(),
                    namespace: self.namespace.clone(),
                    tag: parent_tag.to_string(),
                });
            }
        }
        None
    }

    /// Create a child label by appending to the tag
    pub fn child(&self, suffix: &str) -> Self {
        Self {
            repository: self.repository.clone(),
            namespace: self.namespace.clone(),
            tag: format!("{}.{}", self.tag, suffix),
        }
    }

    /// Check if this label is on the same branch as another
    /// (same repository and namespace)
    pub fn same_branch(&self, other: &Label) -> bool {
        self.repository == other.repository && self.namespace == other.namespace
    }
}

impl fmt::Display for Label {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}@{}:{}", self.repository, self.namespace, self.tag)
    }
}

impl FromStr for Label {
    type Err = LabelParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Label::parse(s)
    }
}

/// Errors that can occur when parsing a label
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LabelParseError {
    /// Missing @ separator
    MissingAt(String),
    /// Missing : separator
    MissingColon(String),
    /// Empty repository component
    EmptyRepository(String),
    /// Empty namespace component
    EmptyNamespace(String),
    /// Empty tag component
    EmptyTag(String),
    /// Invalid characters in repository
    InvalidRepository(String),
    /// Invalid characters in namespace
    InvalidNamespace(String),
    /// Invalid characters in tag
    InvalidTag(String),
}

impl fmt::Display for LabelParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LabelParseError::MissingAt(s) => write!(f, "Missing '@' in label: {}", s),
            LabelParseError::MissingColon(s) => write!(f, "Missing ':' in label: {}", s),
            LabelParseError::EmptyRepository(s) => write!(f, "Empty repository in label: {}", s),
            LabelParseError::EmptyNamespace(s) => write!(f, "Empty namespace in label: {}", s),
            LabelParseError::EmptyTag(s) => write!(f, "Empty tag in label: {}", s),
            LabelParseError::InvalidRepository(s) => write!(f, "Invalid repository name: {}", s),
            LabelParseError::InvalidNamespace(s) => write!(f, "Invalid namespace: {}", s),
            LabelParseError::InvalidTag(s) => write!(f, "Invalid tag: {}", s),
        }
    }
}

impl std::error::Error for LabelParseError {}

/// A label path defines the search order for package resolution
///
/// Labels earlier in the path have higher priority.
#[derive(Debug, Clone, Default)]
pub struct LabelPath {
    /// Ordered list of labels (highest priority first)
    labels: Vec<Label>,
}

impl LabelPath {
    /// Create a new empty label path
    pub fn new() -> Self {
        Self { labels: Vec::new() }
    }

    /// Create a label path from a list of labels
    pub fn from_labels(labels: Vec<Label>) -> Self {
        Self { labels }
    }

    /// Add a label to the end of the path (lowest priority)
    pub fn push(&mut self, label: Label) {
        self.labels.push(label);
    }

    /// Add a label to the front of the path (highest priority)
    pub fn prepend(&mut self, label: Label) {
        self.labels.insert(0, label);
    }

    /// Remove a label from the path
    pub fn remove(&mut self, label: &Label) -> bool {
        if let Some(pos) = self.labels.iter().position(|l| l == label) {
            self.labels.remove(pos);
            true
        } else {
            false
        }
    }

    /// Get the labels in order
    pub fn labels(&self) -> &[Label] {
        &self.labels
    }

    /// Check if the path contains a label
    pub fn contains(&self, label: &Label) -> bool {
        self.labels.contains(label)
    }

    /// Get the priority of a label (0 = highest, None = not found)
    pub fn priority(&self, label: &Label) -> Option<usize> {
        self.labels.iter().position(|l| l == label)
    }

    /// Check if the path is empty
    pub fn is_empty(&self) -> bool {
        self.labels.is_empty()
    }

    /// Get the number of labels in the path
    pub fn len(&self) -> usize {
        self.labels.len()
    }

    /// Parse a label path from a colon-separated string
    ///
    /// Example: `repo1@ns:tag1:repo2@ns:tag2`
    pub fn parse(s: &str) -> Result<Self, LabelParseError> {
        if s.is_empty() {
            return Ok(Self::new());
        }

        let mut labels = Vec::new();
        let mut current = String::new();
        let mut depth = 0; // Track @ and : to properly split

        for c in s.chars() {
            match c {
                '@' => {
                    depth += 1;
                    current.push(c);
                }
                ':' if depth > 0 => {
                    // This colon is part of a label (namespace:tag separator)
                    depth -= 1;
                    current.push(c);
                    if depth == 0 && !current.is_empty() {
                        // End of a complete label
                        labels.push(Label::parse(&current)?);
                        current.clear();
                    }
                }
                ':' => {
                    // This colon separates labels
                    if !current.is_empty() {
                        labels.push(Label::parse(&current)?);
                        current.clear();
                    }
                }
                _ => current.push(c),
            }
        }

        // Handle remaining content
        if !current.is_empty() {
            labels.push(Label::parse(&current)?);
        }

        Ok(Self { labels })
    }
}

impl fmt::Display for LabelPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let labels: Vec<String> = self.labels.iter().map(|l| l.to_string()).collect();
        write!(f, "{}", labels.join(":"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_label_parse() {
        let label = Label::parse("conary.example.com@rpl:2").unwrap();
        assert_eq!(label.repository, "conary.example.com");
        assert_eq!(label.namespace, "rpl");
        assert_eq!(label.tag, "2");
    }

    #[test]
    fn test_label_display() {
        let label = Label::new("repo", "ns", "tag");
        assert_eq!(label.to_string(), "repo@ns:tag");
    }

    #[test]
    fn test_label_parse_errors() {
        assert!(Label::parse("missing-at").is_err());
        assert!(Label::parse("repo@missing-colon").is_err());
        assert!(Label::parse("@ns:tag").is_err()); // empty repo
        assert!(Label::parse("repo@:tag").is_err()); // empty ns
        assert!(Label::parse("repo@ns:").is_err()); // empty tag
    }

    #[test]
    fn test_label_parent() {
        let label = Label::parse("repo@ns:2.1").unwrap();
        let parent = label.parent().unwrap();
        assert_eq!(parent.tag, "2");

        let root = Label::parse("repo@ns:2").unwrap();
        assert!(root.parent().is_none());
    }

    #[test]
    fn test_label_child() {
        let label = Label::parse("repo@ns:2").unwrap();
        let child = label.child("1");
        assert_eq!(child.tag, "2.1");
    }

    #[test]
    fn test_label_matches() {
        let label1 = Label::parse("repo@ns:tag").unwrap();
        let label2 = Label::parse("repo@ns:tag").unwrap();
        let wildcard = Label::new("*", "ns", "tag");

        assert!(label1.matches(&label2));
        assert!(label1.matches(&wildcard));
        assert!(wildcard.matches(&label1));
    }

    #[test]
    fn test_label_path() {
        let mut path = LabelPath::new();
        path.push(Label::parse("repo1@ns:1").unwrap());
        path.push(Label::parse("repo2@ns:2").unwrap());

        assert_eq!(path.len(), 2);
        assert_eq!(path.priority(&Label::parse("repo1@ns:1").unwrap()), Some(0));
        assert_eq!(path.priority(&Label::parse("repo2@ns:2").unwrap()), Some(1));
    }

    #[test]
    fn test_same_branch() {
        let label1 = Label::parse("repo@ns:1").unwrap();
        let label2 = Label::parse("repo@ns:2").unwrap();
        let label3 = Label::parse("repo@other:1").unwrap();

        assert!(label1.same_branch(&label2));
        assert!(!label1.same_branch(&label3));
    }
}
