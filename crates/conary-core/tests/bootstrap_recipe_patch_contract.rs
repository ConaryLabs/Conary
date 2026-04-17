// conary-core/tests/bootstrap_recipe_patch_contract.rs

use std::path::{Path, PathBuf};

use conary_core::recipe::parse_recipe_file;

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .find(|dir| dir.join("recipes/cross-tools/glibc.toml").is_file())
        .expect("workspace root not found from crate manifest ancestors")
}

fn patched_bootstrap_recipes() -> [PathBuf; 5] {
    [
        workspace_root().join("recipes/cross-tools/glibc.toml"),
        workspace_root().join("recipes/system/bzip2.toml"),
        workspace_root().join("recipes/system/coreutils.toml"),
        workspace_root().join("recipes/system/glibc.toml"),
        workspace_root().join("recipes/system/kbd.toml"),
    ]
}

#[test]
fn bootstrap_recipes_with_patches_use_supported_patch_section() {
    for path in patched_bootstrap_recipes() {
        let recipe = parse_recipe_file(&path)
            .unwrap_or_else(|err| panic!("failed to parse {}: {err}", path.display()));

        let patches = recipe
            .patches
            .as_ref()
            .unwrap_or_else(|| panic!("{} must declare top-level patches", path.display()));
        assert!(
            !patches.files.is_empty(),
            "{} must include at least one patch entry",
            path.display()
        );

        let configure = recipe.build.configure.as_deref().unwrap_or("");
        assert!(
            !configure.contains("patch -Np1 -i"),
            "{} should rely on the Kitchen patch phase instead of manually applying patches in configure",
            path.display()
        );
    }
}
