---
name: bollard API migration pattern
description: bollard (Docker/Podman client) periodically moves Options structs between modules; check query_parameters module for relocated types
type: feedback
---

When bollard breaks on upgrade, the pattern is: Options structs move from `bollard::container::*` and `bollard::image::*` to `bollard::query_parameters::*`. Container config structs move to `bollard::models::*`. Generic type parameters get removed (types become owned Strings). Body parameters change to `BodyType` (use `bollard::body_full()` to wrap `Bytes`).

**Why:** bollard restructures its API to align with Docker Engine API versioning in bollard-stubs.

**How to apply:** On bollard upgrade failures, check `bollard::query_parameters`, `bollard::models`, and the `body_full`/`body_stream` helpers first.
