// src/dependencies/mod.rs

//! Language-specific dependency support
//!
//! This module provides support for language-specific dependencies similar to
//! OG Conary's dependency class system. It handles dependencies like:
//! - `python(requests)` - Python module requirements
//! - `perl(DBI)` - Perl module requirements
//! - `ruby(bundler)` - Ruby gem requirements
//! - `java(org.apache.commons.lang)` - Java package requirements
//!
//! # Example
//!
//! ```ignore
//! use conary::dependencies::{DependencyClass, LanguageDep};
//!
//! // Parse a dependency string
//! let dep = LanguageDep::parse("python(requests>=2.0)").unwrap();
//! assert_eq!(dep.class, DependencyClass::Python);
//! assert_eq!(dep.name, "requests");
//! assert_eq!(dep.version_constraint, Some(">=2.0".to_string()));
//! ```

mod classes;
mod detection;

pub use classes::{DependencyClass, LanguageDep};
pub use detection::LanguageDepDetector;
