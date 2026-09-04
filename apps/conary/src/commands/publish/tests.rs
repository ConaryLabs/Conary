// apps/conary/src/commands/publish/tests.rs

use super::*;
use std::ffi::OsString;
use std::process::Command;
use tokio::sync::Mutex;

const TEST_HASH: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

#[tokio::test]
async fn artifact_form_publish_rejects_missing_attestation() {
    let fixture = ArtifactPublishFixture::without_attestation();

    let error = cmd_publish(fixture.options()).await.unwrap_err();
    let error = format!("{error:#}");

    assert!(
        error.contains("artifact is missing a build attestation"),
        "{error}"
    );
}

#[tokio::test]
async fn artifact_form_publish_json_reports_static_gate_failure() {
    let fixture = ArtifactPublishFixture::without_attestation();
    let mut options = fixture.options();
    options.json = true;

    let mut output = Vec::new();
    let error = cmd_publish_with_output(options, &mut output)
        .await
        .unwrap_err();
    let message = format!("{error:#}");
    assert!(
        message.contains("artifact is missing a build attestation"),
        "{message}"
    );
    let value: serde_json::Value = serde_json::from_slice(&output).expect("valid publish json");
    assert_eq!(value["status"], "failed");
    assert_eq!(value["diagnostics"][0]["code"], "publish-gate-failed");
    assert_eq!(
        value["diagnostics"][0]["evidence"][0]["metadata"]["publish_lint_report"]["failures"][0]["code"],
        "missing-attestation"
    );
}

#[tokio::test]
async fn static_artifact_service_helper_returns_structured_gate_failure() {
    let fixture = ArtifactPublishFixture::without_attestation();
    let output = publish_static_artifact_form_service(StaticArtifactPublishServiceInput {
        artifact_path: fixture.package_path.clone(),
        destination: RepoLocation::File {
            root: fixture.repo_dir.clone(),
        },
        key_dir: Some(fixture.key_dir.clone()),
        state_file: Some(fixture.state_file.clone()),
        refresh: false,
        rotate_publish_key: false,
        rotate_root_key: false,
        operation_id: "publish-test".to_string(),
    })
    .unwrap();

    assert_eq!(output.operation_id, "publish-test");
    assert_eq!(
        output.status,
        conary_core::diagnostics::PackagingCommandStatus::Failed
    );
    assert_eq!(
        output.diagnostics[0].code,
        PackagingDiagnosticCode::PublishGateFailed
    );
}

#[tokio::test]
async fn project_form_publish_json_preflight_is_structured() {
    let temp = tempfile::tempdir().unwrap();
    let recipe_path = temp.path().join("recipe.toml");
    let repo_dir = temp.path().join("repo");
    let key_dir = temp.path().join("keys");
    let state_file = temp.path().join("publish-state.toml");
    std::fs::write(
        &recipe_path,
        r#"
[package]
name = "publish-json"
version = "1.0"

[source]
path = "."

[build]
install = "mkdir -p %(destdir)s/usr/share/publish-json"
"#,
    )
    .unwrap();

    let mut output = Vec::new();
    let error = cmd_publish_with_output(
        PublishOptions {
            what: repo_dir.display().to_string(),
            target: None,
            recipe: Some(recipe_path.display().to_string()),
            key_dir: Some(key_dir.display().to_string()),
            state_file: Some(state_file.display().to_string()),
            refresh: false,
            rotate_publish_key: false,
            rotate_root_key: false,
            yes: true,
            json: true,
        },
        &mut output,
    )
    .await
    .unwrap_err();

    let message = format!("{error:#}");
    assert!(message.contains("hermetic config"), "{message}");
    let rendered = String::from_utf8(output).unwrap();
    assert!(!rendered.contains("Reading recipe:"));
    let value: serde_json::Value = serde_json::from_str(&rendered).expect("valid preflight json");
    assert_eq!(value["status"], "failed");
    assert_eq!(
        value["diagnostics"][0]["code"],
        "project-publish-preflight-failed"
    );
}

