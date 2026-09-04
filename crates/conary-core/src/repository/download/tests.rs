// crates/conary-core/src/repository/download/tests.rs

use super::*;
use crate::db::models::RepositoryPackage;
use crate::hash::sha256;
use crate::repository::versioning::VersionScheme;

fn package_for_download(url: String, content: &[u8], size: i64) -> RepositoryPackage {
    RepositoryPackage::new(
        1,
        "static-local".to_string(),
        "1.0.0".to_string(),
        VersionScheme::Conary,
        sha256(content),
        size,
        url,
    )
}

#[test]
fn verify_checksum_accepts_canonical_sha256_identity() {
    let file = tempfile::NamedTempFile::new().unwrap();
    let content = b"canonical repository checksum";
    std::fs::write(file.path(), content).unwrap();

    verify_checksum(file.path(), &format!("sha256:{}", sha256(content))).unwrap();
}

#[test]
fn verify_checksum_rejects_wrong_canonical_sha256_identity() {
    let file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(file.path(), b"canonical repository checksum").unwrap();
    let expected = format!("sha256:{}", sha256(b"different content"));

    let error = verify_checksum(file.path(), &expected).unwrap_err();

    assert!(matches!(
        error,
        Error::ChecksumMismatch {
            expected: actual_expected,
            actual,
        } if actual_expected == expected && actual.starts_with("sha256:")
    ));
}

#[cfg(unix)]
fn make_fifo(path: &Path) {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let path_c = CString::new(path.as_os_str().as_bytes()).unwrap();
    let result = unsafe { libc::mkfifo(path_c.as_ptr(), 0o600) };
    assert_eq!(
        result,
        0,
        "mkfifo failed: {}",
        std::io::Error::last_os_error()
    );
}

#[cfg(unix)]
fn spawn_fifo_writer(path: PathBuf, content: &'static [u8]) -> std::thread::JoinHandle<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    use std::time::Duration;

    std::thread::spawn(move || {
        let path_c = CString::new(path.as_os_str().as_bytes()).unwrap();
        for _ in 0..100 {
            let fd = unsafe { libc::open(path_c.as_ptr(), libc::O_WRONLY | libc::O_NONBLOCK) };
            if fd >= 0 {
                let _ = unsafe { libc::write(fd, content.as_ptr().cast(), content.len()) };
                unsafe {
                    libc::close(fd);
                }
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    })
}

async fn serve_http_package_once(content: Vec<u8>) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let mut buf = [0u8; 1024];
        loop {
            let read = stream.read(&mut buf).await.unwrap();
            if read == 0 {
                break;
            }
            request.extend_from_slice(&buf[..read]);
            if request.windows(4).any(|window| window == b"\r\n\r\n") {
                break;
            }
        }

        let headers = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            content.len()
        );
        stream.write_all(headers.as_bytes()).await.unwrap();
        stream.write_all(&content).await.unwrap();
    });

    format!("http://{addr}/generic-http.ccs")
}

#[tokio::test]
async fn download_package_copies_file_url_and_verifies_checksum() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("static-file.ccs");
    let dest_dir = dir.path().join("downloads");
    let content = b"static package from file url";
    std::fs::write(&source, content).unwrap();

    let package = package_for_download(
        format!("file://{}", source.display()),
        content,
        i64::try_from(content.len()).unwrap(),
    );

    let downloaded = download_static_package_verified(&package, &dest_dir, None)
        .await
        .unwrap();

    assert_eq!(downloaded, dest_dir.join("static-file.ccs"));
    assert_eq!(std::fs::read(downloaded).unwrap(), content);
}

