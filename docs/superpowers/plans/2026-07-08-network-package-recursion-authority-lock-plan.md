# Network Package Recursion Authority Lock Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Keep live network fetches and nested package-manager calls blocked/non-public while broadening command evidence for common alias forms.

**Architecture:** Extend the blocked-class registry and Remi corpus hints for common distro package-manager aliases and `git clone` live-fetch evidence, including global-option forms such as `git -C /tmp clone ...`. Add support-matrix, converter, and Remi publication regressions proving these classes have no Known adapter row, no manifest or policy authority, no replay authority, and no public-ready gate exception. Update docs and fixture metadata to state that future support must model dependency intent or curated offline artifacts instead of live fetch or nested package-manager execution.

**Tech Stack:** Rust unit tests in `conary-core` and `remi`, passive conversion tests, Remi publication tests, docs-audit checks.

## Global Constraints

- No live network fetch or nested package-manager command form becomes public-ready in this slice.
- Network fetch evidence remains `blocked-class-network` with `publication_status = "blocked"`.
- Nested package-manager evidence remains `blocked-class-package-manager-recursion` with `publication_status = "blocked"`.
- Do not add a network adapter, package-manager adapter, dependency-intent projection, offline artifact authority, target-profile package-manager facts, replay behavior, or Remi public gate exception.
- `git` live fetch evidence is blocked only when the first non-global-option subcommand is `clone`, so global git options before `clone` cannot bypass detection without blocking unrelated `git` commands.
- Admin/non-public test serving may continue to serve valid blocked rows only through the existing default-off admin lane; public package, detail, index, sparse, OCI, and chunk routes continue to require public-ready status.
- Corpus summaries remain advisory planning evidence only; they do not declare scriptlets `replaced`.
- Documentation changes must keep `docs/SCRIPTLET_SECURITY.md`, `docs/modules/ccs.md`, `docs/modules/remi.md`, `docs/modules/test-fixtures.md`, docs-audit ledger, and inventory aligned.

---

## File Structure

- Modify `crates/conary-core/src/ccs/convert/blocked_classes.rs`
  - Add blocked-class tests and narrow git-clone-subcommand evidence for common distro package-manager aliases.
- Modify `apps/remi/src/server/scriptlet_corpus.rs`
  - Keep advisory corpus blocked-class hints aligned with the blocked-class registry for the same forms.
- Modify `crates/conary-core/src/ccs/convert/support_matrix.rs`
  - Add explicit support-matrix proof that network and package-manager recursion have blocked rows only and no Known support rows.
- Modify `crates/conary-core/src/ccs/convert/converter.rs`
  - Add passive conversion regressions proving live fetch and nested package-manager helpers remain blocked/private with no native authority.
- Modify `apps/remi/src/server/publication.rs`
  - Add Remi publication regressions proving blocked network and package-manager summaries stay non-public-only for chunks.
- Modify `docs/SCRIPTLET_SECURITY.md`
  - Document the live-fetch and nested package-manager authority boundary.
- Modify `docs/modules/ccs.md`
  - Mirror the blocked/private boundary in CCS public-ready conversion docs.
- Modify `docs/modules/remi.md`
  - Clarify that Remi scan-only corpus output is advisory and does not promote live fetch or nested package-manager evidence.
- Modify `docs/modules/test-fixtures.md`
  - Register the network and package-manager blocked fixture intent.
- Modify `docs/superpowers/documentation-accuracy-audit-ledger.tsv`
  - Register this plan and touched claims.
- Modify `docs/superpowers/documentation-accuracy-audit-inventory.tsv`
  - Regenerate after staging this plan and docs.

## Task 1: Extend Blocked-Class And Corpus Hints

**Files:**
- Modify: `crates/conary-core/src/ccs/convert/blocked_classes.rs`
- Modify: `apps/remi/src/server/scriptlet_corpus.rs`

**Interfaces:**
- Consumes: `BlockedClassRegistry::match_invocation`
- Consumes: `ScriptletCorpusSummary::from_scriptlets`
- Produces: `git clone` forms, including global-option forms such as `git -C /tmp clone ...`, mapped to `blocked-class-network`
- Produces: `apk`, `dnf5`, `microdnf`, and `zypper` mapped to `blocked-class-package-manager-recursion`
- Produces: matching corpus `blocked_class_hints` for scan-only planning evidence

