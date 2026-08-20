# Third-Party Divergence Inventory

This file is the single inventory for Cargo dependencies whose source differs
from their published upstream. The TOML block is both the human audit record
and the input to `scripts/check-third-party-divergence.py`; do not maintain a
second list.

The structural check runs in documentation truth plus every pull-request and
post-merge dependency-consistency job. The upstream exit check runs in the
scheduled security audit every six hours. When an upstream exit condition is
met, that audit fails and the owning dependency must be tested and either
returned to upstream authority or deliberately re-pinned with this record
updated.

<!-- conary-third-party-divergence:start -->
```toml
schema = 1
cadence = "Every six hours in the scheduled-ops audit job"
owner = "Cargo.toml dependency reviewers and the scheduled-ops audit job"

[[dependency]]
id = "aws-creds-quick-xml"
cargo_name = "aws-creds"
kind = "crates-io-patch"
declaration = "Cargo.toml:[patch.crates-io].aws-creds"
path = "third_party/aws-creds-0.39.1-patched"
baseline = "0.39.1"
upstream = "https://crates.io/crates/aws-creds"
upstream_index = "aw/s-/aws-creds"
divergence = "Raises the normal quick-xml dependency from 0.38 to 0.41; the remaining crate source is the crates.io 0.39.1 release."
reason = "Avoid shipping quick-xml releases covered by RUSTSEC-2026-0194 and RUSTSEC-2026-0195."
exit_type = "crates-io-dependency-floor"
exit_dependency = "quick-xml"
exit_requirement = ">=0.41"
exit_condition = "Drop the patch when a newer non-yanked aws-creds release requires quick-xml >=0.41 and the workspace resolves and passes its normal gates without the override."
exit_test = "cargo test --workspace --exclude conary-test"

[[dependency]]
id = "rust-s3-quick-xml"
cargo_name = "rust-s3"
kind = "crates-io-patch"
declaration = "Cargo.toml:[patch.crates-io].rust-s3"
path = "third_party/rust-s3-0.37.2-patched"
baseline = "0.37.2"
upstream = "https://crates.io/crates/rust-s3"
upstream_index = "ru/st/rust-s3"
divergence = "Raises the normal quick-xml dependency from 0.38 to 0.41; the remaining crate source is the crates.io 0.37.2 release."
reason = "Avoid shipping quick-xml releases covered by RUSTSEC-2026-0194 and RUSTSEC-2026-0195."
exit_type = "crates-io-dependency-floor"
exit_dependency = "quick-xml"
exit_requirement = ">=0.41"
exit_condition = "Drop the patch when a newer non-yanked rust-s3 release requires quick-xml >=0.41 and the workspace resolves and passes its normal gates without the override."
exit_test = "cargo test --workspace --exclude conary-test"

[[dependency]]
id = "resolvo-conflict-graph"
cargo_name = "resolvo"
kind = "crates-io-patch"
declaration = "Cargo.toml:[patch.crates-io].resolvo"
path = "third_party/resolvo-0.12.0-patched"
baseline = "0.12.0"
upstream = "https://crates.io/crates/resolvo"
upstream_index = "re/so/resolvo"
divergence = "Filters conflict-graph nodes that are unreachable from the synthetic request root before rendering an unsatisfiable result."
reason = "Upstream 0.12.0 can panic while formatting valid unsatisfiable solver evidence containing a learned but causally unrelated branch."
exit_type = "crates-io-newer-release"
exit_condition = "Evaluate every newer non-yanked resolvo release; drop the patch only when the focused unreachable-branch regression passes against that release."
exit_test = "cargo test --manifest-path third_party/resolvo-0.12.0-patched/Cargo.toml conflict_graph_discards_learned_branches_unreachable_from_root"

[[dependency]]
id = "rpm-rs-builder-authority"
cargo_name = "rpm"
kind = "git-dependency"
declaration = "Cargo.toml:[workspace.dependencies].rpm"
git = "https://github.com/ConaryLabs/rpm-rs"
rev = "1d34711495afe021993707ad64fb60fa8dc34881"
upstream = "https://github.com/rpm-rs/rpm-rs"
upstream_repo = "rpm-rs/rpm-rs"
upstream_ref = "master"
upstream_base = "283563a92383dd63cf92fe1d261d42ecdf71ff12"
divergence = "Two commits add explicit package-local hardlink sets and canonical root-child directory headers/payload paths; the fork differs in builder code and tests, not RPM signature verification."
reason = "Conary's native export must preserve hardlink identity and canonical root-directory payload semantics when authoring RPMs."
exit_type = "upstream-git-integration"
exit_condition = "Prefer upstreaming both builder changes. Drop the fork when upstream contains equivalent behavior, then pin a released or reviewed upstream revision and pass the focused native-export and daily-driver corpus tests."
exit_test = "cargo test -p conary-core ccs::native_export::rpm && cargo test -p conary-test native_corpus"
```
<!-- conary-third-party-divergence:end -->

The vendored crates retain their upstream license metadata and notices. The
resolvo patch has additional implementation context in
[`resolvo-0.12.0-patched/CONARY_PATCH.md`](resolvo-0.12.0-patched/CONARY_PATCH.md).
The rpm-rs fork is pinned by commit because it parses and authors package data
at a trust boundary; changing that revision requires updating the recorded
base and re-running the named exit tests.
