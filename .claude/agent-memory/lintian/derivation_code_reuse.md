---
name: derivation_code_reuse
description: Code reuse audit of conary-core/src/derivation/ module (13 files, ~5600 lines) -- SHA-256 bypass, OOM risk, test helper duplication
type: project
---

Derivation module code reuse audit (2026-03-20):

- 10 call sites in 5 files (id.rs, output.rs, compose.rs, recipe_hash.rs, profile.rs) import sha2::Sha256 directly instead of using crate::hash utilities
- erofs_image_hash() in compose.rs reads entire file into memory (OOM risk on large EROFS images) -- should use hash::hash_reader() streaming
- make_recipe() test helper duplicated identically between stages.rs and pipeline.rs -- should extract to shared test module
- expand_variables() in recipe_hash.rs near-duplicates Recipe::substitute() but adds BTreeMap sorting for hash determinism -- intentional divergence, should document
- Topological sort in stages.rs vs recipe::graph::RecipeGraph -- intentionally different (BTreeMap determinism + stage scoping)
- TOML parsing patterns are idiomatic, no shared utility needed
- No existing tree-walk utility reusable for capture.rs DESTDIR walk

**Why:** The crate::hash module was built specifically to centralize hashing. Bypassing it fragments the codebase and risks inconsistent formatting or algorithm selection. The OOM risk in erofs_image_hash is a production concern for large bootstrap images.

**How to apply:** When reviewing future modules that compute SHA-256, flag direct sha2 imports. The crate::hash::sha256() and hash::Hasher APIs should be the standard path.
