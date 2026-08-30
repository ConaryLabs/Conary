---
last_updated: 2026-08-31
revision: 13
summary: Record commit-bound reproducible performance evidence, exact command resource metrics, production-XFS Remi comparison anchors, and measured optimization results including one-pass CCS payload preparation and bounded parallel archive compression
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

Schema v7 records the exact CCS compression geometry: tar-stream input bytes,
compression workers, fixed block bytes, block count, and the checked buffering
ceiling. Fixed ordered blocks make gzip bytes independent of worker scheduling.
Remi derives each conversion's worker budget from detected logical parallelism
and configured conversion concurrency; no environment variable or
filesystem-specific backend selects archive representation.

The current Remi conversion schema-v7 contract treats internal signed-archive
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

Two current reopen fields must not be conflated:

- `timing.phases[phase=independent_transport_reopen]` is service-owned storage
  verification and contributes to `end_to_end`. It consumes the typed pending
  conversion and is the sole internal direct verified-CAS finalizer.
- `output.independent_transport_reopen_ms` is the benchmark's post-conversion
  proof. It contributes to outer repetition process wall time, but not to
  `conversion_core` or `end_to_end`. The output proof's separate complete CCS
  hash has the same boundary.

Schema v7 retains the schema-v4 deletion of the former converter-owned
`immediate_converter_reopen` phase and both
`immediate_converter_reopen_*` inferred counters. The converter now returns an
explicitly pending artifact; Remi verifies it once under the profile targets
authority directly into permanent CAS before transport or persistence. Local
install and cook select their own single explicit verifier. The retired pass
is not retained as a zero-duration or skipped phase.

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

## Single-finalizer pre-change anchor: 2026-08-30

Issue #765 selects the next physical amplification from the exact merged-#755
result above. The converter-owned immediate reopen consumed 38.720 seconds,
23.937% of the 161.755-second conversion core. It read and decompressed the
1,950,687,953-byte CCS, authenticated the 49,091-object,
2,639,374,118-byte signed set into a disposable spool, reconstructed the signed
FastCDC layouts, and then discarded the complete verification capability.
Remi immediately repeated the same archive authority under the exact profile
targets key while streaming the objects into permanent CAS.

Schema v4 therefore makes conversion emission return a typed pending artifact
and leaves one internal finalizer: `independent_transport_reopen`. The
arithmetic ceilings with every other duration fixed are 123.035 seconds for
`conversion_core` and 194.919 seconds end to end, a 38.720-second or 16.573%
end-to-end reduction. These are pass-removal ceilings, not measured results;
the protected production-XFS rerun must establish the realized change and
retain the separate benchmark-only reopen/hash proof outside both views.

The writer now computes the exact compressed output SHA-256 inline beneath
gzip as `ccs_output_sha256`; `ccs_output_bytes_hashed` records the exact work,
so the pending value is bound to the authored bytes without another file pass.
Remi records three disjoint full-archive operations in physical order: the copy
into sealed same-directory staging, the sole verifier/direct-CAS finalizer, and
the canonical published inode hash. The added fused authoring hash is real CPU
work, so the arithmetic ceiling above is deliberately not treated as a
realized forecast.

## Single-finalizer measured result: 2026-08-30