#[tokio::test]
async fn remi_publish_json_is_structured_unsupported() {
    let temp = tempfile::tempdir().unwrap();
    let artifact = temp.path().join("artifact.ccs");
    std::fs::write(&artifact, b"not read for unsupported route").unwrap();

    let mut output = Vec::new();
    let error = cmd_publish_with_output(
        PublishOptions {
            what: artifact.display().to_string(),
            target: Some("https://remi.example.invalid/v1/admin/releases/test".to_string()),
            recipe: None,
            key_dir: None,
            state_file: None,
            refresh: false,
            rotate_publish_key: false,
            rotate_root_key: false,
            yes: true,
            json: true,
        },
        &mut output,
    )
    .await
    .unwrap_err();

    assert!(format!("{error:#}").contains("Remi publish JSON output is not supported in M3a"));
    let value: serde_json::Value =
        serde_json::from_slice(&output).expect("valid remi unsupported json");
    assert_eq!(value["status"], "failed");
    assert_eq!(value["diagnostics"][0]["code"], "publish-json-unsupported");
}

#[tokio::test]
async fn unrelated_release_path_substring_fails_before_publish_side_effects() {
    let temp = tempfile::tempdir().unwrap();
    let key_dir = temp.path().join("keys");
    let mut output = Vec::new();

    let error = cmd_publish_with_output(
        PublishOptions {
            what: temp.path().join("missing.ccs").display().to_string(),
            target: Some(
                "https://remi.example.invalid/prefix/v1/admin/releases/fedora".to_string(),
            ),
            recipe: None,
            key_dir: Some(key_dir.display().to_string()),
            state_file: None,
            refresh: false,
            rotate_publish_key: false,
            rotate_root_key: false,
            yes: true,
            json: false,
        },
        &mut output,
    )
    .await
    .unwrap_err();

    assert!(
        error
            .to_string()
            .contains("must use the exact Remi release endpoint"),
        "{error:#}"
    );
    assert!(!key_dir.exists());
    assert!(output.is_empty());
}

#[test]
fn publish_kitchen_config_uses_hermetic_defaults() {
    let recipe_path = std::path::Path::new("/work/pkg/recipe.toml");
    let output_dir = std::path::Path::new("/tmp/conary-publish-out");
    let sysroot = std::path::PathBuf::from("/var/lib/conary/sysroots/test");
    let config = publish_kitchen_config(recipe_path, output_dir, sysroot.clone());

    assert!(config.use_isolation);
    assert!(!config.allow_network);
    assert!(config.pristine_mode);
    assert_eq!(config.sysroot, Some(sysroot));
    assert_eq!(
        config.source_download_policy,
        conary_core::recipe::SourceDownloadPolicy::AllowDownloads
    );
    assert_eq!(
        config.recipe_source_base_dir,
        Some(std::path::PathBuf::from("/work/pkg"))
    );
}

#[tokio::test]
async fn project_form_publish_uses_release_dirty_tree_refusal() {
    let fixture = DirtyGitPublishFixture::new();
    let _env_lock = ENV_LOCK.lock().await;
    let _config_guard = EnvVarGuard::set("CONARY_HERMETIC_CONFIG", &fixture.config_path);
    let _conary_ci_guard = EnvVarGuard::set("CONARY_HERMETIC_CI", "0");
    let _ci_guard = EnvVarGuard::remove("CI");

    let error = cmd_publish(fixture.options()).await.unwrap_err();
    let error = format!("{error:#}");

    assert!(error.contains("dirty local tree"), "{error}");
}

#[test]
fn publish_prefetch_config_allows_downloads_before_hermetic_build() {
    let recipe_path = std::path::Path::new("/work/pkg/recipe.toml");
    let output_dir = std::path::Path::new("/tmp/conary-publish-out");
    let sysroot = std::path::PathBuf::from("/tmp/sysroot");
    let config = publish_kitchen_config(recipe_path, output_dir, sysroot);

    assert!(!config.allow_network);
    assert_eq!(
        config.source_download_policy,
        conary_core::recipe::SourceDownloadPolicy::AllowDownloads
    );
}

