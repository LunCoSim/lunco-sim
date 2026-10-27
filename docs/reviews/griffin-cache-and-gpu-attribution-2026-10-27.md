# Griffin cache and GPU attribution

Status: measured diagnosis; no runtime or dependency change. Loading, complete
coupled-step and graphics targets remain open.

## GPU work

The qualified `c4841c194` capture described in the
[constructor review](rumoca-chain-probe-preparation-2026-10-27.md) already
contains native GPU events. Bevy 0.20's `trace_tracy` feature enables
`bevy_render/tracing-tracy`, which installs `RenderDiagnosticsPlugin`; another
feature or profiling build is unnecessary. The matching Tracy 0.14.1 native
GPU CSV export succeeds and contains 40,074 events. Its start and duration
columns use `GpuStart()` and `GpuEnd()`, not CPU submission duration.

Within the existing 72.172305255–78.483054925 s settled selection, 4,063 GPU
events are wholly contained. There are 239 occurrences of each camera shadow
parent and each opaque 3D pass:

| Recorded GPU pass | Median | p95 |
| --- | ---: | ---: |
| Camera early shadows | 1.717 ms | 3.727 ms |
| Opaque 3D | 1.531 ms | 3.652 ms |
| Early prepass | 0.185 ms | 0.242 ms |
| Bloom | 0.132 ms | 0.180 ms |
| Clustering | 0.099 ms | 0.116 ms |
| Camera late shadows | 0.0028 ms | 0.0051 ms |

The camera shadow child durations are contained in their parents and must not
be added again. Neither shared-shadow root appears in this selection; absence
is not a measurement of a shared-light pass. All 238 intervals between
successive camera-early starts contain the same 17-event inventory. Those
intervals are render-cycle groupings, not application frame markers. Their
non-overlapping recorded root durations total median 4.399 ms and p95
7.227 ms per grouping. This is measured pass time, not complete GPU/frame
time: uninstrumented work, submission/presentation and inter-pass gaps remain
outside that sum. Seventeen recorded spans require 34 timestamp slots, below
the native 256-slot limit; this inventory does not independently prove that
every render operation has a supported timestamp.

The capture is diagnostic and contention-affected. It does not certify the
10 ms graphics target or justify lowering shadow or terrain quality. The
qualified scene image was already inspected; no rendering change is made.

## Prepared-solver retention

The later compact-transaction Griffin run misses the prepared-solver disk
record and spends 5.043 s lowering it. Inspection of the shared
`prepared-solve-v13` directory finds 32 records totaling 3,943,086 bytes:
18 for compiler `eca3948416fa`, 14 for `41637b1eba18`. Both compiler cohorts
contain the same initializer source key `e0d18b33ab35b48d`, solver `rk45`,
and empty parameter/initial-input overrides.

`lunco-modelica-worker::worker::cache::PreparedSolveCache` already validates
the full key, updates last-use metadata after a valid read, and publishes under
the storage owner's cross-process lock. Retention admits 32 records across
compiler identities within this cache version. Mixed-cohort capacity pressure
can explain eviction, but the inventory does not identify the historical
writer or prove why that particular record disappeared. No cache size change,
new cache layer, shared-cache cleanup or validation bypass is warranted by
this evidence alone. A future warm/cold comparison must record the exact key
and cache result at each launch.

## Observation allocation

On sampled worker lane 55, 302 exact-zone observation samples include 78
inclusive samples under output-name vector cloning and 65 under string cloning;
these groups overlap. Hashing and ordered-map insertion also appear. Native
`MeSimulationSession::visible_values` clones output names, reads the checked
output vector, validates its length, constructs an `IndexMap`, then overlays
host-applied inputs. The application validates finite time and every value
before transferring the complete observation. It needs those inputs and
diagnostics; replacing the call with an output-only vector is not equivalent.

Both adaptive and fixed application steppers use this native observation
contract. The native `output_values` capability exists, but the adaptive
session does not expose a complete reusable visible-observation buffer.
Removing allocation here requires a separately reviewed native contract that
preserves ordering, host input precedence, errors and complete validation.
No dependency source is modified under the pending chain-preparation request.

## Evidence and cleanup

Exact `usd_projection_facts` sampling from the same capture selects two zones,
2,177 samples and zero missing stacks. Leading resolved leaves are character
iteration (355), path-component traversal (174), identifier validation (108),
and native owning-prim construction (49). These identify native path work but
do not assign every validation sample to one caller.

Six alternating pairs of independent application test binaries measure the
complete 1,001-prim snapshot fixture, with and without authored transforms and
with and without recipe construction. Reusing retained native attribute
handles for UI metadata changes median times by +0.50%, +0.03%, -0.57%, and
+0.82%, respectively (positive means faster). This is not a consistent gain;
the experiment is removed. The regular production executable is unchanged,
SHA-256 `24386a6979fde9b46cbb2d90cae1def42f42500ca030afaa90b8ca87fe10060b`.
Comparison receipts and logs remain ignored in `projection-metadata/`; the
two executable snapshots are obsolete and removed after comparison.

Current Summer/Tesla acceptance is still outstanding. The Summer driver run
on API 5535/PID 1243576 used the directory name as its expected Twin name
instead of the authored `SummerSpaceSchool`, so it timed out without a
qualified measurement. Its owned process and API port are confirmed closed.
Do not treat that harness error as a simulation failure. Tesla was not launched;
its current dirty source selects `roadster_cinematic.usda`, so a future run
must record that source cohort rather than assume the previous default scene.
Work is paused at the user's request.

Raw GPU exports and cycle inventories remain ignored under
`.cache/codex-evidence/performance-20261027/current-tracy/`; the cache header
inventory is under `cache-retention/`. The existing cleanup receipt records
21 obsolete captures, raw exports and executable snapshots removed, freeing
15,004,036,162 bytes (13.97 GiB). The current capture, baseline summaries,
regular executable, sources and shared caches are retained. The GPU/cache
analysis reuses the saved capture. The metadata experiment uses focused test
builds only; no production rebuild or retained runtime change invalidates the
existing passing behavioral evidence.
