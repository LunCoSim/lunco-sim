# Shader input consumer qualification

The shader-port declaration callback already borrows its `ShaderLook`. Slot
resolution now uses that borrow rather than querying the same ECS component
again. The input and material-intent owners, `lunco-usd-sim-shader` and
`lunco-materials`, use optimization level three in the regular development
profile with checks retained. There is no new API, cache, clock, dependency
patch or change to validation, physics settings, precision or graphical quality.

The ignored owner diagnostic exercises 48 shader owners with 32 inputs each,
256 complete transactions and 393,216 writes per phase. It uses the actual
registry and backend, resolving only after topology admission. Constant and
changing samples exercise both no-op and changed-value commit paths.

| Diagnostic elapsed time | Constant inputs | Changing inputs |
|---|---:|---:|
| Baseline | 89.164 ms | 95.482 ms |
| Shader owner optimization | 84.184 ms | 87.994 ms |
| Both owner packages optimized | 79.927 ms | 85.049 ms |
| Both packages and shared declaration borrow | 78.457 ms | 82.115 ms |

These bounded measurements show about 12–14% less elapsed time for the complete
diagnostic, not that improvement for a scene. The final focused command,
`cargo test --locked -p lunco-usd-sim-shader --lib ports::tests -j 4 -- --include-ignored --nocapture`,
passes eight checks, including the diagnostic. Existing negative owner checks
cover absent shader ownership, unrelated simulation inputs, untyped drives and
read-only integer inputs. Stable-slot and unchanged-value checks also pass.

## Production and native evidence

The normal production build passes in 2m23s, SHA-256
`10bf076e8567e8019e31a6db372a4ad7b14a2095e3e6b2b2030aa0d905cf905f`.
Owned PID 1758265/API 5172 loads Griffin, reaches touchdown/handoff, completes a
five-second fully ready terrain soak and ten measured seconds, then closes.
Authored hashes match the preceding collider-frame cohort. Diagnostics and
script-error checks are clear; the final image is inspected. The first post-load
ready observation is 16.757 s. The landed milestone is 39.002 s, including the
descent. Whole-tick median/p95 is 3.925/4.723 ms; frame median/p95 is
10.750/14.219 ms. Another worktree's Tesla remains active, so this is a
contention-affected observation. The small whole-tick difference from
3.952/4.712 ms does not establish a causal scene speedup.

The separate Tracy build passes in 1m38s, SHA-256
`8bfd7796b972d971c233e262ca0ea31049d3d0d32ee5116505c3e20a7f0c8ee4`.
Matching adjacent Tracy 0.14.1 collector PID 1773790 starts before owned app
PID 1773832/API 5173, on profiler port 8422 at 1 kHz. Both exit normally.
Native export verifies 16,814 blocks, 347,015,792 payload bytes and 347,083,054
file bytes, plus the exact owned PID and sampling period. The final 240 complete
FixedMain instances cover 62,765,594,132 through 68,356,145,092 ns after landing
and readiness soak. Source hashes and native counts match the preceding capture:
638 bodies, 20 dynamic, 21 sleeping, 777 colliders, 31 joints, 315 graph pairs
and no orphan pairs. Diagnostics and the inspected image pass.

Inclusive write median is 1.034 ms versus 1.108 ms; complete propagation is
1.239 ms versus 1.326 ms. Native physics remains 1.769 versus 1.763 ms, while
other owners vary. These instrumented measurements support a bounded consumer
improvement, not regular-build acceptance. Whole-tick targets remain unmet.

The write selection contains 292 samples in 240 zones, with no missing stacks.
Saved process maps and the exact binary symbol index attribute 197 executable
leaf samples: 47 fall in resolved-write preparation, 23 in the named slot index,
21 in Bevy component-type lookup, and ten each in driven-name and authored-value
tree lookup. Ninety-five leaf samples fall in libc. The index resolves enclosing
functions, not inline frames. PIE addresses use the executable's zero-offset
mapping; file offsets of other load segments are not interchangeable with ELF
virtual addresses. Full live admission remains a major consumer.

The largest constructor is 5.003 s with 16,414 native hits in 16,482 attempts
and 68 compilations. Its 3,133 selected samples include 157 executable leaves in
`ScalarProgramBlock::program_index_for_output` and 149 in `LinearOp::clone`.
The current source scans stored-output identity and preceding program operations
for each output lookup. Shared-value construction, native key hashing and
Cranelift also remain visible. These are further loading leads; this patch does
not change the dependency or claim sub-second admission.

Normal production is restored after capture. The unrelated stage import warning
remains. Raw capture, samples, process maps, compressed symbol index and summaries
stay ignored under `.cache/codex-evidence/performance-20261023/griffin-shader-input-*`
and `scripts/perf/captures/`. Derived unwrapped CSV is removed. Summer/Tesla's
preceding normal observations remain baseline evidence; they are not rerun for
this focused owner change. All original loading, whole-tick and graphics goals
remain open.
