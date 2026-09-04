// conary-core/src/derivation/recipe_hash/tests.rs

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
    assert_ne!(
        hash_4, hash_8,
        "different job counts must produce different hashes"
    );
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
fn try_source_hash_rejects_local_source_in_m1a() {
    let mut recipe = parse_recipe(SAMPLE_RECIPE);
    recipe.source = SourceSection::Local(crate::recipe::LocalSourceSection { path: "src".into() });

    let error = try_source_hash(&recipe).unwrap_err();

    assert!(
        error
            .to_string()
            .contains("local source recipes are not supported by derivation IDs in M1a"),
        "expected M1a unsupported local source error, got: {error}"
    );
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

#[test]
fn build_script_hash_includes_setup_section() {
    let without_setup = r#"
[package]
name = "test"
version = "1.0"

[source]
archive = "https://example.com/test.tar.gz"
checksum = "sha256:abc"

[build]
make = "make"
"#;

    let with_setup = r#"
[package]
name = "test"
version = "1.0"

[source]
archive = "https://example.com/test.tar.gz"
checksum = "sha256:abc"

[build]
setup = "autoreconf -fi"
make = "make"
"#;

    assert_ne!(
        build_script_hash(&parse_recipe(without_setup)),
        build_script_hash(&parse_recipe(with_setup)),
        "setup section must affect build_script_hash"
    );
}

#[test]
fn build_script_hash_includes_post_install_section() {
    let without = r#"
[package]
name = "test"
version = "1.0"

[source]
archive = "https://example.com/test.tar.gz"
checksum = "sha256:abc"

[build]
make = "make"
"#;

    let with = r#"
[package]
name = "test"
version = "1.0"

[source]
archive = "https://example.com/test.tar.gz"
checksum = "sha256:abc"

[build]
make = "make"
post_install = "ldconfig"
"#;

    assert_ne!(
        build_script_hash(&parse_recipe(without)),
        build_script_hash(&parse_recipe(with)),
        "post_install section must affect build_script_hash"
    );
}

#[test]
fn build_script_hash_includes_environment() {
    let without_env = r#"
[package]
name = "test"
version = "1.0"

[source]
archive = "https://example.com/test.tar.gz"
checksum = "sha256:abc"

[build]
make = "make"
"#;

    let with_env = r#"
[package]
name = "test"
version = "1.0"

[source]
archive = "https://example.com/test.tar.gz"
checksum = "sha256:abc"

[build]
make = "make"

[build.environment]
CFLAGS = "-O2"
"#;

    assert_ne!(
        build_script_hash(&parse_recipe(without_env)),
        build_script_hash(&parse_recipe(with_env)),
        "environment variables must affect build_script_hash"
    );
}

#[test]
fn build_script_hash_includes_workdir() {
    let without_workdir = r#"
[package]
name = "test"
version = "1.0"

[source]
archive = "https://example.com/test.tar.gz"
checksum = "sha256:abc"

[build]
make = "make"
"#;

    let with_workdir = r#"
[package]
name = "test"
version = "1.0"

[source]
archive = "https://example.com/test.tar.gz"
checksum = "sha256:abc"

[build]
make = "make"
workdir = "src/build"
"#;

    assert_ne!(
        build_script_hash(&parse_recipe(without_workdir)),
        build_script_hash(&parse_recipe(with_workdir)),
        "workdir must affect build_script_hash"
    );
}
