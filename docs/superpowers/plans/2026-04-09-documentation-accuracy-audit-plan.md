# Documentation Accuracy Audit Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Complete a release-blocking, evidence-backed audit of every tracked documentation-like file in the repository, archive or delete stale planning material under the approved policy, and leave the repo with an objective ledger proving the retained docs were verified or explicitly framed as historical/WIP.

**Architecture:** Add one shell inventory helper and one ledger gate so the audit can enumerate the tracked doc surface from `git ls-files`, classify files into families, and mechanically prove coverage. Execute the audit in three passes: scaffolding and planning-material triage, active release-facing doc repair, and historical/consistency cleanup. Keep all changes doc-only except for the two audit helper scripts and the minimum `.gitignore` adjustment needed to allow tracked plan/spec archive moves.

**Tech Stack:** Markdown, TSV, Bash, `git ls-files`, `git mv`, `git rm`, `rg`, Cargo CLI help output, checked-in scripts/config/workflows, and repo-local verification commands.

**Commit Convention:** Each commit in this plan should reference `docs/superpowers/plans/2026-04-09-documentation-accuracy-audit-plan.md` in the commit body.

---

## Scope Guard

- Audit only tracked documentation-like files. Do not pull ignored local trees such as `docs/plans/archive/` or `docs/superpowers/reviews/` into scope unless they are intentionally promoted into version control.
- Do not change runtime behavior to make docs read better. If the audit finds a real product gap, document it honestly and record the mismatch in the audit summary instead of expanding this plan into code work.
- Do not archive the current audit design or this implementation plan while the audit is still in progress. They remain active process artifacts until the audit lands.
- For docs with YAML frontmatter, bump `last_updated` and `revision` whenever their content changes.
- Keep archive moves deterministic: same basename, matching archive subtree, no ad hoc renames unless a collision requires a suffix.

## File Map

| File | Responsibility |
|------|----------------|
| `scripts/docs-audit-inventory.sh` | Enumerate tracked documentation-like files from `git ls-files`, classify each file into an audit family, and provide the authoritative inventory order |
| `scripts/check-doc-audit-ledger.sh` | Validate that the ledger covers every tracked doc path and enforces allowed statuses/dispositions/claim-cluster fields |
| `docs/superpowers/documentation-accuracy-audit-inventory.tsv` | Baseline snapshot of the tracked documentation surface at audit start; used to prove every originally tracked doc received a disposition even if later moved or deleted |
| `docs/superpowers/documentation-accuracy-audit-ledger.tsv` | Machine-checkable ledger with one row per tracked doc-like file |
| `docs/superpowers/documentation-accuracy-audit-summary.md` | Human-readable audit report listing major corrections, archival decisions, WIP clarifications, residual risks, and final verification commands |
| `.gitignore` | Permit tracked markdown archives under `docs/superpowers/plans/archive/` and `docs/superpowers/specs/archive/` while leaving ignored review trees ignored |
| `docs/superpowers/plans/*.md` and `docs/superpowers/specs/*.md` | Tracked planning/design docs that must be triaged into keep active, archive, or delete |
| `README.md`, `ROADMAP.md`, `CONTRIBUTING.md`, `SECURITY.md`, `CHANGELOG.md`, `AGENTS.md`, `CLAUDE.md` | Root release-facing and contributor-facing docs |
| `.github/ISSUE_TEMPLATE/*.md`, `.github/PULL_REQUEST_TEMPLATE.md` | Tracked templates and process metadata docs |
| `docs/ARCHITECTURE.md`, `docs/INTEGRATION-TESTING.md`, `docs/SCRIPTLET_SECURITY.md`, `docs/conaryopedia-v2.md`, `docs/llms/*.md`, `docs/modules/*.md`, `docs/operations/*.md`, `docs/specs/ccs-format-v1.md` | Canonical docs that must be re-verified against code, scripts, config, and workflows |
| `deploy/*.md`, `bootstrap/stage0/README.md`, `apps/conary-test/README.md`, `apps/conary/tests/scriptlet_harness/README.md`, `apps/conary/tests/fixtures/adversarial/README.md`, `site/README.md`, `web/README.md` | Deploy/operator/app/frontend docs to verify against scripts, manifests, and current ownership boundaries |
| `docs/llms/archive/*.md`, `docs/superpowers/archive/*.md`, `recipes/archive/core/**/README.md`, any retained archived plan/spec docs | Historical docs that must be kept historical, not current-facing |

## Chunk 1: Audit Scaffolding And Planning Triage

### Task 1: Create the tracked-doc inventory helper and ledger gate

