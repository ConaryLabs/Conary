---
last_updated: 2026-08-30
revision: 6
summary: Record commit-bound reproducible performance evidence, exact command resource metrics, production-XFS Remi comparison anchors, and measured optimization results
---

# Performance evidence

This directory holds reproducible, commit-bound performance evidence for
Conary's user-facing and service workflows. Evidence records name the exact
source commit, hardware label, source identity, cache state, work performed,
and phase timings so later comparisons do not turn a changed fixture or a warm
cache into an apparent improvement.

## Exact command recorder

`apps/conary/tests/fixtures/native/record-command-performance.py` executes one
exact argv without a shell and writes one create-only schema-1 JSON record. It
binds the result to the full product and harness source commits, fixture and
prepared-environment SHA-256 digests, implementation and version, operation,
cache state, sample number, resolved executable path, and executable SHA-256.
Linux child-resource evidence includes wall and CPU time, peak RSS, page
faults, block I/O operations, and context switches. Failed or signalled
commands still produce evidence and retain their command outcome.
Host evidence records both visible and affinity-available logical CPUs plus
the cgroup-v2 CPU, cpuset, and memory ceilings, so a constrained runner cannot
silently look like an implementation regression.

Use one fresh recorder process and one output path per sample. The recorder
refuses to replace an existing record. It deliberately does not infer whether
a cache is cold or warm; the benchmark driver owns and proves that setup before
passing the label. Network bytes, CAS work, SQLite statements, durability
calls, complete-root scans, and internal phase timing remain separate typed
counters rather than estimates derived from elapsed time.

The current Remi conversion schema-v3 contract treats internal signed-archive
authentication and permanent verified-CAS admission as one fused physical
pass. Its full elapsed time is recorded once in the `timing.phases` entry named
`independent_transport_reopen`; `durable_cas_ingestion` is skipped because
there is no later object-source pass solely for CAS insertion. The
archive-authentication and CAS incoming-byte counters describe the same shared
object SHA-256 pass and must not be summed. Cold isolated evidence requires
every signed object byte to be persistently written once. A hot repetition's
conversion `timing.work` is all zero, while its separate benchmark output proof
still reads and authenticates the persisted CCS after `end_to_end` returns.
Required chunk-reconstruction validation remains inside the independent signed-
archive verification boundaries; fusion removes only the later CAS-source pass.

Three reopen fields must not be conflated:

- `timing.phases[phase=immediate_converter_reopen]` is converter-owned signed
  archive verification and contributes to `conversion_core` and `end_to_end`.
- `timing.phases[phase=independent_transport_reopen]` is service-owned storage
  verification after conversion and contributes to `end_to_end`. This is the
  direct verified-CAS fusion boundary.
- `output.independent_transport_reopen_ms` is the benchmark's post-conversion
  proof. It contributes to outer repetition process wall time, but not to
  `conversion_core` or `end_to_end`. The output proof's separate complete CCS
  hash has the same boundary.

## Remi direct verified-CAS pre-change anchor: 2026-08-30

