// apps/conary/src/commands/new.rs

use std::env;
use std::fs;
use std::path::{Component, Path, PathBuf};

use anyhow::{Result, bail};
use conary_core::recipe::inference::{
    InferenceOptions, MaterializeOptions, ResolvedSourceTree, SourceTargetKind,
    infer_recipe_from_path, resolve_new_from_target, scaffold_named_recipe, write_recipe_toml,
};
use conary_core::recipe::{LocalSourceSection, RemoteSourceSection, SourceSection};

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
    let resolved_source = resolve_new_from_target(&source.to_string_lossy())?;
    let result = infer_recipe_from_path(
        &resolved_source.root,
        InferenceOptions::for_source_root(&resolved_source.root),
    )?;
    let output_path = match output {
        Some(output) => absolute(output)?,
        None if resolved_source.kind == SourceTargetKind::Directory => {
            resolved_source.root.join("recipe.toml")
        }
        None => current_dir()?.join("recipe.toml"),
    };
    let source_override = Some(source_override_for_resolved_target(
        &resolved_source,
        &output_path,
        force,
    )?);
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

fn source_override_for_resolved_target(
    source: &ResolvedSourceTree,
    output_path: &Path,
    force: bool,
) -> Result<SourceSection> {
    match source.kind {
        SourceTargetKind::Directory => Ok(SourceSection::Local(LocalSourceSection {
            path: local_source_path_for_recipe(&source.root, output_path)?,
        })),
        SourceTargetKind::Archive => archive_source_override(source, output_path, force),
        SourceTargetKind::Git => git_source_override(source, output_path, force),
    }
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

fn archive_source_override(
    source: &ResolvedSourceTree,
    output_path: &Path,
    force: bool,
) -> Result<SourceSection> {
    let checksum = source
        .provenance
        .archive_checksum
        .clone()
        .ok_or_else(|| anyhow::anyhow!("archive target missing checksum provenance"))?;

    let archive = if is_http_url(&source.original) {
        source.original.clone()
    } else {
        let original = Path::new(&source.original);
        let recipe_dir = recipe_dir_for_output(output_path)?;
        let stable_path = recipe_dir
            .join("sources")
            .join(safe_source_name(&source.original, "source.tar"));
        copy_file_to_stable_path(original, &stable_path, force)?;
        relative_path_string(&recipe_dir, &stable_path)?
    };

    Ok(SourceSection::Remote(RemoteSourceSection {
        archive,
        checksum,
        signature: None,
        additional: Vec::new(),
        extract_dir: None,
    }))
}

fn git_source_override(
    source: &ResolvedSourceTree,
    output_path: &Path,
    force: bool,
) -> Result<SourceSection> {
    let recipe_dir = recipe_dir_for_output(output_path)?;
    let stable_path = recipe_dir
        .join("sources")
        .join(safe_source_name(&source.original, "source"));
    copy_git_source_to_stable_path(&source.root, &stable_path, force)?;

    Ok(SourceSection::Local(LocalSourceSection {
        path: PathBuf::from(relative_path_string(&recipe_dir, &stable_path)?),
    }))
}

fn copy_file_to_stable_path(source: &Path, dest: &Path, force: bool) -> Result<()> {
    let canonical_source = source.canonicalize()?;
    let canonical_dest = dest.canonicalize().ok();
    if canonical_dest.as_deref() == Some(canonical_source.as_path()) {
        return Ok(());
    }

    if dest.exists() && !force {
        bail!(
            "{} already exists; pass force to overwrite it",
            dest.display()
        );
    }
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }

    if dest.exists() {
        let temp_path = temp_sibling_path(dest);
        if temp_path.exists() {
            fs::remove_file(&temp_path)?;
        }
        fs::copy(&canonical_source, &temp_path)?;
        fs::rename(&temp_path, dest)?;
    } else {
        fs::copy(&canonical_source, dest)?;
    }

    Ok(())
}

fn copy_git_source_to_stable_path(source: &Path, dest: &Path, force: bool) -> Result<()> {
    if dest.exists() {
        if !force {
            bail!(
                "{} already exists; pass force to overwrite it",
                dest.display()
            );
        }
        fs::remove_dir_all(dest)?;
    }
    copy_git_dir_recursive(source, dest)
}

fn copy_git_dir_recursive(source: &Path, dest: &Path) -> Result<()> {
    fs::create_dir_all(dest)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        if entry.file_name() == ".git" {
            continue;
        }
        let source_path = entry.path();
        let dest_path = dest.join(entry.file_name());
        let metadata = fs::symlink_metadata(&source_path)?;
        let file_type = metadata.file_type();
        if file_type.is_symlink() {
            copy_safe_relative_symlink(&source_path, &dest_path)?;
        } else if file_type.is_dir() {
            copy_git_dir_recursive(&source_path, &dest_path)?;
        } else if file_type.is_file() {
            fs::copy(&source_path, &dest_path)?;
        }
    }
    Ok(())
}

