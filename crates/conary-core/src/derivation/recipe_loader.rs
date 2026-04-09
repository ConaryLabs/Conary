// conary-core/src/derivation/recipe_loader.rs

//! Shared recipe discovery for derivation-facing commands.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use tracing::warn;

use crate::recipe::{Recipe, parse_recipe_file};

const RECIPE_SEARCH_DIRS: &[&str] = &["cross-tools", "temp-tools", "system", "tier2", ""];

#[derive(Debug, thiserror::Error)]
pub enum RecipeLoaderError {
    #[error("failed to read recipe directory {path}: {source}")]
    ReadDir {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

pub fn load_recipes(recipe_root: &Path) -> Result<HashMap<String, Recipe>, RecipeLoaderError> {
    let mut recipes = HashMap::new();

    for dir in recipe_dirs(recipe_root) {
        if !dir.exists() {
            continue;
        }

        let entries = std::fs::read_dir(&dir).map_err(|source| RecipeLoaderError::ReadDir {
            path: dir.clone(),
            source,
        })?;

        for entry in entries {
            let entry = entry.map_err(|source| RecipeLoaderError::ReadDir {
                path: dir.clone(),
                source,
            })?;
            let path = entry.path();
            if path.extension().is_none_or(|ext| ext != "toml") {
                continue;
            }

            match parse_recipe_file(&path) {
                Ok(recipe) => {
                    recipes.insert(recipe.package.name.clone(), recipe);
                }
                Err(error) => {
                    warn!("Skipping {}: {error}", path.display());
                }
            }
        }
    }

    Ok(recipes)
}

pub fn find_recipe_path(recipe_root: &Path, package: &str) -> Option<PathBuf> {
    let filename = format!("{package}.toml");

    recipe_dirs(recipe_root)
        .into_iter()
        .map(|dir| dir.join(&filename))
        .find(|path| path.exists())
}

fn recipe_dirs(recipe_root: &Path) -> Vec<PathBuf> {
    RECIPE_SEARCH_DIRS
        .iter()
        .map(|subdir| {
            if subdir.is_empty() {
                recipe_root.to_path_buf()
            } else {
                recipe_root.join(subdir)
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{find_recipe_path, load_recipes};
    use std::fs;
    use std::path::Path;

    fn recipe_toml(name: &str) -> String {
        format!(
            r#"[package]
name = "{name}"
version = "1.0"

[source]
archive = "https://example.com/{name}-1.0.tar.gz"
checksum = "sha256:abc123"

[build]
install = "make install DESTDIR=%(destdir)s"
"#
        )
    }

    fn write_recipe(dir: &Path, relative_path: &str, name: &str) {
        let path = dir.join(relative_path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, recipe_toml(name)).unwrap();
    }

    #[test]
    fn load_recipes_reads_conventional_subdirs() {
        let temp = tempfile::tempdir().unwrap();
        write_recipe(temp.path(), "cross-tools/binutils.toml", "binutils");
        write_recipe(temp.path(), "temp-tools/m4.toml", "m4");
        write_recipe(temp.path(), "system/bash.toml", "bash");
        write_recipe(temp.path(), "tier2/nginx.toml", "nginx");

        let recipes = load_recipes(temp.path()).unwrap();

        assert!(recipes.contains_key("binutils"));
        assert!(recipes.contains_key("m4"));
        assert!(recipes.contains_key("bash"));
        assert!(recipes.contains_key("nginx"));
    }

    #[test]
    fn load_recipes_reads_plain_recipe_root_fallback() {
        let temp = tempfile::tempdir().unwrap();
        write_recipe(temp.path(), "hello.toml", "hello");

        let recipes = load_recipes(temp.path()).unwrap();

        assert!(recipes.contains_key("hello"));
    }

    #[test]
    fn load_recipes_skips_invalid_recipe_with_warning() {
        let temp = tempfile::tempdir().unwrap();
        write_recipe(temp.path(), "system/good.toml", "good");
        fs::create_dir_all(temp.path().join("system")).unwrap();
        fs::write(
            temp.path().join("system/bad.toml"),
            "this is not valid toml",
        )
        .unwrap();

        let recipes = load_recipes(temp.path()).unwrap();

        assert!(recipes.contains_key("good"));
        assert!(!recipes.contains_key("bad"));
    }

    #[test]
    fn find_recipe_path_locates_package_in_standard_roots() {
        let temp = tempfile::tempdir().unwrap();
        write_recipe(temp.path(), "tier2/openssl.toml", "openssl");

        let path = find_recipe_path(temp.path(), "openssl").unwrap();

        assert_eq!(path, temp.path().join("tier2/openssl.toml"));
    }
}
