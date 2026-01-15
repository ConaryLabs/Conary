// src/filesystem/vfs.rs

//! Virtual File System tree with arena allocation and O(1) path lookup
//!
//! This module provides an efficient in-memory representation of filesystem
//! hierarchies using arena allocation for nodes and HashMap for path lookup.
//!
//! # Design
//!
//! - **Arena Allocation**: All nodes are stored in a contiguous Vec, referenced
//!   by `NodeId` indices. This provides cache-friendly iteration, eliminates
//!   pointer chasing, and enables efficient bulk operations.
//!
//! - **O(1) Path Lookup**: A HashMap maps absolute paths to node IDs, enabling
//!   constant-time path resolution without tree traversal.
//!
//! - **Tree Structure**: Nodes maintain parent/child relationships for tree
//!   operations like subtree reparenting and hierarchical traversal.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

/// Index into the arena for referencing nodes
///
/// This is a lightweight handle (just a usize) that can be copied freely.
/// Invalid or stale NodeIds will cause panics when used - this is intentional
/// as it indicates a bug in the calling code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NodeId(usize);

impl NodeId {
    /// Get the raw index value
    #[inline]
    pub fn index(self) -> usize {
        self.0
    }
}

/// Type of VFS node
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeKind {
    /// Directory node that can contain children
    Directory,
    /// Regular file with content hash and size
    File {
        /// SHA-256 hash of file contents (for CAS lookup)
        hash: String,
        /// File size in bytes
        size: u64,
    },
    /// Symbolic link pointing to a target path
    Symlink {
        /// Target path of the symlink
        target: PathBuf,
    },
}

/// A node in the VFS tree
///
/// Nodes are stored in an arena and referenced by `NodeId`.
#[derive(Debug)]
pub struct VfsNode {
    /// Name of this node (just the filename, not full path)
    name: String,
    /// Type of node
    kind: NodeKind,
    /// Parent node (None for root)
    parent: Option<NodeId>,
    /// Children node IDs (only populated for directories)
    children: Vec<NodeId>,
    /// File permissions (Unix mode bits)
    permissions: u32,
}

impl VfsNode {
    /// Get the node name
    #[inline]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Get the node kind
    #[inline]
    pub fn kind(&self) -> &NodeKind {
        &self.kind
    }

    /// Get the parent node ID
    #[inline]
    pub fn parent(&self) -> Option<NodeId> {
        self.parent
    }

    /// Get children node IDs
    #[inline]
    pub fn children(&self) -> &[NodeId] {
        &self.children
    }

    /// Get file permissions
    #[inline]
    pub fn permissions(&self) -> u32 {
        self.permissions
    }

    /// Check if this is a directory
    #[inline]
    pub fn is_directory(&self) -> bool {
        matches!(self.kind, NodeKind::Directory)
    }

    /// Check if this is a regular file
    #[inline]
    pub fn is_file(&self) -> bool {
        matches!(self.kind, NodeKind::File { .. })
    }

    /// Check if this is a symlink
    #[inline]
    pub fn is_symlink(&self) -> bool {
        matches!(self.kind, NodeKind::Symlink { .. })
    }
}

/// Arena-allocated VFS tree with O(1) path lookup
///
/// # Example
///
/// ```ignore
/// use conary::filesystem::vfs::VfsTree;
///
/// let mut tree = VfsTree::new();
///
/// // Create directory structure
/// tree.mkdir("/usr")?;
/// tree.mkdir("/usr/bin")?;
///
/// // Add a file
/// tree.add_file("/usr/bin/bash", "abc123...", 1024, 0o755)?;
///
/// // O(1) lookup
/// let node = tree.get("/usr/bin/bash")?;
/// ```
#[derive(Debug)]
pub struct VfsTree {
    /// Arena storage for nodes
    nodes: Vec<VfsNode>,
    /// O(1) path to node lookup
    path_index: HashMap<PathBuf, NodeId>,
    /// Root node ID (always 0)
    root: NodeId,
}

impl Default for VfsTree {
    fn default() -> Self {
        Self::new()
    }
}

