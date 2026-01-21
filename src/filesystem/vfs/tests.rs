// src/filesystem/vfs/tests.rs

use super::*;

#[test]
fn test_new_tree_has_root() {
    let tree = VfsTree::new();
    assert_eq!(tree.len(), 1);
    assert!(tree.exists("/"));
    assert!(tree.get_node(tree.root()).is_directory());
}

#[test]
fn test_mkdir_creates_directory() {
    let mut tree = VfsTree::new();

    let id = tree.mkdir("/usr").expect("should create /usr directory");
    assert!(tree.exists("/usr"));
    assert!(tree.get_node(id).is_directory());
}

#[test]
fn test_mkdir_nested() {
    let mut tree = VfsTree::new();

    tree.mkdir("/usr").expect("should create /usr directory");
    tree.mkdir("/usr/bin").expect("should create /usr/bin directory");
    tree.mkdir("/usr/lib").expect("should create /usr/lib directory");

    assert!(tree.exists("/usr"));
    assert!(tree.exists("/usr/bin"));
    assert!(tree.exists("/usr/lib"));
}

#[test]
fn test_mkdir_fails_without_parent() {
    let mut tree = VfsTree::new();

    let result = tree.mkdir("/usr/bin");
    assert!(result.is_err());
}

#[test]
fn test_mkdir_p_creates_parents() {
    let mut tree = VfsTree::new();

    tree.mkdir_p("/usr/local/bin").unwrap();

    assert!(tree.exists("/usr"));
    assert!(tree.exists("/usr/local"));
    assert!(tree.exists("/usr/local/bin"));
}

#[test]
fn test_add_file() {
    let mut tree = VfsTree::new();

    tree.mkdir("/usr").unwrap();
    tree.mkdir("/usr/bin").unwrap();

    let id = tree
        .add_file("/usr/bin/bash", "abc123", 1024, 0o755)
        .unwrap();

    assert!(tree.exists("/usr/bin/bash"));
    let node = tree.get_node(id);
    assert!(node.is_file());
    assert_eq!(node.permissions(), 0o755);

    if let NodeKind::File { hash, size } = node.kind() {
        assert_eq!(hash, "abc123");
        assert_eq!(*size, 1024);
    } else {
        panic!("expected file node");
    }
}

#[test]
fn test_add_symlink() {
    let mut tree = VfsTree::new();

    tree.mkdir("/usr").unwrap();
    tree.mkdir("/usr/bin").unwrap();

    let id = tree.add_symlink("/usr/bin/sh", "/bin/bash").unwrap();

    assert!(tree.exists("/usr/bin/sh"));
    let node = tree.get_node(id);
    assert!(node.is_symlink());

    if let NodeKind::Symlink { target } = node.kind() {
        assert_eq!(target, Path::new("/bin/bash"));
    } else {
        panic!("expected symlink node");
    }
}

#[test]
fn test_o1_lookup() {
    let mut tree = VfsTree::new();

    tree.mkdir_p("/very/deep/nested/directory/structure").unwrap();
    tree.add_file(
        "/very/deep/nested/directory/structure/file.txt",
        "hash",
        100,
        0o644,
    )
    .unwrap();

    // O(1) lookup regardless of depth
    let id = tree.lookup("/very/deep/nested/directory/structure/file.txt");
    assert!(id.is_some());
}

#[test]
fn test_get_path() {
    let mut tree = VfsTree::new();

    tree.mkdir_p("/usr/local/bin").unwrap();
    let id = tree
        .add_file("/usr/local/bin/myapp", "hash", 100, 0o755)
        .unwrap();

    let path = tree.get_path(id);
    assert_eq!(path, PathBuf::from("/usr/local/bin/myapp"));
}

#[test]
fn test_children() {
    let mut tree = VfsTree::new();

    tree.mkdir("/etc").unwrap();
    tree.add_file("/etc/passwd", "hash1", 100, 0o644).unwrap();
    tree.add_file("/etc/shadow", "hash2", 100, 0o600).unwrap();
    tree.mkdir("/etc/conf.d").unwrap();

    let etc_id = tree.lookup("/etc").unwrap();
    let children: Vec<_> = tree.children(etc_id).collect();

    assert_eq!(children.len(), 3);
    let names: Vec<_> = children.iter().map(|(name, _)| *name).collect();
    assert!(names.contains(&"passwd"));
    assert!(names.contains(&"shadow"));
    assert!(names.contains(&"conf.d"));
}

#[test]
fn test_remove() {
    let mut tree = VfsTree::new();

    tree.mkdir_p("/usr/local/bin").unwrap();
    tree.add_file("/usr/local/bin/app", "hash", 100, 0o755).unwrap();

    assert!(tree.exists("/usr/local/bin/app"));
    tree.remove("/usr/local/bin/app").unwrap();
    assert!(!tree.exists("/usr/local/bin/app"));
}

