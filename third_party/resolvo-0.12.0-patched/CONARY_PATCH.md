# Conary resolvo patch

This directory vendors crates.io `resolvo` 0.12.0 under its BSD-3-Clause
license. Conary carries one diagnostic-only change in `src/conflict.rs`:

- retain only conflict-graph nodes reachable from the synthetic request root
  before rendering an unsatisfiable result.

Resolvo's solver can retain a learned clause for a candidate rejected before
the final root conflict. The clause remains valid SAT evidence, but its node is
not necessarily connected to the request's causal diagnostic graph. Upstream
0.12.0 asserts that every collected node is root-reachable and panics while
formatting such a valid unsatisfiable result. The patch filters those unrelated
nodes instead, preserving the root proof and returning a normal conflict.

`conflict_graph_discards_learned_branches_unreachable_from_root` is the focused
regression test. Remove this vendor patch when an equivalent upstream release
is adopted and that test passes against it.
