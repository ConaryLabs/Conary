// apps/conary/src/commands/new.rs

use std::env;
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use conary_core::recipe::inference::{
    InferenceOptions, MaterializeOptions, infer_recipe_from_path, scaffold_named_recipe,
    write_recipe_toml,
};
use conary_core::recipe::{LocalSourceSection, SourceSection};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NewOutcome {
    pub created_path: PathBuf,
    pub trace_text: Option<String>,
}

pub async fn cmd_new(
    name: Option<&str>,
    from: Option<&str>,
    output: Option<&str>,
    force: bool,
    explain: bool,
) -> Result<()> {
    let outcome = prepare_new(
        name,
        from.map(Path::new),
        output.map(Path::new),
        force,
        explain,
    )?;

    println!("Created recipe: {}", outcome.created_path.display());
    if let Some(trace_text) = outcome.trace_text {
        println!("Inference trace:");
        println!("{trace_text}");
    }

    Ok(())
}

pub(crate) fn prepare_new(
    name: Option<&str>,
    from: Option<&Path>,
    output: Option<&Path>,
    force: bool,
    explain: bool,
) -> Result<NewOutcome> {
    if name.is_some() && from.is_some() {
        bail!("conary new accepts either <name> or --from, not both");
    }

    if let Some(name) = name {
        let recipe = scaffold_named_recipe(name)?;
        let output_dir = match output {
            Some(output) => absolute(output)?,
            None => current_dir()?.join(name),
        };
        let output_path = output_dir.join("recipe.toml");
        write_recipe_toml(
            &recipe,
            &MaterializeOptions {
                output_path: output_path.clone(),
                force,
                source_override: None,
            },
        )?;

        return Ok(NewOutcome {
            created_path: output_path,
            trace_text: None,
        });
    }

    let source = from.unwrap_or_else(|| Path::new("."));
    let source_root = require_local_source_dir(source)?;
    let result = infer_recipe_from_path(
        &source_root,
        InferenceOptions::for_source_root(&source_root),
    )?;
    let output_path = match output {
        Some(output) => absolute(output)?,
        None => source_root.join("recipe.toml"),
    };
    let source_override = Some(SourceSection::Local(LocalSourceSection {
        path: local_source_path_for_recipe(&source_root, &output_path)?,
    }));
    write_recipe_toml(
        &result.recipe,
        &MaterializeOptions {
            output_path: output_path.clone(),
            force,
            source_override,
        },
    )?;

    Ok(NewOutcome {
        created_path: output_path,
        trace_text: explain.then(|| result.trace.render_human()),
    })
}

fn require_local_source_dir(source: &Path) -> Result<PathBuf> {
    if source.is_dir() {
        return source.canonicalize().map_err(Into::into);
    }

    bail!(
        "conary new --from currently supports local directories only; archive and git targets are M1b Task 5"
    );
}

fn local_source_path_for_recipe(source_root: &Path, output_path: &Path) -> Result<PathBuf> {
    let Some(recipe_dir) = output_path.parent() else {
        bail!(
            "recipe output path {} has no parent directory",
            output_path.display()
        );
    };
    let recipe_dir = normalized_absolute(recipe_dir)?;

    if source_root == recipe_dir {
        return Ok(PathBuf::from("."));
    }

    if let Ok(relative) = source_root.strip_prefix(&recipe_dir) {
        if relative.as_os_str().is_empty() {
            return Ok(PathBuf::from("."));
        }
        return Ok(relative.to_path_buf());
    }

    bail!(
        "cannot write a local-source recipe at {} for source {}; local source paths must stay under the recipe directory",
        output_path.display(),
        source_root.display()
    );
}

fn normalized_absolute(path: &Path) -> Result<PathBuf> {
    let absolute_path = absolute(path)?;
    if absolute_path.exists() {
        absolute_path.canonicalize().map_err(Into::into)
    } else {
        Ok(absolute_path)
    }
}

fn absolute(path: &Path) -> Result<PathBuf> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        current_dir().map(|dir| dir.join(path))
    }
}

