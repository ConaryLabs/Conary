---
last_updated: 2026-08-12
revision: 1
summary: Pin the eopkg source ABI and map exact Solus package, repository, lifecycle, adoption, and target capability authority
---

# eopkg source ABI and Solus conformance record

This record pins the source authority used for Conary's eopkg implementation.
It is a source-package ABI, not a distribution-name allowlist. Solus Polaris is
the first conformance target for the ABI.

## Pinned authority

- eopkg source: `getsolus/eopkg` commit
  `59e423ea39088cd46f1903b1094127d8fda28d73`
- Solus recipe source: `getsolus/packages` commit
  `be7024b982ec2d6b95410ac22b0ec45b9c933a77`
- composefs target helper source: `containers/composefs` commit
  `ec2573a0f68f548ae91f3e10acc990a08f9122dc` (upstream 1.0.8)
- binary repository index:
  `https://cdn.getsol.us/repo/polaris/eopkg-index.xml.xz`
- authenticated index snapshot observed during implementation: compressed
  SHA-256
  `efa19936d28e2e84b462cf2e07efb9cf1b3afb983b0c7b5a5f813d2d52a8061f`,
  11,907 package records
- representative package: `jq-1.8.2-13-1-x86_64.eopkg`, index SHA-1
  `98591d2acce52d110a1ae62b775079d79193ad01`, artifact SHA-256
  `33b3d0e36fdeacfc05ea085ea1da87e198bc5aa8e174242e8cb54b6c3100731b`

The GitHub `packages` repository owns recipes, patches, source inputs, and
build provenance. It does not own installable binary identity. The Polaris
index and the exact artifacts it names own binary resolution and installation.
Conary therefore never treats a recipe checkout or its directory names as
repository package authority.

## Typed ownership map

| Source concept | Upstream authority | Conary owner |
| --- | --- | --- |
| artifact envelope | ZIP members `metadata.xml`, `files.xml`, `install.tar.xz` | `packages::eopkg` |
| package identity | metadata name plus current history version and integer release | `InstalledPackageIdentity::Eopkg`, `VersionScheme::Eopkg` |
| target identity | metadata architecture and package format 1.2 | eopkg package authority and source profile |
| payload | XZ-compressed tar plus `files.xml` cross-check | eopkg payload parser and shared typed payload nodes |
| payload integrity | per-file SHA-1, size, mode, uid, gid, xattrs | eopkg payload admission; CAS content is SHA-256 |
| dependencies | `Dependency` and `AnyDependency` XML records | typed eopkg requirement groups |
| provides | package name, `PkgConfig`, `PkgConfig32`, and `COMAR` | typed repository capabilities with source record provenance |
| conflicts/replacements | exact indexed relationship records | shared package-relation model using eopkg version authority |
| config | `permanent` file records | `EopkgConfigDeclaration` and Conary config transactions |
| install reason | installed database plus `info/autoinstalled` | explicit/dependency adoption reason |
| installed identity | `package/<name>-<version>-<release>/metadata.xml` | exact installed eopkg identity |
| installed files | retained `files.xml` plus live inode and bytes | full-adoption payload ownership and CAS |
| repository declarations | `/var/lib/eopkg/info/repos` XML | typed declaration discovery and repository takeover |
| repository metadata | `eopkg-index.xml.xz` | `RepositoryParserConfig::Eopkg` |
| repository trust | same-origin index, `.sha256sum`, and artifact URLs | `RepositoryTrustPolicy::Eopkg` |
| stream/update policy | repository name, URL, active status, rolling Polaris channel | native source policy, rolling stream, follow or pinned update mode |
| transaction lifecycle | one `/usr/sbin/usysconf run` after a successful mutation transaction | typed transaction-after command event |

## Artifact admission

Package format 1.2 admits exactly three ZIP members. Duplicate names, extra
members, unsafe paths, oversized declarations, malformed XML, missing current
history identity, unknown package format, and disagreement between the tar and
`files.xml` fail before publication. The inner tar uses the shared typed payload
grammar for regular files, directories, symlinks, hardlinks, devices, FIFOs,
ownership, mode, PAX timestamps, and xattrs.

