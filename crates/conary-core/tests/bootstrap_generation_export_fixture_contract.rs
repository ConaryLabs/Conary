// conary-core/tests/bootstrap_generation_export_fixture_contract.rs

use std::path::Path;

use conary_core::derivation::load_recipes;
use conary_core::recipe::parse_recipe_file;

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .find(|dir| {
            dir.join("apps/conary/tests/fixtures/bootstrap-generation-export/conaryos.toml")
                .is_file()
        })
        .expect("workspace root not found from crate manifest ancestors")
}

#[test]
fn generation_export_fixture_recipe_parses_and_loads() {
    let fixture = workspace_root().join("apps/conary/tests/fixtures/bootstrap-generation-export");
    let recipe_path = fixture
        .join("recipes")
        .join("system")
        .join("generation-fixture-rootfs.toml");

    let recipe = parse_recipe_file(&recipe_path)
        .unwrap_or_else(|err| panic!("failed to parse {}: {err}", recipe_path.display()));
    assert_eq!(recipe.package.name, "generation-fixture-rootfs");
    let install = recipe
        .build
        .install
        .as_deref()
        .expect("fixture recipe must have an install script");
    assert!(
        install.contains("$dest/usr/lib64/ld-linux-x86-64.so.2")
            && install.contains("ln -s ../lib/ld-linux-x86-64.so.2"),
        "generation export fixture must bridge /lib64 -> /usr/lib64 -> /usr/lib for switch_root ELF interpreter resolution"
    );
    assert!(
        install.contains("sshd_config=\"$dest/etc/ssh/sshd_config\"")
            && install
                .contains("AuthorizedKeysFile .ssh/authorized_keys /etc/ssh/authorized_keys/%u")
            && install.contains("conary-selfhost-vm"),
        "generation export fixture must put QEMU SSH access in generation-owned /etc and inject the main sshd_config because the adopted test config does not include drop-ins"
    );

    let recipes = load_recipes(&fixture.join("recipes"))
        .unwrap_or_else(|err| panic!("failed to load fixture recipes: {err}"));
    assert!(
        recipes.contains_key("generation-fixture-rootfs"),
        "generation export fixture recipe must be visible to bootstrap run"
    );
}
