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