- [ ] **Step 1: Add the blocked-class regression**

In `crates/conary-core/src/ccs/convert/blocked_classes.rs`, replace the existing `blocked_classes_block_network_and_package_manager_recursion` test with:

```rust
#[test]
fn blocked_classes_block_live_fetch_and_package_manager_recursion() {
    let registry = BlockedClassRegistry::default();

    for (command, argv) in [
        ("curl", vec!["https://example.invalid"]),
        ("wget", vec!["https://example.invalid/package.tar.gz"]),
        ("scp", vec!["host:/tmp/pkg", "/tmp/pkg"]),
        ("ssh", vec!["builder.example.invalid", "true"]),
        ("git", vec!["clone", "https://example.invalid/repo.git"]),
        (
            "git",
            vec!["-C", "/tmp", "clone", "https://example.invalid/repo.git"],
        ),
        (
            "git",
            vec![
                "-c",
                "http.sslVerify=false",
                "clone",
                "https://example.invalid/repo.git",
            ],
        ),
    ] {
        let class = registry
            .match_invocation(&invocation(command, &argv))
            .unwrap_or_else(|| panic!("missing network blocked class for {command}"));
        assert_eq!(class.id, "network");
        assert_eq!(class.reason_code, "blocked-class-network");
        assert_eq!(class.default_outcome, BlockedClassOutcome::Blocked);
    }

    assert!(
        registry
            .match_invocation(&invocation("git", &["config", "--global", "demo.value", "1"]))
            .is_none(),
        "only live-fetch git forms are blocked in this slice"
    );

    for (command, argv) in [
        ("apk", vec!["add", "demo"]),
        ("apt", vec!["install", "demo"]),
        ("apt-get", vec!["install", "demo"]),
        ("dnf", vec!["install", "demo"]),
        ("dnf5", vec!["install", "demo"]),
        ("dpkg", vec!["-i", "demo.deb"]),
        ("microdnf", vec!["install", "demo"]),
        ("pacman", vec!["-S", "demo"]),
        ("rpm", vec!["-Uvh", "demo.rpm"]),
        ("yum", vec!["install", "demo"]),
        ("zypper", vec!["install", "demo"]),
    ] {
        let class = registry
            .match_invocation(&invocation(command, &argv))
            .unwrap_or_else(|| {
                panic!("missing package-manager recursion blocked class for {command}")
            });
        assert_eq!(class.id, "package-manager-recursion");
        assert_eq!(
            class.reason_code,
            "blocked-class-package-manager-recursion"
        );
        assert_eq!(class.default_outcome, BlockedClassOutcome::Blocked);
    }
}
```

- [ ] **Step 2: Add the corpus hint regression**

In `apps/remi/src/server/scriptlet_corpus.rs`, replace `corpus_summary_marks_package_manager_recursion` with:

```rust
#[test]
fn corpus_summary_marks_live_fetch_and_package_manager_recursion() {
    let summary = ScriptletCorpusSummary::from_scriptlets(
        "arch",
        "bad-news",
        &[scriptlet(
            "pacman -Syu\ncurl https://example.invalid/script.sh\ngit clone https://example.invalid/repo.git\ngit -C /tmp clone https://example.invalid/repo.git\ngit -c http.sslVerify=false clone https://example.invalid/repo.git\nmicrodnf install demo\napk add demo\n",
        )],
    );

    assert_eq!(summary.command_counts.get("pacman"), Some(&1));
    assert_eq!(summary.command_counts.get("curl"), Some(&1));
    assert_eq!(summary.command_counts.get("git"), Some(&3));
    assert_eq!(summary.command_counts.get("microdnf"), Some(&1));
    assert_eq!(summary.command_counts.get("apk"), Some(&1));
    assert!(
        summary
            .blocked_class_hints
            .contains(&"package-manager-recursion".to_string())
    );
    assert!(summary.blocked_class_hints.contains(&"network".to_string()));
}
```

- [ ] **Step 3: Run the focused failing tests**

Run:

```bash
cargo test -p conary-core blocked_classes_block_live_fetch_and_package_manager_recursion --lib
cargo test -p remi corpus_summary_marks_live_fetch_and_package_manager_recursion
```

Expected: FAIL because option-prefixed `git clone`, `apk`, `dnf5`, `microdnf`, and `zypper` are not yet fully classified in both locations, and negative `git help clone` / `git config ... clone` cases still overmatch.

- [ ] **Step 4: Extend the blocked-class registry**

In `crates/conary-core/src/ccs/convert/blocked_classes.rs`, change the network blocked-class declaration from:

```rust
blocked_class(
    "network",
    "Network access from scriptlets is not replay-safe.",
    "blocked-class-network",
    &["curl", "wget", "scp", "ssh"],
    &[],
    "Provide a declared package dependency or a curated offline artifact.",
),
```

to:

```rust
blocked_class(
    "network",
    "Network access from scriptlets is not replay-safe.",
    "blocked-class-network",
    &["curl", "wget", "scp", "ssh"],
    &[],
    "Provide a declared package dependency or a curated offline artifact.",
),
```

and add a narrow helper that blocks `git` only when the first non-global-option
subcommand is exactly `clone` (including forms like `git -C /tmp clone ...` and
`git -c name=value clone ...`).

Change the package-manager recursion command list from:

```rust
&["dnf", "yum", "rpm", "apt", "apt-get", "dpkg", "pacman"],
```

to:

```rust
&[
    "apk", "apt", "apt-get", "dnf", "dnf5", "dpkg", "microdnf", "pacman",
    "rpm", "yum", "zypper",
],
```

- [ ] **Step 5: Extend the Remi corpus hints**

In `apps/remi/src/server/scriptlet_corpus.rs`, first change `CommandEvidence` from:

```rust
struct CommandEvidence {
    command: String,
    form: String,
}
```

to:

```rust
struct CommandEvidence {
    command: String,
    form: String,
    git_clone_fetch: bool,
}
```

In `command_from_segment`, add the `git_clone_fetch` value after `form` is built:

```rust
let git_clone_fetch = command == "git" && tokens.iter().skip(index + 1).any(|arg| *arg == "clone");
```

and return:

```rust
Some(CommandEvidence {
    command: command.to_string(),
    form,
    git_clone_fetch,
})
```

Then in `ScriptletCorpusSummary::from_scriptlets`, change:

```rust
for class in blocked_class_hints_for_command(&evidence.command, &evidence.form) {
    blocked.insert(class);
}
```

to:

```rust
for class in blocked_class_hints_for_command(
    &evidence.command,
    &evidence.form,
    evidence.git_clone_fetch,
) {
    blocked.insert(class);
}
```

Change `blocked_class_hints_for_command` from:

```rust
fn blocked_class_hints_for_command(command: &str, form: &str) -> Vec<String> {
```

to:

```rust
fn blocked_class_hints_for_command(
    command: &str,
    form: &str,
    git_clone_fetch: bool,
) -> Vec<String> {
```

Finally, change:

```rust
"dnf" | "yum" | "rpm" | "apt" | "apt-get" | "dpkg" | "pacman" => {
    classes.push("package-manager-recursion".to_string());
}
"curl" | "wget" | "scp" | "ssh" => {
    classes.push("network".to_string());
}
```

to:

```rust
"apk" | "apt" | "apt-get" | "dnf" | "dnf5" | "dpkg" | "microdnf" | "pacman"
| "rpm" | "yum" | "zypper" => {
    classes.push("package-manager-recursion".to_string());
}
"curl" | "wget" | "scp" | "ssh" => {
    classes.push("network".to_string());
}
"git" if git_clone_fetch => {
    classes.push("network".to_string());
}
```

- [ ] **Step 6: Verify focused tests pass**

Run:

```bash
cargo test -p conary-core blocked_classes_block_live_fetch_and_package_manager_recursion --lib
cargo test -p remi corpus_summary_marks_live_fetch_and_package_manager_recursion
```

Expected: PASS.

- [ ] **Step 7: Commit**

Run:

```bash
git add crates/conary-core/src/ccs/convert/blocked_classes.rs apps/remi/src/server/scriptlet_corpus.rs
git commit -m "test: cover live fetch and package-manager recursion forms"
```