#[tokio::test]
async fn project_form_publish_fails_without_hermetic_config() {
    let temp = tempfile::tempdir().unwrap();
    let recipe_path = temp.path().join("recipe.toml");
    let repo_dir = temp.path().join("repo");
    let key_dir = temp.path().join("keys");
    let state_file = temp.path().join("publish-state.toml");
    std::fs::write(
            &recipe_path,
            r#"
[package]
name = "publish-local"
version = "1.0"

[source]
path = "."

[build]
install = "mkdir -p %(destdir)s/usr/share/publish-local && printf hi > %(destdir)s/usr/share/publish-local/hello.txt"
"#,
        )
        .unwrap();

    let error = cmd_publish(PublishOptions {
        what: repo_dir.display().to_string(),
        target: None,
        recipe: Some(recipe_path.display().to_string()),
        key_dir: Some(key_dir.display().to_string()),
        state_file: Some(state_file.display().to_string()),
        refresh: false,
        rotate_publish_key: false,
        rotate_root_key: false,
        yes: true,
        json: false,
    })
    .await
    .unwrap_err();
    let error = format!("{error:#}");

    assert!(error.contains("hermetic config"), "{error}");
    assert!(
        !repo_dir.exists(),
        "publish should fail before writing the static repo"
    );
}

#[tokio::test]
async fn http_publish_destination_is_rejected_before_local_side_effects() {
    let temp_dir = tempfile::tempdir().unwrap();
    let key_dir = temp_dir.path().join("keys");
    let error = cmd_publish(PublishOptions {
        what: "https://example.invalid/static/repo".to_string(),
        target: None,
        recipe: Some("missing-recipe.toml".to_string()),
        key_dir: Some(key_dir.display().to_string()),
        state_file: None,
        refresh: false,
        rotate_publish_key: false,
        rotate_root_key: false,
        yes: false,
        json: false,
    })
    .await
    .unwrap_err();

    assert_eq!(
        error.to_string(),
        "static publisher supports local filesystem destinations; Remi HTTP(S) targets use the Remi release path"
    );
    assert!(!key_dir.exists());
}

#[test]
fn static_local_guard_still_rejects_http_static_path() {
    let destination = RepoLocation::parse("https://repo.example.invalid/static").unwrap();
    let error = ensure_static_local_publish_destination(&destination).unwrap_err();

    assert!(
        error
            .to_string()
            .contains("Remi HTTP(S) targets use the Remi release path")
    );
}

#[test]
fn repo_name_is_derived_from_destination_tail() {
    let local = RepoLocation::parse("./repo").unwrap();
    assert_eq!(derive_repo_name(&local, "./repo").unwrap(), "repo");
    let http = RepoLocation::parse("https://example.invalid/static/acme").unwrap();
    assert_eq!(
        derive_repo_name(&http, "https://example.invalid/static/acme").unwrap(),
        "acme"
    );
}

struct DirtyGitPublishFixture {
    _temp: tempfile::TempDir,
    recipe_path: PathBuf,
    repo_dir: PathBuf,
    key_dir: PathBuf,
    state_file: PathBuf,
    config_path: PathBuf,
}

impl DirtyGitPublishFixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let project_dir = temp.path().join("project");
        let recipe_path = project_dir.join("recipe.toml");
        let sysroot = temp.path().join("sysroot");
        let config_path = temp.path().join("hermetic.toml");
        std::fs::create_dir_all(&project_dir).unwrap();
        std::fs::create_dir_all(&sysroot).unwrap();
        std::fs::write(project_dir.join("source.txt"), "clean\n").unwrap();
        std::fs::write(
                &recipe_path,
                r#"
[package]
name = "dirty-release"
version = "1.0"

[source]
path = "."

[build]
install = "mkdir -p %(destdir)s/usr/share/dirty-release && printf hi > %(destdir)s/usr/share/dirty-release/payload"
"#,
            )
            .unwrap();
        run_git(&project_dir, &["init"]);
        run_git(
            &project_dir,
            &["config", "user.email", "test@example.invalid"],
        );
        run_git(&project_dir, &["config", "user.name", "Conary Test"]);
        run_git(&project_dir, &["add", "."]);
        run_git(&project_dir, &["commit", "-m", "initial"]);
        std::fs::write(project_dir.join("source.txt"), "dirty\n").unwrap();
        std::fs::write(
            &config_path,
            format!(
                r#"
default_builder = "test"

[builders.test]
kind = "pristine"
sysroot_path = "{}"
sysroot_hash = "{TEST_HASH}"
"#,
                sysroot.display()
            ),
        )
        .unwrap();

        Self {
            repo_dir: temp.path().join("repo"),
            key_dir: temp.path().join("keys"),
            state_file: temp.path().join("publish-state.toml"),
            _temp: temp,
            recipe_path,
            config_path,
        }
    }

    fn options(&self) -> PublishOptions {
        PublishOptions {
            what: self.repo_dir.display().to_string(),
            target: None,
            recipe: Some(self.recipe_path.display().to_string()),
            key_dir: Some(self.key_dir.display().to_string()),
            state_file: Some(self.state_file.display().to_string()),
            refresh: false,
            rotate_publish_key: false,
            rotate_root_key: false,
            yes: true,
            json: false,
        }
    }
}

