// src/derived/mod.rs

//! Derived package builder
//!
//! Builds derived packages by taking an existing package and applying
//! modifications (patches and file overrides) to create a customized version.
//!
//! # Example
//!
//! ```ignore
//! use conary::derived::{DerivedBuilder, DerivedSpec};
//!
//! let spec = DerivedSpec {
//!     name: "nginx-custom".to_string(),
//!     parent_name: "nginx".to_string(),
//!     version_suffix: Some("+corp".to_string()),
//!     patches: vec![("security.patch".to_string(), patch_content)],
//!     overrides: vec![("/etc/nginx/nginx.conf".to_string(), config_content)],
//!     removals: vec!["/etc/nginx/default.conf".to_string()],
//! };
//!
//! let builder = DerivedBuilder::new(spec, &db)?;
//! let result = builder.build()?;
//! ```

mod builder;

pub use builder::{build_from_definition, store_in_cas, DerivedBuilder, DerivedResult, DerivedSpec};
