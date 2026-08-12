---
last_updated: 2026-08-12
revision: 2
summary: Define signed package repository intents and their atomic install, update, remove, retention, and rollback semantics
---

# Transactional Package Repository Enrollment

## Status And Boundary

Issue #382 is the fifth bounded W10 slice. It consumes the repository identity,
trust, source-policy, takeover, and owned-projection contracts established by
#377 and #379 through #381. It makes a repository declaration installed by a
package future resolution authority in the same transaction as that package.

A package payload is inspected before mutation. This slice admits RPM's exact
DNF/YUM declaration ABI and grammar. Future APT, libzypp, and ALPM enrollment
must add their own issue-backed exact ABI and grammar implementations; a URL
substring, package name, distribution name, or script-text match may not stand
in for one. Placement in `/etc/yum.repos.d/*.repo` selects DNF's documented
consumer ABI, while only the closed DNF parse supplies repository semantics.

Repository files created only by arbitrary program behavior are not discovered
by scraping the completed root. Their source ABI needs a future typed operation
or an exact process-boundary contract; command classification is never mutation
authority.

## Signed Intent

CCS v3 lifecycle authority carries a bounded, ordered set of
`PackageRepositoryEnrollmentIntent` values. Direct native installation builds
the identical values from the authenticated native payload before mutation;
foreign conversion signs those values into the resulting CCS authority.

Each intent records:

- a package-stable intent identity;
- every exact repository definition produced by one DNF declaration;
- ecosystem and native version ordering;
- repository and source identities, release/channel/rolling stream identity,
  follow-or-pin policy, endpoint, parser input, enabled state, priority,
  filters, and metadata expiry;
- role-separated repository and package trust with full pinned fingerprints;
- declaration and trust projection paths, modes, content SHA-256 values, and
  roles; and
- an explicit last-owner policy: `remove-when-unowned` or `retain`.

The intent references signed payload objects by exact guest path and content
identity. The converter does not duplicate a mutable path lookup into the
contract. Local OpenPGP trust references are admitted only when every
certificate in the referenced payload object parses and contributes its pinned
full fingerprint for the declared role. Persisted trust uses bounded embedded
key authority, so a later sync cannot fetch a different certificate from an
unbound URL. A remote key URL, missing payload object, fingerprint mismatch,
ambiguous keyring, disabled signature verification, or incomplete metadata
authority blocks enrollment before mutation.

## Transaction Plan

The planner runs while the selected-root mutation lock is held and before the
selected root, SQLite authority, package payload, lifecycle programs, or
generation candidate changes. It compares the incoming signed desired state to
the exact intents persisted for the installed package version and emits only
typed operations:

- `create` adds a previously absent repository definition and projection;
- `replace` changes one stable intent to its complete new definition;
- `remove` releases the old package's ownership and restores or removes state
  when no owner remains; and
- `retain` releases package ownership but leaves an inspectable retained owner
  with the exact last definition.

An update is not an install followed by an unrelated remove. The old and new
intent sets are compared as one operation, so a declaration/key replacement
cannot expose a half-old trust chain. Operations are sorted by stable intent,
repository identity, and projection path before hashing or applying.

Two packages may own the same repository or projection only when the complete
definition, bytes, mode, trust, and policy are identical. Removing either
owner leaves the shared authority unchanged. A conflicting second owner or an
update that would split shared authority fails preflight. Releasing any
`retain` owner creates durable retained authority regardless of removal order;
`remove-when-unowned` owners never override retained authority.

## Atomic Apply And Projection State

Package enrollment reuses the selected-root session and its single SQLite
transaction. Payload application first materializes the exact declaration and
key objects in the isolated selected root. Enrollment then verifies those
objects against the preflighted identities, writes repository/source-policy,
owner, and projection rows, and only then allows the changeset and generation
candidate to become applied.

Any parse, trust, projection, database, lifecycle, candidate-persistence, or
commit failure rolls back both the selected root and SQLite. Repository sync is
never part of this mutation transaction; it starts only after the committed
repository and trust authority can be reopened exactly.

Native takeover projections remain a distinct immutable authority. A package
intent that names an already takeover-owned path fails preflight before root or
database mutation; it cannot silently advance or strand takeover state. A
package-created projection absent from takeover authority follows its package
owners and is deleted only after the final remove-when-unowned owner leaves.

Generic repository mutation remains forbidden for package-owned rows. A
definition change must pass through a package enrollment transaction;
authenticated snapshot and last-sync observations remain the only independent
mutable repository fields.

## Remove, Retention, And Rollback

Package removal loads its persisted signed intent rather than parsing current
files. Shared owners are released without changing the common authority. The
last `remove-when-unowned` owner deletes only package-owned authority no longer
owned; enrollment rejects collisions with existing native or operator-owned
authority rather than overwriting and later restoring it. The last `retain`
owner becomes a durable retained record whose source package identity and
intent digest remain inspectable; it is not silently converted to operator
authority.

Every install, update, and remove changeset captures normalized package
repository owners, retained owners, repository definitions, source policies,
and projection state in rollback system authority before mutation. System
rollback restores that exact database state in the same transaction as package
and lifecycle authority, while the captured selected-root manifest restores
the exact files. A rollback never reparses a current projection or reconstructs
an old intent from a newer package.

## Later Update Proof

The end-to-end acceptance fixture is a Chrome-shaped signed RPM: version 1
installs an RPM repository declaration plus role-separated signing roots. The
install transaction enrolls it, an authenticated sync imports version 2, and
normal exact repository resolution selects version 2 with installable source
profile provenance. The fixture
then proves update replacement, remove-when-unowned, explicit retention,
multiple identical owners, trust failure, projection failure, and rollback.

The fixture name is descriptive only. No product or vendor name enters runtime
selection.

## Schema Hard Cut And Proof

Schema revision 33 adds package/retained enrollment ownership and exact
projection-owner bindings. Revision 32 is retired
pre-alpha state. Recovery is `conary system rebuild-db --discard-state --yes`
followed by takeover and package reinstallation from authoritative inputs; no
migration or legacy reader is retained.

Focused proof covers signed-intent round trips and budgets, exact grammar and
trust derivation, deterministic create/replace/remove planning, identical and
conflicting multiple owners, retention, install/update/remove/rollback,
failure-before-mutation, and authenticated sync plus later exact update. The
install and CCS feature-card suites, workspace Clippy, formatting, schema
rebuild checks, and documentation truth gate remain required.