#[test]
fn test_remove_directory_with_children() {
    let mut tree = VfsTree::new();

    tree.mkdir_p("/usr/local/bin").unwrap();
    tree.add_file("/usr/local/bin/app1", "hash1", 100, 0o755).unwrap();
    tree.add_file("/usr/local/bin/app2", "hash2", 100, 0o755).unwrap();

    tree.remove("/usr/local").unwrap();

    assert!(tree.exists("/usr"));
    assert!(!tree.exists("/usr/local"));
    assert!(!tree.exists("/usr/local/bin"));
    assert!(!tree.exists("/usr/local/bin/app1"));
    assert!(!tree.exists("/usr/local/bin/app2"));
}

#[test]
fn test_cannot_remove_root() {
    let mut tree = VfsTree::new();

    let result = tree.remove("/");
    assert!(result.is_err());
}

#[test]
fn test_walk() {
    let mut tree = VfsTree::new();

    tree.mkdir("/etc").unwrap();
    tree.add_file("/etc/passwd", "hash", 100, 0o644).unwrap();
    tree.mkdir("/usr").unwrap();
    tree.mkdir("/usr/bin").unwrap();

    let mut visited = Vec::new();
    tree.walk(|_id, _node, path| {
        visited.push(path.to_path_buf());
    });

    assert!(visited.contains(&PathBuf::from("/")));
    assert!(visited.contains(&PathBuf::from("/etc")));
    assert!(visited.contains(&PathBuf::from("/etc/passwd")));
    assert!(visited.contains(&PathBuf::from("/usr")));
    assert!(visited.contains(&PathBuf::from("/usr/bin")));
}

#[test]
fn test_stats() {
    let mut tree = VfsTree::new();

    tree.mkdir("/etc").unwrap();
    tree.mkdir("/usr").unwrap();
    tree.add_file("/etc/passwd", "hash1", 1000, 0o644).unwrap();
    tree.add_file("/etc/shadow", "hash2", 500, 0o600).unwrap();
    tree.add_symlink("/etc/localtime", "/usr/share/zoneinfo/UTC").unwrap();

    let stats = tree.stats();

    assert_eq!(stats.directories, 3); // root + etc + usr
    assert_eq!(stats.files, 2);
    assert_eq!(stats.symlinks, 1);
    assert_eq!(stats.total_size, 1500);
    assert_eq!(stats.total_nodes, 6);
}

#[test]
fn test_with_capacity() {
    let tree = VfsTree::with_capacity(1000);
    assert!(tree.is_empty());
    assert_eq!(tree.len(), 1); // Just root
}

#[test]
fn test_duplicate_path_fails() {
    let mut tree = VfsTree::new();

    tree.mkdir("/etc").unwrap();
    let result = tree.mkdir("/etc");
    assert!(result.is_err());
}

#[test]
fn test_normalize_path() {
    assert_eq!(normalize_path(Path::new("/usr")), PathBuf::from("/usr"));
    assert_eq!(normalize_path(Path::new("/usr/")), PathBuf::from("/usr"));
    assert_eq!(normalize_path(Path::new("usr")), PathBuf::from("/usr"));
    assert_eq!(normalize_path(Path::new("/")), PathBuf::from("/"));
}

#[test]
fn test_file_permissions() {
    let mut tree = VfsTree::new();

    tree.mkdir_with_permissions("/etc", 0o750).unwrap();
    tree.add_file("/etc/shadow", "hash", 100, 0o600).unwrap();

    let etc = tree.get("/etc").unwrap();
    assert_eq!(etc.permissions(), 0o750);

    let shadow = tree.get("/etc/shadow").unwrap();
    assert_eq!(shadow.permissions(), 0o600);
}

#[test]
fn test_reparent_simple() {
    let mut tree = VfsTree::new();

    tree.mkdir("/src").unwrap();
    tree.mkdir("/dest").unwrap();
    tree.add_file("/src/file.txt", "hash", 100, 0o644).unwrap();

    tree.reparent("/src/file.txt", "/dest").unwrap();

    assert!(!tree.exists("/src/file.txt"));
    assert!(tree.exists("/dest/file.txt"));
}

#[test]
fn test_reparent_directory_with_children() {
    let mut tree = VfsTree::new();

    tree.mkdir_p("/project/src/components").unwrap();
    tree.add_file("/project/src/components/button.rs", "hash1", 100, 0o644).unwrap();
    tree.add_file("/project/src/components/input.rs", "hash2", 100, 0o644).unwrap();
    tree.mkdir("/project/lib").unwrap();

    // Move entire components directory to lib
    tree.reparent("/project/src/components", "/project/lib").unwrap();

    // Old paths should not exist
    assert!(!tree.exists("/project/src/components"));
    assert!(!tree.exists("/project/src/components/button.rs"));
    assert!(!tree.exists("/project/src/components/input.rs"));

    // New paths should exist
    assert!(tree.exists("/project/lib/components"));
    assert!(tree.exists("/project/lib/components/button.rs"));
    assert!(tree.exists("/project/lib/components/input.rs"));
}