## Task 2: Pin Support-Matrix Non-Authority Boundary

**Files:**
- Modify: `crates/conary-core/src/ccs/convert/support_matrix.rs`

**Interfaces:**
- Consumes: `SupportMatrix::default()`
- Produces: explicit proof that `network` and `package-manager-recursion` have blocked-class rows only
- Produces: a test helper that fails if a future Known row carries network, package-manager-recursion, live-fetch, dependency-intent, or offline-artifact support identity

- [ ] **Step 1: Add the support-matrix regressions**

In `crates/conary-core/src/ccs/convert/support_matrix.rs`, inside the existing `#[cfg(test)] mod tests`, add these tests after `pam_class_remains_blocked_without_native_adapter`:

```rust
#[test]
fn live_fetch_and_package_manager_known_row_guard_rejects_fake_rows() {
    let mut entries = SupportMatrix::default().entries().to_vec();
    entries.push(SupportMatrixEntry {
        id: "network-fetch/v0-test-only",
        command: Some("git clone"),
        class_id: None,
        adapter_id: Some("network-fetch/v0-test-only"),
        outcome: SupportOutcome::Known,
        reason_code: "helper-complete-network-fetch",
        source_families: &["rpm", "deb", "arch"],
        lifecycle_notes: "temporary in-test support row",
        fixture_names: &["adapter-network-fetch-test-only"],
    });
    entries.push(SupportMatrixEntry {
        id: "package-manager-recursion/v0-test-only",
        command: Some("microdnf install"),
        class_id: None,
        adapter_id: Some("package-manager-recursion/v0-test-only"),
        outcome: SupportOutcome::Known,
        reason_code: "helper-complete-package-manager-recursion",
        source_families: &["rpm", "deb", "arch"],
        lifecycle_notes: "temporary in-test support row",
        fixture_names: &["adapter-package-manager-test-only"],
    });

    let known_rows = live_fetch_or_package_manager_known_rows(&entries);
    assert_eq!(
        known_rows.iter().map(|entry| entry.id).collect::<Vec<_>>(),
        vec![
            "network-fetch/v0-test-only",
            "package-manager-recursion/v0-test-only"
        ]
    );
}

#[test]
fn live_fetch_and_package_manager_classes_remain_blocked_without_native_adapters() {
    let matrix = SupportMatrix::default();

    for (class_id, fixture_name) in [
        ("network", "blocked-class-network"),
        (
            "package-manager-recursion",
            "blocked-class-package-manager-recursion",
        ),
    ] {
        let row = matrix
            .entries()
            .iter()
            .find(|entry| entry.class_id == Some(class_id))
            .unwrap_or_else(|| panic!("missing support row for {class_id}"));

        assert_eq!(row.outcome, SupportOutcome::Blocked);
        assert!(row.adapter_id.is_none());
        assert_eq!(row.fixture_names, &[fixture_name]);
    }

    let known_rows = live_fetch_or_package_manager_known_rows(matrix.entries());
    assert!(
        known_rows.is_empty(),
        "live fetch and package-manager recursion should not have Known support rows: {:?}",
        known_rows.iter().map(|entry| entry.id).collect::<Vec<_>>()
    );
}
```

- [ ] **Step 2: Run the focused failing tests**

Run:

```bash
cargo test -p conary-core live_fetch_and_package_manager_known_row_guard_rejects_fake_rows --lib
cargo test -p conary-core live_fetch_and_package_manager_classes_remain_blocked_without_native_adapters --lib
```

Expected: FAIL to compile because `live_fetch_or_package_manager_known_rows` is not defined yet.

- [ ] **Step 3: Add the support-matrix helper**

In the same test module, add this helper after `pam_associated_known_rows`:

```rust
fn live_fetch_or_package_manager_known_rows(
    entries: &[SupportMatrixEntry],
) -> Vec<&SupportMatrixEntry> {
    entries
        .iter()
        .filter(|entry| {
            entry.outcome == SupportOutcome::Known
                && (matches!(
                    entry.class_id,
                    Some("network" | "package-manager-recursion")
                ) || entry.adapter_id.is_some_and(|adapter_id| {
                    adapter_id.contains("network")
                        || adapter_id.contains("package-manager")
                        || adapter_id.contains("live-fetch")
                        || adapter_id.contains("dependency-intent")
                        || adapter_id.contains("offline-artifact")
                }))
        })
        .collect()
}
```

