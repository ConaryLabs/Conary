// conary-core/src/recipe/inference/targets.rs

use std::ffi::OsStr;
use std::fs;
use std::fs::File;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use flate2::read::GzDecoder;
use tempfile::TempDir;

use crate::error::{Error, Result};
use crate::hash::sha256_reader_hex;
use crate::recipe::kitchen::archive::download_file;

#[derive(Debug)]
pub enum CookTarget {
    RecipeFile(PathBuf),
    SourceTree(ResolvedSourceTree),
}

#[derive(Debug)]
pub struct ResolvedSourceTree {
    pub root: PathBuf,
    pub temporary: Option<TempDir>,
    pub original: String,
    pub kind: SourceTargetKind,
    pub provenance: SourceTargetProvenance,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceTargetKind {
    Directory,
    Archive,
    Git,
}

#[derive(Debug, Clone)]
pub struct SourceTargetProvenance {
    pub original: String,
    pub kind: SourceTargetKind,
    pub archive_checksum: Option<String>,
    pub git_commit: Option<String>,
}

pub fn resolve_cook_target(
    target: Option<&str>,
    explicit_recipe: Option<&str>,
) -> Result<CookTarget> {
    if let Some(recipe) = explicit_recipe {
        return Ok(CookTarget::RecipeFile(canonical_file(recipe, "--recipe")?));
    }

    let Some(target) = target else {
        let cwd = std::env::current_dir()?;
        let recipe = cwd.join("recipe.toml");
        if recipe.is_file() {
            return Ok(CookTarget::RecipeFile(recipe.canonicalize()?));
        }
        return Ok(CookTarget::SourceTree(resolve_directory_target(
            &cwd,
            cwd.display().to_string(),
        )?));
    };

    let path = Path::new(target);
    if is_toml_file(path) && path.is_file() {
        return Ok(CookTarget::RecipeFile(path.canonicalize()?));
    }
    if looks_like_git_target(target) && !is_supported_archive_target(target) {
        return Ok(CookTarget::SourceTree(resolve_git_target(target)?));
    }
    if path.is_dir() {
        let recipe = path.join("recipe.toml");
        if recipe.is_file() {
            return Ok(CookTarget::RecipeFile(recipe.canonicalize()?));
        }
        return Ok(CookTarget::SourceTree(resolve_directory_target(
            path, target,
        )?));
    }
    if is_supported_archive_target(target) {
        return Ok(CookTarget::SourceTree(resolve_archive_target(target)?));
    }

    Err(unsupported_target_error(target))
}

pub fn resolve_new_from_target(target: &str) -> Result<ResolvedSourceTree> {
    if looks_like_git_target(target) && !is_supported_archive_target(target) {
        return resolve_git_target(target);
    }

    let path = Path::new(target);
    if path.is_dir() {
        return resolve_directory_target(path, target);
    }
    if is_supported_archive_target(target) {
        return resolve_archive_target(target);
    }

    Err(unsupported_target_error(target))
}

fn canonical_file(path: &str, label: &str) -> Result<PathBuf> {
    let path = Path::new(path);
    if !path.is_file() {
        return Err(Error::NotFound(format!(
            "{label} path is not a file: {}",
            path.display()
        )));
    }
    Ok(path.canonicalize()?)
}

fn resolve_directory_target(
    path: &Path,
    original: impl Into<String>,
) -> Result<ResolvedSourceTree> {
    if !path.is_dir() {
        return Err(Error::NotFound(format!(
            "Source directory not found: {}",
            path.display()
        )));
    }
    let original = original.into();
    let kind = SourceTargetKind::Directory;
    Ok(ResolvedSourceTree {
        root: path.canonicalize()?,
        temporary: None,
        provenance: SourceTargetProvenance {
            original: original.clone(),
            kind,
            archive_checksum: None,
            git_commit: None,
        },
        original,
        kind,
    })
}

fn resolve_archive_target(target: &str) -> Result<ResolvedSourceTree> {
    let temporary = TempDir::new()?;
    let archive_path = temporary.path().join(archive_filename(target));
    if is_http_url(target) {
        download_file(target, &archive_path)?;
    } else {
        let source = Path::new(target);
        if !source.is_file() {
            return Err(Error::NotFound(format!(
                "Archive source not found: {}",
                source.display()
            )));
        }
        fs::copy(source, &archive_path).map_err(|error| {
            Error::IoError(format!(
                "Failed to copy archive {} to {}: {error}",
                source.display(),
                archive_path.display()
            ))
        })?;
    }

    let checksum = sha256_file_prefixed(&archive_path)?;
    let extraction_root = temporary.path().join("source");
    fs::create_dir_all(&extraction_root)?;
    safe_extract_archive(&archive_path, &extraction_root)?;
    let root = extracted_source_root(&extraction_root)?;
    let original = target.to_string();
    let kind = SourceTargetKind::Archive;

    Ok(ResolvedSourceTree {
        root,
        temporary: Some(temporary),
        provenance: SourceTargetProvenance {
            original: original.clone(),
            kind,
            archive_checksum: Some(checksum),
            git_commit: None,
        },
        original,
        kind,
    })
}

fn resolve_git_target(target: &str) -> Result<ResolvedSourceTree> {
    ensure_git_available()?;

    let temporary = TempDir::new()?;
    let checkout = temporary.path().join("checkout");
    let output = Command::new("git")
        .args(["clone", "--depth", "1", target])
        .arg(&checkout)
        .output()
        .map_err(|error| Error::ConfigError(format!("git clone failed to start: {error}")))?;
    if !output.status.success() {
        return Err(Error::ConfigError(format!(
            "git clone --depth 1 failed for {target}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }

    let commit = git_head_commit(&checkout)?;
    let root = checkout.canonicalize()?;
    let original = target.to_string();
    let kind = SourceTargetKind::Git;

    Ok(ResolvedSourceTree {
        root,
        temporary: Some(temporary),
        provenance: SourceTargetProvenance {
            original: original.clone(),
            kind,
            archive_checksum: None,
            git_commit: Some(commit),
        },
        original,
        kind,
    })
}

fn ensure_git_available() -> Result<()> {
    let output = Command::new("git")
        .arg("--version")
        .output()
        .map_err(|error| {
            Error::ConfigError(format!("git is required for git source targets: {error}"))
        })?;
    if !output.status.success() {
        return Err(Error::ConfigError(
            "git is required for git source targets but `git --version` failed".to_string(),
        ));
    }
    Ok(())
}

fn git_head_commit(checkout: &Path) -> Result<String> {
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(checkout)
        .output()
        .map_err(|error| Error::ConfigError(format!("git rev-parse failed to start: {error}")))?;
    if !output.status.success() {
        return Err(Error::ConfigError(format!(
            "git rev-parse HEAD failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn safe_extract_archive(archive_path: &Path, destination: &Path) -> Result<()> {
    validate_archive_entries(archive_path)?;
    unpack_archive(archive_path, destination)
}

fn validate_archive_entries(archive_path: &Path) -> Result<()> {
    let reader = archive_reader(archive_path)?;
    let mut archive = tar::Archive::new(reader);
    for entry in archive.entries()? {
        let entry = entry?;
        let entry_type = entry.header().entry_type();
        let path = entry.path()?.into_owned();
        validate_archive_path(&path)?;
        if entry_type.is_symlink() || entry_type.is_hard_link() {
            return Err(Error::PathTraversal(format!(
                "unsafe archive entry {}: symlink and hard link entries are not supported",
                path.display()
            )));
        }
    }
    Ok(())
}

fn unpack_archive(archive_path: &Path, destination: &Path) -> Result<()> {
    let reader = archive_reader(archive_path)?;
    let mut archive = tar::Archive::new(reader);
    archive.unpack(destination).map_err(|error| {
        Error::IoError(format!(
            "Failed to extract archive {}: {error}",
            archive_path.display()
        ))
    })
}

fn archive_reader(archive_path: &Path) -> Result<Box<dyn io::Read>> {
    let file = File::open(archive_path)?;
    if path_has_archive_extension(archive_path, &["tar.gz", "tgz"]) {
        Ok(Box::new(GzDecoder::new(file)))
    } else if path_has_archive_extension(archive_path, &["tar"]) {
        Ok(Box::new(file))
    } else {
        Err(Error::ParseError(format!(
            "Unknown archive format: {}",
            archive_path.display()
        )))
    }
}

fn validate_archive_path(path: &Path) -> Result<()> {
    if path.as_os_str().is_empty() || path.is_absolute() {
        return Err(Error::PathTraversal(format!(
            "unsafe archive entry {}",
            path.display()
        )));
    }
    for component in path.components() {
        match component {
            Component::Normal(_) | Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(Error::PathTraversal(format!(
                    "unsafe archive entry {}",
                    path.display()
                )));
            }
        }
    }
    Ok(())
}

fn extracted_source_root(extraction_root: &Path) -> Result<PathBuf> {
    let mut entries = fs::read_dir(extraction_root)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    entries.sort();

    if entries.len() == 1 && entries[0].is_dir() {
        return Ok(entries.remove(0).canonicalize()?);
    }

    Ok(extraction_root.canonicalize()?)
}

fn sha256_file_prefixed(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let hex = sha256_reader_hex(&mut file)?;
    Ok(format!("sha256:{hex}"))
}

fn is_recipe_file(path: &Path) -> bool {
    path.file_name()
        .and_then(OsStr::to_str)
        .is_some_and(|name| name == "recipe.toml")
}

fn is_toml_file(path: &Path) -> bool {
    is_recipe_file(path)
        || path
            .extension()
            .and_then(OsStr::to_str)
            .is_some_and(|extension| extension == "toml")
}

fn is_supported_archive_target(target: &str) -> bool {
    let target = target_without_query(target);
    target.ends_with(".tar") || target.ends_with(".tar.gz") || target.ends_with(".tgz")
}

fn path_has_archive_extension(path: &Path, extensions: &[&str]) -> bool {
    let name = path.file_name().and_then(OsStr::to_str).unwrap_or_default();
    extensions
        .iter()
        .any(|extension| name.ends_with(&format!(".{extension}")))
}

fn archive_filename(target: &str) -> String {
    let target = target_without_query(target);
    let name = target
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or("source.tar");
    sanitize_filename(name)
}

fn looks_like_git_target(target: &str) -> bool {
    if target.starts_with("ssh://") || target.starts_with("git@") {
        return true;
    }

    if target.starts_with("https://") || target.starts_with("http://") {
        return looks_like_http_git_target(target);
    }

    target_without_query(target).ends_with(".git")
}

fn looks_like_http_git_target(target: &str) -> bool {
    let target = target_without_query(target).trim_end_matches('/');
    let last_segment = target.rsplit('/').next().unwrap_or_default();
    let lower_segment = last_segment.to_ascii_lowercase();

    !is_supported_archive_target(target)
        && !unsupported_http_artifact_suffixes()
            .iter()
            .any(|suffix| lower_segment.ends_with(suffix))
}

fn unsupported_http_artifact_suffixes() -> &'static [&'static str] {
    &[
        ".zip",
        ".tar.xz",
        ".txz",
        ".tar.bz2",
        ".tbz2",
        ".tar.zst",
        ".zst",
        ".tar.lz",
        ".tar.lzma",
        ".rar",
        ".7z",
    ]
}

fn is_http_url(target: &str) -> bool {
    target.starts_with("http://") || target.starts_with("https://")
}

fn target_without_query(target: &str) -> &str {
    target
        .split_once(['?', '#'])
        .map(|(head, _)| head)
        .unwrap_or(target)
}

fn sanitize_filename(name: &str) -> String {
    let sanitized = name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_') {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>();
    let sanitized = sanitized.trim_matches(['.', '-']).to_string();
    if sanitized.is_empty() {
        "source.tar".to_string()
    } else {
        sanitized
    }
}

fn unsupported_target_error(target: &str) -> Error {
    Error::ConfigError(format!(
        "Unsupported source target {target:?}; supported target forms: directory, recipe.toml, .tar, .tar.gz, .tgz, or git URL"
    ))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::path::Path;
    use std::process::Command;
    use std::thread;

    use flate2::Compression;
    use flate2::write::GzEncoder;
    use tar::{Builder, EntryType, Header};

    use crate::recipe::inference::{
        CookTarget, SourceTargetKind, resolve_cook_target, resolve_new_from_target,
    };

    fn cargo_source_tree(root: &Path) {
        fs::write(
            root.join("Cargo.toml"),
            r#"[package]
name = "target-demo"
version = "0.1.0"
"#,
        )
        .unwrap();
    }

    fn append_file(builder: &mut Builder<impl Write>, path: &str, body: &[u8]) {
        let mut header = Header::new_gnu();
        header.set_entry_type(EntryType::Regular);
        header.set_size(body.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder.append_data(&mut header, path, body).unwrap();
    }

    fn make_tar(path: &Path) {
        let file = fs::File::create(path).unwrap();
        let mut builder = Builder::new(file);
        append_file(
            &mut builder,
            "source/Cargo.toml",
            br#"[package]
name = "archive-demo"
version = "0.3.0"
"#,
        );
        builder.finish().unwrap();
    }

    fn make_tgz(path: &Path) {
        let file = fs::File::create(path).unwrap();
        let encoder = GzEncoder::new(file, Compression::default());
        let mut builder = Builder::new(encoder);
        append_file(
            &mut builder,
            "source/Cargo.toml",
            br#"[package]
name = "archive-demo"
version = "0.3.0"
"#,
        );
        builder.finish().unwrap();
    }

    fn make_traversal_tar(path: &Path) {
        let mut file = fs::File::create(path).unwrap();
        write_raw_tar_entry(&mut file, "../escape.txt", b"escape");
        file.write_all(&[0; 1024]).unwrap();
    }

    fn write_raw_tar_entry(file: &mut fs::File, name: &str, body: &[u8]) {
        let mut header = [0u8; 512];
        header[..name.len()].copy_from_slice(name.as_bytes());
        write_octal(&mut header[100..108], 0o644);
        write_octal(&mut header[108..116], 0);
        write_octal(&mut header[116..124], 0);
        write_octal(&mut header[124..136], body.len() as u64);
        write_octal(&mut header[136..148], 0);
        for byte in &mut header[148..156] {
            *byte = b' ';
        }
        header[156] = b'0';
        header[257..263].copy_from_slice(b"ustar\0");
        header[263..265].copy_from_slice(b"00");
        let checksum = header.iter().map(|byte| *byte as u32).sum::<u32>();
        let checksum_text = format!("{checksum:06o}\0 ");
        header[148..156].copy_from_slice(checksum_text.as_bytes());
        file.write_all(&header).unwrap();
        file.write_all(body).unwrap();
        let padding = (512 - (body.len() % 512)) % 512;
        if padding > 0 {
            file.write_all(&vec![0; padding]).unwrap();
        }
    }

    fn write_octal(field: &mut [u8], value: u64) {
        let text = format!("{value:0width$o}\0", width = field.len() - 1);
        field.copy_from_slice(text.as_bytes());
    }

    fn make_symlink_escape_tar(path: &Path) {
        let file = fs::File::create(path).unwrap();
        let mut builder = Builder::new(file);
        let mut header = Header::new_gnu();
        header.set_entry_type(EntryType::Symlink);
        header.set_size(0);
        header.set_mode(0o777);
        header.set_cksum();
        builder
            .append_link(&mut header, "source/outside", "../../outside")
            .unwrap();
        builder.finish().unwrap();
    }

    fn serve_once(bytes: Vec<u8>, name: &'static str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0; 1024];
            let _ = stream.read(&mut request);
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/octet-stream\r\nContent-Disposition: attachment; filename=\"{}\"\r\nConnection: close\r\n\r\n",
                bytes.len(),
                name
            );
            stream.write_all(response.as_bytes()).unwrap();
            stream.write_all(&bytes).unwrap();
        });
        format!("http://{addr}/{name}")
    }

    fn git_available() -> bool {
        Command::new("git")
            .arg("--version")
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false)
    }

    fn create_git_repo(root: &Path) {
        cargo_source_tree(root);
        assert!(
            Command::new("git")
                .args(["init"])
                .current_dir(root)
                .output()
                .unwrap()
                .status
                .success()
        );
        assert!(
            Command::new("git")
                .args(["config", "user.email", "tests@example.invalid"])
                .current_dir(root)
                .output()
                .unwrap()
                .status
                .success()
        );
        assert!(
            Command::new("git")
                .args(["config", "user.name", "Conary Tests"])
                .current_dir(root)
                .output()
                .unwrap()
                .status
                .success()
        );
        assert!(
            Command::new("git")
                .args(["add", "Cargo.toml"])
                .current_dir(root)
                .output()
                .unwrap()
                .status
                .success()
        );
        assert!(
            Command::new("git")
                .args(["commit", "-m", "seed"])
                .current_dir(root)
                .output()
                .unwrap()
                .status
                .success()
        );
    }

    #[test]
    fn directory_target_returns_source_tree() {
        let dir = tempfile::tempdir().unwrap();
        cargo_source_tree(dir.path());

        let target = resolve_cook_target(Some(dir.path().to_str().unwrap()), None).unwrap();

        let CookTarget::SourceTree(source) = target else {
            panic!("directory without recipe.toml should resolve as a source tree");
        };
        assert_eq!(source.kind, SourceTargetKind::Directory);
        assert_eq!(source.root, dir.path().canonicalize().unwrap());
        assert!(source.temporary.is_none());
        assert_eq!(source.provenance.archive_checksum, None);
        assert_eq!(source.provenance.git_commit, None);
    }

    #[test]
    fn recipe_toml_target_is_explicit_recipe() {
        let dir = tempfile::tempdir().unwrap();
        let recipe = dir.path().join("recipe.toml");
        fs::write(&recipe, "[package]\nname = \"demo\"\nversion = \"0.1.0\"\n").unwrap();

        let target = resolve_cook_target(Some(recipe.to_str().unwrap()), None).unwrap();

        let CookTarget::RecipeFile(path) = target else {
            panic!("recipe.toml path should resolve as an explicit recipe");
        };
        assert_eq!(path, recipe.canonicalize().unwrap());
    }

    #[test]
    fn custom_toml_target_is_explicit_recipe() {
        let dir = tempfile::tempdir().unwrap();
        let recipe = dir.path().join("custom.toml");
        fs::write(&recipe, "[package]\nname = \"demo\"\nversion = \"0.1.0\"\n").unwrap();

        let target = resolve_cook_target(Some(recipe.to_str().unwrap()), None).unwrap();

        let CookTarget::RecipeFile(path) = target else {
            panic!("existing positional TOML file should resolve as an explicit recipe");
        };
        assert_eq!(path, recipe.canonicalize().unwrap());
    }

    #[test]
    fn directory_with_recipe_toml_is_explicit_recipe() {
        let dir = tempfile::tempdir().unwrap();
        let recipe = dir.path().join("recipe.toml");
        fs::write(&recipe, "[package]\nname = \"demo\"\nversion = \"0.1.0\"\n").unwrap();

        let target = resolve_cook_target(Some(dir.path().to_str().unwrap()), None).unwrap();

        let CookTarget::RecipeFile(path) = target else {
            panic!("directory with recipe.toml should resolve as an explicit recipe");
        };
        assert_eq!(path, recipe.canonicalize().unwrap());
    }

    #[test]
    fn local_tar_archives_extract_to_temporary_source_root() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("source.tar");
        make_tar(&archive);

        let source = resolve_new_from_target(archive.to_str().unwrap()).unwrap();

        assert_eq!(source.kind, SourceTargetKind::Archive);
        assert!(source.temporary.is_some());
        assert_eq!(
            source.provenance.archive_checksum.as_deref().unwrap().len(),
            71
        );
        assert!(source.root.join("Cargo.toml").is_file());
    }

    #[test]
    fn local_gzip_archives_extract_to_temporary_source_root() {
        for filename in ["source.tar.gz", "source.tgz"] {
            let dir = tempfile::tempdir().unwrap();
            let archive = dir.path().join(filename);
            make_tgz(&archive);

            let source = resolve_new_from_target(archive.to_str().unwrap()).unwrap();

            assert_eq!(source.kind, SourceTargetKind::Archive);
            assert!(source.temporary.is_some());
            assert!(source.root.join("Cargo.toml").is_file());
        }
    }

    #[test]
    fn archive_entries_must_not_escape_extraction_root() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("bad.tar");
        make_traversal_tar(&archive);

        let error = resolve_new_from_target(archive.to_str().unwrap()).unwrap_err();

        assert!(
            error.to_string().contains("unsafe archive entry"),
            "expected traversal rejection, got {error}"
        );
    }

    #[test]
    fn archive_symlinks_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("bad-link.tar");
        make_symlink_escape_tar(&archive);

        let error = resolve_new_from_target(archive.to_str().unwrap()).unwrap_err();

        assert!(
            error.to_string().contains("symlink"),
            "expected symlink rejection, got {error}"
        );
    }

    #[test]
    fn http_archive_url_uses_downloaded_archive_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("source.tgz");
        make_tgz(&archive);
        let bytes = fs::read(&archive).unwrap();
        let url = serve_once(bytes, "source.tgz");

        let source = resolve_new_from_target(&url).unwrap();

        assert_eq!(source.kind, SourceTargetKind::Archive);
        assert_eq!(source.original, url);
        assert!(source.provenance.archive_checksum.is_some());
        assert!(source.root.join("Cargo.toml").is_file());
    }