**Files:**
- Create: `scripts/docs-audit-inventory.sh`
- Create: `scripts/check-doc-audit-ledger.sh`

- [ ] **Step 1: Write `scripts/docs-audit-inventory.sh` around `git ls-files`**

Requirements:
- enumerate tracked files only
- treat these as documentation-like:
  - `README.md`
  - `AGENTS.md`
  - `CONTRIBUTING.md`
  - `ROADMAP.md`
  - `CHANGELOG.md`
  - `SECURITY.md`
  - `CLAUDE.md`
  - `*.md`
- `*.mdx`
- `*.rst`
- `*.adoc`
- classify each row into one of:
  - `root`
  - `template`
  - `canonical`
  - `deploy`
  - `app-local`
  - `planning`
  - `historical`
  - `frontend`
- output one header row plus tab-separated data rows in stable sorted order:

```text
path	family	audience
README.md	root	user
docs/ARCHITECTURE.md	canonical	contributor
...
```

- [ ] **Step 2: Write `scripts/check-doc-audit-ledger.sh` with pending and complete modes**

Requirements:
- accept `docs/superpowers/documentation-accuracy-audit-ledger.tsv` as input
- read `docs/superpowers/documentation-accuracy-audit-inventory.tsv` as the baseline inventory snapshot
- support:
  - `--allow-pending`
  - `--require-complete`
- compare ledger `origin_path` values against the baseline inventory snapshot
- compare ledger `path` values for retained rows against the current tracked inventory from `scripts/docs-audit-inventory.sh`
- fail if:
  - a baseline tracked doc `origin_path` is missing from the ledger
  - a baseline tracked doc `origin_path` appears more than once
  - a retained row references a non-tracked current `path`
  - a current tracked doc path is missing from the ledger `path` column for a non-deleted row
  - `family` is outside the allowed set
  - `status` is not `pending` or `verified`
  - `disposition` is not blank in pending mode or one of:
    - `verified-no-change`
    - `corrected`
    - `clarified-as-wip`
    - `reframed-as-historical`
    - `retained-historical`
    - `archived`
    - `deleted`
- in `--require-complete` mode, also fail if:
  - any row still has `status=pending`
  - any retained doc has empty `claim_clusters`
  - any retained doc has empty `evidence_sources`
  - any historical doc row lacks a historical disposition
  - any deleted row has a non-empty current `path`

- [ ] **Step 3: Verify both helpers parse and the inventory shape is correct**

Run:

```bash
bash -n scripts/docs-audit-inventory.sh scripts/check-doc-audit-ledger.sh
bash scripts/docs-audit-inventory.sh | sed -n '1,20p'
```

Expected:
- `bash -n` exits successfully
- the inventory output includes `README.md`, `docs/ARCHITECTURE.md`, and `docs/superpowers/specs/2026-04-09-documentation-accuracy-audit-design.md`
- the inventory output does **not** include `docs/plans/archive/2026-03-04-composefs-design.md`

- [ ] **Step 4: Verify the gate fails before the ledger exists**

Run:

```bash
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --allow-pending
```

Expected: FAIL with a clear missing-ledger or missing-coverage error.

- [ ] **Step 5: Commit**

```bash
git add scripts/docs-audit-inventory.sh scripts/check-doc-audit-ledger.sh
git commit -m "docs(audit): add documentation inventory helpers" -m "Part of docs/superpowers/plans/2026-04-09-documentation-accuracy-audit-plan.md"
```

### Task 2: Seed the ledger and audit summary from the tracked inventory

