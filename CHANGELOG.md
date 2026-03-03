# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [0.2.0] - 2026-03-02

### Added

- **CCS native package format**: Content-addressable package format with chunked storage, policy engine, OCI export, lockfiles, and redirect support
- **Hermetic builds**: BuildStream-grade reproducible builds with network isolation, PID/UTS/IPC namespaces, and dependency-hash-based cache invalidation
- **Remi server**: On-demand CCS conversion proxy with chunk serving, Bloom filter acceleration, batch endpoints, pull-through caching, sparse index, search, and TUF trust metadata
- **conaryd daemon**: REST API for package operations with Unix socket transport, SSE event streaming, persistent job queue, systemd integration, and peer credential authentication
- **CAS federation**: Distributed chunk sharing across nodes with hierarchical peer selection, mDNS discovery, request coalescing, circuit breakers, signed manifests, rate limiting, and Prometheus metrics
- **System Model**: Declarative OS state management with remote includes, signature verification, diff engine, state capture, and model publishing
- **Capability enforcement**: Landlock filesystem and seccomp-bpf syscall restrictions with capability declarations, auditing, and inference
- **Package provenance (DNA)**: Full provenance tracking covering source, build, signatures, and content verification
- **Recipe system**: Build packages from source with TOML recipes, pkgbuild conversion, and hermetic build support
- **TUF supply chain trust**: Repository metadata verification with timestamp, snapshot, targets, and root role delegation
- **Remote model resolution**: Remi-native model includes with diff, publish, lockfile, and Ed25519 signing
- **Retroactive CCS enhancement**: Background capability inference and subpackage relationship tracking for converted packages
- **Binary delta updates**: Efficient package updates using binary deltas with compression
- **Bootstrap system**: Bootstrap a complete Conary system from scratch
- **1200+ tests** across unit and integration test suites

### Changed

- Database schema now at v36 with 40+ tables
- Unified package parser supporting RPM, DEB, and Arch formats
- Unified decompression supporting Gzip, Xz, and Zstd with format auto-detection

## [0.1.0] - 2025-06-01

### Added

- Initial package management: install, remove, update, rollback
- SQLite-backed state management
- RPM and DEB package parsing
- Dependency resolution with topological sort
- Content-addressable file storage
- Basic repository sync
