# Design: Public Documentation Feature Audit Update

*2026-03-05*

## Problem

A codebase audit found 15 major features that are implemented and CLI-ready but absent or barely mentioned in public-facing documentation. The most significant gap: system generations (EROFS + composefs + live system switching) is listed under "What's Next" in the README despite being fully implemented with CLI commands.

## Approach

Top-down rewrite (Approach A). Restructure the README and site narrative around the generation/composefs story as the headline, then layer in all other underdocumented features. The framing shifts from "cross-distribution package manager" to "cross-distribution system manager."

## Constraints

- Only document features with working CLI commands (no pre-CLI infrastructure like automation engine)
- No Rust code changes
- No changes to web/ (packages.conary.io)
- Site must build cleanly and deploy via `./deploy/deploy-sites.sh site`

## Design

### README.md -- Major Restructure

New structure:

1. **Hero** -- Updated tagline: system manager, not just package manager
2. **Why Conary** -- 5 pillars:
   - Atomic generations (NEW -- EROFS/composefs, live system switching)
   - Atomic transactions (existing)
   - Format-agnostic (existing)
   - Declarative state (existing)
   - 68K+ packages on day one (existing)
3. **Comparison table** -- Add rows: immutable generations, system takeover, hermetic builds, bootstrap from scratch
4. **Quick Start** -- Add generation commands after basic install flow
5. **Features** -- Reorganized:
   - Top-level (not collapsed): System Generations, System Takeover, Atomic Transactions, Multi-Format, Declarative Model, CAS, Dependency Resolution, Component Model, Bootstrap, Derived Packages
   - Collapsed: CCS Format, Recipe System, Dev Shells, Collections, Labels, Scriptlets, Capabilities, Config Management, Provenance/DNA, Triggers
6. **"What's Next"** -- Remove completed items, replace with actual future work

### Site Updates

**Home page** -- Updated hero, new feature cards for generations/takeover/bootstrap.

**New /features page** -- Deep-dive covering all CLI-ready features in 4 categories:
- System Management (generations, takeover, bootstrap, snapshots)
- Package Management (multi-format, resolver, components, derived, config)
- Build & Distribution (CCS, recipes, hermetic, OCI, dev shells)
- Infrastructure (CAS, federation, provenance, triggers, capabilities, scriptlets)

Each feature: description + CLI examples + requirements.

**Compare page** -- New comparison table rows for generations, takeover, bootstrap, derived packages.

**About page** -- Updated architecture grid with Generations/EROFS and Bootstrap.

**Nav** -- Add "Features" link in layout.

### RELEASE_NOTES.md

Add sections: System Generations, System Takeover, Bootstrap, Derived Packages, Config Management. Expand triggers and provenance. Update "What's Next."

### docs/ARCHITECTURE.md

Add: System Generations architecture (EROFS + composefs lifecycle), conary-erofs crate, Bootstrap pipeline (Stage 0-3), Derived Packages concept.

### ROADMAP.md

Move completed items from planned to complete. Add automation engine as "implemented, CLI pending."

## Files Changed

| File | Action |
|------|--------|
| README.md | Major restructure |
| RELEASE_NOTES.md | Add missing feature sections |
| docs/ARCHITECTURE.md | Add generations/EROFS/bootstrap |
| ROADMAP.md | Move completed items |
| site/src/routes/+page.svelte | Updated hero + feature cards |
| site/src/routes/about/+page.svelte | Updated architecture grid |
| site/src/routes/compare/+page.svelte | New comparison rows |
| site/src/routes/features/+page.svelte | NEW -- deep-dive page |
| site/src/routes/+layout.svelte | Add Features nav link |

## Success Criteria

- Every CLI-ready feature documented in at least one public-facing file
- System generations is the lead story
- "What's Next" sections contain only actual future work
- Site builds and deploys cleanly
