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
    include_str!("../../../../../deploy/remi-repositories.toml")
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
