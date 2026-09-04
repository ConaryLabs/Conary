// crates/conary-core/src/recipe/format/tests.rs

use super::*;

const SAMPLE_RECIPE: &str = r#"
[package]
name = "nginx"
version = "1.24.0"
summary = "High-performance HTTP server"
license = "BSD-2-Clause"
homepage = "https://nginx.org"

[source]
archive = "https://nginx.org/download/nginx-%(version)s.tar.gz"
checksum = "sha256:77a2541637b92a621e3ee76571f6e9af0b4e6a6a1f5b0fd3d5c9cf6c8c55e3"

[build]
requires = ["openssl:devel", "pcre:devel", "zlib:devel"]
configure = "./configure --prefix=/usr --with-http_ssl_module --with-http_v2_module"
make = "make -j%(jobs)s"
install = "make install DESTDIR=%(destdir)s"

[patches]
files = [
    { file = "nginx-1.24-fix-headers.patch", strip = 1 },
]

[variables]
jobs = "4"
"#;

#[test]
fn test_parse_recipe() {
    let recipe: Recipe = toml::from_str(SAMPLE_RECIPE).unwrap();

    assert_eq!(recipe.package.name, "nginx");
    assert_eq!(recipe.package.version, "1.24.0");
    assert_eq!(recipe.package.license.as_deref(), Some("BSD-2-Clause"));

    let SourceSection::Remote(source) = &recipe.source else {
        panic!("sample archive recipe should parse as remote source");
    };
    assert!(source.archive.contains("%(version)s"));
    assert!(source.checksum.starts_with("sha256:"));

    assert_eq!(recipe.build.requires.len(), 3);
    assert!(recipe.build.configure.is_some());
}

#[test]
fn test_variable_substitution() {
    let recipe: Recipe = toml::from_str(SAMPLE_RECIPE).unwrap();

    let url = recipe.archive_url();
    assert!(url.contains("1.24.0"));
    assert!(!url.contains("%(version)s"));

    let install = recipe.substitute(recipe.build.install.as_ref().unwrap(), "/tmp/dest");
    assert!(install.contains("/tmp/dest"));
    assert!(!install.contains("%(destdir)s"));
}

#[test]
fn test_archive_filename() {
    let recipe: Recipe = toml::from_str(SAMPLE_RECIPE).unwrap();
    assert_eq!(recipe.archive_filename(), "nginx-1.24.0.tar.gz");
}

#[test]
fn test_remote_archive_recipe_parses_unchanged() {
    let recipe: Recipe = toml::from_str(SAMPLE_RECIPE).unwrap();
    let SourceSection::Remote(source) = recipe.source else {
        panic!("archive recipe should parse as remote source");
    };

    assert_eq!(
        source.archive,
        "https://nginx.org/download/nginx-%(version)s.tar.gz"
    );
    assert_eq!(
        source.checksum,
        "sha256:77a2541637b92a621e3ee76571f6e9af0b4e6a6a1f5b0fd3d5c9cf6c8c55e3"
    );
    assert!(source.signature.is_none());
    assert!(source.additional.is_empty());
}

#[test]
fn test_local_source_path_dot_parses() {
    let recipe: Recipe = toml::from_str(
        r#"
[package]
name = "local"
version = "1.0"

[source]
path = "."

[build]
make = "true"
"#,
    )
    .unwrap();

    let SourceSection::Local(source) = recipe.source else {
        panic!("path source should parse as local source");
    };
    assert_eq!(source.path, std::path::PathBuf::from("."));
}

#[test]
fn test_local_source_resolves_against_recipe_directory() {
    let source = LocalSourceSection {
        path: std::path::PathBuf::from("./src"),
    };

    assert_eq!(
        source
            .resolve_against(std::path::Path::new("/work/recipes/pkg"))
            .unwrap(),
        std::path::PathBuf::from("/work/recipes/pkg/src")
    );
}

#[test]
fn test_source_rejects_archive_and_path() {
    let error = toml::from_str::<Recipe>(
        r#"
[package]
name = "ambiguous"
version = "1.0"

[source]
archive = "https://example.invalid/pkg.tar.gz"
checksum = "sha256:abc"
path = "."

[build]
make = "true"
"#,
    )
    .unwrap_err();

    assert!(
        error.to_string().contains("both 'archive' and 'path'"),
        "expected clear ambiguous source error, got: {error}"
    );
}