impl VfsTree {
    /// Create a new VFS tree with an empty root directory
    pub fn new() -> Self {
        let root_node = VfsNode {
            name: String::new(),
            kind: NodeKind::Directory,
            parent: None,
            children: Vec::new(),
            permissions: 0o755,
        };

        let root_id = NodeId(0);
        let mut path_index = HashMap::new();
        path_index.insert(PathBuf::from("/"), root_id);

        Self {
            nodes: vec![root_node],
            path_index,
            root: root_id,
        }
    }

    /// Create a new VFS tree with pre-allocated capacity
    ///
    /// Use this when you know approximately how many nodes will be added.
    pub fn with_capacity(node_capacity: usize) -> Self {
        let root_node = VfsNode {
            name: String::new(),
            kind: NodeKind::Directory,
            parent: None,
            children: Vec::new(),
            permissions: 0o755,
        };

        let root_id = NodeId(0);
        let mut path_index = HashMap::with_capacity(node_capacity);
        path_index.insert(PathBuf::from("/"), root_id);

        let mut nodes = Vec::with_capacity(node_capacity);
        nodes.push(root_node);

        Self {
            nodes,
            path_index,
            root: root_id,
        }
    }

    /// Get the root node ID
    #[inline]
    pub fn root(&self) -> NodeId {
        self.root
    }

