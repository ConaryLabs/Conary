# Conary

## Vision

Package management is fundamentally broken. You're locked to one distro's format, updates are coin flips that might brick your system, and when shit goes wrong you're reinstalling from scratch like it's 1999. We're stuck with tools designed when "rollback" meant restore from backup and "atomic" wasn't even a consideration.

Conary is package management rebuilt on principles that should've been standard a decade ago:

### Transactional Everything
Every install, upgrade, or removal is atomic. It works completely or not at all. No half-configured systems, no dependency limbo. Powered by SQLite because your package state shouldn't be scattered across text files and shell scripts.

### Format Agnostic
RPM, DEB, Arch packages - they're all just compressed files with metadata. Stop letting package format dictate your entire OS choice. Install what you need from wherever it comes from.

### Time Travel Built In
Every system state is tracked. Rollback isn't an afterthought or a separate tool - it's core functionality. Bad update? Go back. Want to test something? Branch your system state like a git repo.

### Memory Safe Foundation
Written in Rust because package managers touch everything on your system and should never segfault or have buffer overflows. The infrastructure layer should be bulletproof.

### Queryable State
SQLite backend means you can actually query your system: "What installed this dependency?", "What will break if I remove X?", "Show me everything from this repo". No more grepping logs and parsing command output.

---

The goal isn't to replace distros - it's to decouple package management from distro politics and give users the reliability and flexibility they deserve.

## Status

Early development. Watch this space.
