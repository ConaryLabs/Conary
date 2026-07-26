// apps/conary/src/commands/install/payload_identity.rs

//! Exact source-identity resolution against the selected target root.
//!
//! Native package formats may name payload owners instead of carrying numeric
//! IDs.  Those names are meaningful only in the target root after typed
//! pre-payload lifecycle events (notably sysusers) have run.  This module reads
//! only that root's account databases and rejects missing or ambiguous
//! authority.

use anyhow::{Context, Result, bail};
use conary_core::payload::{PayloadIdentity, PayloadNode, ResolvedPayloadNode};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

const MAX_IDENTITY_DATABASE_SIZE: u64 = 16 * 1024 * 1024;

pub(super) fn resolve_payload_nodes(
    root: &Path,
    nodes: impl IntoIterator<Item = PayloadNode>,
) -> Result<Vec<ResolvedPayloadNode>> {
    let nodes = nodes.into_iter().collect::<Vec<_>>();
    let named_users = named_identities(nodes.iter().map(|node| &node.user));
    let named_groups = named_identities(nodes.iter().map(|node| &node.group));

    let users = if named_users.is_empty() {
        BTreeMap::new()
    } else {
        parse_identity_database(root, IdentityDatabaseKind::Passwd)?
    };
    let groups = if named_groups.is_empty() {
        BTreeMap::new()
    } else {
        parse_identity_database(root, IdentityDatabaseKind::Group)?
    };

    require_all_names(&named_users, &users, "user", root)?;
    require_all_names(&named_groups, &groups, "group", root)?;

    nodes
        .into_iter()
        .map(|source| {
            source.validate()?;
            let uid = resolve_identity(&source.user, &users, "user")?;
            let gid = resolve_identity(&source.group, &groups, "group")?;
            let resolved = ResolvedPayloadNode { source, uid, gid };
            resolved.validate()?;
            Ok(resolved)
        })
        .collect()
}

fn named_identities<'a>(
    identities: impl IntoIterator<Item = &'a PayloadIdentity>,
) -> BTreeSet<&'a str> {
    identities
        .into_iter()
        .filter_map(|identity| match identity {
            PayloadIdentity::Named { name } => Some(name.as_str()),
            PayloadIdentity::Numeric { .. } => None,
        })
        .collect()
}

fn require_all_names(
    required: &BTreeSet<&str>,
    resolved: &BTreeMap<String, u64>,
    kind: &str,
    root: &Path,
) -> Result<()> {
    let missing = required
        .iter()
        .copied()
        .filter(|name| !resolved.contains_key(*name))
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        bail!(
            "target root {} does not define payload {kind} name(s): {}",
            root.display(),
            missing.join(", ")
        );
    }
    Ok(())
}

fn resolve_identity(
    identity: &PayloadIdentity,
    names: &BTreeMap<String, u64>,
    kind: &str,
) -> Result<u64> {
    match identity {
        PayloadIdentity::Numeric { id } => Ok(*id),
        PayloadIdentity::Named { name } => names
            .get(name)
            .copied()
            .with_context(|| format!("payload {kind} name {name:?} was not resolved")),
    }
}

#[derive(Debug, Clone, Copy)]
enum IdentityDatabaseKind {
    Passwd,
    Group,
}

impl IdentityDatabaseKind {
    fn relative_path(self) -> &'static str {
        match self {
            Self::Passwd => "etc/passwd",
            Self::Group => "etc/group",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Passwd => "passwd",
            Self::Group => "group",
        }
    }

    fn expected_fields(self) -> usize {
        match self {
            Self::Passwd => 7,
            Self::Group => 4,
        }
    }

    fn id_field(self) -> usize {
        match self {
            Self::Passwd => 2,
            Self::Group => 2,
        }
    }
}

fn parse_identity_database(
    root: &Path,
    kind: IdentityDatabaseKind,
) -> Result<BTreeMap<String, u64>> {
    let path = safe_identity_database_path(root, kind)?;
    let metadata = fs::symlink_metadata(&path)
        .with_context(|| format!("target-root {} database is missing", kind.label()))?;
    if !metadata.file_type().is_file() {
        bail!(
            "target-root {} database is not a regular file: {}",
            kind.label(),
            path.display()
        );
    }
    if metadata.len() > MAX_IDENTITY_DATABASE_SIZE {
        bail!(
            "target-root {} database exceeds {} bytes: {}",
            kind.label(),
            MAX_IDENTITY_DATABASE_SIZE,
            path.display()
        );
    }
    let bytes =
        fs::read(&path).with_context(|| format!("failed to read target-root {}", kind.label()))?;
    let text = std::str::from_utf8(&bytes)
        .with_context(|| format!("target-root {} database is not UTF-8", kind.label()))?;
    parse_identity_records(text, kind)
        .map_err(|error| anyhow::anyhow!("invalid target-root {} database: {error}", kind.label()))
}

