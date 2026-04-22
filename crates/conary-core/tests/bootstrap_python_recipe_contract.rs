use std::path::{Path, PathBuf};

use conary_core::recipe::parse_recipe_file;

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .find(|dir| dir.join("recipes/system/python.toml").is_file())
        .expect("workspace root not found from crate manifest ancestors")
}

fn recipe_path(name: &str) -> PathBuf {
    workspace_root()
        .join("recipes/system")
        .join(format!("{name}.toml"))
}

#[test]
fn phase3_python_wheel_recipes_install_directly_into_the_chroot() {
    for name in [
        "flit-core",
        "packaging",
        "wheel",
        "setuptools",
        "meson",
        "markupsafe",
        "jinja2",
    ] {
        let path = recipe_path(name);
        let recipe = parse_recipe_file(&path)
            .unwrap_or_else(|err| panic!("failed to parse {}: {err}", path.display()));
        let install = recipe
            .build
            .install
            .as_deref()
            .unwrap_or_else(|| panic!("{} must define an install script", path.display()));

        assert!(
            install.contains("pip3 install --no-index --find-links dist"),
            "{} must install from the locally built wheel",
            path.display()
        );
        assert!(
            !install.contains("--root=$DESTDIR"),
            "{} must not use --root=$DESTDIR during Phase 3 chroot installs; LFS installs these Python packages directly into /usr",
            path.display()
        );
    }
}
