---
last_updated: 2026-08-15
revision: 2
summary: Record commit-bound reproducible performance evidence and the first measured Remi conversion optimization
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

The targeted result is
[remi-conversion-2026-08-15-after.jsonl](remi-conversion-2026-08-15-after.jsonl).
It was captured on the same hardware and byte-identical clean metadata
snapshot from release commit
31438a532160526f83b739e7681b7e01b985b145.

| Class | Before hot | After hot | Improvement | After hot transfer/hash |
| --- | ---: | ---: | ---: | ---: |
| small | 331 ms | 10 ms | 97.0% | 0 / 0 bytes |
| median | 498 ms | 12 ms | 97.6% | 0 / 0 bytes |
| large | 47,595 ms | 381 ms | 99.2% | 0 / 0 bytes |

The fix moves the exact source-checksum and repository-capability-bound cache
lookup before source transfer. Across the three hot samples it avoids
1,881,912,042 bytes of transfer and the same amount of source hashing. Cold
totals remained in the same workload shape (514 ms, 565 ms, and 269,813 ms);
their network-dependent elapsed-time differences are not attributed to the
cache-hit fix.