- [ ] **Step 4: Verify the real tests pass**

Run:

```bash
cargo test -p conary-core live_fetch_and_package_manager_known_row_guard_rejects_fake_rows --lib
cargo test -p conary-core live_fetch_and_package_manager_classes_remain_blocked_without_native_adapters --lib
cargo test -p conary-core support_matrix --lib
```

Expected: PASS.

- [ ] **Step 5: Commit**

Run:

```bash
git add crates/conary-core/src/ccs/convert/support_matrix.rs
git commit -m "test: pin live fetch support matrix boundary"
```

## Task 3: Prove Conversion And Publication Stay Non-Public

**Files:**
- Modify: `crates/conary-core/src/ccs/convert/converter.rs`
- Modify: `apps/remi/src/server/publication.rs`

**Interfaces:**
- Consumes: `PassiveConverter::convert`
- Consumes: `ScriptletBundleSummary::from_bundle`
- Consumes: `classify_converted_package`
- Produces: conversion proof that blocked live fetch and package-manager recursion produce no native effects or policy authority
- Produces: Remi proof that blocked rows stay `ChunkPublicationState::NonPublicOnly`

- [ ] **Step 1: Add the converter helper and regression**

In `crates/conary-core/src/ccs/convert/converter.rs`, add this helper near the existing scriptlet conversion tests, immediately before `pam_helper_remains_blocked_without_manifest_authority`:

```rust
fn assert_blocked_scriptlet_has_no_native_authority(
    scriptlet_content: &str,
    expected_class: &str,
    expected_reason: &str,
) {
    let temp_dir = tempfile::tempdir().unwrap();
    let mut metadata = make_test_metadata();
    metadata.scriptlets = vec![Scriptlet {
        phase: ScriptletPhase::PostInstall,
        interpreter: "/bin/sh".to_string(),
        content: scriptlet_content.to_string(),
        flags: None,
    }];
    let files = make_test_files();
    let converter = passive_test_converter(temp_dir.path());

    let result = converter
        .convert(&metadata, &files, "rpm", "sha256:test")
        .expect("conversion succeeds");
    let package =
        crate::ccs::CcsPackage::parse(result.package_path.as_ref().unwrap().to_str().unwrap())
            .expect("converted CCS package should parse");
    let bundle = package
        .manifest()
        .legacy_scriptlets
        .as_ref()
        .expect("written CCS archive should carry passive scriptlet bundle");
    let bundle_summary = ScriptletBundleSummary::from_bundle(bundle, bundle.evidence_digest.clone());

    assert_eq!(bundle.publication_status.as_str(), "blocked");
    assert_eq!(bundle.decision_counts.blocked, 1);
    assert_eq!(bundle.entries.len(), 1);
    let entry = &bundle.entries[0];
    assert_eq!(entry.decision.as_str(), "blocked");
    assert_eq!(entry.reason_code, expected_reason);
    assert_eq!(entry.blocked_classes, vec![expected_class]);
    assert!(entry.effects.is_empty());
    assert!(entry.boot_security_intents.is_empty());
    assert!(entry.security_policy_intents.is_empty());
    assert!(bundle_summary.boot_security_intents.is_empty());
    assert!(bundle_summary.security_policy_intents.is_empty());
    assert!(bundle.security_policy_intents.is_empty());
    assert_eq!(result.scriptlet_metadata.publication_status, "blocked");
    assert_eq!(
        result.scriptlet_metadata.blocked_classes,
        vec![expected_class.to_string()]
    );
    assert!(result.scriptlet_metadata.boot_security_intents.is_empty());
    assert!(result.scriptlet_metadata.security_policy_intents.is_empty());
    assert_ne!(result.scriptlet_metadata.publication_status, "public");
}
```

Then add this test immediately after the helper:

```rust
#[test]
fn live_fetch_and_package_manager_helpers_remain_blocked_without_manifest_authority() {
    assert_blocked_scriptlet_has_no_native_authority(
        "git -C /tmp clone https://example.invalid/repo.git\n",
        "network",
        "blocked-class-network",
    );
    assert_blocked_scriptlet_has_no_native_authority(
        "microdnf install demo\n",
        "package-manager-recursion",
        "blocked-class-package-manager-recursion",
    );
}
```

- [ ] **Step 2: Run the focused converter regression**

Run:

```bash
cargo test -p conary-core live_fetch_and_package_manager_helpers_remain_blocked_without_manifest_authority --lib
```

Expected: PASS after Task 1's classification changes; if it fails, inspect the classification result instead of adding adapter evidence.

- [ ] **Step 3: Add the Remi publication regression**

In `apps/remi/src/server/publication.rs`, add this test after `blocked_pam_report_stays_private_and_non_public_only`:

```rust
#[test]
fn blocked_live_fetch_and_package_manager_reports_stay_private_and_non_public_only() {
    let temp = tempfile::TempDir::new().unwrap();
    let db_path = temp.path().join("remi.db");
    conary_core::db::init(&db_path).unwrap();
    let conn = conary_core::db::open(&db_path).unwrap();

    for (name, chunk, class_id, reason_code) in [
        (
            "network-private",
            "network-chunk",
            "network",
            "blocked-class-network",
        ),
        (
            "pm-private",
            "pm-chunk",
            "package-manager-recursion",
            "blocked-class-package-manager-recursion",
        ),
    ] {
        let mut summary = golden_summary("blocked", "blocked", "blocked");
        summary.decision_counts = ScriptletDecisionCountsSummary {
            blocked: 1,
            ..ScriptletDecisionCountsSummary::default()
        };
        summary.blocked_reason_codes.push(reason_code.to_string());
        summary.blocked_classes.push(class_id.to_string());
        insert_golden_converted(&conn, name, chunk, &summary);

        let converted = ConvertedPackage::find_publication_candidates(&conn, "fedora", None)
            .unwrap()
            .into_iter()
            .find(|converted| converted.package_name.as_deref() == Some(name))
            .unwrap_or_else(|| panic!("private converted row {name} should remain queryable"));
        assert!(!converted.is_scriptlet_public_ready());
        assert_eq!(
            ConvertedPackage::chunk_publication_state(&conn, chunk).unwrap(),
            ChunkPublicationState::NonPublicOnly
        );

        let report = match classify_converted_package(&converted) {
            PublicationDecision::Blocked(report) => report,
            other => panic!("expected blocked {class_id} report, got {other:?}"),
        };
        assert_eq!(report.publication_status, "blocked");
        assert_eq!(report.blocked_classes, vec![class_id.to_string()]);
        assert!(report.boot_security_intents.is_empty());
        assert!(report.security_policy_intents.is_empty());
    }
}
```

- [ ] **Step 4: Run the focused publication regression**

Run:

```bash
cargo test -p remi blocked_live_fetch_and_package_manager_reports_stay_private_and_non_public_only
```

Expected: PASS.

- [ ] **Step 5: Run broader conversion/publication proof**

Run:

```bash
cargo test -p conary-core blocked_classes_block_live_fetch_and_package_manager_recursion --lib
cargo test -p conary-core live_fetch_and_package_manager_helpers_remain_blocked_without_manifest_authority --lib
cargo test -p remi corpus_summary_marks_live_fetch_and_package_manager_recursion
cargo test -p remi blocked_live_fetch_and_package_manager_reports_stay_private_and_non_public_only
cargo test -p remi publication
```

Expected: PASS.

- [ ] **Step 6: Commit**

Run:

```bash
git add crates/conary-core/src/ccs/convert/converter.rs apps/remi/src/server/publication.rs
git commit -m "test: keep live fetch conversions private"
```

## Task 4: Document Network And Package-Manager Authority Boundary

