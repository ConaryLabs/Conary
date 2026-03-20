---
name: derivation_code_quality
description: Code quality audit of conary-core/src/derivation/ module (13 files, ~5600 lines) -- 5 P1 findings including symlink data loss, hash collision risks, canonical_string duplication
type: project
---

Derivation module code quality audit (2026-03-20): 16 findings (5 P1, 9 P2, 2 P3)

Critical (P1):
- compose_file_entries() drops all symlinks from OutputManifest -- EROFS images will be broken for packages with shared lib symlinks
- compute_output_hash() excludes file size and mode, so permission-only changes don't invalidate the cache
- canonical_string() copy-pasted between DerivationId and SourceDerivationId (id.rs lines 86-111 and 204-227)
- BuildProfile::canonical_string() has zero input validation for colon/newline injection (unlike DerivationId which validates)
- glibc appears in both TOOLCHAIN_NAMED and FOUNDATION_PACKAGES arrays (stages.rs) -- works by accident due to check ordering

Notable (P2):
- SeedSource serialized via Debug format (`format!("{:?}", ...)`) in pipeline.rs -- fragile
- erofs_image_hash() reads full file into memory (OOM risk on large images)
- sha256_of_path in commands/derivation.rs duplicates erofs_image_hash in compose.rs
- make_recipe and test_cas test helpers duplicated across multiple test modules
- SubstituterSection.trust is a raw String where an enum should be used
- DerivationExecutor.cas_dir is misleadingly named (used as build work dir, not CAS root)

**Why:** The symlink loss is a ship-blocker for system image composition. The hash collision risks (P1 findings 3 and 5) threaten cache correctness. The canonical_string duplication is the kind of copy-paste that drifts over time.

**How to apply:** When reviewing compose_file_entries changes or EROFS composition, verify symlinks are handled. When reviewing canonical hash formats, verify input validation matches id.rs's validate_inputs pattern. Flag direct sha2 imports per derivation_code_reuse.md.