Protected production-XFS workflow
[run 33295190850](https://github.com/FieldmouseWorks/Conary/actions/runs/33295190850)
measured deployed merge commit
`44a345e5650a8fefe21508f10b2c689abf00a1e6`, Remi 0.16.1, binary SHA-256
`c7901e117e4903baf03e57557a2307af2c705efceb19e5eaa7d90aab34c15d51`.
The binary came from exact protected candidate build
[33294444190](https://github.com/FieldmouseWorks/Conary/actions/runs/33294444190)
and successful `private-candidates` deployment
[33295114338](https://github.com/FieldmouseWorks/Conary/actions/runs/33295114338).
The run reused the #755 Fedora profile revision
`c758167a34de67e28a3c516efad0128182d1fe136a0606b2ecb9ef634ebd79e4`,
package key
`7646cb1313853d1a8ae069e3c42967fccdf417d178bc647bbaa500a3b1753fc4`,
and 1,881,853,676-byte source at SHA-256
`986cfa5c47b82141f298aefa66b4c68008568b1d00abd80dece8e3d50cd7c73e`,
complete catalog authority and subject, hardware label, and all ten 4,096-byte
block XFS root geometries.

The sole artifact was ID `9727280726`, named
`remi-conversion-benchmark-33295114338-33295190850-1`, and 34,695 bytes.
Its Actions API digest and an independently downloaded archive both had
SHA-256
`ef19a888d6e2fc0ffb6c2a0c886e40fc274732e225f9da75c939f2e85b311569`.
The four-entry archive contained the public report and exact source,
deployment, and candidate bindings. The 26,091-byte public schema-v2 report
had SHA-256
`d6202c7e5819b64c4ff43c889d37ab5ec19e38a868f6ac84ecc7552609313b0c`
and bound the private 29,857-byte raw schema-v4 report at SHA-256
`d94d9a0387f6d568d6c06d4c2c2c551e09529dd4382c499cd3c0e87ec9ce3841`.

| Measurement boundary | Before | After | Change |
| --- | ---: | ---: | ---: |
| Cold repetition process wall | 281.729328 s | 265.826899 s | -15.902429 s (-5.645%) |
| Cold `views.end_to_end` | 233.639 s | 212.009 s | -21.630 s (-9.258%) |
| Cold `views.conversion_core` | 161.755 s | 132.486 s | -29.269 s (-18.095%) |
| Retired `immediate_converter_reopen` | 38.720 s | absent | one complete disposable verifier removed |
| `archive_assembly_and_gzip` | 50.199 s | 58.595 s | +8.396 s (+16.725%) |
| `complete_archive_copy` | 0.000 s | 0.029 s | +0.029 s |
| Retained `independent_transport_reopen` | 45.140 s | 52.954 s | +7.814 s (+17.311%) |
| `complete_archive_hash` | 7.299 s | 7.278 s | -0.021 s (-0.288%) |
| Hot `views.end_to_end` | 0.428 s | 0.419 s | -0.009 s (-2.103%) |
| Hot repetition process wall | 48.167632 s | 53.733936 s | +5.566304 s (+11.556%) |

The measured core is 9.451 seconds above the 123.035-second pass-removal
ceiling, and measured end to end is 17.090 seconds above the 194.919-second
ceiling, realizing 75.591% and 55.863% of the corresponding arithmetic
pass-removal improvements. The authoring phase now includes the exact inline
compressed-output hash and took 8.396 seconds longer in this sample. The
retained verifier and direct-CAS phase took 7.814 seconds longer. These are
observed phase changes, not causal attribution from one sample. The
29-millisecond sealed staging copy did not create the feared multi-second copy
boundary on this XFS run.

The benchmark-only output proof remains outside both timing views. Its cold
independent reopen changed from 39.380 to 45.731 seconds, while its complete
archive hash changed from 7.899 to 7.265 seconds. Those changes explain why
the outer process wall improved less than the end-to-end view. The hot
conversion remained an exact cache hit with zero conversion work; its larger
outer wall likewise reflects the independent output proof rather than hot-path
conversion work.

| Cold process resource | Before | After | Change |
| --- | ---: | ---: | ---: |
| User CPU | 256.963429 s | 244.456278 s | -12.507151 s (-4.867%) |
| System CPU | 16.915275 s | 14.533032 s | -2.382243 s (-14.083%) |
| Total CPU | 273.878704 s | 258.989310 s | -14.889394 s (-5.436%) |
| Process-lifetime peak RSS | 1,609,515,008 B | 1,541,996,544 B | -67,518,464 B (-4.195%) |
| Logical reads | 36,602,432,246 B | 31,925,663,680 B | -4,676,768,566 B (-12.777%) |
| Logical writes | 18,355,156,589 B | 15,715,782,001 B | -2,639,374,588 B (-14.379%) |
| Storage reads | 3,901,267,968 B | 1,966,776,320 B | -1,934,491,648 B (-49.586%) |
| Storage writes | 7,673,360,384 B | 7,673,380,864 B | +20,480 B (+0.000267%) |

The logical-read reduction is within 585 bytes of the retired CCS reopen plus
reconstructed-content work. The logical-write reduction is within 470 bytes
of the exact signed object set. Those independent process counters support the
typed pass-removal evidence without treating storage-read variation as causal.

Schema v4 contains no executed or skipped `immediate_converter_reopen` phase
and no retired immediate-reopen counters. It records exactly one internal
verifier, after `complete_archive_copy` and before `complete_archive_hash`.
The writer hashed all 1,950,687,824 CCS bytes inline, and the copy, retained
reopen, canonical hash, and output geometry each bind that exact length. The
retained verifier and CAS incoming counters each describe the same
2,639,374,118 signed-object bytes and must not be added as separate passes.

The stable signed set remained exact at SHA-256
`cf4f448fdcf9f228febaf1767c98adbb0ca2053de4bfe231d7291f7ecf23d186`,
49,091 objects, and 2,639,374,118 bytes. All substantive cold work counters
matched #755 after excluding the retired/new schema fields and time-varying
signed-wrapper geometry. The new CCS was 129 bytes smaller at SHA-256
`b590929e13a8164bf939ce337c33a567b430f4e7bc4b9a56440849417ad25531`;
its transport SHA-256 was
`4d2383c289d748ea48a396404eb4826ea34a5f914f0cd2d602326c7a49cde4de`.
Cold and hot identities and byte lengths matched within the run. The
cross-run wrapper difference is the expected timestamped-signature variation,
not payload-work drift.

Cold CAS admission retained 49,091 misses, the exact signed byte count, one
staged-data barrier, one canonical-name barrier, and zero hits, race losers,
canonical reread bytes, or fallback syncs. R2 remained disabled. Both catalog
reopens retained one portable-manifest validation and one stored binding check;
their authenticated VFS reads had zero complete userspace hashes, SQLite
integrity scans, logical replays, short reads, or integrity failures.

This is one paired production sample, not a latency distribution. `cold`
still means empty application conversion, cache, and CAS state rather than a
dropped kernel page cache. It proves the exact single-finalizer physical-pass
change and does not constitute a same-host native-package-manager comparison.

## RPM file-digest fusion pre-change anchor: 2026-08-30

The same protected production-XFS run
[33295190850](https://github.com/FieldmouseWorks/Conary/actions/runs/33295190850)
at deployed commit `44a345e5650a8fefe21508f10b2c689abf00a1e6` is the
exact pre-#766 anchor. Its cold `native_archive_parse_and_spool` phase took
23.567 seconds. RPM decode produced 21,636 spool files and
2,757,056,284 spooled bytes while reading 2,761,335,228 decompressed archive
bytes. Projection then physically reopened 21,083 regular content sources and
reread all 2,757,056,284 content bytes. The other 553 spool files were
zero-byte non-owner hardlink members and were not projection sources.

The schema-v4 instrumentation reported zero
`native_payload_spool_bytes_reread` and only 2,757,056,284
`native_payload_bytes_hashed`. The implementation had actually performed a
second SHA-256 over the reopened content, so its cryptographic hash input was
5,514,112,568 bytes. Those counters therefore described only the decode/spool
pass and hid the complete projection reopen, reread, and duplicate hash.

At the measured #766 step, schema v5 hard-cut the raw report and schema v3
hard-cut its public projection. That historical run's raw filename was
`conversion-benchmark-v5.json`; its public filename was
`conversion-benchmark-public-v3.json`. Schema v5 added exact
`native_payload_spool_file_reopens` accounting, including successful opens of
zero-length files, and makes `native_payload_spool_bytes_reread` describe the
bytes physically read after spooling. `native_payload_bytes_hashed` is the
aggregate input presented to cryptographic hash states: one times spooled
bytes when RPM `FILEDIGESTALGO` code 8 shares the content SHA-256, or two times
spooled bytes when another supported `FILEDIGESTALGO` requires a concurrent
declared-digest state alongside content SHA-256. Additive CPIO CRC work is not
cryptographic hash input.

RPM projection now consumes typed, algorithm-tagged evidence derived from
`FILEDIGESTALGO` and `FILEDIGESTS` during the bounded decode/spool copy. It does
not reopen payload files to reconstruct that evidence. For this code-8
FlightGear subject, the resulting improvement is projected at approximately
10.3 seconds. That linear estimate applies the same run's independent complete
archive SHA-256 rate (1,950,687,824 bytes in 7.265 seconds) to the hidden
2,757,056,284-byte reread; it is not a measurement and does not assume that
many-file reopen overhead is identical. A protected schema-v5 production run
must establish the realized change.

## RPM file-digest fusion measured result: 2026-08-30

Protected production-XFS workflow
[run 33299133807](https://github.com/FieldmouseWorks/Conary/actions/runs/33299133807)
measured deployed merge commit
`7df65d2d2c68465948b06c4301b64ea1dd78fd6b`, Remi 0.16.1, binary SHA-256
`4442fec7797c5416c92bb3418075b5794a5da7ca3dea69a2c130cf46e48753da`.
The binary came from exact protected push-to-main candidate build
[33298421092](https://github.com/FieldmouseWorks/Conary/actions/runs/33298421092)
and successful `private-candidates` deployment
[33299026425](https://github.com/FieldmouseWorks/Conary/actions/runs/33299026425),
whose inspection selected that build. The run retained the exact #765 Fedora
profile revision, package key, 1,881,853,676-byte source and source SHA-256,
complete catalog authority and subject, production hardware, kernel, memory,
and all ten 4,096-byte block XFS root geometries.

The sole artifact was ID `9728447873`, named
`remi-conversion-benchmark-33299026425-33299133807-1`, and 34,808 bytes. Its
Actions API digest and an independently downloaded archive both had SHA-256
`82148c62e6a645c39c44206e30fa597286edd8d9e7f64f7422378cdc34205773`.
The four-entry archive contained the public report and exact source,
deployment, and candidate bindings. The 26,208-byte public schema-v3 report
had SHA-256
`f1a4959dd92855e2a7655e2ce2141f76a083caed47a148eba633b57d56241fe2`
and bound the private 29,978-byte raw schema-v5 report at SHA-256
`8a32a7ca23a49ae945a6d78ec6e615a6da8601f6ab45e27f70ba1305bf9e91b4`.

| Measurement boundary | Before | After | Change |
| --- | ---: | ---: | ---: |
| `native_archive_parse_and_spool` | 23.567 s | 12.647 s | -10.920 s (-46.336%) |
| Cold `views.conversion_core` | 132.486 s | 116.590 s | -15.896 s (-11.998%) |
| Cold `views.end_to_end` | 212.009 s | 198.660 s | -13.349 s (-6.296%) |
| Cold repetition process wall | 265.826899 s | 252.402529 s | -13.424370 s (-5.050%) |
| `archive_assembly_and_gzip` | 58.595 s | 54.072 s | -4.523 s (-7.719%) |
| `complete_archive_copy` | 0.029 s | 1.462 s | +1.433 s (+4,941.379%) |
| Retained `independent_transport_reopen` | 52.954 s | 54.271 s | +1.317 s (+2.487%) |
| Benchmark-proof independent reopen | 45.731 s | 45.583 s | -0.148 s (-0.324%) |
| Hot `views.end_to_end` | 0.419 s | 0.386 s | -0.033 s (-7.876%) |
| Hot repetition process wall | 53.733936 s | 54.044081 s | +0.310145 s (+0.577%) |

The targeted phase improved by 10.920 seconds, closely matching and slightly
exceeding the transparent approximately 10.3-second pre-change projection.
The larger 15.896-second conversion-core improvement also includes a
4.523-second lower archive-assembly sample. Conversely, the sealed archive
copy and retained verifier sampled 1.433 and 1.317 seconds higher. Those
observations explain the measured propagation boundaries but are not causal
attribution from one sample. The separate benchmark-only output proof remained
outside both timing views and was essentially flat.

| Cold process resource | Before | After | Change |
| --- | ---: | ---: | ---: |
| User CPU | 244.456278 s | 229.352956 s | -15.103322 s (-6.178%) |
| System CPU | 14.533032 s | 14.873123 s | +0.340091 s (+2.340%) |
| Total CPU | 258.989310 s | 244.226079 s | -14.763231 s (-5.700%) |
| Process-lifetime peak RSS | 1,541,996,544 B | 1,544,904,704 B | +2,908,160 B (+0.189%) |
| Logical reads | 31,925,663,680 B | 29,168,607,398 B | -2,757,056,282 B (-8.636%) |
| Read syscalls | 2,233,144 | 2,153,361 | -79,783 (-3.573%) |
| Logical writes | 15,715,782,001 B | 15,715,782,003 B | +2 B |
| Storage reads | 1,966,776,320 B | 1,961,574,400 B | -5,201,920 B (-0.264%) |

Schema v5 proves the physical change directly. The RPM projection's corrected
baseline was 21,083 successful spool-file reopens, 2,757,056,284 reread bytes,
and 5,514,112,568 cryptographic hash-input bytes. The measured result is zero
reopens, zero reread bytes, and 2,757,056,284 hash-input bytes: one shared
SHA-256 pass over the code-8 FlightGear content. The 2,757,056,282-byte
process-level logical-read reduction is within two bytes of the eliminated
reread and independently supports the typed counters.

The source geometry stayed exact at 24,096 payload entries, 21,083 regular
files, 21,636 spool files, 2,757,056,284 declared and spooled bytes, and
2,761,335,228 decompressed archive bytes. The later CCS-emission boundary
still reopened 21,083 payload files and read 2,757,056,284 object bytes; it is
not the eliminated native-projection pass. The stable signed set remained
exact at SHA-256
`cf4f448fdcf9f228febaf1767c98adbb0ca2053de4bfe231d7291f7ecf23d186`,
49,091 objects, and 2,639,374,118 bytes. Cold and hot outputs agreed within the
run; cross-run CCS and transport digests changed only with the expected
timestamped wrapper material.

This is one paired production sample, not a latency distribution. `cold`
still means empty application conversion, cache, and CAS state rather than a
dropped kernel page cache. It proves the exact RPM file-digest fusion and does
not constitute a same-host native-package-manager performance comparison.

## One-pass CCS payload preparation pre-change anchor: 2026-08-30

The same protected production-XFS run
[33299133807](https://github.com/FieldmouseWorks/Conary/actions/runs/33299133807)
is the exact pre-#771 anchor. After native decode, CCS reference derivation took
12.846 seconds and object emission took 35.871 seconds, or 48.717 seconds
combined. Those phases operated over 21,083 regular content owners totaling
2,757,056,284 bytes. Of that set, 7,967 files and 2,726,080,028 bytes used the
canonical FastCDC layout; 13,116 files and 30,976,256 bytes used whole objects.

The split implementation opened the 7,967 chunked sources once for reference
derivation, then reopened all 21,083 regular sources for object emission. It
therefore performed 29,050 payload-source opens and read 5,483,136,312 source
bytes. The instrumented hash counters covered 10,935,296,368 bytes: first-pass
chunk identities, second-pass chunk identities, second-pass large-file whole
content, and temporary-store incoming object identities. Duplicate verification
then SHA-256-hashed another 117,682,166 canonical reread bytes, making the
complete physical payload-boundary cryptographic input 11,052,978,534 bytes.
The store attempted 51,434 objects, wrote all 2,757,056,284 attempted bytes,
and reread those 117,682,166 canonical duplicate bytes before emitting the
exact 49,091-object, 2,639,374,118-byte signed set.

Schema v6 and public schema v4 replace that split evidence with one
`payload_derivation_and_object_staging` phase. The exact target for the pinned
subject is 21,083 source opens, 2,757,056,284 source bytes, zero source reopens
or reread bytes, 2,726,080,028 chunk-identity hash bytes,
2,757,056,284 whole-content hash bytes, and 5,483,136,312 aggregate payload
crypto bytes. Staging must write only the 2,639,374,118 unique signed bytes,
avoid 117,682,166 duplicate-write bytes, and perform zero canonical staging
rereads or durability calls. Large files retain both required identities; the
work reduction does not remove whole-content validation or chunk authority.

Those are deterministic physical-work gates, not a latency promise. The merged
candidate must be deployed and rerun against the same authority, subject, XFS
host, and cache state before any realized wall/CPU/RSS improvement is claimed.
No same-host native-package-manager diagnostic has yet been recorded for this
subject.

## One-pass CCS payload preparation measured result: 2026-08-30

Protected production-XFS workflow
[run 33305607313](https://github.com/FieldmouseWorks/Conary/actions/runs/33305607313)
measured deployed merge commit
`6370afca919ce7c932162a3069d8b906ed0ed3d1`, Remi 0.16.1, binary SHA-256
`b48f16a807bb905925710a32b003934b8881858a30915f43e22ad6961f5cf586`.
The binary came from exact protected push-to-main candidate build
[33304905924](https://github.com/FieldmouseWorks/Conary/actions/runs/33304905924)
and successful `private-candidates` deployment
[33305505612](https://github.com/FieldmouseWorks/Conary/actions/runs/33305505612),
whose inspection selected that build. The run retained the exact baseline
Fedora profile revision, package key, 1,881,853,676-byte source and source
SHA-256, complete catalog authority and subject, production hardware, kernel,
memory, and all ten 4,096-byte block XFS root geometries.

The sole artifact was ID `9730465537`, named
`remi-conversion-benchmark-33305505612-33305607313-1`, and 34,452 bytes. Its
Actions API digest and an independently downloaded archive both had SHA-256
`d40d6a6bed60f355950949ead9ac1be4eb89c15ca75a82e059bdc1f2eeaef9cc`.
The four-entry archive contained the public report and exact source,
deployment, and candidate bindings. The 25,849-byte public schema-v4 report
had SHA-256
`8fffdf61d071681acc4ccb3577f722423af30631e4cc6524ca1d2cad29058c5d`
and bound the private 29,502-byte raw schema-v6 report at SHA-256
`cc12493de27830f2687b4cf5a322378db88a5a088eab4aed11659a6903e0e288`.

| Measurement boundary | Before | After | Change |
| --- | ---: | ---: | ---: |
| Payload derivation plus staging | 48.717 s | 25.306 s | -23.411 s (-48.055%) |
| Cold `views.conversion_core` | 116.590 s | 92.986 s | -23.604 s (-20.245%) |
| Cold `views.end_to_end` | 198.660 s | 175.527 s | -23.133 s (-11.645%) |
| Cold repetition process wall | 252.402529 s | 228.975566 s | -23.426963 s (-9.282%) |
| `native_archive_parse_and_spool` | 12.647 s | 12.486 s | -0.161 s (-1.273%) |
| `archive_assembly_and_gzip` | 54.072 s | 53.990 s | -0.082 s (-0.152%) |
| `complete_archive_copy` | 1.462 s | 2.539 s | +1.077 s (+73.666%) |
| Retained `independent_transport_reopen` | 54.271 s | 52.796 s | -1.475 s (-2.718%) |
| `complete_archive_hash` | 7.277 s | 7.293 s | +0.016 s (+0.220%) |
| Benchmark-proof independent reopen | 45.583 s | 45.369 s | -0.214 s (-0.469%) |
| Benchmark-proof complete archive hash | 7.341 s | 7.260 s | -0.081 s (-1.103%) |
| Hot `views.end_to_end` | 0.386 s | 0.495 s | +0.109 s (+28.238%) |
| Hot repetition process wall | 54.044081 s | 53.789178 s | -0.254903 s (-0.472%) |

The fused phase improved by 23.411 seconds, accounting for nearly all of the
23.604-second conversion-core reduction. Archive assembly/gzip was effectively
flat, while the complete archive copy sampled 1.077 seconds higher and the
retained verifier 1.475 seconds lower. Those observations describe the
measured propagation boundaries but are not causal attribution from one
sample. The separate benchmark-only output proof remained outside both timing
views and was also effectively flat.

| Cold process resource | Before | After | Change |
| --- | ---: | ---: | ---: |
| User CPU | 229.352956 s | 205.588164 s | -23.764792 s (-10.362%) |
| System CPU | 14.873123 s | 14.658700 s | -0.214423 s (-1.442%) |
| Total CPU | 244.226079 s | 220.246864 s | -23.979215 s (-9.818%) |
| Process-lifetime peak RSS | 1,544,904,704 B | 1,544,351,744 B | -552,960 B (-0.036%) |
| Logical reads | 29,168,607,398 B | 26,308,931,269 B | -2,859,676,129 B (-9.804%) |
| Read syscalls | 2,153,361 | 2,096,579 | -56,782 (-2.637%) |
| Logical writes | 15,715,782,003 B | 15,582,419,515 B | -133,362,488 B (-0.849%) |
| Write syscalls | 1,308,578 | 1,281,081 | -27,497 (-2.101%) |
| Storage reads | 1,961,574,400 B | 1,964,400,640 B | +2,826,240 B (+0.144%) |
| Storage writes | 7,673,352,192 B | 7,534,903,296 B | -138,448,896 B (-1.804%) |

Schema v6 proves the targeted physical change directly:

| Deterministic physical work | Before | After | Change |
| --- | ---: | ---: | ---: |
| Payload-source opens | 29,050 | 21,083 | -7,967 (-27.425%) |
| Payload-source bytes | 5,483,136,312 | 2,757,056,284 | -2,726,080,028 (-49.718%) |
| Payload cryptographic hash input | 11,052,978,534 B | 5,483,136,312 B | -5,569,842,222 B (-50.392%) |
| Staging bytes written | 2,757,056,284 B | 2,639,374,118 B | -117,682,166 B (-4.268%) |
| Canonical duplicate rereads | 117,682,166 B | 0 B | -117,682,166 B (-100%) |

The result opened each of the 21,083 regular content owners exactly once and
reported zero source reopens or reread bytes. It derived 38,318 chunks, hashed
2,726,080,028 chunk-identity bytes and 2,757,056,284 whole-content bytes, and
staged the exact 49,091 unique objects without file or shard syncs. The 2,343
duplicate objects avoided 117,682,166 bytes of writes and canonical rereads.
Eliminating those canonical rereads also eliminated their SHA-256 verification.
The stable signed set remained exact at SHA-256
`cf4f448fdcf9f228febaf1767c98adbb0ca2053de4bfe231d7291f7ecf23d186`,
49,091 objects, and 2,639,374,118 bytes. Cold and hot outputs agreed within the
run. The retained independent reopen rehashed all 2,639,374,118 signed-object
bytes; the separate complete-archive hash covered all 1,950,609,956 CCS bytes.
Registered catalog access retained the same authenticated portable-VFS
counters with no userspace full hash, SQLite integrity scan, logical replay,
or integrity failure.

This is one paired production sample, not a latency distribution. `cold`
still means empty application conversion, cache, and CAS state rather than a
dropped kernel page cache. It proves the exact one-pass payload-preparation
change and does not constitute a same-host native-package-manager performance
comparison.

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
