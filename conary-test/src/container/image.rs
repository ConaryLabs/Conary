// conary-test/src/container/image.rs

use anyhow::Result;
use std::collections::HashMap;
use std::path::Path;

use super::backend::ContainerBackend;

/// Build a distro-specific test image from a Containerfile.
///
/// Tags the image as `conary-test-{distro}:latest`.
pub async fn build_distro_image(
    backend: &dyn ContainerBackend,
    containerfile: &Path,
    distro: &str,
) -> Result<String> {
    let tag = format!("conary-test-{distro}:latest");
    backend
        .build_image(containerfile, &tag, HashMap::new())
        .await
}