    /// Get the total number of nodes in the tree
    #[inline]
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Check if the tree is empty (only root exists)
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.nodes.len() == 1
    }

    /// Get a node by ID
    ///
    /// # Panics
    ///
    /// Panics if the NodeId is invalid (out of bounds).
    #[inline]
    pub fn get_node(&self, id: NodeId) -> &VfsNode {
        &self.nodes[id.0]
    }

    /// Get a mutable reference to a node by ID
    ///
    /// # Panics
    ///
    /// Panics if the NodeId is invalid (out of bounds).
    #[inline]
    fn get_node_mut(&mut self, id: NodeId) -> &mut VfsNode {
        &mut self.nodes[id.0]
    }

    /// Look up a node by path - O(1) operation
    ///
    /// Returns None if the path doesn't exist.
    #[inline]
    pub fn lookup(&self, path: impl AsRef<Path>) -> Option<NodeId> {
        self.path_index.get(path.as_ref()).copied()
    }

    /// Get a node by path - O(1) operation
    ///
    /// Returns an error if the path doesn't exist.
    pub fn get(&self, path: impl AsRef<Path>) -> Result<&VfsNode> {
        let path = path.as_ref();
        self.lookup(path)
            .map(|id| self.get_node(id))
            .ok_or_else(|| Error::NotFound(format!("path not found: {}", path.display())))
    }

    /// Check if a path exists - O(1) operation
    #[inline]
    pub fn exists(&self, path: impl AsRef<Path>) -> bool {
        self.path_index.contains_key(path.as_ref())
    }

    /// Get the full path of a node by traversing up to root
    pub fn get_path(&self, id: NodeId) -> PathBuf {
        let mut components = Vec::new();
        let mut current = id;

        loop {
            let node = self.get_node(current);
            if node.parent.is_none() {
                break;
            }
            components.push(node.name.clone());
            current = node.parent.unwrap();
        }

        components.reverse();
        let mut path = PathBuf::from("/");
        for component in components {
            path.push(component);
        }
        path
    }

    /// Create a directory at the given path
    ///
    /// Parent directories must already exist. Use `mkdir_p` for recursive creation.
    pub fn mkdir(&mut self, path: impl AsRef<Path>) -> Result<NodeId> {
        self.mkdir_with_permissions(path, 0o755)
    }

    /// Create a directory with specific permissions
    pub fn mkdir_with_permissions(
        &mut self,
        path: impl AsRef<Path>,
        permissions: u32,
    ) -> Result<NodeId> {
        let path = normalize_path(path.as_ref());

        // Check if already exists
        if self.exists(&path) {
            return Err(Error::AlreadyExists(format!(
                "path already exists: {}",
                path.display()
            )));
        }

        // Get parent directory
        let parent_path = path
            .parent()
            .ok_or_else(|| Error::InvalidPath("cannot create root".into()))?;

        let parent_id = self.lookup(parent_path).ok_or_else(|| {
            Error::NotFound(format!("parent directory not found: {}", parent_path.display()))
        })?;

        // Verify parent is a directory
        if !self.get_node(parent_id).is_directory() {
            return Err(Error::InvalidPath(format!(
                "parent is not a directory: {}",
                parent_path.display()
            )));
        }

        // Get the directory name
        let name = path
            .file_name()
            .ok_or_else(|| Error::InvalidPath("invalid path".into()))?
            .to_string_lossy()
            .to_string();

        // Create the node
        let node_id = self.allocate_node(VfsNode {
            name,
            kind: NodeKind::Directory,
            parent: Some(parent_id),
            children: Vec::new(),
            permissions,
        });

        // Add to parent's children
        self.get_node_mut(parent_id).children.push(node_id);

        // Add to path index
        self.path_index.insert(path, node_id);

        Ok(node_id)
    }

    /// Create a directory and all parent directories as needed
    pub fn mkdir_p(&mut self, path: impl AsRef<Path>) -> Result<NodeId> {
        self.mkdir_p_with_permissions(path, 0o755)
    }

    /// Create a directory and all parent directories with specific permissions
    pub fn mkdir_p_with_permissions(
        &mut self,
        path: impl AsRef<Path>,
        permissions: u32,
    ) -> Result<NodeId> {
        let path = normalize_path(path.as_ref());

        // If it already exists, return it (if it's a directory)
        if let Some(id) = self.lookup(&path) {
            if self.get_node(id).is_directory() {
                return Ok(id);
            }
            return Err(Error::InvalidPath(format!(
                "path exists but is not a directory: {}",
                path.display()
            )));
        }

        // Build list of directories to create
        let mut to_create = Vec::new();
        let mut current = path.clone();

        while !self.exists(&current) {
            to_create.push(current.clone());
            if let Some(parent) = current.parent() {
                current = parent.to_path_buf();
            } else {
                break;
            }
        }

        // Create directories from top to bottom
        to_create.reverse();
        let mut last_id = self.root;

        for dir_path in to_create {
            last_id = self.mkdir_with_permissions(&dir_path, permissions)?;
        }

        Ok(last_id)
    }

    /// Add a regular file to the tree
    pub fn add_file(
        &mut self,
        path: impl AsRef<Path>,
        hash: impl Into<String>,
        size: u64,
        permissions: u32,
    ) -> Result<NodeId> {
        let path = normalize_path(path.as_ref());

        // Check if already exists
        if self.exists(&path) {
            return Err(Error::AlreadyExists(format!(
                "path already exists: {}",
                path.display()
            )));
        }

        // Get parent directory
        let parent_path = path
            .parent()
            .ok_or_else(|| Error::InvalidPath("cannot create file at root".into()))?;

        let parent_id = self.lookup(parent_path).ok_or_else(|| {
            Error::NotFound(format!("parent directory not found: {}", parent_path.display()))
        })?;

        // Verify parent is a directory
        if !self.get_node(parent_id).is_directory() {
            return Err(Error::InvalidPath(format!(
                "parent is not a directory: {}",
                parent_path.display()
            )));
        }

        // Get the file name
        let name = path
            .file_name()
            .ok_or_else(|| Error::InvalidPath("invalid path".into()))?
            .to_string_lossy()
            .to_string();

        // Create the node
        let node_id = self.allocate_node(VfsNode {
            name,
            kind: NodeKind::File {
                hash: hash.into(),
                size,
            },
            parent: Some(parent_id),
            children: Vec::new(),
            permissions,
        });

        // Add to parent's children
        self.get_node_mut(parent_id).children.push(node_id);

        // Add to path index
        self.path_index.insert(path, node_id);

        Ok(node_id)
    }

    /// Add a symlink to the tree
    pub fn add_symlink(
        &mut self,
        path: impl AsRef<Path>,
        target: impl AsRef<Path>,
    ) -> Result<NodeId> {
        let path = normalize_path(path.as_ref());

        // Check if already exists
        if self.exists(&path) {
            return Err(Error::AlreadyExists(format!(
                "path already exists: {}",
                path.display()
            )));
        }

        // Get parent directory
        let parent_path = path
            .parent()
            .ok_or_else(|| Error::InvalidPath("cannot create symlink at root".into()))?;

        let parent_id = self.lookup(parent_path).ok_or_else(|| {
            Error::NotFound(format!("parent directory not found: {}", parent_path.display()))
        })?;

        // Verify parent is a directory
        if !self.get_node(parent_id).is_directory() {
            return Err(Error::InvalidPath(format!(
                "parent is not a directory: {}",
                parent_path.display()
            )));
        }

        // Get the symlink name
        let name = path
            .file_name()
            .ok_or_else(|| Error::InvalidPath("invalid path".into()))?
            .to_string_lossy()
            .to_string();

        // Create the node
        let node_id = self.allocate_node(VfsNode {
            name,
            kind: NodeKind::Symlink {
                target: target.as_ref().to_path_buf(),
            },
            parent: Some(parent_id),
            children: Vec::new(),
            permissions: 0o777, // Symlinks typically have 777 permissions
        });

        // Add to parent's children
        self.get_node_mut(parent_id).children.push(node_id);

        // Add to path index
        self.path_index.insert(path, node_id);

        Ok(node_id)
    }

    /// Remove a node and all its children from the tree
    ///
    /// Note: This marks nodes as removed but doesn't compact the arena.
    /// Use `compact()` periodically if many removals are performed.
    pub fn remove(&mut self, path: impl AsRef<Path>) -> Result<()> {
        let path = normalize_path(path.as_ref());

        if path == Path::new("/") {
            return Err(Error::InvalidPath("cannot remove root".into()));
        }

        let node_id = self.lookup(&path).ok_or_else(|| {
            Error::NotFound(format!("path not found: {}", path.display()))
        })?;

        // Collect all descendants to remove from path index
        let mut to_remove = Vec::new();
        self.collect_descendants(node_id, &mut to_remove);
        to_remove.push(node_id);

        // Remove from parent's children list
        let parent_id = self.get_node(node_id).parent.unwrap();
        let parent = self.get_node_mut(parent_id);
        parent.children.retain(|&id| id != node_id);

        // Remove all paths from index
        for &id in &to_remove {
            let node_path = self.get_path(id);
            self.path_index.remove(&node_path);
        }

        Ok(())
    }

    /// Reparent a subtree to a new location
    ///
    /// Moves a node and all its descendants to become a child of a new parent.
    /// This is useful for component operations that reorganize file hierarchies.
    ///
    /// # Arguments
    ///
    /// * `source` - Path of the node to move
    /// * `new_parent` - Path of the new parent directory
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - Source path doesn't exist
    /// - Source is the root node
    /// - New parent doesn't exist
    /// - New parent is not a directory
    /// - New parent is a descendant of source (would create a cycle)
    /// - A node with the same name already exists in the new parent
    pub fn reparent(
        &mut self,
        source: impl AsRef<Path>,
        new_parent: impl AsRef<Path>,
    ) -> Result<()> {
        let source_path = normalize_path(source.as_ref());
        let new_parent_path = normalize_path(new_parent.as_ref());

        // Cannot reparent root
        if source_path == Path::new("/") {
            return Err(Error::InvalidPath("cannot reparent root".into()));
        }

        // Get source node
        let source_id = self.lookup(&source_path).ok_or_else(|| {
            Error::NotFound(format!("source path not found: {}", source_path.display()))
        })?;

        // Get new parent node
        let new_parent_id = self.lookup(&new_parent_path).ok_or_else(|| {
            Error::NotFound(format!(
                "new parent not found: {}",
                new_parent_path.display()
            ))
        })?;

        // Verify new parent is a directory
        if !self.get_node(new_parent_id).is_directory() {
            return Err(Error::InvalidPath(format!(
                "new parent is not a directory: {}",
                new_parent_path.display()
            )));
        }

        // Check if new parent is a descendant of source (would create cycle)
        if self.is_descendant_of(new_parent_id, source_id) {
            return Err(Error::InvalidPath(
                "cannot reparent a node into its own subtree".into(),
            ));
        }

        // Get source node name and check for name collision in new parent
        let source_name = self.get_node(source_id).name.clone();
        let new_path = new_parent_path.join(&source_name);

        if self.exists(&new_path) {
            return Err(Error::AlreadyExists(format!(
                "path already exists: {}",
                new_path.display()
            )));
        }

        // Get old parent ID
        let old_parent_id = self.get_node(source_id).parent.unwrap();

        // Collect all nodes in the subtree for path index updates
        let mut subtree_nodes = vec![source_id];
        self.collect_descendants(source_id, &mut subtree_nodes);

        // Collect old paths before modifying the tree
        let old_paths: Vec<(NodeId, PathBuf)> = subtree_nodes
            .iter()
            .map(|&id| (id, self.get_path(id)))
            .collect();

        // Remove from old parent's children list
        self.get_node_mut(old_parent_id)
            .children
            .retain(|&id| id != source_id);

        // Update source node's parent
        self.get_node_mut(source_id).parent = Some(new_parent_id);

        // Add to new parent's children list
        self.get_node_mut(new_parent_id).children.push(source_id);

        // Update path index for all nodes in the subtree
        for (id, old_path) in old_paths {
            self.path_index.remove(&old_path);
            let new_node_path = self.get_path(id);
            self.path_index.insert(new_node_path, id);
        }

        Ok(())
    }

    /// Reparent with rename - move a subtree to a new location with a new name
    ///
    /// Similar to `reparent`, but also renames the moved node.
    pub fn reparent_with_rename(
        &mut self,
        source: impl AsRef<Path>,
        new_parent: impl AsRef<Path>,
        new_name: impl Into<String>,
    ) -> Result<()> {
        let source_path = normalize_path(source.as_ref());
        let new_parent_path = normalize_path(new_parent.as_ref());
        let new_name = new_name.into();

        // Cannot reparent root
        if source_path == Path::new("/") {
            return Err(Error::InvalidPath("cannot reparent root".into()));
        }

        // Validate new name
        if new_name.is_empty() || new_name.contains('/') {
            return Err(Error::InvalidPath(format!("invalid name: {}", new_name)));
        }

        // Get source node
        let source_id = self.lookup(&source_path).ok_or_else(|| {
            Error::NotFound(format!("source path not found: {}", source_path.display()))
        })?;

        // Get new parent node
        let new_parent_id = self.lookup(&new_parent_path).ok_or_else(|| {
            Error::NotFound(format!(
                "new parent not found: {}",
                new_parent_path.display()
            ))
        })?;

        // Verify new parent is a directory
        if !self.get_node(new_parent_id).is_directory() {
            return Err(Error::InvalidPath(format!(
                "new parent is not a directory: {}",
                new_parent_path.display()
            )));
        }

        // Check if new parent is a descendant of source (would create cycle)
        if self.is_descendant_of(new_parent_id, source_id) {
            return Err(Error::InvalidPath(
                "cannot reparent a node into its own subtree".into(),
            ));
        }

        // Check for name collision in new parent
        let new_path = new_parent_path.join(&new_name);
        if self.exists(&new_path) {
            return Err(Error::AlreadyExists(format!(
                "path already exists: {}",
                new_path.display()
            )));
        }

        // Get old parent ID
        let old_parent_id = self.get_node(source_id).parent.unwrap();

        // Collect all nodes in the subtree for path index updates
        let mut subtree_nodes = vec![source_id];
        self.collect_descendants(source_id, &mut subtree_nodes);

        // Collect old paths before modifying the tree
        let old_paths: Vec<(NodeId, PathBuf)> = subtree_nodes
            .iter()
            .map(|&id| (id, self.get_path(id)))
            .collect();

        // Remove from old parent's children list
        self.get_node_mut(old_parent_id)
            .children
            .retain(|&id| id != source_id);

        // Update source node's parent and name
        let source_node = self.get_node_mut(source_id);
        source_node.parent = Some(new_parent_id);
        source_node.name = new_name;

        // Add to new parent's children list
        self.get_node_mut(new_parent_id).children.push(source_id);

        // Update path index for all nodes in the subtree
        for (id, old_path) in old_paths {
            self.path_index.remove(&old_path);
            let new_node_path = self.get_path(id);
            self.path_index.insert(new_node_path, id);
        }

        Ok(())
    }

    /// Check if a node is a descendant of another node
    fn is_descendant_of(&self, potential_descendant: NodeId, potential_ancestor: NodeId) -> bool {
        let mut current = potential_descendant;
        while let Some(parent_id) = self.get_node(current).parent {
            if parent_id == potential_ancestor {
                return true;
            }
            current = parent_id;
        }
        false
    }

    /// Collect all descendant node IDs
    fn collect_descendants(&self, id: NodeId, result: &mut Vec<NodeId>) {
        let node = self.get_node(id);
        for &child_id in &node.children {
            result.push(child_id);
            self.collect_descendants(child_id, result);
        }
    }

    /// Allocate a new node in the arena
    fn allocate_node(&mut self, node: VfsNode) -> NodeId {
        let id = NodeId(self.nodes.len());
        self.nodes.push(node);
        id
    }

    /// Iterate over all nodes in the tree
    pub fn iter(&self) -> impl Iterator<Item = (NodeId, &VfsNode)> {
        self.nodes
            .iter()
            .enumerate()
            .map(|(i, node)| (NodeId(i), node))
    }

    /// Iterate over all paths in the tree
    pub fn paths(&self) -> impl Iterator<Item = &PathBuf> {
        self.path_index.keys()
    }

    /// Get children of a directory as an iterator of (name, NodeId) pairs
    pub fn children(&self, id: NodeId) -> impl Iterator<Item = (&str, NodeId)> {
        let node = self.get_node(id);
        node.children
            .iter()
            .map(|&child_id| (self.get_node(child_id).name.as_str(), child_id))
    }

    /// Walk the tree depth-first, calling the visitor for each node
    pub fn walk<F>(&self, mut visitor: F)
    where
        F: FnMut(NodeId, &VfsNode, &Path),
    {
        let root_path = PathBuf::from("/");
        self.walk_recursive(self.root, &root_path, &mut visitor);
    }

    fn walk_recursive<F>(&self, id: NodeId, current_path: &Path, visitor: &mut F)
    where
        F: FnMut(NodeId, &VfsNode, &Path),
    {
        let node = self.get_node(id);
        visitor(id, node, current_path);

        for &child_id in &node.children {
            let child = self.get_node(child_id);
            let child_path = current_path.join(&child.name);
            self.walk_recursive(child_id, &child_path, visitor);
        }
    }

    /// Get statistics about the tree
    pub fn stats(&self) -> VfsStats {
        let mut stats = VfsStats::default();

        for node in &self.nodes {
            match &node.kind {
                NodeKind::Directory => stats.directories += 1,
                NodeKind::File { size, .. } => {
                    stats.files += 1;
                    stats.total_size += size;
                }
                NodeKind::Symlink { .. } => stats.symlinks += 1,
            }
        }

        stats.total_nodes = self.nodes.len();
        stats
    }
}

