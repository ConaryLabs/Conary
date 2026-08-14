// conary-core/src/generation/root_manifest/delta.rs

//! Normalized changed-path authority for selected-root transactions.

#[cfg(test)]
use super::{
    CapturedSelectedRoot, GENERATION_ROOT_MANIFEST_VERSION, GenerationRootManifest,
    MutableStateManifest,
};
use super::{GenerationRootEntry, RootPathDomain, classify_root_path, validate_root_path};
use crate::payload::{PayloadNodeKind, ResolvedPayloadNode};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const SELECTED_ROOT_MANIFEST_DELTA_VERSION: u32 = 1;

/// One normalized transaction-owned change to prior selected-root authority.
///
/// Overlayfs artifacts are decoded before this boundary. Whiteouts become
/// recursive removals, opaque directories remove prior descendants, and every
/// surviving changed node becomes one complete upsert. Unchanged content can
/// therefore retain its prior CAS authority without rereading its bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectedRootManifestDelta {
    pub version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root: Option<ResolvedPayloadNode>,
    pub removals: Vec<String>,
    pub opaque_directories: Vec<String>,
    pub upserts: Vec<GenerationRootEntry>,
}

impl SelectedRootManifestDelta {
    /// Validate canonical changed-path authority without consulting a base.
    pub fn validate(&self) -> crate::Result<()> {
        if self.version != SELECTED_ROOT_MANIFEST_DELTA_VERSION {
            return Err(crate::Error::InvalidPath(format!(
                "selected-root manifest delta has unsupported version {}; expected {}",
                self.version, SELECTED_ROOT_MANIFEST_DELTA_VERSION
            )));
        }
        if let Some(root) = &self.root {
            root.validate()
                .map_err(|error| crate::Error::InvalidPath(error.to_string()))?;
            if !matches!(root.source.kind, PayloadNodeKind::Directory) {
                return Err(crate::Error::InvalidPath(
                    "selected-root delta root metadata must describe a directory".to_string(),
                ));
            }
        }
        validate_changed_paths(&self.removals, "removals")?;
        validate_changed_paths(&self.opaque_directories, "opaque directories")?;
        reject_redundant_removals(&self.removals)?;

        let mut previous: Option<&str> = None;
        let mut upserts = BTreeMap::new();
        for entry in &self.upserts {
            entry.validate()?;
            require_publishable_domain(&entry.path)?;
            if previous.is_some_and(|path| path >= entry.path.as_str()) {
                return Err(crate::Error::InvalidPath(
                    "selected-root delta upserts must be unique and sorted by path".to_string(),
                ));
            }
            previous = Some(&entry.path);
            upserts.insert(entry.path.as_str(), entry);
        }
        for path in &self.opaque_directories {
            let Some(entry) = upserts.get(path.as_str()) else {
                return Err(crate::Error::InvalidPath(format!(
                    "opaque directory {path} has no complete directory upsert"
                )));
            };
            if !matches!(entry.node.source.kind, PayloadNodeKind::Directory) {
                return Err(crate::Error::InvalidPath(format!(
                    "opaque directory {path} upsert is not a directory"
                )));
            }
        }
        Ok(())
    }

    /// Materialize a complete projection for unit-level contract proof.
    /// Production mutation authority is [`super::SelectedRootSnapshot`].
    #[cfg(test)]
    pub(super) fn apply(
        &self,
        prior: &CapturedSelectedRoot,
    ) -> crate::Result<CapturedSelectedRoot> {
        self.validate()?;
        prior.generation.validate()?;
        prior.state.validate()?;

        let mut entries = prior
            .generation
            .entries
            .iter()
            .chain(&prior.state.entries)
            .cloned()
            .map(|entry| (entry.path.clone(), entry))
            .collect::<BTreeMap<_, _>>();

        for path in &self.removals {
            let removed = remove_path_and_descendants(&mut entries, path, true);
            if !removed {
                return Err(crate::Error::InvalidPath(format!(
                    "selected-root delta removal {path} does not name prior authority"
                )));
            }
        }
        for path in &self.opaque_directories {
            let Some(prior_entry) = entries.get(path) else {
                return Err(crate::Error::InvalidPath(format!(
                    "opaque directory {path} does not name prior authority"
                )));
            };
            if !matches!(prior_entry.node.source.kind, PayloadNodeKind::Directory) {
                return Err(crate::Error::InvalidPath(format!(
                    "opaque directory {path} does not replace a prior directory"
                )));
            }
            remove_path_and_descendants(&mut entries, path, false);
        }
        for entry in &self.upserts {
            entries.insert(entry.path.clone(), entry.clone());
        }

        let mut immutable = Vec::new();
        let mut state = Vec::new();
        for entry in entries.into_values() {
            match classify_root_path(&entry.path)? {
                RootPathDomain::Immutable => immutable.push(entry),
                RootPathDomain::ConfigState | RootPathDomain::MutableState => state.push(entry),
                RootPathDomain::EphemeralMountOrUser => {
                    return Err(crate::Error::InvalidPath(format!(
                        "selected-root delta produced ephemeral path {}",
                        entry.path
                    )));
                }
            }
        }
        let captured = CapturedSelectedRoot {
            generation: GenerationRootManifest {
                version: GENERATION_ROOT_MANIFEST_VERSION,
                root: self
                    .root
                    .clone()
                    .unwrap_or_else(|| prior.generation.root.clone()),
                entries: immutable,
            },
            state: MutableStateManifest {
                version: GENERATION_ROOT_MANIFEST_VERSION,
                entries: state,
            },
        };
        captured.generation.validate()?;
        captured.state.validate()?;
        Ok(captured)
    }
}

