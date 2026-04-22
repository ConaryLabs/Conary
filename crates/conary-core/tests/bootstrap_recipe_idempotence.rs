// conary-core/tests/bootstrap_recipe_idempotence.rs

use std::path::Path;

use conary_core::recipe::parse_recipe_file;

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .find(|dir| dir.join("recipes/system/flex.toml").is_file())
        .expect("workspace root not found from crate manifest ancestors")
}

#[test]
fn flex_recipe_overwrites_lex_compat_symlinks_on_rerun() {
    let path = workspace_root().join("recipes/system/flex.toml");
    let recipe = parse_recipe_file(&path)
        .unwrap_or_else(|err| panic!("failed to parse {}: {err}", path.display()));
    let install = recipe
        .build
        .install
        .as_deref()
        .expect("flex recipe must define an install step");

    assert!(
        install.contains("ln -svf flex $DESTDIR/usr/bin/lex"),
        "flex install step must force-replace the lex compatibility symlink for reruns"
    );
    assert!(
        install.contains("ln -svf flex.1 $DESTDIR/usr/share/man/man1/lex.1"),
        "flex install step must force-replace the lex.1 compatibility symlink for reruns"
    );
}