#[test]
fn test_local_source_rejects_parent_directory_escape() {
    let error = toml::from_str::<Recipe>(
        r#"
[package]
name = "escape"
version = "1.0"

[source]
path = "../outside"

[build]
make = "true"
"#,
    )
    .unwrap_err();

    assert!(
        error
            .to_string()
            .contains("must stay within the recipe directory"),
        "expected outside path rejection, got: {error}"
    );
}

#[test]
fn test_minimal_recipe() {
    let minimal = r#"
[package]
name = "hello"
version = "1.0"

[source]
archive = "https://example.com/hello-1.0.tar.gz"
checksum = "sha256:abc123"

[build]
configure = "./configure"
make = "make"
install = "make install DESTDIR=%(destdir)s"
"#;

    let recipe: Recipe = toml::from_str(minimal).unwrap();
    assert_eq!(recipe.package.name, "hello");
    assert_eq!(recipe.package.release, "1"); // default
    assert!(recipe.patches.is_none());
}

const CROSS_RECIPE: &str = r#"
[package]
name = "glibc"
version = "2.38"

[source]
archive = "https://ftp.gnu.org/gnu/glibc/glibc-%(version)s.tar.xz"
checksum = "sha256:abc123"

[build]
requires = ["linux-headers"]
makedepends = ["gcc", "make", "bison", "gawk", "texinfo"]
configure = "../configure --prefix=/usr --host=%(target)s"
make = "make"
install = "make install DESTDIR=%(destdir)s"

[cross]
target = "x86_64-conary-linux-gnu"
sysroot = "/opt/sysroot/stage0"
cross_tools = "/opt/cross/bin"
stage = "stage1"
tool_prefix = "x86_64-conary-linux-gnu"
"#;

#[test]
fn test_parse_cross_recipe() {
    let recipe: Recipe = toml::from_str(CROSS_RECIPE).unwrap();

    assert_eq!(recipe.package.name, "glibc");
    assert!(recipe.cross.is_some());

    let cross = recipe.cross.as_ref().unwrap();
    assert_eq!(cross.target.as_deref(), Some("x86_64-conary-linux-gnu"));
    assert_eq!(cross.sysroot.as_deref(), Some("/opt/sysroot/stage0"));
    assert_eq!(cross.cross_tools.as_deref(), Some("/opt/cross/bin"));
    assert_eq!(cross.stage, Some(BuildStage::Stage1));
    assert_eq!(
        cross.tool_prefix.as_deref(),
        Some("x86_64-conary-linux-gnu")
    );
}

#[test]
fn test_is_cross_build() {
    // Recipe without cross section
    let recipe: Recipe = toml::from_str(SAMPLE_RECIPE).unwrap();
    assert!(!recipe.is_cross_build());

    // Recipe with cross section
    let recipe: Recipe = toml::from_str(CROSS_RECIPE).unwrap();
    assert!(recipe.is_cross_build());

    // Recipe with empty cross section (no actual cross settings)
    let empty_cross = r#"
[package]
name = "test"
version = "1.0"

[source]
archive = "https://example.com/test.tar.gz"
checksum = "sha256:abc"

[build]
make = "make"

[cross]
"#;
    let recipe: Recipe = toml::from_str(empty_cross).unwrap();
    assert!(!recipe.is_cross_build());
}

#[test]
fn test_build_stage() {
    // Default stage is Final
    let recipe: Recipe = toml::from_str(SAMPLE_RECIPE).unwrap();
    assert_eq!(recipe.build_stage(), BuildStage::Final);

    // Cross recipe with explicit stage
    let recipe: Recipe = toml::from_str(CROSS_RECIPE).unwrap();
    assert_eq!(recipe.build_stage(), BuildStage::Stage1);

    // Test each stage
    for (stage_str, expected) in [
        ("stage0", BuildStage::Stage0),
        ("stage1", BuildStage::Stage1),
        ("stage2", BuildStage::Stage2),
        ("final", BuildStage::Final),
    ] {
        let toml = format!(
            r#"
[package]
name = "test"
version = "1.0"

[source]
archive = "https://example.com/test.tar.gz"
checksum = "sha256:abc"

[build]
make = "make"

[cross]
stage = "{}"
"#,
            stage_str
        );
        let recipe: Recipe = toml::from_str(&toml).unwrap();
        assert_eq!(recipe.build_stage(), expected);
    }
}