struct ArtifactPublishFixture {
    _temp: tempfile::TempDir,
    package_path: PathBuf,
    repo_dir: PathBuf,
    key_dir: PathBuf,
    state_file: PathBuf,
}

impl ArtifactPublishFixture {
    fn without_attestation() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let source_dir = temp.path().join("source");
        let package_path = temp.path().join("dist/widget-1.0.0.ccs");
        let key_dir = temp.path().join("keys");
        std::fs::create_dir_all(source_dir.join("usr/share/widget")).unwrap();
        std::fs::create_dir_all(package_path.parent().unwrap()).unwrap();
        std::fs::write(source_dir.join("usr/share/widget/payload"), "hello\n").unwrap();
        let manifest = conary_core::ccs::CcsManifest::parse(
            r#"
[package]
name = "widget"
version = "1.0.0"
version_scheme = "conary"
release = "1"
kind = "package"
description = "fixture package"
license = "MIT"

[package.platform]
os = "linux"
arch = "x86_64"
libc = "gnu"

[provenance]
origin_class = "native-built"
hardening_level = "hermetic"
"#,
        )
        .unwrap();
        let result = conary_core::ccs::CcsBuilder::new(manifest, &source_dir)
            .unwrap()
            .build()
            .unwrap();
        let key = conary_core::ccs::SigningKeyPair::generate().with_key_id("publish");
        key.save_to_files(
            &key_dir.join("publish.private"),
            &key_dir.join("publish.public"),
        )
        .unwrap();
        conary_core::ccs::builder::write_signed_current_ccs_package(
            &result,
            &package_path,
            &key,
            false,
        )
        .unwrap();

        Self {
            repo_dir: temp.path().join("repo"),
            state_file: temp.path().join("artifact-publish-state.toml"),
            _temp: temp,
            package_path,
            key_dir,
        }
    }

    fn options(&self) -> PublishOptions {
        PublishOptions {
            what: self.package_path.display().to_string(),
            target: Some(self.repo_dir.display().to_string()),
            recipe: None,
            key_dir: Some(self.key_dir.display().to_string()),
            state_file: Some(self.state_file.display().to_string()),
            refresh: false,
            rotate_publish_key: false,
            rotate_root_key: false,
            yes: true,
            json: false,
        }
    }
}

fn run_git(root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {:?} failed: {}{}",
        args,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

struct EnvVarGuard {
    key: &'static str,
    previous: Option<OsString>,
}

impl EnvVarGuard {
    fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
        let previous = std::env::var_os(key);
        unsafe {
            std::env::set_var(key, value);
        }
        Self { key, previous }
    }

    fn remove(key: &'static str) -> Self {
        let previous = std::env::var_os(key);
        unsafe {
            std::env::remove_var(key);
        }
        Self { key, previous }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        unsafe {
            if let Some(previous) = &self.previous {
                std::env::set_var(self.key, previous);
            } else {
                std::env::remove_var(self.key);
            }
        }
    }
}

static ENV_LOCK: Mutex<()> = Mutex::const_new(());
