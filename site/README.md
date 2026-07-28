# conary.io (Main Site)

This is the marketing/landing site for Conary, served at **conary.io**.

**Deploy target:** `/conary/site` on the Remi server

This shares the same Remi host as `remi.conary.io`, but it is deployed as a
separate static site root rather than through Remi's `[web]` frontend mount.

```bash
# Build and deploy
../deploy/deploy-sites.sh site
```

This is NOT the packages site. For remi.conary.io, see `../web/`.

## Brand tokens and shared assets

The brand palette, font stacks, shared geometry, and the byte-identical logo and
favicon files are owned by [`../shared/brand/`](../shared/brand/README.md), not
by this tree. `scripts/sync-brand.sh` materializes them here as `predev`,
`precheck`, and `prebuild`, so `npm run dev`, `npm run check`, and
`npm run build` all pick up a change automatically.

Generated paths (`src/brand.generated.css` and the mirrored `static/` files) are
gitignored. To change a brand fact, edit `shared/brand/`; do not edit the
generated copies.