**Files:**
- Modify: `docs/SCRIPTLET_SECURITY.md`
- Modify: `docs/modules/ccs.md`
- Modify: `docs/modules/remi.md`
- Modify: `docs/modules/test-fixtures.md`
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-inventory.tsv`

**Interfaces:**
- Consumes: Workstream F in `docs/superpowers/specs/2026-07-08-scriptlet-public-authority-roadmap-design.md`
- Produces: docs aligned with blocked/non-public live fetch and package-manager recursion policy

- [ ] **Step 1: Update scriptlet security docs**

In `docs/SCRIPTLET_SECURITY.md`, after the PAM paragraph in "Boot And Security Scriptlet Evidence", add:

```markdown
Live network fetches and nested package-manager calls are also blocked for
public serving. Commands such as `curl`, `wget`, `scp`, `ssh`, `git clone`,
`dnf`, `apt`, `dpkg`, `rpm`, `pacman`, `apk`, `microdnf`, and `zypper` may be
preserved as sanitized refusal evidence, but they are not native authority.
Future support must model dependency intent or curated offline artifacts; it
must not run a foreign package manager or fetch live network content during
conversion or install.
```

- [ ] **Step 2: Update CCS module docs**

In `docs/modules/ccs.md`, after the paragraph that starts `Common PAM stack helpers (` in the public-ready conversion section, add:

```markdown
Live network fetches and nested package-manager calls remain blocked conversion
evidence. A scriptlet that fetches content with `curl`, `wget`, `scp`, `ssh`, or
`git clone`, or that invokes a nested package manager such as `dnf`, `apt`,
`dpkg`, `rpm`, `pacman`, `apk`, `microdnf`, or `zypper`, does not project native
manifest authority and cannot become public-ready without a future dependency
or offline-artifact authority model.
```

- [ ] **Step 3: Update Remi module docs**

In `docs/modules/remi.md`, after the paragraph that says corpus summaries are adapter-planning evidence only, add:

```markdown
Scan-only network and package-manager hints are advisory. They help maintainers
find packages that attempted live fetch or nested package-manager recursion,
but those hints do not make a conversion `replaced` and do not bypass the
public-ready gate. Valid blocked rows remain available only through the
default-off admin test lane.
```

- [ ] **Step 4: Update fixture docs**

In `docs/modules/test-fixtures.md`, in the `scriptlet-public-authority-fixtures` list near `blocked-class-pam`, add:

```markdown
- `blocked-class-network`: live-fetch evidence such as `curl` or `git clone`,
  expected blocked outcome until dependency intent or curated offline artifact
  authority is modeled.
- `blocked-class-package-manager-recursion`: nested package-manager evidence
  such as `dnf`, `apt`, `pacman`, `apk`, or `microdnf`, expected blocked
  outcome until native dependency or artifact authority is modeled.
```

- [ ] **Step 5: Register the plan in the documentation accuracy ledger**

Append this exact row to `docs/superpowers/documentation-accuracy-audit-ledger.tsv`:

```tsv
docs/superpowers/plans/2026-07-08-network-package-recursion-authority-lock-plan.md	docs/superpowers/plans/2026-07-08-network-package-recursion-authority-lock-plan.md	planning	maintainer	scriptlet-security; network; package-manager-recursion; remi-publication-gate; implementation-plan	docs/superpowers/specs/2026-07-08-scriptlet-public-authority-roadmap-design.md; docs/SCRIPTLET_SECURITY.md; docs/modules/ccs.md; docs/modules/remi.md; docs/modules/test-fixtures.md; crates/conary-core/src/ccs/convert/blocked_classes.rs; crates/conary-core/src/ccs/convert/support_matrix.rs; crates/conary-core/src/ccs/convert/converter.rs; apps/remi/src/server/scriptlet_corpus.rs; apps/remi/src/server/publication.rs	verified	corrected	Implementation plan for locking live network fetch and nested package-manager recursion scriptlet handling: common distro package-manager aliases and git clone remain blocked/private conversion evidence until dependency intent or curated offline artifact authority is designed.
```

- [ ] **Step 6: Regenerate inventory after staging docs**

Run:

```bash
git add docs/superpowers/plans/2026-07-08-network-package-recursion-authority-lock-plan.md docs/SCRIPTLET_SECURITY.md docs/modules/ccs.md docs/modules/remi.md docs/modules/test-fixtures.md docs/superpowers/documentation-accuracy-audit-ledger.tsv
LC_ALL=C bash scripts/docs-audit-inventory.sh > docs/superpowers/documentation-accuracy-audit-inventory.tsv
git add docs/superpowers/documentation-accuracy-audit-inventory.tsv
```

- [ ] **Step 7: Run docs verification**

Run:

```bash
bash scripts/check-doc-truth.sh
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
LC_ALL=C bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -
grep -n 'docs/SCRIPTLET_SECURITY.md\|docs/modules/ccs.md\|docs/modules/remi.md\|docs/modules/test-fixtures.md' docs/superpowers/feature-coherency-ledger.tsv
bash scripts/check-coherency-ledger.sh docs/superpowers/feature-coherency-ledger.tsv
git diff --check
```

Expected: PASS.

- [ ] **Step 8: Commit**

Run:

```bash
git add docs/SCRIPTLET_SECURITY.md docs/modules/ccs.md docs/modules/remi.md docs/modules/test-fixtures.md docs/superpowers/documentation-accuracy-audit-ledger.tsv docs/superpowers/documentation-accuracy-audit-inventory.tsv
git commit -m "docs: document network recursion boundary"
```

## Task 5: Final Verification And Review

**Files:**
- No source edits expected unless verification or review finds a defect.

**Interfaces:**
- Consumes: all prior task commits.
- Produces: reviewed, verified Workstream F defensive lock slice.

- [ ] **Step 1: Run focused proof**

Run:

```bash
cargo test -p conary-core blocked_classes_block_live_fetch_and_package_manager_recursion --lib
cargo test -p conary-core live_fetch_and_package_manager_classes_remain_blocked_without_native_adapters --lib
cargo test -p conary-core live_fetch_and_package_manager_helpers_remain_blocked_without_manifest_authority --lib
cargo test -p remi corpus_summary_marks_live_fetch_and_package_manager_recursion
cargo test -p remi blocked_live_fetch_and_package_manager_reports_stay_private_and_non_public_only
```

Expected: PASS.

- [ ] **Step 2: Run interaction proof**

Run:

```bash
cargo test -p conary-core support_matrix --lib
cargo test -p conary-core golden_fixtures --lib
cargo test -p conary --test conversion_integration golden_conversion
cargo test -p remi corpus_summary
cargo test -p remi publication
```

Expected: PASS.

- [ ] **Step 3: Run broad package proof**

Run:

```bash
cargo test -p conary-core
cargo test -p remi
```

Expected: PASS.

- [ ] **Step 4: Run final lint and docs gates**

Run:

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
bash scripts/check-doc-truth.sh
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
LC_ALL=C bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -
grep -n 'docs/SCRIPTLET_SECURITY.md\|docs/modules/ccs.md\|docs/modules/remi.md\|docs/modules/test-fixtures.md' docs/superpowers/feature-coherency-ledger.tsv
bash scripts/check-coherency-ledger.sh docs/superpowers/feature-coherency-ledger.tsv
git diff --check
```

Expected: PASS.

- [ ] **Step 5: Run final slice review**

Create a review package from the plan-review base through HEAD and dispatch a read-only final reviewer. The review must verify:

- no live fetch or nested package-manager form is public-ready;
- no network/package-manager adapter, manifest projection, dependency-intent projection, offline artifact authority, replay authority, or public gate exception was added;
- git live-fetch clone forms are blocked after global-option skipping without blocking unrelated `git` commands by name;
- support matrix has blocked rows only and no Known support row for these classes;
- converter/publication/Remi surfaces keep blocked rows non-public-only;
- docs and docs-audit metadata align with Workstream F.

- [ ] **Step 6: Commit only review fixes if needed**

If the final reviewer finds Critical or Important issues, dispatch one fix subagent with the complete findings list, rerun the focused covering tests, and re-review before marking the slice complete.

---

## Plan Self-Review

- Workstream coverage: Implements Workstream F's current defensive boundary only; future dependency extraction, offline artifact requirements, repository hints, and maintainer review clusters remain advisory/future work.
- Public gate: No public-ready path is added for live fetch or package-manager recursion.
- TDD: Task 1 has real red tests for new classification coverage; Task 2 uses an in-test fake Known row so the support-matrix guard proves the intended future bug without temporary production-code injection.
- Docs: Scriptlet security, CCS, Remi, fixture docs, ledger, and inventory are included.
- Verification: Focused, interaction, broad package, Clippy, docs, coherency, and diff gates are listed.
