## Feature 0: Repo Presentation -- Review Findings

### Summary

The repository presents well for a first-time visitor: clean workspace layout, comprehensive README with honest gap analysis, well-organized CI across GitHub Actions and Forgejo, and a polished .claude/ configuration that demonstrates serious AI-assisted development. The biggest concern is a **hardcoded authentication token in a tracked service file** (P0 security). Beyond that, the primary presentation risk is stale numbers throughout the README and CLAUDE.md -- version badges, line counts, schema versions, and test counts that have drifted from reality. There are 1 P0, 5 P1, 10 P2, and 5 P3 findings.

---

### P0 -- Critical

#### 1. Hardcoded authentication token in tracked service file
- **File:** `deploy/conary-test.service:9,11`
- **Category:** Security
- **Finding:** The conary-test systemd service file contains a hardcoded bearer token (`d7975d291a28c87499c7e74e21a67d50a019edf61cb779056fd06e49ce6b9e43`) both as an environment variable and on the command line. This file is tracked in git and will be public when posted to r/claudecode.
- **Fix:** Move the token to an `EnvironmentFile=` directive pointing to an untracked path (e.g., `/etc/conary-test/env`), gitignore the real credentials, and ship only an example file. The token in git history should be rotated immediately after the repo goes public.

---

### P1 -- Important

#### 2. README version badge says v0.6.0 but Cargo.toml is v0.7.0
- **File:** `README.md:5`
- **Category:** Correctness
- **Finding:** The version badge `[![v0.6.0](...)]` and the "Version 0.6.0" in the Project Status section (line 528) do not match the actual crate version of 0.7.0 in `Cargo.toml`. First thing a visitor will notice.
- **Fix:** Update badge to `v0.7.0` and update the Project Status paragraph to say "Version 0.7.0".

#### 3. README claims "schema v56" but actual schema is v57
- **File:** `README.md:58,528`
- **Category:** Correctness
- **Finding:** Two places in the README say "schema v56" but `SCHEMA_VERSION` in `conary-core/src/db/schema.rs` is 57. CLAUDE.md correctly says v57.
- **Fix:** Update both instances in README to "v57".

#### 4. README line count is stale ("174K+" but codebase is 211K)
- **File:** `README.md:9,58`
- **Category:** Correctness
- **Finding:** The README claims "174K+ lines of Rust" in two places but the actual line count is 211K (or ~158K non-blank non-comment). Either way, 174K is a significant undercount that undersells the project. The CLAUDE.md does not make a line count claim.
- **Fix:** Update to "211K+ lines of Rust" (total) or "158K+ lines of non-comment Rust" -- pick one metric and be consistent.

#### 5. CLAUDE.md says "~269 unit tests" but actual count is 2,646+
- **File:** `CLAUDE.md:8`
- **Category:** Correctness
- **Finding:** The CLAUDE.md build section says `cargo test # ~269 unit tests` but `cargo test --list` across conary + conary-core + conary-server finds 2,646 test functions. The README correctly says "2,500+ unit tests". This makes the CLAUDE.md look like it was written months ago and never updated.
- **Fix:** Update to `# ~2,600 unit tests (278 integration tests via conary-test)`.

#### 6. release.sh has dead variable RELEASE_RELEASE_GROUPS
- **File:** `scripts/release.sh:21`
- **Category:** Quality
- **Finding:** Line 21 initializes `RELEASE_RELEASE_GROUPS=()` (with doubled prefix) but the actual loop on line 26 appends to `RELEASE_GROUPS` and line 31 checks `RELEASE_GROUPS`. The `RELEASE_RELEASE_GROUPS` variable is never used. This is harmless (bash creates the array on first `+=`) but looks like an AI copy-paste error to anyone reading the script.
- **Fix:** Change line 21 from `RELEASE_RELEASE_GROUPS=()` to `RELEASE_GROUPS=()`.

---

### P2 -- Improvement

#### 7. infrastructure.md references nonexistent `erofs` release group
- **File:** `.claude/rules/infrastructure.md:62`
- **Category:** Correctness
- **Finding:** The Scripts table lists `scripts/release.sh [conary|erofs|server|test|all]` but `release.sh` only accepts `conary|server|test|all`. The `erofs` group was presumably removed when conary-erofs was folded into conary-core but the doc was not updated.
- **Fix:** Remove `erofs|` from the infrastructure.md table entry.

#### 8. architecture.md says "6-phase pipeline" for bootstrap; CLAUDE.md says "8-stage"
- **File:** `.claude/rules/architecture.md:45`
- **Category:** Correctness
- **Finding:** The architecture.md table describes bootstrap as a "6-phase pipeline" while the README (line 230) describes Stage 0 through Image as a multi-stage pipeline. The terminology is inconsistent and could confuse contributors looking at the rules files.
- **Fix:** Align to the README's description (which matches the CLI subcommands). If the bootstrap module uses "stages" internally, document the mapping.

