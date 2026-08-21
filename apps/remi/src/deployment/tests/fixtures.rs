// apps/remi/src/deployment/tests/fixtures.rs

use crate::deployment::PrepareOptions;
use std::fs;
use std::os::unix::fs::DirBuilderExt;
use std::path::Path;

fn write(path: &Path, content: &str) {
    fs::write(path, content).unwrap();
}

fn base_config(root: &Path) -> String {
    format!(
        r#"
[server]
bind = "127.0.0.1:8080"
admin_bind = "127.0.0.1:8081"

[storage]
root = "{}"
eviction_threshold = 0.90
eviction_min_age = "1h"

[r2]
enabled = true
endpoint = "https://r2.example.test"
account_id = "retired"
write_through = true
r2_redirect = false

[upstream.old]
base_url = "https://legacy.example.test"

[conversion]
max_concurrent = 4
"#,
        root.display()
    )
}

pub(super) fn repository_manifest() -> &'static str {
    r#"
schema_version = 3

[[repositories]]
name = "opaque-source"
url = "https://packages.example.test/repository"
profile = "fedora-44"
source_identity = "example-publisher"
repository_identity = "opaque-source-rpm-x86_64"
stream_kind = "release"
stream_identity = "44"
update_mode = "follow"
enabled = true
priority = 100
metadata_expire_seconds = 21600

[repositories.parser]
package_format = "rpm"
architecture = "x86_64"

[repositories.trust]
ecosystem = "rpm"

[repositories.trust.metadata]
kind = "metalink"
url = "https://mirrors.example.test/metalink"

[[repositories.trust.package_keys]]
url = "https://keys.example.test/fedora.gpg"
fingerprint = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"
"#
}

fn options(root: &Path) -> PrepareOptions {
    PrepareOptions {
        config_path: root.join("etc/remi.toml"),
        repository_manifest_source: root.join("staged-repositories.toml"),
        repository_manifest_target: root.join("etc/remi-repositories.toml"),
        repository_keys_dir: root.join("repository-keys"),
        deployment_id: "remi-0.8.0".to_string(),
        max_concurrent: 32,
    }
}

pub(super) fn arrange() -> (tempfile::TempDir, PrepareOptions) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().to_path_buf();
    fs::create_dir(root.join("etc")).unwrap();
    fs::create_dir(root.join("metadata")).unwrap();
    fs::DirBuilder::new()
        .mode(0o700)
        .create(root.join("repository-keys"))
        .unwrap();
    write(&root.join("etc/remi.toml"), &base_config(&root));
    write(
        &root.join("staged-repositories.toml"),
        repository_manifest(),
    );
    let options = options(&root);
    (temp, options)
}
