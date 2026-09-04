// conary-core/src/derivation/recipe_hash.rs

//! Recipe hashing for content-addressed build identification.
//!
//! Provides two hash functions that feed into `DerivationInputs`:
//!
//! - [`build_script_hash`] — SHA-256 of all build sections (configure, make,
//!   install, check) with variables expanded, producing a deterministic hash
//!   that changes when any build instruction or variable value changes.
//!
//! - [`source_hash`] — SHA-256 of the primary source checksum plus any
//!   additional source checksums.

use std::collections::BTreeMap;

use crate::hash;
use crate::recipe::{Recipe, SourceSection};

/// Errors from hashing recipe source inputs for derivation IDs.
#[derive(Debug, thiserror::Error)]
pub enum RecipeHashError {
    /// Local source recipe hashing needs content provenance not available in M1a.
    #[error(
        "local source recipes are not supported by derivation IDs in M1a; use conary cook/publish or wait for M2 tree hashing"
    )]
    UnsupportedLocalSource,
}

/// Expand `%(name)s`-style variables in a template string.
///
/// Variables come from two sources, applied in order:
/// 1. Built-in variables derived from the recipe (`name`, `version`)
/// 2. Custom variables from the recipe's `[variables]` section
///
/// Unknown variables are left as-is (no error).
#[must_use]
pub fn expand_variables(template: &str, recipe: &Recipe) -> String {
    if !template.contains("%(") {
        return template.to_string();
    }

    let mut result = template.to_string();

    // Built-in variables from the recipe metadata.
    result = result.replace("%(name)s", &recipe.package.name);
    result = result.replace("%(version)s", &recipe.package.version);

    // Custom variables, applied in sorted order for determinism.
    let sorted: BTreeMap<&String, &String> = recipe.variables.iter().collect();
    for (key, value) in sorted {
        result = result.replace(&format!("%({key})s"), value);
    }

    result
}

/// Compute a SHA-256 hash of all recipe build sections that affect the build
/// output.
///
/// Sections are concatenated in a fixed order: `setup`, `configure`, `make`,
/// `install`, `check`, `post_install`. Each present section is preceded by a
/// label line so that, e.g., a configure-only recipe and a make-only recipe
/// with the same command text produce different hashes. Variables (`%(name)s`
/// syntax) are expanded before hashing so different variable values produce
/// different hashes. Environment variables (sorted by key) and workdir are
/// also included after the script sections.
///
/// Returns a 64-char lowercase hex string.
#[must_use]
pub fn build_script_hash(recipe: &Recipe) -> String {
    let mut hasher = hash::Hasher::new(hash::HashAlgorithm::Sha256);

    let sections: [(&str, &Option<String>); 6] = [
        ("setup", &recipe.build.setup),
        ("configure", &recipe.build.configure),
        ("make", &recipe.build.make),
        ("install", &recipe.build.install),
        ("check", &recipe.build.check),
        ("post_install", &recipe.build.post_install),
    ];

    for (label, section) in &sections {
        if let Some(script) = section {
            let expanded = expand_variables(script, recipe);
            hasher.update(format!("{label}:{expanded}\n").as_bytes());
        }
    }

    // Hash environment variables in sorted order for determinism
    let mut env_keys: Vec<&String> = recipe.build.environment.keys().collect();
    env_keys.sort();
    for key in env_keys {
        let value = &recipe.build.environment[key];
        hasher.update(format!("env:{key}={value}\n").as_bytes());
    }

    // Hash workdir if set
    if let Some(ref workdir) = recipe.build.workdir {
        hasher.update(format!("workdir:{workdir}\n").as_bytes());
    }

    hasher.finalize().value
}

/// Compute a SHA-256 hash of all source checksums.
///
/// The primary source checksum is hashed first, followed by additional source
/// checksums in their original order. Each checksum occupies its own line in
/// the hash input.
///
/// Returns a 64-char lowercase hex string.
#[must_use]
pub fn source_hash(recipe: &Recipe) -> String {
    let mut hasher = hash::Hasher::new(hash::HashAlgorithm::Sha256);

    match &recipe.source {
        SourceSection::Remote(source) => {
            hasher.update(format!("primary:{}\n", source.checksum).as_bytes());

            for additional in &source.additional {
                hasher.update(format!("additional:{}\n", additional.checksum).as_bytes());
            }
        }
        SourceSection::Local(source) => {
            hasher.update(format!("local:{}\n", source.path.display()).as_bytes());
        }
    }

    hasher.finalize().value
}

/// Fallibly compute a SHA-256 hash of source inputs for derivation IDs.
///
/// Local source recipes are rejected in M1a because hashing only the path would
/// make derivation IDs unsafe. M2 will add content tree hashing/provenance.
pub fn try_source_hash(recipe: &Recipe) -> Result<String, RecipeHashError> {
    match &recipe.source {
        SourceSection::Remote(_) => Ok(source_hash(recipe)),
        SourceSection::Local(_) => Err(RecipeHashError::UnsupportedLocalSource),
    }
}

#[cfg(test)]
mod tests;
