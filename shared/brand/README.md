# shared/brand

Single owner of the facts that must be identical on **conary.io** (`site/`) and
**remi.conary.io** (`web/`).

```
shared/brand/
  tokens.css              brand palette, font stacks, shared geometry
  static/
    favicon.svg           -> <app>/static/favicon.svg
    favicon-32.png        -> <app>/static/favicon-32.png
    favicon.ico           -> <app>/static/favicon.ico
    apple-touch-icon.png  -> <app>/static/apple-touch-icon.png
    brand/
      conary-mark.svg     -> <app>/static/brand/conary-mark.svg
      conary-social.svg   -> <app>/static/brand/conary-social.svg
      conary-social.png   -> <app>/static/brand/conary-social.png
```

## Changing a brand fact

Edit the file here. That is the whole procedure — `scripts/sync-brand.sh` runs
as `predev`, `precheck`, and `prebuild` in both apps, so the next `npm run dev`
or `npm run build` in either tree picks the change up.

To materialize by hand, or to verify a tree matches this directory:

```bash
bash scripts/sync-brand.sh           # write the generated files
bash scripts/sync-brand.sh --check   # verify only, non-zero on drift
```

Generated paths (`<app>/src/brand.generated.css`, the mirrored `static/` files)
are gitignored. This directory is the only tracked copy. If you find yourself
editing a generated file, the change belongs here instead.

## What does not belong here

Only put something here when both frontends must agree on it. Everything below
is deliberately per-app, because one site can change it without the other
following:

| Concern | Owner |
| --- | --- |
| Base reset, `.container`, focus rings | each app's `src/app.css` |
| Display scale (`--step-*`, `--track-*`) | `site/src/app.css` |
| Semantic aliases, shadows, distro colors | `web/src/app.css` |
| `PageMeta.svelte`, `app.html`, page titles | each app |
| Build adapter and prerender contract | each app's `svelte.config.js` |

The two frontends stay separate builds on purpose: `site/` is fully prerendered
and `site/scripts/verify-build.mjs` asserts its static pages exist, while `web/`
ships an SPA fallback because `packages/[distro]/[name]` is an unbounded
namespace resolved against the live Remi API. Sharing brand tokens does not
change that boundary.
