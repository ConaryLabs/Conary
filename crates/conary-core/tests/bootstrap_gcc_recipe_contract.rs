use std::path::{Path, PathBuf};

use conary_core::recipe::parse_recipe_file;

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .find(|dir| dir.join("recipes/cross-tools/gcc-pass1.toml").is_file())
        .expect("workspace root not found from crate manifest ancestors")
}

fn gcc_recipe_paths() -> [PathBuf; 2] {
    [
        workspace_root().join("recipes/cross-tools/gcc-pass1.toml"),
        workspace_root().join("recipes/temp-tools/gcc-pass2.toml"),
    ]
}

#[test]
fn final_system_gcc_recipe_patches_libgomp_const_warning_with_exact_match() {
    let path = workspace_root().join("recipes/system/gcc.toml");
    let recipe = parse_recipe_file(&path)
        .unwrap_or_else(|err| panic!("failed to parse {}: {err}", path.display()));
    let configure = recipe
        .build
        .configure
        .as_deref()
        .unwrap_or_else(|| panic!("{} must define a configure script", path.display()));

    assert!(
        configure.contains("sed -i 's/char \\*q/const char *q/' libgomp/affinity-fmt.c"),
        "{} must patch libgomp/affinity-fmt.c using the exact pre-GCC-15 line so the sed actually matches",
        path.display()
    );
}

#[test]
fn bootstrap_gcc_recipes_stage_companion_libraries_via_additional_sources() {
    for path in gcc_recipe_paths() {
        let recipe = parse_recipe_file(&path)
            .unwrap_or_else(|err| panic!("failed to parse {}: {err}", path.display()));

        let urls: Vec<&str> = recipe
            .source
            .additional
            .iter()
            .map(|source| source.url.as_str())
            .collect();

        assert_eq!(
            urls.len(),
            3,
            "{} must stage GMP, MPFR, and MPC via source.additional",
            path.display()
        );
        assert!(
            urls.iter().any(|url| url.contains("/gmp/")),
            "{} must stage the GMP companion source via source.additional",
            path.display()
        );
        assert!(
            urls.iter().any(|url| url.contains("/mpfr/")),
            "{} must stage the MPFR companion source via source.additional",
            path.display()
        );
        assert!(
            urls.iter().any(|url| url.contains("/mpc/")),
            "{} must stage the MPC companion source via source.additional",
            path.display()
        );
    }
}