#[tokio::test]
async fn download_package_copies_bare_local_path_and_verifies_checksum() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("bare-local.ccs");
    let dest_dir = dir.path().join("downloads");
    let content = b"static package from bare path";
    std::fs::write(&source, content).unwrap();

    let package = package_for_download(
        source.to_str().unwrap().to_string(),
        content,
        i64::try_from(content.len()).unwrap(),
    );

    let downloaded = download_static_package_verified(&package, &dest_dir, None)
        .await
        .unwrap();

    assert_eq!(downloaded, dest_dir.join("bare-local.ccs"));
    assert_eq!(std::fs::read(downloaded).unwrap(), content);
}

#[tokio::test]
async fn download_package_rejects_local_path_without_static_opt_in() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("native-local.ccs");
    let dest_dir = dir.path().join("downloads");
    let content = b"native package should not copy local paths";
    std::fs::write(&source, content).unwrap();

    let package = package_for_download(
        source.to_str().unwrap().to_string(),
        content,
        i64::try_from(content.len()).unwrap(),
    );

    let error = download_package_inner(&package, &dest_dir, None, None, false)
        .await
        .unwrap_err();

    assert!(
        error.to_string().contains("local")
            || error.to_string().contains("scheme")
            || error.to_string().contains("URL"),
        "expected local path rejection, got: {error}"
    );
    assert!(!dest_dir.join("native-local.ccs").exists());
}

#[tokio::test]
async fn download_package_rejects_http_size_mismatch_after_checksum() {
    let dir = tempfile::tempdir().unwrap();
    let dest_dir = dir.path().join("downloads");
    let content = b"generic HTTP package";
    let url = serve_http_package_once(content.to_vec()).await;
    let package = package_for_download(url, content, i64::try_from(content.len() + 1).unwrap());

    let error = download_package_inner(&package, &dest_dir, None, None, false)
        .await
        .unwrap_err();

    assert!(
        error.to_string().contains("size"),
        "expected size mismatch, got: {error}"
    );
    assert!(!dest_dir.join("generic-http.ccs").exists());
}

#[tokio::test]
async fn cached_debian_package_rechecks_checksum_and_size_without_network() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cached.deb");
    let content = b"authenticated Debian package bytes";
    std::fs::write(&path, content).unwrap();
    let package = package_for_download(
        "https://offline.example.test/cached.deb".to_string(),
        content,
        i64::try_from(content.len()).unwrap(),
    );
    let options = DownloadOptions {
        trust: PreparedOpenPgpTrust::for_test(RepositoryTrustPolicy::Debian {
            release_keys: vec![crate::repository::OpenPgpTrustRoot {
                url: "https://keys.example.test/debian.gpg".to_string(),
                fingerprint: "A".repeat(40),
            }],
        }),
    };

    verify_cached_package_verified(&package, &path, &options, None)
        .await
        .unwrap();
    std::fs::write(&path, b"different bytes").unwrap();
    assert!(
        verify_cached_package_verified(&package, &path, &options, None)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn cached_arch_package_requires_detached_authority() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cached.pkg.tar.zst");
    let content = b"arch package placeholder";
    std::fs::write(&path, content).unwrap();
    let package = package_for_download(
        "https://offline.example.test/cached.pkg.tar.zst".to_string(),
        content,
        i64::try_from(content.len()).unwrap(),
    );
    let options = DownloadOptions {
        trust: PreparedOpenPgpTrust::for_test(RepositoryTrustPolicy::Arch {
            keyring: crate::repository::ArchKeyringTrust {
                url: "https://keys.example.test/archlinux-keyring.pkg.tar.zst".to_string(),
                format: crate::repository::ArchKeyringFormat::AlpmPackageZstd,
                master_fingerprints: vec!["A".repeat(40)],
                packager_key_threshold: 1,
            },
            sig_level: crate::repository::ArchSigLevel::distribution_default(),
        }),
    };

    let error = verify_cached_package_verified(&package, &path, &options, None)
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("no detached package signature"),
        "{error}"
    );
}

