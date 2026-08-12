// conary-core/src/repository/download/durability_tests.rs

use super::{download_delta, download_package_inner};
use crate::db::models::RepositoryPackage;
use crate::hash::sha256;
use crate::repository::metadata::DeltaInfo;
use crate::repository::versioning::VersionScheme;

fn package_for_download(url: String, content: &[u8], size: i64) -> RepositoryPackage {
    RepositoryPackage::new(
        1,
        "http-fixture".to_string(),
        "1.0.0".to_string(),
        VersionScheme::Conary,
        sha256(content),
        size,
        url,
    )
}

async fn serve_http_bytes_once(content: Vec<u8>) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let mut buffer = [0u8; 1024];
        loop {
            let read = stream.read(&mut buffer).await.unwrap();
            if read == 0 {
                break;
            }
            request.extend_from_slice(&buffer[..read]);
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
    format!("http://{addr}/fixture.delta")
}

#[tokio::test]
async fn download_package_preserves_existing_file_on_http_size_mismatch() {
    let dir = tempfile::tempdir().unwrap();
    let dest_dir = dir.path().join("downloads");
    let dest_path = dest_dir.join("fixture.delta");
    let content = b"generic HTTP package";
    std::fs::create_dir_all(&dest_dir).unwrap();
    std::fs::write(&dest_path, b"previous verified package").unwrap();
    let package = package_for_download(
        serve_http_bytes_once(content.to_vec()).await,
        content,
        i64::try_from(content.len() + 1).unwrap(),
    );

    let error = download_package_inner(&package, &dest_dir, None, None, false)
        .await
        .unwrap_err();

    assert!(error.to_string().contains("size"), "{error}");
    assert_eq!(
        std::fs::read(&dest_path).unwrap(),
        b"previous verified package"
    );
}

#[tokio::test]
async fn download_delta_preserves_existing_file_on_checksum_mismatch() {
    let dir = tempfile::tempdir().unwrap();
    let dest_dir = dir.path().join("downloads");
    let dest_path = dest_dir.join("fixture.delta");
    std::fs::create_dir_all(&dest_dir).unwrap();
    std::fs::write(&dest_path, b"previous verified delta").unwrap();
    let delta = DeltaInfo {
        from_version: "1".to_string(),
        from_hash: sha256(b"original package"),
        delta_url: serve_http_bytes_once(b"corrupt delta".to_vec()).await,
        delta_size: 13,
        delta_checksum: sha256(b"expected delta"),
        compression_ratio: 0.5,
    };

    let error = download_delta(&delta, "fixture", "2", &dest_dir)
        .await
        .unwrap_err();

    assert!(error.to_string().contains("Checksum"), "{error}");
    assert_eq!(
        std::fs::read(&dest_path).unwrap(),
        b"previous verified delta"
    );
}
