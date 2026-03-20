---
name: bootstrap_v2_spec_review
description: Design spec review findings for CAS-layered bootstrap v2 -- revision 2 APPROVED (0 HIGH), 6 MEDIUM (recipe format examples wrong, stage naming inconsistent), 5 LOW remaining
type: project
---

Review of `docs/superpowers/specs/2026-03-19-bootstrap-v2-cas-layered-design.md` (revision 2).

Revision 1 had 5 HIGH findings. Revision 2 fixes all:
- Derivation ID now has precise canonical byte string format with CONARY-DERIVATION-V1 prefix
- Section 2.7 clarifies composefs-only build env (no host mounts)
- Overview explicitly distinguishes new code vs reused primitives
- Edge cases section covers circular deps, network builds, multi-output

Remaining issues (revision 2):
- Section 4.2 recipe example still uses `url`/`sha256`/`[phases]` instead of `archive`/`checksum`/`[build]`
- Section 4.3 [cross] example still has non-existent `host_tools`/`configure_flags` fields
- Diverse-verification (Section 5.2) contradicts derivation ID definition: seed_id is in hash, so "same derivation except seed_id" is impossible
- build_script_hash scope doesn't cover `environment`, `check`, `workdir`, `script_file` fields
- Stage naming inconsistent: spec stages (toolchain/foundation) vs BuildStage enum (Stage0/Stage1) vs [cross] example (stage0)
- `network = "fetch"` proposed in edge cases but not in new metadata section

**Why:** Spec is the implementation blueprint. Recipe format mismatches are copy-paste confusing but not blocking. The diverse-verification hash contradiction is a real design gap.

**How to apply:** When implementing, use actual format.rs field names, not spec examples. The diverse-verification comparison hash needs design resolution before Phase 6 implementation.