#[test]
fn test_reparent_to_root() {
    let mut tree = VfsTree::new();

    tree.mkdir_p("/deep/nested/dir").unwrap();
    tree.add_file("/deep/nested/dir/file.txt", "hash", 100, 0o644).unwrap();

    tree.reparent("/deep/nested/dir", "/").unwrap();

    assert!(!tree.exists("/deep/nested/dir"));
    assert!(tree.exists("/dir"));
    assert!(tree.exists("/dir/file.txt"));
}

#[test]
fn test_reparent_cannot_move_root() {
    let mut tree = VfsTree::new();

    tree.mkdir("/dest").unwrap();

    let result = tree.reparent("/", "/dest");
    assert!(result.is_err());
}

#[test]
fn test_reparent_cannot_create_cycle() {
    let mut tree = VfsTree::new();

    tree.mkdir_p("/a/b/c").unwrap();

    // Cannot move /a into /a/b/c (would create a cycle)
    let result = tree.reparent("/a", "/a/b/c");
    assert!(result.is_err());
}

#[test]
fn test_reparent_name_collision() {
    let mut tree = VfsTree::new();

    tree.mkdir("/src").unwrap();
    tree.mkdir("/dest").unwrap();
    tree.add_file("/src/file.txt", "hash1", 100, 0o644).unwrap();
    tree.add_file("/dest/file.txt", "hash2", 200, 0o644).unwrap();

    // Cannot move - name collision
    let result = tree.reparent("/src/file.txt", "/dest");
    assert!(result.is_err());
}

#[test]
fn test_reparent_to_non_directory() {
    let mut tree = VfsTree::new();

    tree.mkdir("/src").unwrap();
    tree.add_file("/src/file1.txt", "hash1", 100, 0o644).unwrap();
    tree.add_file("/dest.txt", "hash2", 100, 0o644).unwrap();

    // Cannot move to a file
    let result = tree.reparent("/src/file1.txt", "/dest.txt");
    assert!(result.is_err());
}

#[test]
fn test_reparent_with_rename() {
    let mut tree = VfsTree::new();

    tree.mkdir("/src").unwrap();
    tree.mkdir("/dest").unwrap();
    tree.add_file("/src/old_name.txt", "hash", 100, 0o644).unwrap();

    tree.reparent_with_rename("/src/old_name.txt", "/dest", "new_name.txt")
        .unwrap();

    assert!(!tree.exists("/src/old_name.txt"));
    assert!(tree.exists("/dest/new_name.txt"));
}

#[test]
fn test_reparent_with_rename_directory() {
    let mut tree = VfsTree::new();

    tree.mkdir_p("/project/old_module").unwrap();
    tree.add_file("/project/old_module/mod.rs", "hash", 100, 0o644).unwrap();
    tree.mkdir("/lib").unwrap();

    tree.reparent_with_rename("/project/old_module", "/lib", "new_module")
        .unwrap();

    assert!(!tree.exists("/project/old_module"));
    assert!(!tree.exists("/project/old_module/mod.rs"));
    assert!(tree.exists("/lib/new_module"));
    assert!(tree.exists("/lib/new_module/mod.rs"));
}

#[test]
fn test_reparent_preserves_node_ids() {
    let mut tree = VfsTree::new();

    tree.mkdir("/src").unwrap();
    tree.mkdir("/dest").unwrap();
    let file_id = tree
        .add_file("/src/file.txt", "hash", 100, 0o644)
        .unwrap();

    tree.reparent("/src/file.txt", "/dest").unwrap();

    // The node ID should still be valid and point to the same node
    let node = tree.get_node(file_id);
    assert_eq!(node.name(), "file.txt");
    assert!(node.is_file());
}

#[test]
fn test_reparent_updates_path_index() {
    let mut tree = VfsTree::new();

    tree.mkdir_p("/a/b/c").unwrap();
    tree.add_file("/a/b/c/file.txt", "hash", 100, 0o644).unwrap();
    tree.mkdir("/x").unwrap();

    // Get the file ID before reparenting
    let file_id = tree.lookup("/a/b/c/file.txt").unwrap();

    tree.reparent("/a/b", "/x").unwrap();

    // O(1) lookup should work with new paths
    let new_file_id = tree.lookup("/x/b/c/file.txt").unwrap();
    assert_eq!(file_id, new_file_id);

    // Old path should not be in index
    assert!(tree.lookup("/a/b/c/file.txt").is_none());
}
