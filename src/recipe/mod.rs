// src/recipe/mod.rs

//! Recipe system for building packages from source
//!
//! Recipes define how to build a package from source, including:
//! - Source archives and their checksums
//! - Patches to apply
//! - Build dependencies
//! - Build instructions (configure, make, install)
//!
//! # Culinary Terminology
//!
//! Following the original Conary tradition, we use cooking metaphors:
//! - **Recipe**: The build specification (like a recipe card)
//! - **Cook**: Build a package from a recipe
//! - **Kitchen**: The isolated build environment
//! - **Ingredients**: Source archives and patches
//! - **Prep**: Fetch and prepare sources
//! - **Simmer**: The actual build process
//!
//! # Example Recipe
//!
//! ```toml
//! [package]
//! name = "nginx"
//! version = "1.24.0"
//!
//! [source]
//! archive = "https://nginx.org/download/nginx-%(version)s.tar.gz"
//! checksum = "sha256:abc123..."
//!
//! [build]
//! requires = ["openssl:devel", "pcre:devel", "zlib:devel"]
//! configure = "./configure --prefix=/usr --with-http_ssl_module"
//! make = "make -j$(nproc)"
//! install = "make install DESTDIR=%(destdir)s"
//! ```
//!
//! # Security
//!
//! All recipe builds run in an isolated container with:
//! - User namespace isolation (no root on host)
//! - Read-only bind mounts for sources
//! - Private `/tmp` and network namespace
//! - Resource limits (CPU, memory, time)

mod format;
mod kitchen;
pub mod parser;

pub use format::{BuildSection, PatchInfo, Recipe, SourceSection};
pub use kitchen::{Cook, CookResult, Kitchen, KitchenConfig};
pub use parser::{parse_recipe, parse_recipe_file, validate_recipe};