**Files:**
- Create: `docs/superpowers/documentation-accuracy-audit-inventory.tsv`
- Create: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`
- Create: `docs/superpowers/documentation-accuracy-audit-summary.md`

- [ ] **Step 1: Create the audit summary scaffold**

Create `docs/superpowers/documentation-accuracy-audit-summary.md` with these sections:
- Scope
- Verification Commands
- Major Corrections
- WIP Clarifications
- Archive/Delete Decisions
- Residual Risks
- Final Counts

Keep claims narrow at this stage. This file will be filled in as the audit progresses and must itself be included in the ledger once it exists.

Immediately stage it so the current tracked-doc inventory can see it:

```bash
git add docs/superpowers/documentation-accuracy-audit-summary.md
```

- [ ] **Step 2: Freeze the baseline inventory snapshot after the summary file exists**

Create:

```bash
bash scripts/docs-audit-inventory.sh > docs/superpowers/documentation-accuracy-audit-inventory.tsv
```

Requirements:
- the snapshot must be taken after `docs/superpowers/documentation-accuracy-audit-summary.md` exists
- the snapshot becomes the audit baseline for original coverage accounting
- do not regenerate this file later to “follow along” with moves/deletions; it records the starting tracked-doc surface

- [ ] **Step 3: Seed the TSV ledger from the baseline inventory**

Create `docs/superpowers/documentation-accuracy-audit-ledger.tsv` with this header:

```text
origin_path	path	family	audience	claim_clusters	evidence_sources	status	disposition	notes
```

Populate one row per tracked doc from `docs/superpowers/documentation-accuracy-audit-inventory.tsv`, skipping the header row, including:
- the new summary file
- the current audit design spec
- this implementation plan

Initialize:
- `origin_path` = baseline tracked path
- `path` = current tracked path (initially equal to `origin_path`)
- `claim_clusters` = empty
- `evidence_sources` = empty
- `status` = `pending`
- `disposition` = empty
- `notes` = optional

- [ ] **Step 4: Verify the seeded ledger covers the entire tracked doc inventory**

Run:

```bash
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --allow-pending
```

Expected: PASS, even though rows are still pending, because every tracked doc path is represented exactly once.

- [ ] **Step 5: Commit**

```bash
git add docs/superpowers/documentation-accuracy-audit-inventory.tsv docs/superpowers/documentation-accuracy-audit-ledger.tsv docs/superpowers/documentation-accuracy-audit-summary.md
git commit -m "docs(audit): seed documentation audit ledger" -m "Part of docs/superpowers/plans/2026-04-09-documentation-accuracy-audit-plan.md"
```

### Task 3: Enable tracked archive targets and triage tracked planning/design docs

**Files:**
- Modify: `.gitignore`
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-summary.md`
- Modify: `docs/superpowers/plans/2026-04-07-docs-source-selection-refresh-plan.md`
- Modify: `docs/superpowers/plans/2026-04-07-source-selection-program-plan.md`
- Modify: `docs/superpowers/plans/2026-04-09-forge-integration-hardening-plan.md`
- Modify: `docs/superpowers/plans/2026-04-09-release-matrix-realignment-plan.md`
- Modify: `docs/superpowers/plans/2026-04-09-documentation-accuracy-audit-plan.md`
- Modify: `docs/superpowers/specs/2026-04-04-deferred-refactors-roadmap-design.md`
- Modify: `docs/superpowers/specs/2026-04-07-cross-distro-root-version-matching-design.md`
- Modify: `docs/superpowers/specs/2026-04-07-source-selection-policy-design.md`
- Modify: `docs/superpowers/specs/2026-04-09-documentation-accuracy-audit-design.md`
- Modify: `docs/superpowers/specs/2026-04-09-forge-integration-hardening-design.md`
- Modify: `docs/superpowers/specs/2026-04-09-release-matrix-realignment-design.md`
- Modify: `docs/superpowers/specs/cross-crate-duplication-findings.md`
- Create or move as needed: `docs/superpowers/plans/archive/*.md`
- Create or move as needed: `docs/superpowers/specs/archive/*.md`

- [ ] **Step 1: Update `.gitignore` so tracked archived plans/specs can exist**

Change the archive rules so:
- `docs/superpowers/plans/archive/*` is ignored by default, but `!docs/superpowers/plans/archive/*.md` is unignored so tracked markdown can live there
- `docs/superpowers/specs/archive/*` is ignored by default, but `!docs/superpowers/specs/archive/*.md` is unignored so tracked markdown can live there
- `docs/superpowers/reviews/` remains ignored
- ignored local `docs/plans/archive/` remains ignored

Do not broaden the ignore exceptions further than needed.

- [ ] **Step 2: Record triage decisions for every tracked active plan/spec doc**

For each tracked doc under `docs/superpowers/plans/` and `docs/superpowers/specs/`, decide one of:
- keep active
- archive
- delete

Use:

```bash
bash scripts/docs-audit-inventory.sh | rg 'docs/superpowers/(plans|specs)/.*\.md'
```

and then verify references with:

```bash
for path in \
  docs/superpowers/plans/2026-04-07-docs-source-selection-refresh-plan.md \
  docs/superpowers/plans/2026-04-07-source-selection-program-plan.md \
  docs/superpowers/plans/2026-04-09-forge-integration-hardening-plan.md \
  docs/superpowers/plans/2026-04-09-release-matrix-realignment-plan.md \
  docs/superpowers/plans/2026-04-09-documentation-accuracy-audit-plan.md \
  docs/superpowers/specs/2026-04-04-deferred-refactors-roadmap-design.md \
  docs/superpowers/specs/2026-04-07-cross-distro-root-version-matching-design.md \
  docs/superpowers/specs/2026-04-07-source-selection-policy-design.md \
  docs/superpowers/specs/2026-04-09-documentation-accuracy-audit-design.md \
  docs/superpowers/specs/2026-04-09-forge-integration-hardening-design.md \
  docs/superpowers/specs/2026-04-09-release-matrix-realignment-design.md \
  docs/superpowers/specs/cross-crate-duplication-findings.md
do
  echo "== $path =="
  rg -n --fixed-strings "$(basename "$path")" AGENTS.md README.md ROADMAP.md CONTRIBUTING.md docs .github deploy apps bootstrap recipes || true
done
```

Expected:
- the audit design and this plan remain active
- any doc moved or deleted has a recorded rationale in the ledger `notes` column
- any doc kept active is fully verified in this task: populate `claim_clusters`, populate `evidence_sources`, set `status=verified`, and set `disposition=verified-no-change`, `corrected`, or `clarified-as-wip` as appropriate

- [ ] **Step 3: Execute archive moves and deletions under the approved policy**

Rules:
- if a tracked plan/spec is superseded, outside an archive subtree, dated `2026-04-01` or later, and still worth retaining, move it to the matching `docs/superpowers/{plans|specs}/archive/` subtree with the same basename
- if a tracked plan/spec is superseded, outside an archive subtree, dated before `2026-04-01`, unreferenced by retained docs, and already captured elsewhere, delete it
- do not archive or delete the current audit design or this implementation plan in this task

After each action:
- for a moved file, keep `origin_path` unchanged, update `path` to the new archive path, keep `status=pending` unless the file was also fully verified in this task, set `disposition=archived`, and add a short note
- for a deleted file, keep `origin_path` unchanged, set `path` to empty, set `status=verified`, set `disposition=deleted`, and add a short note
- for a kept-active file, leave `origin_path` and `path` unchanged, set `status=verified`, and set the appropriate retained disposition

- [ ] **Step 4: Verify archive paths are now tracked cleanly and references are not dangling**

Run:

```bash
git ls-files | rg '^docs/superpowers/(plans|specs)/archive/.*\.md$'
rg -n '2026-04-07-docs-source-selection-refresh-plan|2026-04-07-source-selection-program-plan|2026-04-09-forge-integration-hardening-plan|2026-04-09-release-matrix-realignment-plan|2026-04-09-documentation-accuracy-audit-plan|2026-04-04-deferred-refactors-roadmap-design|2026-04-07-cross-distro-root-version-matching-design|2026-04-07-source-selection-policy-design|2026-04-09-forge-integration-hardening-design|2026-04-09-release-matrix-realignment-design|cross-crate-duplication-findings' .
```

Expected:
- any retained archived plan/spec files appear under tracked archive paths
- deleted file basenames no longer appear in retained active-doc references
- kept-active plan/spec rows are already marked `status=verified` in the ledger
- the ledger still passes pending-mode coverage

Run:

```bash
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --allow-pending
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add .gitignore docs/superpowers/documentation-accuracy-audit-ledger.tsv docs/superpowers/documentation-accuracy-audit-summary.md docs/superpowers/plans docs/superpowers/specs
git commit -m "docs(audit): triage tracked planning material" -m "Part of docs/superpowers/plans/2026-04-09-documentation-accuracy-audit-plan.md"
```

## Chunk 2: Active Release-Facing Documentation

Retained active planning/design docs are verified during Chunk 1 Task 3. This
chunk covers the remaining active release-facing docs outside the planning/spec
triage set, plus the audit artifacts revisited in the final gate.

### Task 4: Audit root docs and tracked templates