fn validate_changed_paths(paths: &[String], label: &str) -> crate::Result<()> {
    let mut previous: Option<&str> = None;
    for path in paths {
        validate_root_path(path)?;
        require_publishable_domain(path)?;
        if previous.is_some_and(|prior| prior >= path.as_str()) {
            return Err(crate::Error::InvalidPath(format!(
                "selected-root delta {label} must be unique and sorted by path"
            )));
        }
        previous = Some(path);
    }
    Ok(())
}

fn require_publishable_domain(path: &str) -> crate::Result<()> {
    if classify_root_path(path)? == RootPathDomain::EphemeralMountOrUser {
        return Err(crate::Error::InvalidPath(format!(
            "selected-root delta cannot own ephemeral path {path}"
        )));
    }
    Ok(())
}

fn reject_redundant_removals(removals: &[String]) -> crate::Result<()> {
    let mut ancestors = BTreeSet::<&str>::new();
    for path in removals {
        if ancestors
            .iter()
            .any(|ancestor| is_descendant(path, ancestor))
        {
            return Err(crate::Error::InvalidPath(format!(
                "selected-root delta removal {path} is redundant with an ancestor removal"
            )));
        }
        ancestors.insert(path.as_str());
    }
    Ok(())
}

#[cfg(test)]
fn remove_path_and_descendants(
    entries: &mut BTreeMap<String, GenerationRootEntry>,
    path: &str,
    include_path: bool,
) -> bool {
    let removed = entries
        .keys()
        .filter(|candidate| {
            (include_path && candidate.as_str() == path) || is_descendant(candidate, path)
        })
        .cloned()
        .collect::<Vec<_>>();
    for candidate in &removed {
        entries.remove(candidate);
    }
    !removed.is_empty()
}

