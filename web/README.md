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