**Files:**
- Modify: `.github/ISSUE_TEMPLATE/bug_report.md`
- Modify: `.github/ISSUE_TEMPLATE/feature_request.md`
- Modify: `.github/PULL_REQUEST_TEMPLATE.md`
- Modify: `README.md`
- Modify: `ROADMAP.md`
- Modify: `CONTRIBUTING.md`
- Modify: `SECURITY.md`
- Modify: `CHANGELOG.md`
- Modify: `AGENTS.md`
- Modify: `CLAUDE.md`
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-summary.md`

- [ ] **Step 1: Fill in claim clusters and evidence sources for the root/template family**

For each file above, populate ledger columns using clusters such as:
- `paths-modules`
- `commands-cli`
- `release-process`
- `security-trust`
- `workflow-template`
- `historical-framing`

Use evidence from:
- `Cargo.toml`
- `AGENTS.md`
- `.github/workflows/*.yml`
- `scripts/release.sh`
- `scripts/release-matrix.sh`
- `scripts/deploy-forge.sh`
- `scripts/forge-smoke.sh`

- [ ] **Step 2: Verify root command/process claims against the repo**

Run:

```bash
cargo run -p conary -- --help >/tmp/conary-help.txt
cargo run -p conary-test -- --help >/tmp/conary-test-help.txt
rg -n 'workspace|apps/conary|apps/remi|apps/conaryd|apps/conary-test|crates/conary-core|GitHub Actions|release|remi-v|conaryd-v|conary-test-v|server-v|test-v' README.md ROADMAP.md CONTRIBUTING.md SECURITY.md CHANGELOG.md AGENTS.md CLAUDE.md .github/ISSUE_TEMPLATE/*.md .github/PULL_REQUEST_TEMPLATE.md Cargo.toml .github/workflows scripts
```

Expected:
- command families named in root docs exist in current CLI help or scripts
- release/process docs use current GitHub terminology, not stale Forgejo-era language unless explicitly historical
- any legacy tag prefixes are clearly framed as continuity/history, not future canonical tags

- [ ] **Step 3: Correct the files and update ledger dispositions**

For each modified file:
- update frontmatter where present
- make claims narrow and provable
- mark visible-but-incomplete behavior honestly
- preserve short map-like guidance in `AGENTS.md`
- update the ledger row to `verified-no-change`, `corrected`, or `clarified-as-wip`
- add major corrections to the summary

- [ ] **Step 4: Verify the family is consistent**

Run:

```bash
rg -n 'Forgejo|server-v|test-v|preview|WIP|not yet supported|GitHub Actions' README.md ROADMAP.md CONTRIBUTING.md SECURITY.md CHANGELOG.md AGENTS.md CLAUDE.md .github/ISSUE_TEMPLATE/*.md .github/PULL_REQUEST_TEMPLATE.md
```

Expected:
- stale names are gone or explicitly historical
- preview/WIP language appears only where intended

- [ ] **Step 5: Commit**

```bash
git add .github/ISSUE_TEMPLATE/bug_report.md .github/ISSUE_TEMPLATE/feature_request.md .github/PULL_REQUEST_TEMPLATE.md README.md ROADMAP.md CONTRIBUTING.md SECURITY.md CHANGELOG.md AGENTS.md CLAUDE.md docs/superpowers/documentation-accuracy-audit-ledger.tsv docs/superpowers/documentation-accuracy-audit-summary.md
git commit -m "docs(audit): refresh root docs and templates" -m "Part of docs/superpowers/plans/2026-04-09-documentation-accuracy-audit-plan.md"
```

### Task 5: Audit canonical architecture, module, assistant, operations, and format docs

**Files:**
- Modify: `docs/ARCHITECTURE.md`
- Modify: `docs/llms/README.md`
- Modify: `docs/llms/subsystem-map.md`
- Modify: `docs/modules/bootstrap.md`
- Modify: `docs/modules/ccs.md`
- Modify: `docs/modules/federation.md`
- Modify: `docs/modules/query.md`
- Modify: `docs/modules/recipe.md`
- Modify: `docs/modules/source-selection.md`
- Modify: `docs/operations/LOCAL_ACCESS.example.md`
- Modify: `docs/operations/infrastructure.md`
- Modify: `docs/specs/ccs-format-v1.md`
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-summary.md`

- [ ] **Step 1: Record claim clusters for each canonical doc before editing**

Use cluster values such as:
- `paths-modules`
- `commands-cli`
- `workflow-deploy-release`
- `urls-hosts-endpoints`
- `support-status`
- `historical-framing`

Populate `evidence_sources` in the ledger from exact files, for example:
- `Cargo.toml`
- `apps/conary/src/commands/*.rs`
- `apps/remi/src/**`
- `apps/conaryd/src/**`
- `apps/conary-test/src/**`
- `crates/conary-core/src/**`
- `scripts/*.sh`
- `.github/workflows/*.yml`

- [ ] **Step 2: Verify module and path claims against the live tree**

Run:

```bash
rg --files apps crates docs/modules docs/operations
rg -n 'apps/conary|apps/remi|apps/conaryd|apps/conary-test|crates/conary-core|crates/conary-bootstrap|crates/conary-mcp|effective_policy|replatform|admin_service|server/service|composefs|federation|ccs' docs/ARCHITECTURE.md docs/llms/README.md docs/llms/subsystem-map.md docs/modules/*.md docs/operations/*.md docs/specs/ccs-format-v1.md Cargo.toml apps crates
```

Expected:
- every module/path called out in the docs still exists
- assistant maps point at current files, not removed ones
- infrastructure docs match current script/workflow ownership

- [ ] **Step 3: Verify release/deploy/endpoint claims against scripts and workflows**

Run:

```bash
rg -n 'release-build|deploy-and-verify|pr-gate|deploy-forge|forge-smoke|release-matrix|R2|Cloudflare|9090|8082|mcp' docs/operations/infrastructure.md docs/ARCHITECTURE.md docs/modules/*.md docs/specs/ccs-format-v1.md scripts .github/workflows apps/remi apps/conary-test
```

Expected:
- workflow/script names in the docs resolve to real files
- endpoint/host/path examples still match checked-in code and docs

- [ ] **Step 4: Correct the docs and update the summary**

Required outcomes:
- update frontmatter where present
- remove stale path references
- narrow any unsupported operational claims
- add WIP labels where code-visible surfaces are not actually supported
- update ledger dispositions and add notable corrections to the summary

- [ ] **Step 5: Commit**

```bash
git add docs/ARCHITECTURE.md docs/llms/README.md docs/llms/subsystem-map.md docs/modules/bootstrap.md docs/modules/ccs.md docs/modules/federation.md docs/modules/query.md docs/modules/recipe.md docs/modules/source-selection.md docs/operations/LOCAL_ACCESS.example.md docs/operations/infrastructure.md docs/specs/ccs-format-v1.md docs/superpowers/documentation-accuracy-audit-ledger.tsv docs/superpowers/documentation-accuracy-audit-summary.md
git commit -m "docs(audit): refresh canonical docs and maps" -m "Part of docs/superpowers/plans/2026-04-09-documentation-accuracy-audit-plan.md"
```

### Task 6: Audit handbooks, testing docs, deploy docs, and app/frontend READMEs

**Files:**
- Modify: `docs/INTEGRATION-TESTING.md`
- Modify: `docs/SCRIPTLET_SECURITY.md`
- Modify: `docs/conaryopedia-v2.md`
- Modify: `deploy/CLOUDFLARE.md`
- Modify: `deploy/FORGE.md`
- Modify: `deploy/dracut/NOTE.md`
- Modify: `bootstrap/stage0/README.md`
- Modify: `apps/conary-test/README.md`
- Modify: `apps/conary/tests/scriptlet_harness/README.md`
- Modify: `apps/conary/tests/fixtures/adversarial/README.md`
- Modify: `site/README.md`
- Modify: `web/README.md`
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-summary.md`

- [ ] **Step 1: Populate ledger claim clusters and evidence sources for this family**

Use evidence from:
- `apps/conary/tests/integration/remi/manifests/*.toml`
- `apps/conary-test/src/**`
- `apps/conary/tests/**`
- `crates/conary-core/src/capability/**`
- `crates/conary-core/src/container/**`
- `scripts/deploy-forge.sh`
- `scripts/forge-smoke.sh`
- `scripts/release.sh`
- `scripts/release-matrix.sh`
- `.github/workflows/*.yml`
- `bootstrap/stage0/**`

- [ ] **Step 2: Verify testing and harness claims against manifests and CLI**

Run:

```bash
cargo run -p conary-test -- list >/tmp/conary-test-list.txt
rg -n 'phase 1|phase 2|phase 3|phase 4|qemu_boot|fixtures|deploy rollout|deploy status|forge-smoke|selection-mode|replatform' docs/INTEGRATION-TESTING.md apps/conary-test/README.md apps/conary/tests/integration/remi/manifests apps/conary-test/src scripts/forge-smoke.sh scripts/deploy-forge.sh /tmp/conary-test-list.txt
```

Expected:
- documented suite/group names still exist in manifests or CLI
- supported operator flows match the checked-in scripts

- [ ] **Step 3: Verify scriptlet/bootstrap/deploy claims against code and docs**

Run:

```bash
rg -n 'scriptlet|sandbox|seccomp|landlock|stage0|dracut|Cloudflare|Forge|R2|tracked config|static root' docs/SCRIPTLET_SECURITY.md deploy/CLOUDFLARE.md deploy/FORGE.md deploy/dracut/NOTE.md bootstrap/stage0/README.md docs/conaryopedia-v2.md crates/conary-core/src apps/remi/src scripts
```

Expected:
- docs reference real code paths or scripts
- any unsupported or partial behavior is clearly marked

- [ ] **Step 4: Correct the docs and update dispositions**

Required outcomes:
- update frontmatter where present
- handbook examples reflect current behavior or explicit WIP status
- testing/deploy docs describe only supported flows as supported
- frontend/app README ownership statements match the current repo
- ledger rows for these files move out of `pending`
- summary captures major correctness changes

- [ ] **Step 5: Commit**

```bash
git add docs/INTEGRATION-TESTING.md docs/SCRIPTLET_SECURITY.md docs/conaryopedia-v2.md deploy/CLOUDFLARE.md deploy/FORGE.md deploy/dracut/NOTE.md bootstrap/stage0/README.md apps/conary-test/README.md apps/conary/tests/scriptlet_harness/README.md apps/conary/tests/fixtures/adversarial/README.md site/README.md web/README.md docs/superpowers/documentation-accuracy-audit-ledger.tsv docs/superpowers/documentation-accuracy-audit-summary.md
git commit -m "docs(audit): refresh operational and handbook docs" -m "Part of docs/superpowers/plans/2026-04-09-documentation-accuracy-audit-plan.md"
```

## Chunk 3: Historical Material And Final Release Gate

### Task 7: Audit historical docs and retain them as historical

**Files:**
- Modify: `docs/llms/archive/claude-era-notes.md`
- Modify: `docs/superpowers/archive/2026-04-04-codebase-simplification-retrospective.md`
- Modify: `recipes/archive/core/README.md`
- Modify: `recipes/archive/core/archive/README.md`
- Modify: `recipes/archive/core/base/README.md`
- Modify: `recipes/archive/core/boot/README.md`
- Modify: `recipes/archive/core/dev/README.md`
- Modify: `recipes/archive/core/editors/README.md`
- Modify: `recipes/archive/core/libs/README.md`
- Modify: `recipes/archive/core/net/README.md`
- Modify: `recipes/archive/core/stage1/README.md`
- Modify: `recipes/archive/core/sys/README.md`
- Modify: `recipes/archive/core/text/README.md`
- Modify: `recipes/archive/core/vcs/README.md`
- Modify as needed: retained files under `docs/superpowers/plans/archive/*.md`
- Modify as needed: retained files under `docs/superpowers/specs/archive/*.md`
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-summary.md`

- [ ] **Step 1: Mark every historical file with historical claim clusters in the ledger**

For these files, use claim clusters such as:
- `historical-framing`
- `links-references`
- `status-language`

and evidence sources such as:
- the doc itself
- current active docs that link to it
- repository layout confirming whether referenced paths still exist

- [ ] **Step 2: Reframe or correct any misleading present-tense language**

Run:

```bash
rg -n 'supported|current|use this|run this|deploy|recommended|canonical|active' docs/llms/archive/claude-era-notes.md docs/superpowers/archive/2026-04-04-codebase-simplification-retrospective.md recipes/archive/core docs/superpowers/plans/archive docs/superpowers/specs/archive 2>/dev/null
```

Expected:
- any present-tense operational wording that could mislead a reader is either removed, reframed as historical, or explicitly contrasted with the current path

- [ ] **Step 3: Verify active docs are not treating historical docs as current source of truth**

Run:

```bash
rg -n 'claude-era-notes|recipes/archive|docs/superpowers/archive|docs/superpowers/plans/archive|docs/superpowers/specs/archive' README.md ROADMAP.md CONTRIBUTING.md AGENTS.md CLAUDE.md docs/ARCHITECTURE.md docs/INTEGRATION-TESTING.md docs/SCRIPTLET_SECURITY.md docs/conaryopedia-v2.md docs/llms/README.md docs/llms/subsystem-map.md docs/modules docs/operations docs/specs/ccs-format-v1.md docs/superpowers/plans docs/superpowers/specs deploy apps/conary-test/README.md apps/conary/tests/scriptlet_harness/README.md apps/conary/tests/fixtures/adversarial/README.md bootstrap/stage0/README.md .github site/README.md web/README.md
```

Expected:
- historical docs are linked only intentionally and with clear historical framing

- [ ] **Step 4: Update dispositions and summary**

For each file:
- update frontmatter where present
- use `retained-historical` if no changes were required beyond verification
- use `reframed-as-historical` if wording or framing changed
- document major historical clarifications in the summary

- [ ] **Step 5: Commit**

```bash
git add docs/llms/archive/claude-era-notes.md docs/superpowers/archive/2026-04-04-codebase-simplification-retrospective.md recipes/archive/core/README.md recipes/archive/core/archive/README.md recipes/archive/core/base/README.md recipes/archive/core/boot/README.md recipes/archive/core/dev/README.md recipes/archive/core/editors/README.md recipes/archive/core/libs/README.md recipes/archive/core/net/README.md recipes/archive/core/stage1/README.md recipes/archive/core/sys/README.md recipes/archive/core/text/README.md recipes/archive/core/vcs/README.md docs/superpowers/documentation-accuracy-audit-ledger.tsv docs/superpowers/documentation-accuracy-audit-summary.md
find docs/superpowers/plans/archive docs/superpowers/specs/archive -maxdepth 1 -name '*.md' -print0 2>/dev/null | xargs -0r git add --
git commit -m "docs(audit): reframe retained historical docs" -m "Part of docs/superpowers/plans/2026-04-09-documentation-accuracy-audit-plan.md"
```

### Task 8: Run the final consistency pass and close the audit

**Files:**
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-summary.md`
- Modify as needed: any retained tracked doc still failing normalization checks

- [ ] **Step 1: Normalize cross-doc vocabulary and record final corrections**

Run:

```bash
rg -n 'Forgejo|server-v|test-v|conary test|not yet supported|preview|WIP|GitHub Actions|docs/plans/archive|docs/superpowers/reviews' README.md ROADMAP.md CONTRIBUTING.md SECURITY.md CHANGELOG.md AGENTS.md CLAUDE.md docs/ARCHITECTURE.md docs/INTEGRATION-TESTING.md docs/SCRIPTLET_SECURITY.md docs/conaryopedia-v2.md docs/llms/README.md docs/llms/subsystem-map.md docs/modules docs/operations docs/specs/ccs-format-v1.md docs/superpowers/plans docs/superpowers/specs deploy apps/conary-test/README.md apps/conary/tests/scriptlet_harness/README.md apps/conary/tests/fixtures/adversarial/README.md bootstrap/stage0/README.md .github site/README.md web/README.md
```

Expected:
- stale or misleading terms are gone
- any retained legacy names are explicitly historical/continuity notes
- `GitHub Actions`, `preview`, and `WIP` are retained where truthful and intended; this search is for normalization and context review, not blind deletion
- preview/WIP wording only appears where intended

If Step 1 changes a file during normalization:
- update frontmatter where present
- update that file's ledger row disposition
- confirm the relevant `claim_clusters` still match the file
- confirm `evidence_sources` still support the normalized wording

- [ ] **Step 2: Re-audit the current audit artifacts themselves**

Before finalizing the ledger:
- verify `docs/superpowers/documentation-accuracy-audit-summary.md` against the actual ledger counts and major decisions
- verify `docs/superpowers/specs/2026-04-09-documentation-accuracy-audit-design.md` and this implementation plan still describe the real tracked scope and process decisions
- update the corresponding ledger rows and set non-pending dispositions

- [ ] **Step 3: Fill in final counts and verification commands in the summary**

The summary must include:
- count by disposition
- list of archived files
- list of deleted files
- notable WIP clarifications
- residual risks not fixed by the audit
- final verification commands actually run

- [ ] **Step 4: Run the completion gate**

Run:

```bash
bash -n scripts/docs-audit-inventory.sh scripts/check-doc-audit-ledger.sh
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
git diff --check
```

Expected:
- shell scripts still parse
- the ledger covers every tracked doc path and no rows remain pending
- every retained active/template doc has claim clusters and evidence sources
- historical docs use historical dispositions
- `git diff --check` reports no whitespace/hunk formatting errors

- [ ] **Step 5: Commit**

```bash
bash scripts/docs-audit-inventory.sh | tail -n +2 | cut -f1 | tr '\n' '\0' | xargs -0r git add --
git add docs/superpowers/documentation-accuracy-audit-inventory.tsv docs/superpowers/documentation-accuracy-audit-ledger.tsv docs/superpowers/documentation-accuracy-audit-summary.md scripts/docs-audit-inventory.sh scripts/check-doc-audit-ledger.sh .gitignore
git commit -m "docs: complete documentation accuracy audit" -m "Part of docs/superpowers/plans/2026-04-09-documentation-accuracy-audit-plan.md"
```

## Completion Checklist

- `scripts/docs-audit-inventory.sh` is the sole authority for the tracked doc inventory
- `scripts/check-doc-audit-ledger.sh --require-complete` passes
- every tracked doc-like file has exactly one ledger row
- every retained active/template doc has claim clusters and evidence sources recorded
- every retained historical doc has a historical disposition
- recent superseded tracked plans/specs have been archived into tracked archive subtrees
- older stale tracked plans/specs have been deleted only after reference checks
- the summary captures major corrections, archive/delete decisions, WIP clarifications, and residual risks