/// Statistics about a VFS tree
#[derive(Debug, Default, Clone)]
pub struct VfsStats {
    /// Total number of nodes
    pub total_nodes: usize,
    /// Number of directories
    pub directories: usize,
    /// Number of regular files
    pub files: usize,
    /// Number of symlinks
    pub symlinks: usize,
    /// Total size of all files in bytes
    pub total_size: u64,
}

/// Normalize a path to ensure it starts with / and has no trailing slashes
fn normalize_path(path: &Path) -> PathBuf {
    let path_str = path.to_string_lossy();

    // Ensure it starts with /
    let normalized = if !path_str.starts_with('/') {
        format!("/{}", path_str)
    } else {
        path_str.to_string()
    };

    // Remove trailing slashes (except for root)
    let normalized = if normalized.len() > 1 && normalized.ends_with('/') {
        normalized.trim_end_matches('/').to_string()
    } else {
        normalized
    };

    PathBuf::from(normalized)
}

#[cfg(test)]
mod tests {
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

        let id = tree.mkdir("/usr").unwrap();
        assert!(tree.exists("/usr"));
        assert!(tree.get_node(id).is_directory());
    }

    #[test]
    fn test_mkdir_nested() {
        let mut tree = VfsTree::new();

        tree.mkdir("/usr").unwrap();
        tree.mkdir("/usr/bin").unwrap();
        tree.mkdir("/usr/lib").unwrap();

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
}