    #[test]
    fn git_targets_clone_to_temporary_source_root() {
        if !git_available() {
            eprintln!("skipping git target test because git is not installed");
            return;
        }

        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("demo.git");
        fs::create_dir(&repo).unwrap();
        create_git_repo(&repo);

        let source = resolve_new_from_target(repo.to_str().unwrap()).unwrap();

        assert_eq!(source.kind, SourceTargetKind::Git);
        assert!(source.temporary.is_some());
        assert!(source.root.join("Cargo.toml").is_file());
        assert_eq!(source.provenance.git_commit.as_deref().unwrap().len(), 40);
    }

    #[test]
    fn unsupported_target_names_supported_forms() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("source.zip");
        fs::write(&target, b"not supported").unwrap();

        let error = resolve_new_from_target(target.to_str().unwrap()).unwrap_err();

        assert!(
            error.to_string().contains(
                "supported target forms: directory, recipe.toml, .tar, .tar.gz, .tgz, or git URL"
            ),
            "expected supported forms message, got {error}"
        );
    }

    #[test]
    fn unsupported_cook_file_target_names_supported_forms() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("source.zip");
        fs::write(&target, b"not supported").unwrap();

        let error = resolve_cook_target(Some(target.to_str().unwrap()), None).unwrap_err();

        assert!(
            error.to_string().contains(
                "supported target forms: directory, recipe.toml, .tar, .tar.gz, .tgz, or git URL"
            ),
            "expected supported forms message, got {error}"
        );
    }

    #[test]
    fn unsupported_http_file_extension_names_supported_forms() {
        let error = resolve_new_from_target("http://127.0.0.1:9/source.zip").unwrap_err();

        assert!(
            error.to_string().contains(
                "supported target forms: directory, recipe.toml, .tar, .tar.gz, .tgz, or git URL"
            ),
            "expected supported forms message, got {error}"
        );
    }

    #[test]
    fn dotted_http_git_repo_names_are_git_targets_without_network() {
        assert!(
            super::looks_like_git_target("https://github.com/rust-lang/rust.vim"),
            "dotted HTTP(S) repository names should remain valid git targets"
        );
    }

    #[test]
    fn explicit_recipe_option_wins_over_target() {
        let dir = tempfile::tempdir().unwrap();
        cargo_source_tree(dir.path());
        let recipe = dir.path().join("custom.toml");
        fs::write(&recipe, "[package]\nname = \"demo\"\nversion = \"0.1.0\"\n").unwrap();

        let target = resolve_cook_target(
            Some(dir.path().to_str().unwrap()),
            Some(recipe.to_str().unwrap()),
        )
        .unwrap();

        let CookTarget::RecipeFile(path) = target else {
            panic!("explicit --recipe should resolve as a recipe file");
        };
        assert_eq!(path, recipe.canonicalize().unwrap());
    }
}
