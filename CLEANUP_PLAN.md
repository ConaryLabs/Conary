# Codebase Cleanup Plan

## Priority 1: Quick Wins (Low Risk, High Value)

### 1.1 Extract Shared Archive Utilities
**Files:** `src/packages/rpm.rs`, `src/packages/deb.rs`, `src/packages/arch.rs`
**New file:** `src/packages/archive_utils.rs`

Deduplicate these identical/near-identical patterns:

| Utility | Current Locations | Lines Saved |
|---------|-------------------|-------------|
| `compute_sha256()` | DEB:531-534, Arch:497-500 | ~8 |
| `normalize_path()` | RPM:318-323, DEB:537, Arch:503 | ~12 |
| `check_file_size()` | RPM:309-315, DEB:513-519, Arch:480-486 | ~18 |
| `is_regular_file()` | RPM:304, DEB:505, Arch:472 | ~6 |

**Implementation:**
```rust
// src/packages/archive_utils.rs

use sha2::{Digest, Sha256};
use tracing::warn;

pub const MAX_EXTRACTION_FILE_SIZE: u64 = 100 * 1024 * 1024; // 100 MB

/// Normalize archive entry path to absolute form
pub fn normalize_path(path: &str) -> String {
    let trimmed = path.trim_start_matches("./").trim_start_matches('.');
    if trimmed.starts_with('/') {
        trimmed.to_string()
    } else {
        format!("/{}", trimmed)
    }
}

/// Compute SHA256 hash of content
pub fn compute_sha256(content: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content);
    format!("{:x}", hasher.finalize())
}

/// Check if file size exceeds limit, warn if so
pub fn check_file_size(path: &str, size: u64) -> bool {
    if size > MAX_EXTRACTION_FILE_SIZE {
        warn!("Skipping oversized file: {} ({} bytes)", path, size);
        false
    } else {
        true
    }
}
```

**Effort:** 1-2 hours
**Risk:** Low (isolated change, easy to test)

---

## Priority 2: Legacy Code Removal (Medium Risk)

### 2.1 Remove `install_package_from_file` from `src/commands/mod.rs`

**Status:** This 241-line function is ALREADY replaced by the modular implementation in:
- `src/commands/install/mod.rs` - Main orchestration (cmd_install)
- `src/commands/install/prepare.rs` - Package parsing, upgrade checks
- `src/commands/install/execute.rs` - File deployment
- `src/transaction/mod.rs` - TransactionEngine for atomic ops

**Callers to migrate:**
1. `src/commands/dependencies.rs` - indirect usage
2. `src/commands/update.rs` - direct usage

**Effort:** 4-6 hours
**Risk:** Medium (need to verify all code paths)

---

## Priority 3: Future Refactoring (Lower Priority)

### 3.1 Unify Tar Archive Iteration

DEB and Arch both use `tar::Archive` with similar iteration patterns.
Could create a generic `TarExtractor` that handles:
- Entry iteration with error handling
- Automatic path normalization
- Size checking
- SHA256 computation

**Effort:** 4-6 hours
**Risk:** Medium

### 3.2 Standardize Directory Filtering

RPM uses mode bits (`mode & 0o170000`), DEB/Arch use tar header API.
Could add `is_directory()` helper that abstracts both approaches.

**Effort:** 1-2 hours
**Risk:** Low

---

## Not Recommended

### Format Detection Centralization
**Reason:** Already well-centralized in:
- `src/compression/mod.rs` - CompressionFormat::from_magic_bytes()
- `src/packages/registry.rs` - detect_format()

No duplication issue exists here. Gemini's assessment was incorrect.

---

## Summary

| Task | Priority | Effort | Risk | Lines Saved |
|------|----------|--------|------|-------------|
| Extract archive_utils.rs | P1 | 2h | Low | ~44 |
| Remove legacy install fn | P2 | 6h | Medium | ~241 |
| Unify tar iteration | P3 | 6h | Medium | ~80 |
| Directory filtering | P3 | 2h | Low | ~10 |

**Total potential reduction:** ~375 lines of duplicated code

---

## Recommended Order

1. [x] Create `src/packages/archive_utils.rs` with shared utilities
2. [x] Update RPM/DEB/Arch to use shared utilities
3. [ ] Audit callers of `install_package_from_file`
4. [ ] Migrate remaining callers to modern install flow
5. [ ] Remove legacy function