fn copy_safe_relative_symlink(source: &Path, dest: &Path) -> Result<()> {
    let target = fs::read_link(source)?;
    validate_safe_relative_symlink(source, &target)?;
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, dest)?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        bail!(
            "cannot materialize symlink {} on this platform",
            source.display()
        )
    }
}

fn validate_safe_relative_symlink(source: &Path, target: &Path) -> Result<()> {
    if target.as_os_str().is_empty() || target.is_absolute() {
        bail!(
            "refusing to materialize unsafe symlink {} -> {}",
            source.display(),
            target.display()
        );
    }
    if target.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        bail!(
            "refusing to materialize unsafe symlink {} -> {}",
            source.display(),
            target.display()
        );
    }
    Ok(())
}

fn temp_sibling_path(dest: &Path) -> PathBuf {
    let mut temp_name = dest
        .file_name()
        .map(|name| name.to_os_string())
        .unwrap_or_else(|| "source.tmp".into());
    temp_name.push(format!(".tmp.{}", std::process::id()));
    dest.with_file_name(temp_name)
}

fn recipe_dir_for_output(output_path: &Path) -> Result<PathBuf> {
    let Some(recipe_dir) = output_path.parent() else {
        bail!(
            "recipe output path {} has no parent directory",
            output_path.display()
        );
    };
    normalized_absolute(recipe_dir)
}

fn relative_path_string(base: &Path, path: &Path) -> Result<String> {
    let relative = path.strip_prefix(base).map_err(|_| {
        anyhow::anyhow!(
            "stable source path {} is not under recipe directory {}",
            path.display(),
            base.display()
        )
    })?;
    Ok(relative.to_string_lossy().replace('\\', "/"))
}

fn safe_source_name(original: &str, fallback: &str) -> String {
    let trimmed = original.trim_end_matches(['/', '\\']);
    let name = Path::new(trimmed)
        .file_name()
        .and_then(|name| name.to_str())
        .or_else(|| trimmed.rsplit('/').find(|name| !name.is_empty()))
        .unwrap_or(fallback);
    let safe = name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_') {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>();
    let safe = safe.trim_matches(['.', '-']).to_string();
    if safe.is_empty() {
        fallback.to_string()
    } else {
        safe
    }
}

