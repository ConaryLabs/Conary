// apps/conary/src/commands/try_session/watch_source.rs
//! Source identity and debounce for try watch mode.

use std::fs::File;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use conary_core::hash::{HashAlgorithm, Hasher};
use conary_core::recipe::hermetic::source_identity::{
    canonical_local_file_list, detect_ci_mode, validate_canonical_local_file_list,
};
use conary_core::recipe::inference::{
    CookTarget, InferenceOptions, SourceTargetKind, infer_recipe_from_path, resolve_cook_target,
};
use conary_core::recipe::{Recipe, is_remote_url, parse_recipe_file};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum WatchSourceMode {
    ExplicitRecipe,
    InferredSourceTree,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct WatchSourceSet {
    pub(super) mode: WatchSourceMode,
    pub(super) recipe_path: Option<PathBuf>,
    pub(super) local_roots: Vec<PathBuf>,
    pub(super) local_files: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct WatchIdentity {
    pub(super) digest: String,
    pub(super) file_count: usize,
}

pub(super) fn resolve_watch_source_set(
    target: Option<&str>,
    recipe: Option<&str>,
) -> Result<WatchSourceSet> {
    match resolve_cook_target(target, recipe)? {
        CookTarget::RecipeFile(recipe_path) => {
            let parsed = parse_recipe_file(&recipe_path)
                .with_context(|| format!("failed to parse recipe {}", recipe_path.display()))?;
            watch_source_set_for_recipe(recipe_path, &parsed)
        }
        CookTarget::SourceTree(source_tree) => {
            if source_tree.kind != SourceTargetKind::Directory {
                bail!(
                    "conary try --watch only supports local source directories and recipe projects"
                );
            }
            let _ = infer_recipe_from_path(
                &source_tree.root,
                InferenceOptions::for_source_root(source_tree.root.clone()),
            )
            .with_context(|| {
                format!(
                    "failed to infer recipe from watched source tree {}",
                    source_tree.root.display()
                )
            })?;
            Ok(WatchSourceSet {
                mode: WatchSourceMode::InferredSourceTree,
                recipe_path: None,
                local_roots: vec![source_tree.root],
                local_files: Vec::new(),
            })
        }
    }
}

fn watch_source_set_for_recipe(recipe_path: PathBuf, recipe: &Recipe) -> Result<WatchSourceSet> {
    let recipe_path = recipe_path.canonicalize()?;
    let recipe_dir = recipe_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let recipe_dir = recipe_dir.canonicalize()?;
    let mut local_roots = Vec::new();
    let mut local_files = vec![recipe_path.clone()];

    if let Some(local) = recipe.local_source() {
        let source_root = local
            .resolve_against(&recipe_dir)
            .map_err(anyhow::Error::msg)?
            .canonicalize()
            .with_context(|| "failed to canonicalize watched local source root")?;
        if !source_root.starts_with(&recipe_dir) {
            bail!("watched local source root must stay within the recipe directory");
        }
        local_roots.push(source_root);
    }

    if let Some(remote) = recipe.remote_source() {
        for additional in &remote.additional {
            let url = recipe.substitute(&additional.url, "");
            if !is_remote_url(&url) {
                local_files.push(resolve_local_recipe_file(&recipe_dir, &url)?);
            }
        }
    }

    if let Some(patches) = &recipe.patches {
        for patch in &patches.files {
            let patch_file = recipe.substitute(&patch.file, "");
            if !is_remote_url(&patch_file) {
                local_files.push(resolve_local_recipe_file(&recipe_dir, &patch_file)?);
            }
        }
    }

    local_files.sort();
    local_files.dedup();
    local_roots.sort();
    local_roots.dedup();

    Ok(WatchSourceSet {
        mode: WatchSourceMode::ExplicitRecipe,
        recipe_path: Some(recipe_path),
        local_roots,
        local_files,
    })
}

fn resolve_local_recipe_file(recipe_dir: &Path, relative: &str) -> Result<PathBuf> {
    let path = Path::new(relative);
    if path.as_os_str().is_empty() || path.is_absolute() {
        bail!("watched local recipe file must be relative to the recipe directory: {relative}");
    }
    if path.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        bail!("watched local recipe file must stay within the recipe directory: {relative}");
    }
    let canonical_dir = recipe_dir.canonicalize()?;
    let canonical_file = canonical_dir.join(path).canonicalize()?;
    if !canonical_file.starts_with(&canonical_dir) {
        bail!("watched local recipe file must stay within the recipe directory: {relative}");
    }
    Ok(canonical_file)
}

pub(super) fn compute_watch_identity(source_set: &WatchSourceSet) -> Result<WatchIdentity> {
    let ci_mode = detect_ci_mode();
    let mut hasher = Hasher::new(HashAlgorithm::Sha256);
    let mut file_count = 0usize;

    hasher.update(format!("{:?}\0", source_set.mode).as_bytes());
    if let Some(recipe_path) = &source_set.recipe_path {
        hasher.update(recipe_path.to_string_lossy().as_bytes());
        hasher.update(b"\0");
    }
    for root in &source_set.local_roots {
        hasher.update(root.to_string_lossy().as_bytes());
        hasher.update(b"\0");
    }
    for file in &source_set.local_files {
        hasher.update(file.to_string_lossy().as_bytes());
        hasher.update(b"\0");
    }

    for file in &source_set.local_files {
        let mut reader = File::open(file)
            .with_context(|| format!("failed to open watched file {}", file.display()))?;
        let file_hash = conary_core::hash::sha256_reader_hex(&mut reader)
            .with_context(|| format!("failed to hash watched file {}", file.display()))?;
        hasher.update(file.to_string_lossy().as_bytes());
        hasher.update(format!("sha256:{file_hash}").as_bytes());
        file_count += 1;
    }

    for root in &source_set.local_roots {
        let files = canonical_local_file_list(root, ci_mode)?;
        validate_canonical_local_file_list(root, &files)?;
        for file in files {
            hasher.update(root.to_string_lossy().as_bytes());
            hasher.update(file.relative_path.to_string_lossy().as_bytes());
            hasher.update(file.hash.as_bytes());
            if let Some(target) = file.symlink_target {
                hasher.update(target.to_string_lossy().as_bytes());
            }
            file_count += 1;
        }
    }

    let hash = hasher.finalize();
    Ok(WatchIdentity {
        digest: format!("sha256:{}", hash.value),
        file_count,
    })
}

#[derive(Debug, Clone)]
pub(super) struct DebounceState {
    delay: Duration,
    ready_at: Option<Instant>,
}

impl DebounceState {
    pub(super) fn new(delay: Duration) -> Self {
        Self {
            delay,
            ready_at: None,
        }
    }

    pub(super) fn record_wakeup(&mut self, now: Instant) -> Option<Instant> {
        self.ready_at = Some(now + self.delay);
        None
    }

    #[cfg(test)]
    pub(super) fn ready_at(&self) -> Option<Instant> {
        self.ready_at
    }

    pub(super) fn clear(&mut self) {
        self.ready_at = None;
    }

    pub(super) fn take_ready(&mut self, now: Instant) -> Option<()> {
        if self.ready_at.is_some_and(|ready| now >= ready) {
            self.ready_at = None;
            Some(())
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_recipe(root: &std::path::Path) {
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("patches")).unwrap();
        std::fs::create_dir_all(root.join("extras")).unwrap();
        std::fs::write(root.join("src/main.txt"), "hello\n").unwrap();
        std::fs::write(root.join("patches/fix.patch"), "diff --git a/a b/a\n").unwrap();
        std::fs::write(root.join("extras/data.txt"), "extra\n").unwrap();
        std::fs::write(
            root.join("recipe.toml"),
            r#"
[package]
name = "watch-demo"
version = "1.0.0"

[source]
archive = "https://example.invalid/watch-demo-%(version)s.tar.gz"
checksum = "sha256:0000000000000000000000000000000000000000000000000000000000000000"
additional = [
    { url = "extras/data.txt", checksum = "sha256:1111111111111111111111111111111111111111111111111111111111111111" }
]

[build]
install = "mkdir -p %(destdir)s/usr/share/watch-demo && cp main.txt %(destdir)s/usr/share/watch-demo/main.txt"

[patches]
files = [{ file = "patches/fix.patch", strip = 1 }]
"#,
        )
        .unwrap();
    }

    fn switch_recipe_to_local_source(root: &std::path::Path) {
        let recipe = root.join("recipe.toml");
        let edited = std::fs::read_to_string(&recipe)
            .unwrap()
            .replace(
                r#"[source]
archive = "https://example.invalid/watch-demo-%(version)s.tar.gz"
checksum = "sha256:0000000000000000000000000000000000000000000000000000000000000000"
additional = [
    { url = "extras/data.txt", checksum = "sha256:1111111111111111111111111111111111111111111111111111111111111111" }
]"#,
                r#"[source]
path = "src""#,
            );
        std::fs::write(recipe, edited).unwrap();
    }

    #[test]
    fn source_set_tracks_recipe_local_sources_patches_and_additional_files() {
        let temp = tempfile::tempdir().unwrap();
        write_recipe(temp.path());
        switch_recipe_to_local_source(temp.path());

        let set = resolve_watch_source_set(
            Some(temp.path().join("recipe.toml").to_str().unwrap()),
            None,
        )
        .unwrap();

        assert_eq!(set.mode, WatchSourceMode::ExplicitRecipe);
        let recipe_path = temp.path().join("recipe.toml").canonicalize().unwrap();
        assert_eq!(set.recipe_path.as_deref(), Some(recipe_path.as_path()));
        assert!(set.local_roots.iter().any(|root| root.ends_with("src")));
        assert!(
            set.local_files
                .iter()
                .any(|path| path.ends_with("patches/fix.patch"))
        );
        assert!(
            !set.local_files
                .iter()
                .any(|path| path.ends_with("extras/data.txt"))
        );

        write_recipe(temp.path());
        let archive_set = resolve_watch_source_set(
            Some(temp.path().join("recipe.toml").to_str().unwrap()),
            None,
        )
        .unwrap();

        assert!(
            archive_set
                .local_files
                .iter()
                .any(|path| path.ends_with("extras/data.txt")),
            "local additional source should be watched: {:?}",
            archive_set.local_files
        );
    }

    #[test]
    fn source_identity_changes_when_patch_changes() {
        let temp = tempfile::tempdir().unwrap();
        write_recipe(temp.path());
        let recipe = temp.path().join("recipe.toml");
        let set = resolve_watch_source_set(Some(recipe.to_str().unwrap()), None).unwrap();
        let first = compute_watch_identity(&set).unwrap();

        std::fs::write(temp.path().join("patches/fix.patch"), "changed patch\n").unwrap();
        let patch_changed = compute_watch_identity(&set).unwrap();

        assert_ne!(first.digest, patch_changed.digest);
    }

    #[cfg(unix)]
    #[test]
    fn watch_source_set_rejects_symlink_escape_in_patch_file() {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("project");
        write_recipe(&project);
        let outside = temp.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("escape.patch"), "secret\n").unwrap();
        std::fs::remove_file(project.join("patches/fix.patch")).unwrap();
        std::os::unix::fs::symlink(
            "../../outside/escape.patch",
            project.join("patches/fix.patch"),
        )
        .unwrap();

        let err =
            resolve_watch_source_set(Some(project.join("recipe.toml").to_str().unwrap()), None)
                .unwrap_err();

        assert!(
            err.to_string()
                .contains("must stay within the recipe directory"),
            "{err:#}"
        );
    }

    #[test]
    fn identity_changes_when_recipe_points_to_different_local_source_root() {
        let temp = tempfile::tempdir().unwrap();
        write_recipe(temp.path());
        switch_recipe_to_local_source(temp.path());
        std::fs::create_dir_all(temp.path().join("src2")).unwrap();
        std::fs::write(temp.path().join("src2/main.txt"), "hello two\n").unwrap();
        let recipe = temp.path().join("recipe.toml");
        let first_set = resolve_watch_source_set(Some(recipe.to_str().unwrap()), None).unwrap();
        let first = compute_watch_identity(&first_set).unwrap();

        let edited = std::fs::read_to_string(&recipe)
            .unwrap()
            .replace("path = \"src\"", "path = \"src2\"");
        std::fs::write(&recipe, edited).unwrap();
        let second_set = resolve_watch_source_set(Some(recipe.to_str().unwrap()), None).unwrap();
        let second = compute_watch_identity(&second_set).unwrap();

        assert_ne!(first.digest, second.digest);
    }

    #[test]
    fn debounce_coalesces_rapid_changes() {
        let start = Instant::now();
        let mut debounce = DebounceState::new(Duration::from_millis(750));
        assert_eq!(debounce.record_wakeup(start), None);
        assert_eq!(
            debounce.record_wakeup(start + Duration::from_millis(100)),
            None
        );
        assert_eq!(
            debounce.ready_at(),
            Some(start + Duration::from_millis(850))
        );
        assert!(
            debounce
                .take_ready(start + Duration::from_millis(849))
                .is_none()
        );
        assert!(
            debounce
                .take_ready(start + Duration::from_millis(850))
                .is_some()
        );
    }
}