fn safe_identity_database_path(root: &Path, kind: IdentityDatabaseKind) -> Result<PathBuf> {
    let root_metadata = fs::symlink_metadata(root)
        .with_context(|| format!("target root does not exist: {}", root.display()))?;
    if !root_metadata.file_type().is_dir() {
        bail!("target root is not a directory: {}", root.display());
    }

    let etc = root.join("etc");
    let etc_metadata = fs::symlink_metadata(&etc)
        .with_context(|| format!("target root has no /etc directory: {}", root.display()))?;
    if !etc_metadata.file_type().is_dir() {
        bail!(
            "target-root /etc is not a real directory: {}",
            etc.display()
        );
    }

    let path = root.join(kind.relative_path());
    if fs::symlink_metadata(&path).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        bail!(
            "target-root {} database must not be a symlink: {}",
            kind.label(),
            path.display()
        );
    }
    Ok(path)
}

fn parse_identity_records(text: &str, kind: IdentityDatabaseKind) -> Result<BTreeMap<String, u64>> {
    let mut identities = BTreeMap::new();
    for (index, line) in text.lines().enumerate() {
        let line_number = index + 1;
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('+') || line.starts_with('-') {
            bail!(
                "{} line {} uses an unsupported external identity directive",
                kind.label(),
                line_number
            );
        }
        let fields = line.split(':').collect::<Vec<_>>();
        if fields.len() != kind.expected_fields() {
            bail!(
                "{} line {} has {} fields, expected {}",
                kind.label(),
                line_number,
                fields.len(),
                kind.expected_fields()
            );
        }
        let name = fields[0];
        if name.is_empty() {
            bail!("{} line {} has an empty name", kind.label(), line_number);
        }
        let id_text = fields[kind.id_field()];
        if id_text.is_empty() || !id_text.bytes().all(|byte| byte.is_ascii_digit()) {
            bail!(
                "{} line {} has a non-decimal numeric ID",
                kind.label(),
                line_number
            );
        }
        let id = id_text.parse::<u32>().with_context(|| {
            format!(
                "{} line {} numeric ID is outside the target platform range",
                kind.label(),
                line_number
            )
        })?;
        if identities.insert(name.to_string(), u64::from(id)).is_some() {
            bail!("{} name {name:?} is defined more than once", kind.label());
        }
    }
    Ok(identities)
}

#[cfg(test)]
mod tests {
    use super::*;
    use conary_core::payload::{PayloadIdentity, PayloadNode};

    fn named_node(user: &str, group: &str) -> PayloadNode {
        let mut node = PayloadNode::regular(0o755);
        node.user = PayloadIdentity::Named {
            name: user.to_string(),
        };
        node.group = PayloadIdentity::Named {
            name: group.to_string(),
        };
        node
    }

    #[test]
    fn resolves_names_only_from_selected_target_root() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("etc")).unwrap();
        fs::write(
            root.path().join("etc/passwd"),
            "root:x:0:0:root:/root:/bin/sh\nsvc:x:417:819:svc:/:/sbin/nologin\n",
        )
        .unwrap();
        fs::write(
            root.path().join("etc/group"),
            "root:x:0:\nsvc-group:x:819:\n",
        )
        .unwrap();

        let resolved =
            resolve_payload_nodes(root.path(), [named_node("svc", "svc-group")]).unwrap();

        assert_eq!(resolved[0].uid, 417);
        assert_eq!(resolved[0].gid, 819);
        assert_eq!(
            resolved[0].source.user,
            PayloadIdentity::Named {
                name: "svc".to_string()
            }
        );
    }

    #[test]
    fn numeric_payload_does_not_require_account_databases() {
        let root = tempfile::tempdir().unwrap();
        let mut node = PayloadNode::regular(0o644);
        node.user = PayloadIdentity::Numeric { id: 123 };
        node.group = PayloadIdentity::Numeric { id: 456 };

        let resolved = resolve_payload_nodes(root.path(), [node]).unwrap();

        assert_eq!(resolved[0].uid, 123);
        assert_eq!(resolved[0].gid, 456);
    }

    #[test]
    fn missing_name_rejects_the_full_payload() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("etc")).unwrap();
        fs::write(
            root.path().join("etc/passwd"),
            "root:x:0:0:root:/root:/bin/sh\n",
        )
        .unwrap();
        fs::write(
            root.path().join("etc/group"),
            "root:x:0:\nsvc-group:x:819:\n",
        )
        .unwrap();

        let error =
            resolve_payload_nodes(root.path(), [named_node("missing", "svc-group")]).unwrap_err();

        assert!(error.to_string().contains("does not define payload user"));
    }

    #[test]
    fn duplicate_name_is_ambiguous_and_rejected() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("etc")).unwrap();
        fs::write(
            root.path().join("etc/passwd"),
            "svc:x:417:819:svc:/:/sbin/nologin\nsvc:x:418:819:svc:/:/sbin/nologin\n",
        )
        .unwrap();
        fs::write(root.path().join("etc/group"), "svc-group:x:819:\n").unwrap();

        let error =
            resolve_payload_nodes(root.path(), [named_node("svc", "svc-group")]).unwrap_err();

        assert!(error.to_string().contains("defined more than once"));
    }

    #[test]
    fn symlinked_identity_database_is_rejected() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("etc")).unwrap();
        std::os::unix::fs::symlink("/etc/passwd", root.path().join("etc/passwd")).unwrap();
        fs::write(root.path().join("etc/group"), "svc-group:x:819:\n").unwrap();

        let error =
            resolve_payload_nodes(root.path(), [named_node("svc", "svc-group")]).unwrap_err();

        assert!(error.to_string().contains("must not be a symlink"));
    }
}
