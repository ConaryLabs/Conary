---
last_updated: 2026-08-15
revision: 1
summary: Record commit-bound reproducible performance evidence and the initial Remi conversion baseline
---

# Performance evidence

This directory holds reproducible, commit-bound performance evidence for
Conary's user-facing and service workflows. Evidence records name the exact
source commit, hardware label, source identity, cache state, work performed,
and phase timings so later comparisons do not turn a changed fixture or a warm
cache into an apparent improvement.

## Remi conversion baseline: 2026-08-15

The raw baseline is
[remi-conversion-2026-08-15-before.jsonl](remi-conversion-2026-08-15-before.jsonl).
It was captured from release commit
981e051b626d700765e3cf1f2489009351c133c0 on the
remi-i7-8700-raid1 hardware label with an isolated authenticated Fedora 44
metadata snapshot and R2 disabled.

| Class | Source bytes | Cold total | Hot total | Hot download | Hot checksum |
| --- | ---: | ---: | ---: | ---: | ---: |
| small | 6,733 | 417 ms | 331 ms | 314 ms | 0 ms |
| median | 51,633 | 524 ms | 498 ms | 482 ms | 1 ms |
| large | 1,881,853,676 | 289,984 ms | 47,595 ms | 39,957 ms | 7,005 ms |

The cold large-package profile is dominated by CCS emission at 130,822 ms.
The first bounded optimization targets a separate, unambiguous hot-path cost:
an exact metadata-bound cache hit still downloads and hashes the entire source
artifact. Download plus checksum account for 46,962 ms (98.7%) of the large hot
sample and repeat 3,763,707,352 bytes of transfer and source hashing across its
cold/hot pair.
