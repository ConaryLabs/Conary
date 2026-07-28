# remi.conary.io (Package Index)

This is the package browsing/search frontend for the Remi server, served at **remi.conary.io**.

**Deploy target:** `/conary/web` on the Remi server

This shares the same Remi host as `conary.io`, but it is kept as a separate
build and deploy target. In tracked config, Remi serves this frontend via its
`[web]` root.

```bash
# Build and deploy (historical `packages` subcommand; deploys remi.conary.io)
../deploy/deploy-sites.sh packages
```

This is NOT the main site. For conary.io, see `../site/`.

## Brand tokens and shared assets

The brand palette, font stacks, shared geometry, and the byte-identical logo and
favicon files are owned by [`../shared/brand/`](../shared/brand/README.md), not
by this tree. `scripts/sync-brand.sh` materializes them here as `predev`,
`precheck`, and `prebuild`, so `npm run dev`, `npm run check`, and
`npm run build` all pick up a change automatically.

Generated paths (`src/brand.generated.css` and the mirrored `static/` files) are
gitignored. To change a brand fact, edit `shared/brand/`; do not edit the
generated copies.
