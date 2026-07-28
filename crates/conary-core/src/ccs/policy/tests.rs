// conary-core/src/ccs/policy/tests.rs

use super::*;
use std::fs::File;
use std::io::Read;
use tempfile::TempDir;

fn make_entry(path: &str, mode: u32) -> FileEntry {
    FileEntry {
        path: path.to_string(),
        node: crate::payload::PayloadNode::regular(mode),
        content: None,
        component: "runtime".to_string(),
        chunks: None,
    }
}

fn context_paths(temp: &TempDir, bytes: &[u8]) -> (PathBuf, PathBuf) {
    let source = temp.path().join("source");
    let replacement = temp.path().join("replacement");
    std::fs::write(&source, bytes).unwrap();
    (source, replacement)
}

fn read_all(path: &Path) -> Vec<u8> {
    let mut bytes = Vec::new();
    File::open(path).unwrap().read_to_end(&mut bytes).unwrap();
    bytes
}

#[test]
fn deny_paths_policy_rejects_configured_paths() {
    let policy = DenyPathsPolicy::new(&["/home/*".to_string()]).unwrap();
    let temp = TempDir::new().unwrap();
    let (source, replacement) = context_paths(&temp, b"content");
    let mut entry = make_entry("/home/user/file", 0o644);
    let mut context = PolicyContext {
        source_path: &source,
        replacement_path: &replacement,
        entry: &mut entry,
        config: &BuildPolicyConfig::default(),
    };
    assert!(matches!(
        policy.apply(&mut context).unwrap(),
        PolicyAction::Reject(_)
    ));
}

#[test]
fn fix_shebangs_streams_replacement_to_disk() {
    let policy = FixShebangsPolicy::new(HashMap::from([(
        "/usr/bin/env python".to_string(),
        "/usr/bin/python3".to_string(),
    )]));
    let temp = TempDir::new().unwrap();
    let (source, replacement) = context_paths(&temp, b"#!/usr/bin/env python\nprint('hello')\n");
    let mut entry = make_entry("/usr/bin/script.py", 0o755);
    let mut context = PolicyContext {
        source_path: &source,
        replacement_path: &replacement,
        entry: &mut entry,
        config: &BuildPolicyConfig::default(),
    };
    assert!(matches!(
        policy.apply(&mut context).unwrap(),
        PolicyAction::Replace(_)
    ));
    assert_eq!(
        read_all(&replacement),
        b"#!/usr/bin/python3\nprint('hello')\n"
    );
}

#[test]
fn compress_manpages_streams_deterministic_gzip() {
    let policy = CompressManpagesPolicy::new();
    let temp = TempDir::new().unwrap();
    let (source, replacement) = context_paths(&temp, b".TH APP 1\n.SH NAME\napp\n");
    let mut entry = make_entry("/usr/share/man/man1/app.1", 0o644);
    let mut context = PolicyContext {
        source_path: &source,
        replacement_path: &replacement,
        entry: &mut entry,
        config: &BuildPolicyConfig::default(),
    };
    assert!(matches!(
        policy.apply(&mut context).unwrap(),
        PolicyAction::Replace(_)
    ));
    assert_eq!(&read_all(&replacement)[..2], &[0x1f, 0x8b]);
}

#[test]
fn policy_chain_returns_disk_replacement() {
    let mut chain = PolicyChain::new();
    chain.add(Box::new(FixShebangsPolicy::new(HashMap::from([(
        "/usr/bin/env python".to_string(),
        "/usr/bin/python3".to_string(),
    )]))));
    let temp = TempDir::new().unwrap();
    let source = temp.path().join("source");
    let workspace = temp.path().join("workspace");
    std::fs::write(&source, b"#!/usr/bin/env python\n").unwrap();
    std::fs::create_dir(&workspace).unwrap();
    let mut entry = make_entry("/usr/bin/script.py", 0o755);
    let action = chain
        .apply(
            &mut entry,
            &source,
            &workspace,
            &BuildPolicyConfig::default(),
        )
        .unwrap();
    let PolicyAction::Replace(path) = action else {
        panic!("expected disk-backed replacement");
    };
    assert_eq!(read_all(&path), b"#!/usr/bin/python3\n");
}

#[test]
fn strip_setuid_policy_preserves_only_explicit_setuid() {
    let temp = TempDir::new().unwrap();
    let (source, replacement) = context_paths(&temp, b"payload");
    let mut entry = make_entry("/usr/bin/helper", 0o6755);
    let config = BuildPolicyConfig {
        allow_setuid_paths: vec!["/usr/bin/helper".to_string()],
        ..BuildPolicyConfig::default()
    };
    let mut context = PolicyContext {
        source_path: &source,
        replacement_path: &replacement,
        entry: &mut entry,
        config: &config,
    };
    assert_eq!(
        StripSetuidPolicy::new().apply(&mut context).unwrap(),
        PolicyAction::Keep
    );
    assert_eq!(context.entry.node.mode, libc::S_IFREG | 0o4755);
}

#[test]
fn native_elf_strip_is_disk_backed() {
    let Some(binary) = ["/bin/true", "/usr/bin/true", "/bin/ls", "/usr/bin/ls"]
        .into_iter()
        .find(|path| Path::new(path).exists())
    else {
        return;
    };
    let temp = TempDir::new().unwrap();
    let output = temp.path().join("stripped");
    match super::content::strip_elf_for_test(Path::new(binary), &output) {
        Ok(size) => {
            assert!(size >= 52);
            assert_eq!(&read_all(&output)[..4], b"\x7fELF");
        }
        Err(error) => assert!(
            error.contains("No sections") || error.contains("Not an executable"),
            "{error}"
        ),
    }
}
