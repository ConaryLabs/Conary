// src/db/paths.rs
//! Centralized path derivation for Conary directories

use std::path::{Path, PathBuf};

/// Get the directory containing the database
pub fn db_dir(db_path: &str) -> PathBuf {
    Path::new(db_path)
        .parent()
        .unwrap_or(Path::new("/var/lib/conary"))
        .to_path_buf()
}

/// Get the objects (CAS) directory
pub fn objects_dir(db_path: &str) -> PathBuf {
    db_dir(db_path).join("objects")
}

/// Get the keyring directory for GPG keys
pub fn keyring_dir(db_path: &str) -> PathBuf {
    std::env::var("CONARY_DB_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| db_dir(db_path))
        .join("keys")
}

/// Get the temporary directory for operations
pub fn temp_dir(db_path: &str) -> PathBuf {
    db_dir(db_path).join("tmp")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_db_dir() {
        assert_eq!(
            db_dir("/var/lib/conary/conary.db"),
            PathBuf::from("/var/lib/conary")
        );
    }

    #[test]
    fn test_objects_dir() {
        assert_eq!(
            objects_dir("/var/lib/conary/conary.db"),
            PathBuf::from("/var/lib/conary/objects")
        );
    }

    #[test]
    fn test_keyring_dir() {
        assert_eq!(
            keyring_dir("/var/lib/conary/conary.db"),
            PathBuf::from("/var/lib/conary/keys")
        );
    }
}