Protected production-XFS workflow
[run 33282246922](https://github.com/FieldmouseWorks/Conary/actions/runs/33282246922)
is the exact pre-#755 comparison anchor. It ran workflow commit
`d5c7901734e8c0643f6b718480f56f6e07ef2a43`, selected deployment
`33278154553`, and measured deployed Remi commit
`2a8faeb9de6e29afc2883c3b52f87c24c306b3e4`, version 0.16.1, binary SHA-256
`11680ebb982e49fb38c8eca1d87e87525c8298c8543de327b94e781f4a88bc02`.
The pinned subject was Fedora profile `fedora-44`, revision
`c758167a34de67e28a3c516efad0128182d1fe136a0606b2ecb9ef634ebd79e4`,
package key
`7646cb1313853d1a8ae069e3c42967fccdf417d178bc647bbaa500a3b1753fc4`,
and the 1,881,853,676-byte source with SHA-256
`986cfa5c47b82141f298aefa66b4c68008568b1d00abd80dece8e3d50cd7c73e`.

The sole workflow artifact was ID `9723420073`, 35,038 bytes, with matching API
and independently downloaded archive SHA-256
`24778fc85b8073bc90daad75585181ec36e72321a3cdfb539622d2590255fb80`.
Its public projection was 26,445 bytes with SHA-256
`6df7d8becdc467a17d87558e1974a87b67ed5811a2b66d32f8717e75138a978f`;
that projection binds the 30,186-byte private raw schema-v3 report at SHA-256
`075324524bd620bf576654f67d2415f27ad80e2483c86a68fec3d16c15425408`.
This commit-bound pre-change schema-v3 record retains the formerly executed CAS
phase. It is immutable comparison input, not evidence satisfying the current
fused-phase validation rule.

| Measurement boundary | Cold value | Meaning for #755 |
| --- | ---: | --- |
| Repetition process wall | 295.524674 s | Includes conversion and later benchmark output proof |
| `views.end_to_end` | 246.678 s | Service conversion call compared before and after |
| `views.conversion_core` | 163.994 s | Includes converter-owned immediate reopen |
| phase `immediate_converter_reopen` | 39.100 s | Converter verification; not the fusion target |
| phase `independent_transport_reopen` | 38.282 s | Internal verification half of the fusion target |
| phase `durable_cas_ingestion` | 17.974 s | Retired second object-source pass |
| output `independent_transport_reopen_ms` | 40.099 s | Separate post-conversion proof; never credited |
| output `independent_complete_archive_hash_ms` | 7.876 s | Separate post-conversion complete CCS hash |

The correct pre-change target boundary and comparison formulas are:

```text
before_target_ms     = 38,282 + 17,974 = 56,256
target_improvement   = 56,256 - fused_after_ms
total_improvement    = 246,678 - total_after_ms
before_residual_ms   = 246,678 - 56,256 = 190,422
residual_improvement = 190,422 - (total_after_ms - fused_after_ms)
```

Deleting only the 17.974-second second pass with every other phase fixed gives
`246.678 - 17.974 = 228.704` seconds. The 17.974-second saving is 7.286% of the
cold total and 31.950% of the 56.256-second target boundary; 228.704 seconds is
only the corresponding arithmetic total ceiling, not a latency forecast. The
after report must execute one fused internal
`independent_transport_reopen`, skip `durable_cas_ingestion` with the fixed
reason `fused into independent transport reopen; no post-verification object
pass`, and retain the separate output proof.

Within that pre-change run, both repetitions returned the identical cached
1,950,687,822-byte CCS at SHA-256
`3b7b0afbaf1b63d32c5c82aa783f1beb1c8278b16b4e4addd792ef94c02866a0`,
transport SHA-256
`37502b624ea15f5195fc139fec85a9c4b41213c659d431f175ae3ea8a7fc9fb8`,
and 49,091-object, 2,639,374,118-byte signed set at SHA-256
`cf4f448fdcf9f228febaf1767c98adbb0ca2053de4bfe231d7291f7ecf23d186`.
A cross-run comparison must preserve the complete catalog authority, subject,
verified profile/package/source identity, parsed conversion-source identity,
host and filesystem geometry, signed object-set identity, and substantive
conversion work. Each report still binds its own exact deployment commit and
binary. A comparison must not require the whole CCS or transport hashes and
byte lengths to match across separately executed conversions:
`SigningKeyPair::sign` records the current RFC 3339 time in `MANIFEST.sig`, and
the transport envelope retains that signature JSON. Those authenticated wrapper
identities must match between the cold result and its cached hot repetition, but
they are intentionally time-varying between independent runs. Cross-run
comparisons report the exact wrapper difference and compare every non-wrapper
work counter rather than silently treating the wrappers as deterministic.

Cold work hashed and persistently wrote 2,639,374,118 incoming CAS bytes across
49,091 misses, with one staged-data barrier and one canonical-name barrier.
`cas_canonical_bytes_reread`, `cas_fallback_object_syncs`, and
`cas_fallback_directory_syncs` were all zero on XFS and must remain zero after
fusion. In the fused result, verifier object hashing and CAS incoming hashing
describe this same physical pass and must not be added. Hot conversion work,
including all CAS counters and barriers, was zero. Its 0.412-second
`end_to_end` view must be distinguished from 48.578324 seconds of outer process
wall, which includes a 39.394-second independent output reopen and a
7.891-second complete archive hash.

All ten retained roots reported XFS. Exact profile and source queries demanded
6,291,456 and 6,619,136 carrier bytes respectively across 11,102,527,488
catalog bytes, with zero complete userspace catalog hashes,
SQLite integrity scans, logical replays, short reads, or integrity failures.
This anchor is one sample; its eventual exact before/after pair is paired
evidence, not a distribution. `cold` means empty application conversion, cache,
and CAS state; it does not mean the kernel page cache was dropped. R2 was
intentionally excluded, and this evidence does not constitute a same-host
native-package-manager performance comparison.

## Remi direct verified-CAS result: 2026-08-30

Protected production-XFS workflow
[run 33287383338](https://github.com/FieldmouseWorks/Conary/actions/runs/33287383338)
measured deployed Remi commit
`644520707bf549eff41eac5b1820562067fe9081`, version 0.16.1, binary SHA-256
`db41f1ec211d49d8da5e16ec8133c783f0d9ebff641850573ee98d1a4b1a7b5e`.
It was bound to successful private-candidates deployment
[33287271654](https://github.com/FieldmouseWorks/Conary/actions/runs/33287271654)
and reused the exact pre-change Fedora profile revision, package key, source
digest, source size, hardware label, and ten XFS root roles.

The sole benchmark artifact was ID `9724965198`, 34,971 bytes, with matching
API and independently downloaded archive SHA-256
`0bdb446f05249b259687f3db08333ea7e5d769be0799f64735b0f162f39faf9e`.
Its public projection was 26,377 bytes with SHA-256
`d4804994e824082e18436ee58225b7ead417980d2f607700fe6a088b8cb32d23`;
that projection binds the 30,252-byte private raw schema-v3 report at SHA-256
`2681d7def65ae3dc2ab01384e69abdd82c22b62beee2448c900ef72595dcafb3`.

| Measurement boundary | Before | After | Change |
| --- | ---: | ---: | ---: |
| Cold repetition process wall | 295.524674 s | 281.729328 s | -13.795346 s (-4.668%) |
| Cold `views.end_to_end` | 246.678 s | 233.639 s | -13.039 s (-5.286%) |
| Cold `views.conversion_core` | 163.994 s | 161.755 s | -2.239 s (-1.365%) |
| Target storage boundary | 56.256 s | 45.140 s | -11.116 s (-19.760%) |
| Non-target residual | 190.422 s | 188.499 s | -1.923 s (-1.010%) |
| Hot `views.end_to_end` | 0.412 s | 0.428 s | +0.016 s (+3.883%) |
| Hot repetition process wall | 48.578324 s | 48.167632 s | -0.410692 s (-0.845%) |

The fused phase measured 6.858 seconds longer than the old verification-only
half of the boundary, 45.140 versus 38.282 seconds. Unlike that former
temporary-spool verification boundary, it owns permanent-CAS missing-object
writes and both durability barriers; this single sample does not attribute the
entire phase delta causally. Removing the old 17.974-second second CAS pass
still produced the measured 11.116-second target-boundary win. The additional
1.923-second residual change is reported separately instead of being credited
to fusion. The 16-millisecond hot difference is one-sample variance on an
unchanged exact-cache path, whose conversion work remained all zero.

Cold logical reads fell from 39,241,805,642 to 36,602,432,246 bytes and logical
writes from 20,994,530,380 to 18,355,156,589 bytes. Each reduction is within
one kilobyte of the exact 2,639,374,118-byte signed object set, which is the
physical-pass removal #755 targeted. The after run hashed and persistently
wrote that exact set across 49,091 CAS misses, retained one staged-data and one
canonical-name barrier, and recorded zero CAS hits, race losers, canonical
reread bytes, fallback object syncs, or fallback directory syncs.

The stable signed object set remained exact at SHA-256
`cf4f448fdcf9f228febaf1767c98adbb0ca2053de4bfe231d7291f7ecf23d186`,
49,091 objects, and 2,639,374,118 bytes. Every substantive cold work counter
also matched the anchor. The independently signed CCS wrapper was 131 bytes
larger, 1,950,687,953 bytes, at SHA-256
`b08b08015ef5e75d8f3e02fb3c73790057ee5c851dd5e2b92d341484f6867774`;
its transport SHA-256 was
`88ad41722f940c7f1e49ed600bb6f59b701aca86ba5634fa14742cc62f353f6b`.
Accordingly, only the six cold `timing.work` counters derived from complete CCS
wrapper length grew by 131 bytes; the three corresponding `output` byte fields
reported the same geometry. This is the expected timestamped-signature
variation described above, not changed payload work or a #755 converter-output
change.

This result is one paired production sample, not a latency distribution. It
proves the named direct-CAS physical-pass improvement and does not constitute a
same-host native-package-manager performance comparison.

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
