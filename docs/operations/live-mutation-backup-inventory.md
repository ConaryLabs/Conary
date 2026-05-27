---
last_updated: 2026-05-26
revision: 1
summary: First-wave live-mutation backup coverage and release-scope decisions for Conary
---

# Live-Mutation Backup Inventory

This inventory is the release-hardening boundary for the limited preview. It
maps commands that can mutate the active host or live Conary database to their
current database-checkpoint coverage and first-wave documentation scope.

The first-wave public path should stay centered on adoption, refresh,
unadoption, native handoff, and the explicit DB backup recovery commands. Other
live mutation surfaces remain available for development and VM testing, but
they should not be presented as daily-driver recovery-covered paths until their
own checkpoint or generation-bound backup work lands.

## Covered Before First Tester Post

These paths write pre-mutation and post-success SQLite checkpoint backups via
`conary_core::db::backup`:

| Surface | Risk class | Coverage |
| --- | --- | --- |
| `conary system adopt --system` | `DbMutation` | Pre checkpoint before adoption tracking transaction; post checkpoint after transaction and state snapshot. |
| `conary system adopt <pkg>` | `DbMutation` | Pre checkpoint before per-package adoption transaction; post checkpoint after transaction and state snapshot. |
| `conary system adopt --refresh` | `DbMutation` | Pre checkpoint before refresh transaction; post checkpoint after transaction and any state snapshot. |
| `conary system adopt --refresh --quiet --from-sync-hook` | `HookRefreshDbMutation` | Uses the same refresh checkpoint path; still reserved for installed native package-manager hooks. |
| `conary system adopt --convert` | `DbMutation` | Pre checkpoint before adopted-source backfill/conversion DB writes; post checkpoint after conversion transaction and any state snapshot. |
| `conary system unadopt` | `ActiveHostMutation` | Pre checkpoint before removing adopted tracking rows; post checkpoint after transaction and state snapshot. |
| `conary system native-handoff` | `ActiveHostMutation` | Pre checkpoint before removing adopted tracking rows; post checkpoint after transaction and state snapshot. Current-link handoff record remains the host-side recovery artifact. |
| `conary system db-backup list` | `ReadOnly` | Lists checkpoint manifests without opening or migrating the live DB. |
| `conary system db-backup verify --latest` | `ReadOnly` | Verifies the latest checkpoint by checksum, SQLite `integrity_check`, and Conary schema version. |
| `conary system db-backup recover --latest --dry-run` | `ReadOnly` | Verifies the selected checkpoint without requiring a healthy live DB. |
| `conary --allow-live-system-mutation system db-backup recover --latest --yes` | `ActiveHostMutation` | Restores a missing/corrupt live DB from the latest verified checkpoint and quarantines existing DB/WAL/SHM sidecars. |

## Excluded From First-Wave Public Docs

These paths are intentionally not first-wave quickstart material. They either
need their own DB checkpoint coverage, generation-bound DB backup work, or a
clear VM-only story before they become daily-driver guidance.

| Surface | Risk class | First-wave decision |
| --- | --- | --- |
| `conary install` and `conary install @collection` | `ActiveHostMutation` | VM-only/follow-up until package mutation and generation publication backups are covered end to end. |
| `conary remove` | `ActiveHostMutation` | VM-only/follow-up for the same package-mutation backup gap. |
| `conary update` and `conary update @collection` | `ActiveHostMutation` | VM-only/follow-up until update/package mutation recovery is covered. |
| `conary autoremove` | `ActiveHostMutation` | VM-only/follow-up until package removal recovery is covered. |
| `conary ccs install` | `ActiveHostMutation` | VM-only/follow-up; not part of the adoption escape-hatch story. |
| `conary model apply` | `ActiveHostMutation` | VM-only/follow-up until model apply has the same recovery evidence as package mutation. |
| `conary automation apply` | `ActiveHostMutation` | VM-only/follow-up; automation should not be in the limited-preview first-run path. |
| `conary system restore` | `ActiveHostMutation` | Follow-up before widened beta; touches live files and needs separate recovery evidence. |
| `conary system state revert` | `ActiveHostMutation` | VM-only/follow-up until state restore has DB backup and live-file recovery evidence. |
| `conary system state rollback` | `ActiveHostMutation` | VM-only/follow-up until rollback has backup coverage and live-root evidence. |
| `conary self-update` | `ActiveHostMutation` | Excluded from first-wave docs; binary update recovery belongs to release tooling. |

## Generation And Takeover Follow-Up

These paths are `AlwaysLive` and remain outside the adoption-only DB checkpoint
slice. They should move forward with generation-bound SQLite-native backups and
artifact recovery checks before a widened beta:

| Surface | First-wave decision |
| --- | --- |
| `conary system generation build` | Follow-up before generation switching becomes a headline path. |
| `conary system generation publish` | Follow-up; needs generation-bound DB backup metadata next to the generation artifact. |
| `conary system generation switch` | VM-only/debug until generation recovery and DB backups are paired. |
| `conary system generation rollback` | VM-only/debug until generation recovery and DB backups are paired. |
| `conary system generation gc` | Follow-up; recovery must verify backup generation artifacts were not garbage-collected. |
| `conary system generation recover` | Follow-up; should grow copied-backup dry-run and apply paths. |
| `conary system takeover` | VM-only/follow-up; takeover crosses adoption, native ownership, generation publication, and boot-entry state. |

## conaryd Package Jobs

`conaryd` package install/remove/update routes queue durable daemon jobs that
reuse CLI package mutation contracts. They are not part of the first-wave
adoption quickstart. Treat them as follow-up before widened beta until daemon
jobs have the same package-mutation DB backup and support-bundle evidence as
the CLI package paths.
