// apps/remi/src/server/private_output.rs

//! Private staging and durable publication primitives shared by Remi outputs.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};

const PRIVATE_MODE: u32 = 0o600;
const PRIVATE_DIRECTORY_MODE: u32 = 0o700;

#[derive(Clone, Copy)]
struct PublishedInode {
    device: u64,
    inode: u64,
}

impl PublishedInode {
    fn from_metadata(metadata: &fs::Metadata) -> Self {
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
        }
    }

    fn matches(self, metadata: &fs::Metadata) -> bool {
        metadata.file_type().is_file()
            && metadata.dev() == self.device
            && metadata.ino() == self.inode
    }
}

pub(crate) fn require_real_directory(path: &Path, label: &str) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("inspect {label} {}", path.display()))?;
    ensure!(
        metadata.file_type().is_dir() && !metadata.file_type().is_symlink(),
        "{label} {} must be a real directory",
        path.display()
    );
    Ok(())
}

pub(crate) fn output_parent(path: &Path, label: &str) -> Result<PathBuf> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    require_real_directory(parent, label)?;
    Ok(parent.to_path_buf())
}

pub(crate) fn create_new_private_directory(path: &Path, label: &str) -> Result<()> {
    let parent = output_parent(path, &format!("{label} parent"))?;
    fs::DirBuilder::new()
        .mode(PRIVATE_DIRECTORY_MODE)
        .create(path)
        .with_context(|| format!("create {label} {}", path.display()))?;
    require_real_directory(path, label)?;
    File::open(parent)?.sync_all()?;
    Ok(())
}

pub(crate) fn ensure_real_subdirectory(parent: &Path, name: &str) -> Result<PathBuf> {
    require_real_directory(parent, "directory parent")?;
    let path = parent.join(name);
    match fs::create_dir(&path) {
        Ok(()) => File::open(parent)?.sync_all()?,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.into()),
    }
    require_real_directory(&path, name)?;
    Ok(path)
}

pub(crate) fn read_regular_nofollow(path: &Path, label: &str, max_bytes: u64) -> Result<Vec<u8>> {
    let named = fs::symlink_metadata(path)
        .with_context(|| format!("inspect {label} {}", path.display()))?;
    ensure!(
        named.file_type().is_file() && !named.file_type().is_symlink(),
        "{label} {} must be a regular non-symlink file",
        path.display()
    );
    ensure!(
        named.len() <= max_bytes,
        "{label} {} exceeds the {max_bytes}-byte limit",
        path.display()
    );
    let mut input = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
        .with_context(|| format!("open {label} {} without following links", path.display()))?;
    let opened = input.metadata()?;
    ensure!(
        opened.file_type().is_file() && opened.dev() == named.dev() && opened.ino() == named.ino(),
        "{label} {} changed while opening",
        path.display()
    );
    let capacity = usize::try_from(opened.len()).context("bounded input size exceeds usize")?;
    let mut bytes = Vec::with_capacity(capacity);
    (&mut input)
        .take(max_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .with_context(|| format!("read {label} {}", path.display()))?;
    let final_size = u64::try_from(bytes.len()).context("bounded input size exceeds u64")?;
    let opened_after = input.metadata()?;
    let named_after = fs::symlink_metadata(path)?;
    ensure!(
        final_size == opened.len()
            && final_size == opened_after.len()
            && final_size <= max_bytes
            && named_after.file_type().is_file()
            && named_after.dev() == opened.dev()
            && named_after.ino() == opened.ino(),
        "{label} {} changed while reading",
        path.display()
    );
    Ok(bytes)
}

pub(crate) fn publish_new_private_file(
    path: &Path,
    temporary_label: &str,
    bytes: &[u8],
    label: &str,
    verify: impl FnOnce(&Path) -> Result<()>,
) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(_) => bail!("{label} already exists: {}", path.display()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let parent = output_parent(path, &format!("{label} parent"))?;
    let temporary = parent.join(format!(".{temporary_label}.{}.tmp", uuid::Uuid::new_v4()));
    let mut linked_inode = None;
    let publication = (|| -> Result<PublishedInode> {
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(PRIVATE_MODE)
            .open(&temporary)?;
        output.set_permissions(fs::Permissions::from_mode(PRIVATE_MODE))?;
        output.write_all(bytes)?;
        output.sync_all()?;
        let staged_metadata = output.metadata()?;
        let staged_inode = PublishedInode::from_metadata(&staged_metadata);
        match fs::hard_link(&temporary, path) {
            Ok(()) => linked_inode = Some(staged_inode),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                bail!("{label} already exists: {}", path.display())
            }
            Err(error) => return Err(error.into()),
        }
        let published = fs::symlink_metadata(path)?;
        ensure!(
            staged_inode.matches(&published)
                && published.nlink() == 2
                && published.mode() & 0o7777 == PRIVATE_MODE,
            "published {label} is not the private staged file"
        );
        fs::remove_file(&temporary)?;
        let published = fs::symlink_metadata(path)?;
        ensure!(
            staged_inode.matches(&published)
                && published.nlink() == 1
                && published.mode() & 0o7777 == PRIVATE_MODE,
            "published {label} retained an unexpected link"
        );
        File::open(&parent)?.sync_all()?;
        Ok(staged_inode)
    })();
    let published_inode = match publication {
        Ok(inode) => inode,
        Err(error) => {
            rollback(path, &temporary, linked_inode);
            return Err(error);
        }
    };
    if let Err(error) = verify(path).and_then(|()| {
        let metadata = fs::symlink_metadata(path)?;
        ensure!(
            published_inode.matches(&metadata)
                && metadata.nlink() == 1
                && metadata.mode() & 0o7777 == PRIVATE_MODE,
            "published {label} changed during verification"
        );
        Ok(())
    }) {
        rollback(path, &temporary, Some(published_inode));
        return Err(error);
    }
    Ok(())
}

fn rollback(path: &Path, temporary: &Path, published: Option<PublishedInode>) {
    if let Some(published) = published
        && fs::symlink_metadata(path)
            .map(|metadata| published.matches(&metadata))
            .unwrap_or(false)
    {
        let _ = fs::remove_file(path);
    }
    let _ = fs::remove_file(temporary);
    if let Some(parent) = path.parent() {
        let _ = File::open(parent).and_then(|directory| directory.sync_all());
    }
}
