// apps/conary/src/commands/new.rs

//! Exact named recipe scaffolding.

use std::env;
use std::path::{Path, PathBuf};

use anyhow::Result;
use conary_core::recipe::{MaterializeOptions, scaffold_named_recipe, write_recipe_toml};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NewOutcome {
    pub created_path: PathBuf,
}

pub async fn cmd_new(name: &str, output: Option<&str>, force: bool) -> Result<()> {
    let outcome = prepare_new(name, output.map(Path::new), force)?;
    println!("Created recipe: {}", outcome.created_path.display());
    Ok(())
}

pub(crate) fn prepare_new(name: &str, output: Option<&Path>, force: bool) -> Result<NewOutcome> {
    let recipe = scaffold_named_recipe(name)?;
    let current_dir = env::current_dir()?;
    let output_dir = match output {
        Some(path) if path.is_absolute() => path.to_path_buf(),
        Some(path) => current_dir.join(path),
        None => current_dir.join(name),
    };
    let output_path = output_dir.join("recipe.toml");

    write_recipe_toml(
        &recipe,
        &MaterializeOptions {
            output_path: output_path.clone(),
            force,
        },
    )?;

    Ok(NewOutcome {
        created_path: output_path,
    })
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::prepare_new;

    #[test]
    fn named_scaffold_writes_recipe_to_explicit_output_directory() {
        let output = tempfile::tempdir().unwrap();
        let outcome = prepare_new("hello", Some(output.path()), false).unwrap();
        assert_eq!(outcome.created_path, output.path().join("recipe.toml"));
        let rendered = fs::read_to_string(outcome.created_path).unwrap();
        assert!(rendered.contains("name = \"hello\""));
        assert!(rendered.contains("version = \"0.1.0\""));
    }

    #[test]
    fn named_scaffold_refuses_overwrite_without_force() {
        let output = tempfile::tempdir().unwrap();
        prepare_new("hello", Some(output.path()), false).unwrap();
        let error = prepare_new("hello", Some(output.path()), false).unwrap_err();
        assert!(error.to_string().contains("already exists"));
    }
}