The authenticated index supplies SHA-1 package digests. Conary verifies that
exact source digest during acquisition and separately computes SHA-256 for CAS,
CCS lifecycle provenance, conversion deduplication, and published artifact
identity. A SHA-1 string is never parsed as an internal SHA-256 digest.

eopkg extracts PAX timestamps through Python `tarfile`. Regular-node timestamp
comparison projects the decimal PAX timestamp through the same binary64
`TarInfo.mtime` and `os.utime` boundary. Python cannot restore an archived
symlink timestamp, so the resulting live symlink mtime is not source authority;
kind, target, ownership, mode, xattrs, and content authority remain exact.

## Version and dependency semantics

`VersionScheme::Eopkg` is separate from RPM, Debian, Arch, and Conary ordering.
The comparator follows the pinned eopkg implementation's tokenization,
separator, numeric, alphabetic, and prerelease behavior. Package identity
retains both the upstream version string and integer release; the canonical
installed/repository version is `<version>-<release>`.

Dependency alternatives remain one typed requirement group. Version and
release bounds remain distinct source fields until lowered into eopkg-aware
solver predicates. Package-name, pkg-config, 32-bit pkg-config, COMAR,
conflict, and replacement capabilities retain their source provenance and do
not depend on token lists or filename inference.

## Repository and trust semantics

Repository declaration takeover binds an exact discovered XML record (path,
line, name, URL, status, and media) to an enrollment manifest. The preview
digest binds declarations, imported trust, source identity, repository
identity, stream, update policy, parser, and projected bytes. Apply requires
that digest; repeat is idempotent; rollback restores the original bytes and
authority.

The current Polaris repository publishes a lowercase 64-hex same-origin
`.sha256sum` for its compressed index and no detached `.sig`. The typed eopkg
trust policy requires the declared HTTPS origin, same-origin metadata and
artifact URLs, the index checksum companion, and each indexed artifact digest.
Redirects or origins outside that policy fail closed.

The index grammar contains delta metadata, and Conary retains it as typed
source evidence. The pinned Polaris snapshot contains no active delta records,
so full artifact acquisition is the proven current path. Delta application is
not claimed until a live corpus exercises it.

## Lifecycle and configuration

Current Polaris packages carry no per-package script bodies. The pinned eopkg
transaction implementation runs `usysconf run` once after all successful
install, upgrade, or remove payload changes. Conary lowers that behavior to one
ordered `TransactionAfter` command event. It does not invoke eopkg or mutate the
eopkg database during converted-package operation.

`permanent` configuration records use the eopkg `.newconfig` contract: a local
edit is preserved and incoming content is staged for reconciliation. Unedited
configuration follows the incoming payload. Remove and rollback use Conary's
persisted configuration transaction state, not an eopkg query.

## Adoption and takeover

Adoption reads retained installed metadata, files, automatic-install state,
repository declarations, and live payload bytes. Preview performs no SQLite,
CAS, native database, hook, generation, or live-root mutation. Full adoption
stores exact live nodes and bytes in CAS. Repository takeover and package
adoption are separate digest-bound operations so either can be inspected and
rolled back independently.

The supported takeover path CAS-backs the complete root, converts exact native
artifacts, removes eopkg database authority, and publishes a bootable Conary
generation. `eopkg` is not a runtime dependency after ownership transfer.

Solus Polaris supplies current `dracut` and `erofs-utils`, but its repository
does not currently supply the `mount.composefs` target helper. The acceptance
image therefore carries the helper and `libcomposefs` built from the pinned
upstream commit as an explicit generation-runtime capability. This is target
capability provisioning, not source-package or distro-name compatibility
authority. The image contract verifies the helper, EROFS, dracut, bootctl,
kernel modules, and a complete generation build before claiming takeover.

## Conformance boundary

The implementation claims the package/archive, repository, installed-state,
configuration, and transaction semantics proved by the pinned corpus and Solus
VM matrix. Native eopkg export is deliberately absent: a writer would require a
complete build and signing contract and cannot be inferred from the reader.
Unknown source records or target capabilities are implementation defects that
fail preflight; they are not permanent unsupported package classes.