fn is_http_url(value: &str) -> bool {
    value.starts_with("http://") || value.starts_with("https://")
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
    use std::sync::OnceLock;

    use conary_core::recipe::parse_recipe_file;
    use tokio::sync::Mutex;

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

    fn write_tar_archive(path: &Path) {
        use flate2::Compression;
        use flate2::write::GzEncoder;
        use std::io::Write;
        use tar::{Builder, EntryType, Header};

        fn append_file(builder: &mut Builder<impl Write>, path: &str, body: &[u8]) {
            let mut header = Header::new_gnu();
            header.set_entry_type(EntryType::Regular);
            header.set_size(body.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder.append_data(&mut header, path, body).unwrap();
        }

        let file = fs::File::create(path).unwrap();
        let encoder = GzEncoder::new(file, Compression::default());
        let mut builder = Builder::new(encoder);
        append_file(
            &mut builder,
            "source/Cargo.toml",
            br#"[package]
name = "archive-new"
version = "0.4.0"
"#,
        );
        builder.finish().unwrap();
    }

    fn git_available() -> bool {
        std::process::Command::new("git")
            .arg("--version")
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false)
    }

    fn create_git_repo(root: &Path) {
        cargo_source_tree(root);
        for args in [
            vec!["init"],
            vec!["config", "user.email", "tests@example.invalid"],
            vec!["config", "user.name", "Conary Tests"],
            vec!["add", "Cargo.toml"],
            vec!["commit", "-m", "seed"],
        ] {
            assert!(
                std::process::Command::new("git")
                    .args(args)
                    .current_dir(root)
                    .output()
                    .unwrap()
                    .status
                    .success()
            );
        }
    }

    #[tokio::test]
    async fn from_current_dir_writes_inferred_recipe_and_trace() {
        let _lock = cwd_lock().lock().await;
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
        let _lock = cwd_lock().lock().await;
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
    async fn from_local_archive_writes_recipe_and_stable_archive_source() {
        let _lock = cwd_lock().lock().await;
        let command_dir = tempfile::tempdir().unwrap();
        let archive = command_dir.path().join("source.tgz");
        write_tar_archive(&archive);
        let _guard = CwdGuard::enter(command_dir.path());

        let outcome = prepare_new(None, Some(archive.as_path()), None::<&Path>, false, true)
            .expect("archive --from should materialize");

        assert_eq!(outcome.created_path, command_dir.path().join("recipe.toml"));
        assert!(outcome.trace_text.as_deref().unwrap().contains("cargo"));
        let recipe = parse_recipe_file(&outcome.created_path).unwrap();
        assert_eq!(recipe.package.name, "archive-new");
        let source = recipe.remote_source().unwrap();
        assert_eq!(source.archive, "sources/source.tgz");
        assert!(source.checksum.starts_with("sha256:"));
        assert!(command_dir.path().join("sources/source.tgz").is_file());
    }

    #[tokio::test]
    async fn from_local_archive_already_in_sources_reuses_input_archive() {
        let _lock = cwd_lock().lock().await;
        let command_dir = tempfile::tempdir().unwrap();
        let sources_dir = command_dir.path().join("sources");
        fs::create_dir(&sources_dir).unwrap();
        let archive = sources_dir.join("source.tgz");
        write_tar_archive(&archive);
        let before = fs::read(&archive).unwrap();
        let _guard = CwdGuard::enter(command_dir.path());

        let outcome =
            prepare_new(None, Some(archive.as_path()), None::<&Path>, false, false).unwrap();

        assert_eq!(outcome.created_path, command_dir.path().join("recipe.toml"));
        assert_eq!(fs::read(&archive).unwrap(), before);
        let recipe = parse_recipe_file(&outcome.created_path).unwrap();
        let source = recipe.remote_source().unwrap();
        assert_eq!(source.archive, "sources/source.tgz");
    }

    #[tokio::test]
    async fn from_local_archive_already_in_sources_is_safe_with_force() {
        let _lock = cwd_lock().lock().await;
        let command_dir = tempfile::tempdir().unwrap();
        let sources_dir = command_dir.path().join("sources");
        fs::create_dir(&sources_dir).unwrap();
        let archive = sources_dir.join("source.tgz");
        write_tar_archive(&archive);
        let before = fs::read(&archive).unwrap();
        let _guard = CwdGuard::enter(command_dir.path());

        prepare_new(None, Some(archive.as_path()), None::<&Path>, true, false).unwrap();

        assert_eq!(fs::read(&archive).unwrap(), before);
    }

    #[tokio::test]
    async fn from_git_target_writes_recipe_and_stable_local_source() {
        if !git_available() {
            eprintln!("skipping git new --from test because git is not installed");
            return;
        }

        let _lock = cwd_lock().lock().await;
        let command_dir = tempfile::tempdir().unwrap();
        let repo = command_dir.path().join("demo.git");
        fs::create_dir(&repo).unwrap();
        create_git_repo(&repo);
        let _guard = CwdGuard::enter(command_dir.path());

        let outcome = prepare_new(None, Some(repo.as_path()), None::<&Path>, false, false).unwrap();

        assert_eq!(outcome.created_path, command_dir.path().join("recipe.toml"));
        let recipe = parse_recipe_file(&outcome.created_path).unwrap();
        assert_eq!(recipe.package.name, "demo");
        let local_source = recipe.local_source().unwrap();
        assert_eq!(local_source.path, PathBuf::from("sources/demo.git"));
        assert!(
            command_dir
                .path()
                .join("sources/demo.git/Cargo.toml")
                .is_file()
        );
    }

    #[tokio::test]
    async fn from_git_target_does_not_materialize_git_metadata() {
        if !git_available() {
            eprintln!("skipping git new --from test because git is not installed");
            return;
        }

        let _lock = cwd_lock().lock().await;
        let command_dir = tempfile::tempdir().unwrap();
        let repo = command_dir.path().join("demo.git");
        fs::create_dir(&repo).unwrap();
        create_git_repo(&repo);
        let _guard = CwdGuard::enter(command_dir.path());

        prepare_new(None, Some(repo.as_path()), None::<&Path>, false, false).unwrap();

        assert!(
            command_dir
                .path()
                .join("sources/demo.git/Cargo.toml")
                .is_file()
        );
        assert!(
            !command_dir.path().join("sources/demo.git/.git").exists(),
            "materialized git source must not persist clone metadata"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn from_git_target_preserves_safe_relative_symlink() {
        use std::os::unix::fs as unix_fs;

        if !git_available() {
            eprintln!("skipping git new --from test because git is not installed");
            return;
        }

        let _lock = cwd_lock().lock().await;
        let command_dir = tempfile::tempdir().unwrap();
        let repo = command_dir.path().join("demo.git");
        fs::create_dir(&repo).unwrap();
        create_git_repo(&repo);
        fs::write(repo.join("README.md"), "hello\n").unwrap();
        unix_fs::symlink("README.md", repo.join("README.link")).unwrap();
        assert!(
            std::process::Command::new("git")
                .args(["add", "README.md", "README.link"])
                .current_dir(&repo)
                .output()
                .unwrap()
                .status
                .success()
        );
        assert!(
            std::process::Command::new("git")
                .args(["commit", "-m", "add symlink"])
                .current_dir(&repo)
                .output()
                .unwrap()
                .status
                .success()
        );
        let _guard = CwdGuard::enter(command_dir.path());

        prepare_new(None, Some(repo.as_path()), None::<&Path>, false, false).unwrap();

        let link = command_dir.path().join("sources/demo.git/README.link");
        let metadata = fs::symlink_metadata(&link).unwrap();
        assert!(metadata.file_type().is_symlink());
        assert_eq!(fs::read_link(link).unwrap(), PathBuf::from("README.md"));
    }

    #[tokio::test]
    async fn named_new_scaffolds_recipe_under_name() {
        let _lock = cwd_lock().lock().await;
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