#### 9. Three RUSTSEC advisories suppressed without explanation
- **File:** `.github/workflows/ci.yml:78-81`
- **Category:** Security
- **Finding:** The security audit job ignores RUSTSEC-2024-0447, RUSTSEC-2023-0071, and RUSTSEC-2025-0136 with no inline comments explaining why. A security-conscious visitor will flag this immediately. Even if the advisories do not apply, the lack of explanation looks like carelessness.
- **Fix:** Add a comment above each `--ignore` line explaining which crate is affected and why it is safe to suppress (e.g., "sequoia-openpgp: we use crypto-rust backend, not the affected code path").

#### 10. README Building section says "~260 unit tests"
- **File:** `README.md:513`
- **Category:** Correctness
- **Finding:** A third test count reference says "~260 unit tests" which is the oldest stale number. There are now three different test count claims in the repo: ~260, ~269, and 2,500+.
- **Fix:** Consolidate to one accurate number across all files.

#### 11. conary-test Cargo.toml missing authors and license fields
- **File:** `conary-test/Cargo.toml:1-7`
- **Category:** Quality
- **Finding:** The conary-test crate has no `authors` or `license` field, unlike the other three crates which all declare both. While this crate is not published to crates.io, the inconsistency looks sloppy in a showcase repo.
- **Fix:** Add `authors = ["Conary Contributors"]` and `license = "MIT OR Apache-2.0"` to match the other crates.

#### 12. README comparison table claims some features that are aspirational
- **File:** `README.md:62-84`
- **Category:** Slop
- **Finding:** The comparison table marks "Dev shells: Yes" and "Hermetic builds: Yes" for Conary but these features are behind `<details>` sections and the Project Status paragraph says the system is under active development. A skeptical reader will check the status section, see "limited production testing", and question the table. This is not dishonest but warrants more nuance.
- **Fix:** Consider adding "(alpha)" or "(experimental)" annotations to features that have not seen production use, or add a footnote. The honest-gap paragraph at the bottom of the table is a good start but the table itself could be more precise.

#### 13. GitHub Actions CI caches are suboptimal
- **File:** `.github/workflows/ci.yml:28-41`
- **Category:** Quality
- **Finding:** The CI workflow uses three separate cache entries (registry, git index, target dir) all keyed only on `Cargo.lock`. The target cache has no restore-keys fallback and will miss on any dependency change, rebuilding from scratch. This is not a bug but a CI best practice gap that will be noticed.
- **Fix:** Use a single cache action with `path: |` combining all three dirs, and add `restore-keys: ${{ runner.os }}-cargo-` for partial cache hits.

#### 14. E2E workflow Phase 3 does not build `conary` binary
- **File:** `.forgejo/workflows/e2e.yaml:81`
- **Category:** Correctness
- **Finding:** The `e2e-phase3` job builds `conary-test` but does not build the `conary` binary (unlike Phase 1 and Phase 2 jobs which both run `cargo build` first). If any Phase 3 test needs the conary binary in a container, it will fail or use a stale binary.
- **Fix:** Add `cargo build` step before `cargo build -p conary-test` in the Phase 3 job, or add a comment explaining why it is not needed.

---

### P3 -- Nitpick

#### 15. CLAUDE.md references "69 tables" but verification is difficult
- **File:** `CLAUDE.md:62`
- **Category:** Correctness
- **Finding:** The claim of "69 tables" is hard to verify from the source code because tables are created across multiple migration files and model modules. The `CREATE TABLE IF NOT EXISTS` count in migration files alone is 70, plus additional tables in model files. The number is approximately correct but cannot be verified without running the migrations.
- **Fix:** Either maintain a table list or remove the specific count in favor of "extensive schema".

#### 16. CLAUDE.md "Tool Selection" section is sparse
- **File:** `CLAUDE.md:64-69`
- **Category:** Quality
- **Finding:** This section mentions Context7 and Grep/Glob but reads as Claude-specific tooling instructions that would confuse human contributors. Since this is a showcase repo, visitors will read CLAUDE.md closely.
- **Fix:** Either label this section clearly as "Claude Code Instructions" or expand it with a brief note about what these tools are.

#### 17. deploy/FORGE.md workflow table says "37-test suite" -- stale number
- **File:** `deploy/FORGE.md:36`
- **Category:** Correctness
- **Finding:** The integration workflow description says "37-test suite" but Phase 1 alone covers T01-T37, and the full suite is 278 tests.
- **Fix:** Update to match current test count.