#[tokio::test]
async fn download_package_removes_static_copy_on_size_mismatch() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("size-mismatch.ccs");
    let dest_dir = dir.path().join("downloads");
    let content = b"static package with wrong size";
    std::fs::write(&source, content).unwrap();

    let package = package_for_download(
        source.to_str().unwrap().to_string(),
        content,
        i64::try_from(content.len() + 1).unwrap(),
    );

    let error = download_static_package_verified(&package, &dest_dir, None)
        .await
        .unwrap_err();

    assert!(
        error.to_string().contains("size"),
        "expected size mismatch, got: {error}"
    );
    assert!(!dest_dir.join("size-mismatch.ccs").exists());
}

#[tokio::test]
async fn download_package_preserves_existing_file_on_static_size_mismatch() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("preserve-existing.ccs");
    let dest_dir = dir.path().join("downloads");
    let dest_path = dest_dir.join("preserve-existing.ccs");
    let content = b"static package with wrong size";
    std::fs::create_dir_all(&dest_dir).unwrap();
    std::fs::write(&source, content).unwrap();
    std::fs::write(&dest_path, b"previous package").unwrap();

    let package = package_for_download(
        source.to_str().unwrap().to_string(),
        content,
        i64::try_from(content.len() + 1).unwrap(),
    );

    let error = download_static_package_verified(&package, &dest_dir, None)
        .await
        .unwrap_err();

    assert!(
        error.to_string().contains("size"),
        "expected size mismatch, got: {error}"
    );
    assert_eq!(std::fs::read(&dest_path).unwrap(), b"previous package");
}

#[tokio::test]
async fn download_package_rejects_local_source_larger_than_metadata_before_copying() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("oversized-local.ccs");
    let dest_dir = dir.path().join("downloads");
    let dest_path = dest_dir.join("oversized-local.ccs");
    let content = b"local package larger than metadata";
    std::fs::create_dir_all(&dest_dir).unwrap();
    std::fs::write(&source, content).unwrap();
    std::fs::write(&dest_path, b"previous package").unwrap();

    let package = package_for_download(
        source.to_str().unwrap().to_string(),
        content,
        i64::try_from(content.len() - 1).unwrap(),
    );

    let error = download_static_package_verified(&package, &dest_dir, None)
        .await
        .unwrap_err();

    assert!(
        error.to_string().contains("source file size"),
        "expected source-size rejection, got: {error}"
    );
    assert_eq!(std::fs::read(&dest_path).unwrap(), b"previous package");
}

#[cfg(unix)]
#[tokio::test]
async fn download_package_rejects_local_non_regular_source() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("pipe.ccs");
    let dest_dir = dir.path().join("downloads");
    let content = b"pipe package";
    make_fifo(&source);
    let writer = spawn_fifo_writer(source.clone(), content);

    let package = package_for_download(
        source.to_str().unwrap().to_string(),
        content,
        i64::try_from(content.len()).unwrap(),
    );

    let error = download_static_package_verified(&package, &dest_dir, None)
        .await
        .unwrap_err();
    writer.join().unwrap();

    assert!(
        error.to_string().contains("regular file"),
        "expected regular-file rejection, got: {error}"
    );
    assert!(!dest_dir.join("pipe.ccs").exists());
}

#[tokio::test]
async fn download_package_treats_local_remi_shaped_path_as_local() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("v1/local/packages/remi-lookalike/download");
    let dest_dir = dir.path().join("downloads");
    let content = b"static package from remi-shaped local path";
    std::fs::create_dir_all(source.parent().unwrap()).unwrap();
    std::fs::write(&source, content).unwrap();

    let package = package_for_download(
        source.to_str().unwrap().to_string(),
        content,
        i64::try_from(content.len()).unwrap(),
    );

    let downloaded = download_static_package_verified(&package, &dest_dir, None)
        .await
        .unwrap();

    assert_eq!(downloaded, dest_dir.join("download"));
    assert_eq!(std::fs::read(downloaded).unwrap(), content);
}