fn is_descendant(candidate: &str, ancestor: &str) -> bool {
    candidate
        .strip_prefix(ancestor)
        .is_some_and(|suffix| suffix.starts_with('/'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::payload::{PayloadContentAuthority, PayloadIdentity, PayloadNode, PayloadNodeKind};

    #[test]
    fn applies_changed_paths_across_publication_domains_without_rereading_content() {
        let prior_usr_content = content('a', 3);
        let prior = captured(vec![
            directory_entry("/etc"),
            regular_entry("/etc/keep", content('b', 4)),
            regular_entry("/etc/remove", content('c', 5)),
            directory_entry("/usr"),
            directory_entry("/usr/bin"),
            regular_entry("/usr/bin/keep", prior_usr_content.clone()),
            regular_entry("/usr/bin/replace", content('d', 6)),
            directory_entry("/var"),
            regular_entry("/var/state", content('e', 7)),
        ]);
        let delta = SelectedRootManifestDelta {
            version: SELECTED_ROOT_MANIFEST_DELTA_VERSION,
            root: None,
            removals: vec!["/etc/remove".to_string()],
            opaque_directories: Vec::new(),
            upserts: vec![
                regular_entry("/etc/new", content('f', 8)),
                regular_entry("/usr/bin/replace", content('0', 9)),
            ],
        };

        let applied = delta.apply(&prior).unwrap();
        assert!(entry(&applied, "/etc/remove").is_none());
        assert_eq!(
            entry(&applied, "/usr/bin/keep").and_then(|entry| entry.content.as_ref()),
            Some(&prior_usr_content)
        );
        assert_eq!(
            entry(&applied, "/usr/bin/replace")
                .and_then(|entry| entry.content.as_ref())
                .map(|content| content.size),
            Some(9)
        );
        assert!(
            applied
                .state
                .entries
                .iter()
                .any(|entry| entry.path == "/etc/new")
        );
    }

    #[test]
    fn opaque_directory_discards_only_prior_descendants() {
        let prior = captured(vec![
            directory_entry("/opt"),
            directory_entry("/opt/cache"),
            regular_entry("/opt/cache/lower", content('a', 1)),
            directory_entry("/usr"),
        ]);
        let delta = SelectedRootManifestDelta {
            version: SELECTED_ROOT_MANIFEST_DELTA_VERSION,
            root: None,
            removals: Vec::new(),
            opaque_directories: vec!["/opt/cache".to_string()],
            upserts: vec![
                directory_entry("/opt/cache"),
                regular_entry("/opt/cache/upper", content('b', 2)),
            ],
        };

        let applied = delta.apply(&prior).unwrap();
        assert!(entry(&applied, "/opt/cache/lower").is_none());
        assert!(entry(&applied, "/opt/cache/upper").is_some());
        assert!(entry(&applied, "/usr").is_some());
    }

    #[test]
    fn directory_removal_and_replacement_is_recursive() {
        let prior = captured(vec![
            directory_entry("/opt"),
            directory_entry("/opt/tool"),
            regular_entry("/opt/tool/old", content('a', 1)),
        ]);
        let delta = SelectedRootManifestDelta {
            version: SELECTED_ROOT_MANIFEST_DELTA_VERSION,
            root: None,
            removals: vec!["/opt/tool".to_string()],
            opaque_directories: Vec::new(),
            upserts: vec![regular_entry("/opt/tool", content('b', 2))],
        };

        let applied = delta.apply(&prior).unwrap();
        assert!(entry(&applied, "/opt/tool/old").is_none());
        assert!(matches!(
            entry(&applied, "/opt/tool").unwrap().node.source.kind,
            PayloadNodeKind::Regular { .. }
        ));
    }

    #[test]
    fn malformed_or_unrepresentable_changes_fail_closed() {
        let prior = captured(vec![directory_entry("/usr")]);
        for delta in [
            SelectedRootManifestDelta {
                version: SELECTED_ROOT_MANIFEST_DELTA_VERSION + 1,
                root: None,
                removals: Vec::new(),
                opaque_directories: Vec::new(),
                upserts: Vec::new(),
            },
            SelectedRootManifestDelta {
                version: SELECTED_ROOT_MANIFEST_DELTA_VERSION,
                root: None,
                removals: vec!["/run/runtime".to_string()],
                opaque_directories: Vec::new(),
                upserts: Vec::new(),
            },
            SelectedRootManifestDelta {
                version: SELECTED_ROOT_MANIFEST_DELTA_VERSION,
                root: None,
                removals: vec!["/usr/missing".to_string()],
                opaque_directories: Vec::new(),
                upserts: Vec::new(),
            },
            SelectedRootManifestDelta {
                version: SELECTED_ROOT_MANIFEST_DELTA_VERSION,
                root: None,
                removals: Vec::new(),
                opaque_directories: vec!["/usr".to_string()],
                upserts: Vec::new(),
            },
        ] {
            assert!(delta.apply(&prior).is_err());
        }
    }

    #[test]
    fn final_tree_validation_rejects_missing_parents_and_broken_hardlinks() {
        let prior = captured(vec![directory_entry("/usr")]);
        let missing_parent = SelectedRootManifestDelta {
            version: SELECTED_ROOT_MANIFEST_DELTA_VERSION,
            root: None,
            removals: Vec::new(),
            opaque_directories: Vec::new(),
            upserts: vec![regular_entry("/usr/bin/tool", content('a', 1))],
        };
        assert!(missing_parent.apply(&prior).is_err());

        let mut hardlink = regular_entry("/usr/link", content('b', 2));
        hardlink.content = None;
        hardlink.node.source.kind = PayloadNodeKind::Hardlink {
            target: "/usr/missing".to_string(),
            identity: "delta:1".to_string(),
        };
        let broken_hardlink = SelectedRootManifestDelta {
            version: SELECTED_ROOT_MANIFEST_DELTA_VERSION,
            root: None,
            removals: Vec::new(),
            opaque_directories: Vec::new(),
            upserts: vec![hardlink],
        };
        assert!(broken_hardlink.apply(&prior).is_err());
    }

    #[test]
    fn changed_hardlink_can_target_unchanged_primary_authority() {
        let identity = "prior-generation:7";
        let mut primary = regular_entry("/usr/lib/tool", content('a', 4));
        primary.node.source.kind = PayloadNodeKind::Regular {
            hardlink_identity: Some(identity.to_string()),
        };
        let prior_alias = hardlink_entry("/usr/lib/tool.old", &primary, identity);
        let prior = captured(vec![
            directory_entry("/usr"),
            directory_entry("/usr/lib"),
            primary.clone(),
            prior_alias,
        ]);
        let delta = SelectedRootManifestDelta {
            version: SELECTED_ROOT_MANIFEST_DELTA_VERSION,
            root: None,
            removals: Vec::new(),
            opaque_directories: Vec::new(),
            upserts: vec![hardlink_entry("/usr/lib/tool.new", &primary, identity)],
        };

        let applied = delta.apply(&prior).unwrap();
        assert_eq!(
            entry(&applied, "/usr/lib/tool.new").map(|entry| &entry.node.source.kind),
            Some(&PayloadNodeKind::Hardlink {
                target: "/usr/lib/tool".to_string(),
                identity: identity.to_string(),
            })
        );
        assert_eq!(
            entry(&applied, "/usr/lib/tool").and_then(|entry| entry.content.as_ref()),
            primary.content.as_ref()
        );
    }

    fn captured(entries: Vec<GenerationRootEntry>) -> CapturedSelectedRoot {
        let (generation, state): (Vec<_>, Vec<_>) = entries.into_iter().partition(|entry| {
            classify_root_path(&entry.path).unwrap() == RootPathDomain::Immutable
        });
        let captured = CapturedSelectedRoot {
            generation: GenerationRootManifest {
                version: GENERATION_ROOT_MANIFEST_VERSION,
                root: directory_node(),
                entries: sorted(generation),
            },
            state: MutableStateManifest {
                version: GENERATION_ROOT_MANIFEST_VERSION,
                entries: sorted(state),
            },
        };
        captured.generation.validate().unwrap();
        captured.state.validate().unwrap();
        captured
    }

    fn sorted(mut entries: Vec<GenerationRootEntry>) -> Vec<GenerationRootEntry> {
        entries.sort_by(|left, right| left.path.cmp(&right.path));
        entries
    }

    fn entry<'a>(
        captured: &'a CapturedSelectedRoot,
        path: &str,
    ) -> Option<&'a GenerationRootEntry> {
        captured
            .generation
            .entries
            .iter()
            .chain(&captured.state.entries)
            .find(|entry| entry.path == path)
    }

    fn directory_entry(path: &str) -> GenerationRootEntry {
        GenerationRootEntry {
            path: path.to_string(),
            node: directory_node(),
            content: None,
        }
    }

    fn regular_entry(path: &str, content: PayloadContentAuthority) -> GenerationRootEntry {
        GenerationRootEntry {
            path: path.to_string(),
            node: regular_node(),
            content: Some(content),
        }
    }

    fn hardlink_entry(
        path: &str,
        primary: &GenerationRootEntry,
        identity: &str,
    ) -> GenerationRootEntry {
        let mut node = primary.node.clone();
        node.source.kind = PayloadNodeKind::Hardlink {
            target: primary.path.clone(),
            identity: identity.to_string(),
        };
        GenerationRootEntry {
            path: path.to_string(),
            node,
            content: None,
        }
    }

    fn directory_node() -> ResolvedPayloadNode {
        let mut node = source_node();
        node.kind = PayloadNodeKind::Directory;
        node.mode = libc::S_IFDIR | 0o755;
        ResolvedPayloadNode::from_numeric_source(node).unwrap()
    }

    fn regular_node() -> ResolvedPayloadNode {
        ResolvedPayloadNode::from_numeric_source(source_node()).unwrap()
    }

    fn source_node() -> PayloadNode {
        let mut node = PayloadNode::regular(0o644);
        node.user = PayloadIdentity::Numeric { id: 0 };
        node.group = PayloadIdentity::Numeric { id: 0 };
        node
    }

    fn content(byte: char, size: u64) -> PayloadContentAuthority {
        PayloadContentAuthority {
            sha256: byte.to_string().repeat(64),
            size,
        }
    }
}
