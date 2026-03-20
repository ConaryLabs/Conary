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
use crate::recipe::Recipe;

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
/// Sections are concatenated in a fixed order: `configure`, `make`, `install`,
/// `check`. Each present section is preceded by a label line so that, e.g., a
/// configure-only recipe and a make-only recipe with the same command text
/// produce different hashes. Variables (`%(name)s` syntax) are expanded before
/// hashing so different variable values produce different hashes.
///
/// Returns a 64-char lowercase hex string.
#[must_use]
pub fn build_script_hash(recipe: &Recipe) -> String {
    let mut hasher = hash::Hasher::new(hash::HashAlgorithm::Sha256);

    let sections: [(&str, &Option<String>); 4] = [
        ("configure", &recipe.build.configure),
        ("make", &recipe.build.make),
        ("install", &recipe.build.install),
        ("check", &recipe.build.check),
    ];

    for (label, section) in &sections {
        if let Some(script) = section {
            let expanded = expand_variables(script, recipe);
            hasher.update(format!("{label}:{expanded}\n").as_bytes());
        }
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

    hasher.update(format!("primary:{}\n", recipe.source.checksum).as_bytes());

    for additional in &recipe.source.additional {
        hasher.update(format!("additional:{}\n", additional.checksum).as_bytes());
    }

    hasher.finalize().value
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_RECIPE: &str = r#"
[package]
name = "hello"
version = "1.0.0"

[source]
archive = "https://example.com/hello-%(version)s.tar.gz"
checksum = "sha256:abc123def456"

[build]
configure = "./configure --prefix=/usr --with-feature=%(name)s"
make = "make -j%(jobs)s"
install = "make install DESTDIR=%(destdir)s"

[variables]
jobs = "4"
"#;

    fn parse_recipe(toml_str: &str) -> Recipe {
        toml::from_str(toml_str).expect("valid recipe TOML")
    }

    #[test]
    fn same_recipe_produces_same_build_script_hash() {
        let recipe = parse_recipe(SAMPLE_RECIPE);
        let hash1 = build_script_hash(&recipe);
        let hash2 = build_script_hash(&recipe);
        assert_eq!(hash1, hash2);
        assert_eq!(hash1.len(), 64);
        assert!(hash1.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn different_configure_flags_produce_different_hashes() {
        let recipe1 = parse_recipe(SAMPLE_RECIPE);

        let modified = r#"
[package]
name = "hello"
version = "1.0.0"

[source]
archive = "https://example.com/hello-%(version)s.tar.gz"
checksum = "sha256:abc123def456"

[build]
configure = "./configure --prefix=/usr --enable-extra"
make = "make -j%(jobs)s"
install = "make install DESTDIR=%(destdir)s"

[variables]
jobs = "4"
"#;
        let recipe2 = parse_recipe(modified);

        assert_ne!(build_script_hash(&recipe1), build_script_hash(&recipe2));
    }

    #[test]
    fn variable_expansion_changes_hash() {
        let with_4_jobs = parse_recipe(SAMPLE_RECIPE);

        let with_8_jobs = r#"
[package]
name = "hello"
version = "1.0.0"

[source]
archive = "https://example.com/hello-%(version)s.tar.gz"
checksum = "sha256:abc123def456"

[build]
configure = "./configure --prefix=/usr --with-feature=%(name)s"
make = "make -j%(jobs)s"
install = "make install DESTDIR=%(destdir)s"

[variables]
jobs = "8"
"#;
        let recipe_8 = parse_recipe(with_8_jobs);

        let hash_4 = build_script_hash(&with_4_jobs);
        let hash_8 = build_script_hash(&recipe_8);
        assert_ne!(hash_4, hash_8, "different job counts must produce different hashes");
    }

    #[test]
    fn expand_variables_works_correctly() {
        let recipe = parse_recipe(SAMPLE_RECIPE);

        let expanded = expand_variables("%(name)s-%(version)s-j%(jobs)s", &recipe);
        assert_eq!(expanded, "hello-1.0.0-j4");
    }

    #[test]
    fn expand_variables_leaves_unknown_intact() {
        let recipe = parse_recipe(SAMPLE_RECIPE);

        let expanded = expand_variables("%(unknown)s stays", &recipe);
        assert_eq!(expanded, "%(unknown)s stays");
    }

    #[test]
    fn source_hash_includes_additional_sources() {
        let single_source = r#"
[package]
name = "multi"
version = "2.0"

[source]
archive = "https://example.com/multi-2.0.tar.gz"
checksum = "sha256:primary111"

[build]
make = "make"
"#;

        let multi_source = r#"
[package]
name = "multi"
version = "2.0"

[source]
archive = "https://example.com/multi-2.0.tar.gz"
checksum = "sha256:primary111"
additional = [
    { url = "https://example.com/extra.tar.gz", checksum = "sha256:extra222" },
]

[build]
make = "make"
"#;

        let recipe_single = parse_recipe(single_source);
        let recipe_multi = parse_recipe(multi_source);

        let hash_single = source_hash(&recipe_single);
        let hash_multi = source_hash(&recipe_multi);

        assert_ne!(
            hash_single, hash_multi,
            "additional sources must affect source_hash"
        );
    }

    #[test]
    fn source_hash_is_deterministic() {
        let recipe = parse_recipe(SAMPLE_RECIPE);
        let hash1 = source_hash(&recipe);
        let hash2 = source_hash(&recipe);
        assert_eq!(hash1, hash2);
        assert_eq!(hash1.len(), 64);
    }

    #[test]
    fn build_script_hash_includes_check_section() {
        let without_check = r#"
[package]
name = "test"
version = "1.0"

[source]
archive = "https://example.com/test.tar.gz"
checksum = "sha256:abc"

[build]
configure = "./configure"
make = "make"
install = "make install"
"#;

        let with_check = r#"
[package]
name = "test"
version = "1.0"

[source]
archive = "https://example.com/test.tar.gz"
checksum = "sha256:abc"

[build]
configure = "./configure"
make = "make"
install = "make install"
check = "make check"
"#;

        let hash_no_check = build_script_hash(&parse_recipe(without_check));
        let hash_with_check = build_script_hash(&parse_recipe(with_check));
        assert_ne!(
            hash_no_check, hash_with_check,
            "check section must affect build_script_hash"
        );
    }

    #[test]
    fn build_script_hash_empty_build_is_valid() {
        let empty_build = r#"
[package]
name = "data"
version = "1.0"

[source]
archive = "https://example.com/data.tar.gz"
checksum = "sha256:abc"

[build]
"#;

        let hash = build_script_hash(&parse_recipe(empty_build));
        assert_eq!(hash.len(), 64);
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn different_section_same_text_produces_different_hash() {
        // A recipe with only configure="make" vs only make="make" should differ.
        let configure_only = r#"
[package]
name = "test"
version = "1.0"

[source]
archive = "https://example.com/test.tar.gz"
checksum = "sha256:abc"

[build]
configure = "make"
"#;

        let make_only = r#"
[package]
name = "test"
version = "1.0"

[source]
archive = "https://example.com/test.tar.gz"
checksum = "sha256:abc"

[build]
make = "make"
"#;

        let hash_configure = build_script_hash(&parse_recipe(configure_only));
        let hash_make = build_script_hash(&parse_recipe(make_only));
        assert_ne!(
            hash_configure, hash_make,
            "same command in different sections must produce different hashes"
        );
    }

    #[test]
    fn source_hash_different_primary_checksum() {
        let recipe_a = parse_recipe(
            r#"
[package]
name = "a"
version = "1.0"

[source]
archive = "https://example.com/a.tar.gz"
checksum = "sha256:aaaa"

[build]
make = "make"
"#,
        );

        let recipe_b = parse_recipe(
            r#"
[package]
name = "a"
version = "1.0"

[source]
archive = "https://example.com/a.tar.gz"
checksum = "sha256:bbbb"

[build]
make = "make"
"#,
        );

        assert_ne!(source_hash(&recipe_a), source_hash(&recipe_b));
    }

    #[test]
    fn expand_variables_deterministic_with_multiple_vars() {
        let recipe = parse_recipe(
            r#"
[package]
name = "multi"
version = "3.0"

[source]
archive = "https://example.com/multi.tar.gz"
checksum = "sha256:abc"

[build]
make = "make"

[variables]
foo = "F"
bar = "B"
baz = "Z"
"#,
        );

        // Run multiple times to verify determinism.
        let r1 = expand_variables("%(foo)s-%(bar)s-%(baz)s", &recipe);
        let r2 = expand_variables("%(foo)s-%(bar)s-%(baz)s", &recipe);
        assert_eq!(r1, r2);
        assert_eq!(r1, "F-B-Z");
    }
}
