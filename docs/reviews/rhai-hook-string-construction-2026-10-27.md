# Native string construction at the hook boundary

`lunco-hooks-rhai::hook_to_dynamic` converts borrowed `HookValue::Str` text
directly into Rhai's `ImmutableString`. Rhai owns the representation and uses
inline storage for short text. The adapter does not allocate a temporary
owned Rust string before this conversion. Array/map traversal, map ordering,
duplicate-key behavior and native scalar precision are unchanged.

The consumer is every pure Rhai hook, including asynchronous runtime UI
visibility and property preparation. This is a representation conversion at
the existing interpreter boundary, with no new policy, cache, hook, execution
context or lifecycle. No dependency source changes.

## Attribution and focused validation

The qualified Griffin capture documented in the
[constructor review](rumoca-chain-probe-preparation-2026-10-27.md) supplies
252 exact visibility zones in the settled selection. Native export contains
575 CPU samples and no missing stacks. Map insertion, string comparisons and
allocation are visible; `mi_page_malloc_zero` has 104 leaf samples. The source
trace confirms string leaves previously required an intermediate allocation
even when Rhai could hold their contents inline. The profile does not identify
every allocation's caller or establish this change's whole-frame saving.

The focused conversion tests preserve nested strings, long Unicode text,
empty strings, blobs, arrays, unsigned integers, signed zero, map order and
last-duplicate values. Both the semantic test and manual diagnostic pass.
Ten alternating independent executable pairs measure complete nested-map
conversion, including construction and destruction:

| Entries | Sorted improvement | Reverse improvement | Permuted improvement |
| --- | ---: | ---: | ---: |
| 8 | 7.86% | 6.73% | 9.14% |
| 128 | 6.17% | 10.69% | 7.70% |
| 2,048 | 5.41% | 11.27% | 5.51% |

Each case converts 262,144 entries in total. Inputs and benchmark code match;
artifact hashes differ and snapshots have independent file identities. No
Cargo build overlaps these final pairs. A foreign rendering/encoding workload
is recorded and left untouched. The fixture uses the test allocator, so these
measurements are mechanism evidence rather than production performance.

Logs, executable snapshots and raw comparison JSON are ignored under
`.cache/codex-evidence/performance-20261027/current-tracy/`.
## Production qualification

The locked regular production rebuild passes. Owned PID 1114516/API 5531
passes all nine UI-preparation checks, including invalid-result rejection and
recovery while physics is held, and all sixteen authored runtime-contract
checks. It exits through the API and releases its port.

Owned PID 1114801/API 5532 preserves all 271 Griffin source hashes at model
`2b3c0a9`, reaches sleeping finite four-pad handoff, stays unpaused and has
zero registered warnings/errors. The inspected screenshot shows the lander,
terrain, shadow and handoff HUD. The driver qualifies its settled window and
verifies process exit and port release. Ordinary log warnings are distinct
from registered diagnostics.

Physics readiness is 16.189 s and initializer construction 3.086 s, with all
16,482 native kernels hitting and zero compilation/eviction. Fixed-schedule
service median/p95 is 3.834/5.576 ms; sampled frame median/p95 is
16.114/20.748 ms. A foreign rendering/encoding session runs concurrently.
This validates behavior, not an isolated loading/frame improvement against
the earlier, lighter-load run. Fixed-schedule service excludes asynchronous
Modelica work and is not the complete coupled-step duration.
The three-Twin loading, complete coupled-step and graphics targets remain open.
