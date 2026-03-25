---
paths:
  - "conary-core/src/ccs/**"
---

# CCS Module

Conary Component Specification -- the native package format. Packages are built from
TOML manifests, use CDC chunking for efficient delta updates, and can be exported to
OCI container images. Components are auto-classified by file path.

## Key Types
- `CcsBuilder` -- builder pipeline: scan files, classify components, chunk, package
- `BuildResult` -- manifest + components + files + blobs + total_size
- `CcsManifest` -- parsed `ccs.toml` manifest
- `BinaryManifest` -- CBOR-encoded manifest with `MerkleTree` root hash
- `CcsPackage` -- installable CCS package (implements `PackageFormat` trait)
- `FileEntry` -- file in a CCS package (path, hash, size, mode, component, file_type, chunks)
- `FileType` -- `Regular`, `Symlink`, `Directory`
- `ComponentData` -- component with files, combined hash, total size
- `Chunker` / `ChunkStore` -- CDC (Content-Defined Chunking) with `MIN_CHUNK_SIZE`
- `PolicyChain` / `BuildPolicy` -- build-time policy enforcement
- `LegacyConverter` -- converts RPM/DEB/Arch to CCS with `FidelityLevel` tracking
- `SigningKeyPair` -- Ed25519 signing for package integrity
- `Lockfile` -- dependency lockfile (`LOCKFILE_NAME`, `LOCKFILE_VERSION`)

## Invariants
- File hashes are SHA-256 of full content (even when chunked)
- Component assignment is automatic via `ComponentClassifier`
- CDC chunks stored by hash in `objects/` -- concatenate in order to reconstruct
- Binary manifest uses CBOR encoding with Merkle root for tamper detection
- Enhancement system has its own versioning (`ENHANCEMENT_VERSION`)

## Gotchas
- `builder.rs` uses `walkdir` for recursive scanning -- respects symlinks
- `chunking.rs` has both `Chunk` (individual) and `ChunkedFile` (all chunks for a file)
- `convert/` handles legacy format conversion with fidelity reporting
- `hooks/` provides declarative hook execution (directory, rpm, deb hooks)
- `export/` handles OCI image export

## Files
- `builder.rs` -- `CcsBuilder`, `BuildResult`, `FileEntry`, `ComponentData`
- `manifest.rs` -- `CcsManifest` TOML parsing
- `binary_manifest.rs` -- CBOR manifest, `MerkleTree`, `ComponentRef`
- `chunking.rs` -- CDC chunker, `ChunkStore`, delta stats
- `policy.rs` -- `BuildPolicy`, `PolicyChain`, `PolicyAction`
- `package.rs` -- `CcsPackage` installation
- `archive_reader.rs` -- single-pass tar archive reading
- `inspector.rs` -- CCS package inspection
- `verify.rs` -- CCS verification
- `convert/` -- `LegacyConverter`, `FidelityLevel` (7 files: analyzer.rs, capture.rs, converter.rs, fidelity.rs, legacy_provenance.rs, mock.rs, mod.rs)
- `enhancement/` -- enhancement system (5 files: context.rs, error.rs, mod.rs, registry.rs, runner.rs)
- `legacy/` -- legacy format submodule (4 files: arch.rs, deb.rs, mod.rs, rpm.rs)
- `hooks/` -- hook execution system
- `export/` -- OCI export (`oci.rs`)
- `signing.rs` -- `SigningKeyPair`
- `lockfile.rs` -- dependency lockfile