fn current_dir() -> Result<PathBuf> {
    env::current_dir().map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use std::env;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::{Mutex, OnceLock};

    use conary_core::recipe::parse_recipe_file;

    use super::super::new::{cmd_new, prepare_new};

    static CWD_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

    fn cwd_lock() -> &'static Mutex<()> {
        CWD_LOCK.get_or_init(|| Mutex::new(()))
    }

    struct CwdGuard {
        previous: PathBuf,
    }

    impl CwdGuard {
        fn enter(path: &Path) -> Self {
            let previous = env::current_dir().unwrap();
            env::set_current_dir(path).unwrap();
            Self { previous }
        }
    }

    impl Drop for CwdGuard {
        fn drop(&mut self) {
            env::set_current_dir(&self.previous).unwrap();
        }
    }

    fn cargo_source_tree(root: &Path) {
        fs::write(
            root.join("Cargo.toml"),
            r#"[package]
name = "demo"
version = "0.2.0"
"#,
        )
        .unwrap();
    }

    #[tokio::test]
    async fn from_current_dir_writes_inferred_recipe_and_trace() {
        let _lock = cwd_lock().lock().unwrap();
        let command_dir = tempfile::tempdir().unwrap();
        cargo_source_tree(command_dir.path());
        let _guard = CwdGuard::enter(command_dir.path());

        cmd_new(None, Some("."), None, false, true).await.unwrap();
        assert!(command_dir.path().join("recipe.toml").exists());

        let helper_dir = tempfile::tempdir().unwrap();
        cargo_source_tree(helper_dir.path());
        let output = helper_dir.path().join("recipe.toml");
        let outcome = prepare_new(
            None,
            Some(helper_dir.path()),
            Some(output.as_path()),
            false,
            true,
        )
        .unwrap();
        assert_eq!(outcome.created_path, output);
        assert!(outcome.trace_text.as_deref().unwrap().contains("cargo"));
        assert!(output.exists());

        let recipe = parse_recipe_file(&output).unwrap();
        assert_eq!(recipe.package.name, "demo");
    }

    #[tokio::test]
    async fn bare_new_uses_current_dir_when_marker_is_supported() {
        let _lock = cwd_lock().lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        cargo_source_tree(dir.path());
        let _guard = CwdGuard::enter(dir.path());

        let outcome = prepare_new(None, None, None::<&Path>, false, false).unwrap();
        assert_eq!(outcome.created_path, dir.path().join("recipe.toml"));
        assert!(outcome.trace_text.is_none());
        assert!(dir.path().join("recipe.toml").exists());
    }

    #[tokio::test]
    async fn from_mode_output_outside_source_points_back_to_source() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        fs::create_dir(&source).unwrap();
        cargo_source_tree(&source);
        let output = dir.path().join("recipe.toml");

        prepare_new(
            None,
            Some(source.as_path()),
            Some(output.as_path()),
            false,
            false,
        )
        .unwrap();

        let recipe = parse_recipe_file(&output).unwrap();
        let local_source = recipe.local_source().unwrap();
        let resolved = local_source
            .resolve_against(output.parent().unwrap())
            .unwrap();

        assert_ne!(local_source.path, PathBuf::from("."));
        assert_eq!(
            resolved.canonicalize().unwrap(),
            source.canonicalize().unwrap()
        );
    }

    #[tokio::test]
    async fn named_new_scaffolds_recipe_under_name() {
        let _lock = cwd_lock().lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let _guard = CwdGuard::enter(dir.path());

        let outcome = prepare_new(Some("demo"), None, None::<&Path>, false, false).unwrap();
        assert_eq!(
            outcome.created_path,
            dir.path().join("demo").join("recipe.toml")
        );
        assert!(outcome.trace_text.is_none());

        let recipe = parse_recipe_file(&dir.path().join("demo").join("recipe.toml")).unwrap();
        assert_eq!(recipe.package.name, "demo");
    }

    #[tokio::test]
    async fn existing_output_without_force_fails() {
        let dir = tempfile::tempdir().unwrap();
        cargo_source_tree(dir.path());
        let output = dir.path().join("recipe.toml");
        fs::write(&output, "already here").unwrap();

        let error = prepare_new(None, Some(dir.path()), Some(output.as_path()), false, false)
            .unwrap_err()
            .to_string();
        assert!(error.contains("already exists"));
    }
}
