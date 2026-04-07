// crates/conary-core/src/operations.rs

use serde::{Deserialize, Serialize};

/// Canonical shared vocabulary for daemon and CLI operation kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationKind {
    Install,
    Remove,
    Update,
    DryRun,
    Rollback,
    Verify,
    GarbageCollect,
    Enhance,
}

impl OperationKind {
    /// Return the snake_case string representation used for serde and persistence.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Install => "install",
            Self::Remove => "remove",
            Self::Update => "update",
            Self::DryRun => "dry_run",
            Self::Rollback => "rollback",
            Self::Verify => "verify",
            Self::GarbageCollect => "garbage_collect",
            Self::Enhance => "enhance",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::OperationKind;

    #[test]
    fn test_operation_kind_as_str_matches_canonical_storage_strings() {
        let expected_pairs = [
            (OperationKind::Install, "install"),
            (OperationKind::Remove, "remove"),
            (OperationKind::Update, "update"),
            (OperationKind::DryRun, "dry_run"),
            (OperationKind::Rollback, "rollback"),
            (OperationKind::Verify, "verify"),
            (OperationKind::GarbageCollect, "garbage_collect"),
            (OperationKind::Enhance, "enhance"),
        ];

        for (kind, expected) in expected_pairs {
            assert_eq!(kind.as_str(), expected);
        }
    }

    #[test]
    fn test_operation_kind_serde_uses_canonical_storage_strings() {
        let expected_pairs = [
            (OperationKind::Install, "\"install\""),
            (OperationKind::Remove, "\"remove\""),
            (OperationKind::Update, "\"update\""),
            (OperationKind::DryRun, "\"dry_run\""),
            (OperationKind::Rollback, "\"rollback\""),
            (OperationKind::Verify, "\"verify\""),
            (OperationKind::GarbageCollect, "\"garbage_collect\""),
            (OperationKind::Enhance, "\"enhance\""),
        ];

        for (kind, expected_json) in expected_pairs {
            assert_eq!(serde_json::to_string(&kind).unwrap(), expected_json);
            assert_eq!(
                serde_json::from_str::<OperationKind>(expected_json).unwrap(),
                kind
            );
        }
    }
}
