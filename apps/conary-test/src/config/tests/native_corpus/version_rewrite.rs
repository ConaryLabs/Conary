// conary-test/src/config/tests/native_corpus/version_rewrite.rs

use super::conary_fixture_path;

#[test]
fn daily_driver_version_rewrite_changes_only_typed_package_version() {
    let directory = tempfile::tempdir().expect("create version rewrite test directory");
    let manifest_path = directory.path().join("ccs.toml");
    let source =
        std::fs::read_to_string(conary_fixture_path("phase4-daily-driver-corpus/ccs.toml"))
            .expect("read daily-driver source manifest");
    assert!(
        source.matches("version = \"1.0.0\"").count() > 1,
        "regression fixture must retain an unrelated native dependency version"
    );
    std::fs::write(&manifest_path, &source).expect("stage source manifest");

    let before: toml::Value = toml::from_str(&source).expect("parse source manifest");
    let script = conary_fixture_path("native/rewrite-package-version.py");
    let output = std::process::Command::new("python3")
        .arg(&script)
        .arg(&manifest_path)
        .arg("1.0.0")
        .arg("1.0.1")
        .output()
        .expect("run package version rewrite");
    assert!(
        output.status.success(),
        "package version rewrite failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let rewritten = std::fs::read_to_string(&manifest_path).expect("read rewritten manifest");
    let after: toml::Value = toml::from_str(&rewritten).expect("parse rewritten manifest");
    let mut expected = before;
    expected["package"]["version"] = toml::Value::String("1.0.1".to_owned());
    assert_eq!(after, expected);
    assert!(
        rewritten.contains("version = \"1.0.0\""),
        "unrelated native dependency version must remain unchanged"
    );

    let stale = std::process::Command::new("python3")
        .arg(script)
        .arg(&manifest_path)
        .arg("1.0.0")
        .arg("1.0.2")
        .output()
        .expect("run stale package version rewrite");
    assert!(!stale.status.success());
    assert!(
        String::from_utf8_lossy(&stale.stderr)
            .contains("package.version must equal expected version 1.0.0")
    );
    assert_eq!(
        std::fs::read_to_string(manifest_path).expect("read rejected manifest"),
        rewritten,
        "a rejected rewrite must not mutate the manifest"
    );
}