#[test]
fn test_build_stage_methods() {
    assert_eq!(BuildStage::Stage0.as_str(), "stage0");
    assert_eq!(BuildStage::Stage1.as_str(), "stage1");
    assert_eq!(BuildStage::Stage2.as_str(), "stage2");
    assert_eq!(BuildStage::Final.as_str(), "final");

    assert!(BuildStage::Stage0.is_bootstrap());
    assert!(BuildStage::Stage1.is_bootstrap());
    assert!(BuildStage::Stage2.is_bootstrap());
    assert!(!BuildStage::Final.is_bootstrap());
}

#[test]
fn test_all_build_deps() {
    let recipe: Recipe = toml::from_str(CROSS_RECIPE).unwrap();
    let deps = recipe.all_build_deps();

    // Should include both requires and makedepends
    assert!(deps.contains(&"linux-headers"));
    assert!(deps.contains(&"gcc"));
    assert!(deps.contains(&"make"));
    assert!(deps.contains(&"bison"));
    assert!(deps.contains(&"gawk"));
    assert!(deps.contains(&"texinfo"));
    assert_eq!(deps.len(), 6); // 1 require + 5 makedepends
}

#[test]
fn test_cross_env_basic() {
    let recipe: Recipe = toml::from_str(CROSS_RECIPE).unwrap();
    let env = recipe.cross_env();

    // Should have cross-compiler paths
    assert_eq!(
        env.get("CC").unwrap(),
        "/opt/cross/bin/x86_64-conary-linux-gnu-gcc"
    );
    assert_eq!(
        env.get("CXX").unwrap(),
        "/opt/cross/bin/x86_64-conary-linux-gnu-g++"
    );
    assert_eq!(
        env.get("AR").unwrap(),
        "/opt/cross/bin/x86_64-conary-linux-gnu-ar"
    );

    // Should have target and sysroot
    assert_eq!(env.get("TARGET").unwrap(), "x86_64-conary-linux-gnu");
    assert_eq!(env.get("SYSROOT").unwrap(), "/opt/sysroot/stage0");

    // Should have sysroot in CFLAGS
    assert!(
        env.get("CFLAGS")
            .unwrap()
            .contains("--sysroot=/opt/sysroot/stage0")
    );

    // Should have stage marker
    assert_eq!(env.get("CONARY_STAGE").unwrap(), "stage1");
}

#[test]
fn test_cross_env_with_overrides() {
    let toml = r#"
[package]
name = "test"
version = "1.0"

[source]
archive = "https://example.com/test.tar.gz"
checksum = "sha256:abc"

[build]
make = "make"

[cross]
target = "aarch64-linux-gnu"
tool_prefix = "aarch64-linux-gnu"
cc = "/custom/path/clang"
cxx = "/custom/path/clang++"
"#;
    let recipe: Recipe = toml::from_str(toml).unwrap();
    let env = recipe.cross_env();

    // Overridden tools should use custom paths
    assert_eq!(env.get("CC").unwrap(), "/custom/path/clang");
    assert_eq!(env.get("CXX").unwrap(), "/custom/path/clang++");

    // Non-overridden tools should use prefix
    assert_eq!(env.get("AR").unwrap(), "aarch64-linux-gnu-ar");
    assert_eq!(env.get("LD").unwrap(), "aarch64-linux-gnu-ld");
}

#[test]
fn test_cross_env_empty_for_non_cross() {
    let recipe: Recipe = toml::from_str(SAMPLE_RECIPE).unwrap();
    let env = recipe.cross_env();

    // Non-cross recipe should return empty env
    assert!(env.is_empty());
}

#[test]
fn test_makedepends_parsing() {
    let toml = r#"
[package]
name = "test"
version = "1.0"

[source]
archive = "https://example.com/test.tar.gz"
checksum = "sha256:abc"

[build]
requires = ["runtime-dep"]
makedepends = ["cmake", "ninja", "pkgconf"]
configure = "cmake -B build"
make = "cmake --build build"
install = "cmake --install build --prefix %(destdir)s"
"#;
    let recipe: Recipe = toml::from_str(toml).unwrap();

    assert_eq!(recipe.build.requires, vec!["runtime-dep"]);
    assert_eq!(recipe.build.makedepends, vec!["cmake", "ninja", "pkgconf"]);
}

#[test]
fn test_cross_section_defaults() {
    let cross = CrossSection::default();

    assert!(cross.target.is_none());
    assert!(cross.sysroot.is_none());
    assert!(cross.cross_tools.is_none());
    assert!(cross.stage.is_none());
    assert!(cross.tool_prefix.is_none());
    assert!(cross.cc.is_none());
    assert!(cross.cxx.is_none());
    assert!(cross.ar.is_none());
    assert!(cross.ld.is_none());
}
