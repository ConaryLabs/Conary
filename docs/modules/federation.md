---
last_updated: 2026-09-04
revision: 5
summary: Record the dormant Remi federation code, its peer-identity semantics, and the work required before serving
---

# Federation Module (apps/remi/src/federation/)

Remi federation is dormant implementation material, not a serving feature.
Default builds do not compile or expose the federation coordinator, public
directory route, admin routes, or MCP peer tools. The non-default
`dormant-federation` Cargo feature keeps that code available for focused
development and tests while preventing operators and agents from mistaking it
for a live API.

Remi Lite's LAN mDNS discovery is separate: it consumes
`conary_core::federation_discovery` directly and remains available in default
builds.

## Measured State

The dormant library contains routing, request coalescing, circuit breakers,
peer scoring, signed manifests, and a `FederatedChunkFetcher`. Nothing creates
a `Federation` instance in the serving runtime, and the normal chunk miss path
never calls the federated fetcher. Persisted `federation_peers` rows therefore
do not affect chunk serving.

The HTTPS `tls_fingerprint` value is validated as a lowercase SHA-256 digest
and used as the peer identifier. The shared reqwest client uses ordinary
platform TLS validation; it does not compare the presented certificate with
that fingerprint. The fingerprint is peer identity, not transport
verification. HTTP peers derive identity from their normalized endpoint and
provide no authenticated transport.

Issue #844 owns the hard cut required to make federation live: certificate
identity extraction and verification in the TLS client, wiring into the chunk
serving path, runtime application of peer administration, and a two-node
failure matrix. Until that work lands, no documentation or API should describe
fingerprint pinning as enforced.

## Dormant Types

| Type | Current role |
| --- | --- |
| `Federation` | Unwired coordinator for selection, coalescing, and fallback |
| `FederatedChunkFetcher` | Unwired `ChunkFetcher` implementation |
| `Peer` / `PeerRegistry` | In-memory peer identity and scoring |
| `RendezvousRouter` | Deterministic peer ordering |
| `CircuitBreakerRegistry` | Per-peer failure state |
| `FederationManifest` | Signed resource descriptor |

The server's `[federation]` configuration currently belongs only to the
separate federated sparse-index client (`enabled` and `peers`). Removed
certificate, private-key, CA, and signing-key options had no reader and never
established security authority.

See also: [docs/ARCHITECTURE.md](/docs/ARCHITECTURE.md).
