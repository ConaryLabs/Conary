# Remi Non-Public Test Serving Design

**Status:** Approved design
**Date:** 2026-07-08
**Related issue:** GitHub issue #35 and the blocked/review-required conversion classes that currently prevent package-level testing

## Goal

Remi should let maintainers and test harnesses fetch converted CCS artifacts
that are blocked or private-review so the artifacts can be inspected and
tested. That test access must not change the public publication contract:
normal public package, index, sparse, OCI, and chunk routes continue to expose
only public-ready conversions or active native publications.

This is a testing lane, not a public serving promotion. A converted row whose
scriptlet summary says `blocked`, `private-review`, or `local-only` keeps that
status while the test lane serves the CCS file with explicit warning metadata.

## Current Repo Facts

- `apps/remi/src/server/publication.rs` classifies converted packages as
  public-ready only when scriptlet publication metadata is valid and
  `publication_status == "public"`.
- `apps/remi/src/server/handlers/packages.rs` returns refusal JSON for
  blocked or review-required converted package manifests and downloads.
- `apps/remi/src/server/handlers/chunks.rs` calls the publication gate before
  serving chunks. Hashes referenced only by non-public converted rows return
  `404`.
- Public index, sparse, detail, and OCI handlers filter converted rows through
  `ConvertedPackage::is_scriptlet_public_ready()`.
- Admin routes already have authenticated or loopback-only surfaces for
  conversion, package upload, review artifacts, and scriptlet evidence.

## Safety Invariants

- Do not rewrite `publication_status` to `public`.
- Do not make blocked, private-review, local-only, stale, or malformed rows
  appear in public metadata, search, sparse indexes, OCI tags, TUF targets, or
  public chunk routes.
- Do not expose raw `review_artifact_path` values through test-serving
  metadata.
- Do not serve malformed scriptlet publication metadata through the test lane.
  Malformed rows need reconversion or repair before artifact testing.
- Do not record non-public test downloads as normal public download analytics.
- Keep the feature disabled by default in runtime config.

## Decision

Add an explicit Remi admin/test lane for non-public converted CCS artifacts.

The first slice adds:

- a default-off runtime policy, `non_public_test_serving.enabled`;
- an admin-only metadata endpoint for non-public test artifacts;
- an admin-only whole-file CCS download endpoint for those artifacts;
- tests proving public routes still refuse or hide non-public rows;
- docs that describe the lane as maintainer/test access, not public
  publication.

The route names should be intentionally loud:

- `GET /v1/admin/packages/{distro}/{package}/test-manifest`
- `GET /v1/admin/packages/{distro}/{package}/test-download`

Both require `version`. `arch` is optional and `architecture` is accepted as a
compatibility alias, matching normal package-route query vocabulary. An
ambiguous no-arch lookup must fail closed with an explicit architecture-required
error. The first slice does not add public or unauthenticated test routes.

## Behavior

### Disabled Policy

When `non_public_test_serving.enabled` is false, both test endpoints return:

- HTTP `403`
- code `NON_PUBLIC_TEST_SERVING_DISABLED`
- a message saying non-public test serving is disabled

The default config keeps the flag false.

### Eligible Rows

A row is eligible when all of these are true:

- it matches the requested distro, package, version, and optional architecture;
- it is current for the conversion version;
- it has valid scriptlet publication metadata;
- it has a readable `ccs_path`;
- `classify_converted_package()` returns `ReviewRequired` or `Blocked`.

The response includes the same sanitized `PublicationGateReport` used by
public refusal responses. `review_artifact_available` may be true, but the
private path is never serialized.

### Ineligible Rows

- Public-ready rows return `409 ALREADY_PUBLIC`; callers should use the normal
  public package route.
- Stale conversion rows return `409 STALE_CONVERSION`.
- Malformed scriptlet summary rows return `409 MALFORMED_SCRIPTLET_SUMMARY`.
- Ambiguous architecture matches return `409 AMBIGUOUS_ARCHITECTURE`.
- Missing rows or missing files return `404 NOT_FOUND`.

### Download Semantics

`test-download` streams the stored CCS file directly from `ccs_path` with:

- `Content-Type: application/octet-stream`;
- `Content-Disposition` using a sanitized file name;
- `Cache-Control: no-store`;
- no normal public analytics recording.

The first slice does not loosen `/v1/chunks/{hash}`. Test clients that need the
artifact should use the whole-file admin download. If a future test harness
needs chunk-by-chunk install behavior, add a separate authenticated test chunk
route rather than changing the public chunk gate.

## Architecture

### Config

Add a `NonPublicTestServingSection` to `apps/remi/src/server/config.rs`:

```toml
[non_public_test_serving]
enabled = false
```

Map it into `ServerConfig` so handlers can read the policy from runtime state.
The section exists only to govern test-serving access; it does not affect the
conversion registry or publication decisions.

### Admin Handler

Implement lookup and response code under
`apps/remi/src/server/handlers/admin/non_public_test_serving.rs`, re-exported
from the admin handler module. This keeps non-public artifact access in the
admin package-serving boundary without adding more behavior to the existing
package upload/review-artifact handler.

The lookup helper should return a typed enum so route handlers can distinguish
disabled policy, stale rows, malformed rows, public rows, blocked rows,
review-required rows, ambiguous architecture, and missing files without parsing
strings.

### Public Gate Preservation

The implementation must not change:

- `classify_converted_package()`;
- `ConvertedPackage::is_scriptlet_public_ready()`;
- `local_chunk_servable_by_public_gate()`;
- public package/index/search/sparse/OCI filtering.

Tests should pin that public refusal behavior remains unchanged while the admin
test lane is enabled.

## Tests

Required focused tests:

- config defaults keep non-public test serving disabled;
- config TOML can enable non-public test serving;
- public converted package download still refuses blocked rows;
- public chunk route still hides hashes referenced only by non-public rows;
- admin test manifest returns `403` while the policy is disabled;
- admin test manifest returns sanitized metadata for a blocked row when enabled;
- admin test download streams blocked CCS bytes when enabled;
- admin test manifest rejects public-ready rows with `ALREADY_PUBLIC`;
- admin test manifest rejects malformed metadata with
  `MALFORMED_SCRIPTLET_SUMMARY`.

Focused proof:

```bash
cargo test -p remi non_public_test_serving
cargo test -p remi publication
```

Interaction proof when the docs and routes are both touched:

```bash
cargo test -p remi
bash scripts/check-doc-truth.sh
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
LC_ALL=C bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -
bash scripts/check-coherency-ledger.sh docs/superpowers/feature-coherency-ledger.tsv
git diff --check
```

## Non-Goals

- No public serving of blocked or private-review packages.
- No adapter promotion for kernel, initramfs, bootloader, PAM, broad sysctl
  forms, unsupported setcap forms, setgid/broad chmod, network, or
  package-manager-recursion classes in this slice. Narrow
  `sysctl -w <key>=<value>` promotion is allowed only when it projects into
  native `hooks.sysctl`; narrow setuid chmod promotion is allowed only for
  payload executables projected into native file mode plus
  `policy.allow_setuid_paths`; narrow known Linux `setcap cap_*=+ep`
  promotion is allowed only for payload executables projected into
  `[[file_capabilities]]`; narrow AppArmor profile reload promotion is allowed
  only for payload-backed `apparmor_parser -r|--replace
  /etc/apparmor.d/<profile>` projected into generic security-policy intent.
- No raw legacy scriptlet replay.
- No public test URLs.
- No changes to external release publication, native CCS validation, or TUF
  target generation.
- No chunk-gate bypass on `/v1/chunks/{hash}`.

## Follow-On Work

After this lane exists, work through blocked/review classes with real package
evidence:

1. Use issue #35-style kernel packages to exercise the lane and collect
   kernel-module, initramfs, and bootloader evidence without public promotion.
2. Promote additional SELinux/AppArmor policy forms only when adapters fully
   replace raw host mutation.
3. Continue native authority design for PAM, broad sysctl forms, unsupported
   file-capability forms, AppArmor mode changes, setgid/broad chmod classes,
   and any setuid form that cannot be represented as payload file mode plus
   explicit build-policy allowlist.
4. Treat network and package-manager-recursion classes as corpus evidence for
   dependency extraction or offline artifact modeling, not public raw replay.