#### 18. README "What's Next" section is generic
- **File:** `README.md:536-539`
- **Category:** Slop
- **Finding:** The four bullet points ("Shell integration", "Composable systems", "Federation peer discovery", "P2P chunk distribution") read like AI-generated filler. They are vague and don't link to the ROADMAP.md where presumably more detail exists. For an r/claudecode showcase, this is where people will look for proof of project direction.
- **Fix:** Either link each item to a specific roadmap section or expand with one sentence each about what is planned.

#### 19. packaging/dracut duplicated across deploy/ and packaging/
- **File:** `deploy/dracut/`, `packaging/dracut/`
- **Category:** Architecture
- **Finding:** There are two dracut directories -- `deploy/dracut/` (module-setup.sh + mount-conary.sh) and `packaging/dracut/` (90conary). The split is not obvious and could confuse contributors.
- **Fix:** Consolidate into one location or document the split (deploy/ = install scripts, packaging/ = packaged module).

---

### Cross-Domain Notes

- **[Feature 8]** The `conary-server/Cargo.toml` lists `flume = "0.11"` with comment "mdns-sd 0.18 no longer uses flume; kept for other server uses" -- verify this dep is actually used elsewhere or remove it.
- **[Feature 1]** The `conary-test` crate currently fails to compile (`cargo test --workspace` fails with 10 errors in conary-test). This is a build breakage that should be fixed before public posting.

---

### Strengths

1. **CLAUDE.md is excellent.** At 103 lines it is concise, practical, and reads like real project instructions written by someone who uses Claude Code daily -- not AI boilerplate. The core principles, commit convention, agent table, and MCP server documentation are exactly what a visitor to r/claudecode wants to see.

2. **The .claude/ directory is a showcase in itself.** Six named agents with specific roles, pre/post hooks (sensitive file blocking, auto-clippy), 19 path-scoped rules files, persistent agent memory with durability criteria -- this demonstrates deep familiarity with Claude Code's capabilities.

3. **CI/CD is genuinely professional.** The dual CI setup (GitHub Actions for public builds/releases, Forgejo for integration/E2E) with a verification step where Forgejo confirms the GitHub release landed is a nice touch. The release pipeline with 4 parallel package builds + automated Remi deployment is production-grade.

4. **Packaging is thorough.** RPM spec, PKGBUILD, DEB with debian/, CCS manifest -- all with vendored deps, shell completions, man pages, and post-install initialization. Each has both local and container build modes. This demonstrates the project is not just source code.

5. **The README comparison table is honest.** The "Mature ecosystem: No (early)" row and the "honest gap" paragraph show self-awareness that distinguishes this from vaporware.

6. **MCP integration is well-factored.** Shared helpers in `conary-core/src/mcp/mod.rs` (path validation, error mapping, server info) consumed by both server and test MCP implementations. Clean separation with tests in the same file.

---

### Recommendations

1. **Fix the token leak (P0) before posting.** Rotate `d7975d291a28c87499c7e74e21a67d50a019edf61cb779056fd06e49ce6b9e43` on Forge, convert `deploy/conary-test.service` to use `EnvironmentFile=`, and add the real env file to `.gitignore`.

2. **Do a single pass to fix stale numbers.** Version badge (0.6.0 -> 0.7.0), schema version (v56 -> v57), line counts (174K -> 211K), test counts (~260/~269 -> ~2,600), and the FORGE.md test count. This is a 15-minute fix that eliminates the most obvious "nobody checked this" signal.

3. **Fix the conary-test build breakage.** The crate does not compile, which means `cargo test --workspace` fails. A showcase repo should have a green `cargo test` on a fresh clone.

---

### Assessment

**Ready to merge?** No -- with fixes

**Reasoning:** The hardcoded token in a tracked file is a blocker that must be fixed before any public posting. The stale version numbers across README, CLAUDE.md, and FORGE.md are the primary presentation risk -- they undermine trust in a project that is otherwise genuinely impressive. Fix those two categories and this repo will present very well on r/claudecode.

---

### Work Breakdown

1. **[P0] Rotate token and fix conary-test.service** -- Move token to EnvironmentFile, gitignore real env, rotate on Forge
2. **[P1] Update all stale version/count references** -- README badge (v0.7.0), schema version (v57), line counts (211K), test counts (~2,600), Project Status section
3. **[P1] Fix release.sh dead variable** -- `RELEASE_RELEASE_GROUPS` -> `RELEASE_GROUPS`
4. **[P2] Fix infrastructure.md erofs reference** -- Remove `erofs|` from release.sh docs
5. **[P2] Add RUSTSEC suppression comments** -- Explain each --ignore in ci.yml
6. **[P2] Add missing Cargo.toml fields to conary-test** -- authors, license
7. **[P2] Fix E2E Phase 3 missing conary build step** -- Add `cargo build` to e2e-phase3
8. **[P2] Improve CI caching** -- Single cache action with restore-keys
9. **[Cross-domain] Fix conary-test compilation errors** -- Build breakage blocks cargo test --workspace
