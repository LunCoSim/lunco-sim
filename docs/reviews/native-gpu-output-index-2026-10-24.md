# Native GPU allocation and scalar output ownership

The application pins published `bevy_render` revision
`1df9fc155f97223600ae3f3aecfeede32ab986ce` and Rumoca revision
`b825929bf32cc44797eb35a55d52d8d8af636569`. Both dependency edits and publication
were explicitly authorized. The existing WESL `bevy_shader` pin is unchanged.
The GPU validation cohort below uses the native PBR instance builder without an
override. The current desktop selection is recorded in the
[native instance-builder review](native-instance-builder-2026-10-25.md).

## Allocation owner and consumer

Deterministic scalar math makes the native `MeshUniform` 168 bytes while its
shader array stride is 176 bytes. Native GPU output allocation must use encase's
`<[T; 1]>::SHADER_SIZE`. The allocation owner now accounts for both structure
padding and vector-array alignment. Its two focused low-level regressions pass;
the structure case fails before the fix, and a minimum-element-size allocation
still fails the vector case. No deterministic-math feature, shadow, terrain,
mesh, camera or quality setting changes.

The published render package uses released dependency identities for its direct
Bevy dependencies, permitting a single-crate Git patch without duplicate ECS
types. The lockfile changes only that owner and the Rumoca source cohort;
unrelated package records and render dependency identities are preserved.

The headless closure check also identified `lunco-usd-sim-domain-api` enabling
document UI defaults. Its only document integration consumer is
`DocumentRegistry`; that dependency now disables default features, matching the
other headless API owners. `cargo check --locked -p lunco-usd-sim-domain-api -j 4`
and `bash scripts/check_render_decoupling.sh` pass. The 43-skill catalogue also
passes its validator.

## Scalar program owner and consumer

Rumoca's checked `ScalarProgramBlock` constructor derives an immutable sparse
logical-output index from the existing output-binding iterator. Queries use the
index, clones share it, and checked serialization replay reconstructs it. No
wire field, public API, evaluator, numerical order or application cache is added.
Duplicate identities retain their first stored occurrence.

Thirty focused tests pass, including sparse/multiple outputs, duplicate
identities, distinct spans, clones and checked serialization. Owner-only
all-target/all-feature Clippy passes. The isolated 131,072-query diagnostic over
1,024 four-output programs changes from 490.615 ms to 0.462 ms. This is a lookup
measurement, not a complete constructor speedup.

The fixed twenty-model MSL 4.1.0 / OpenModelica 1.27.1 canary has exactly equal
phase/error and channel-band rows to the preceding proof-lookup cohort: twenty
parse/flatten, fourteen DAE/solve and twelve initialized/simulated/high-band
traces. Six DAE and two solve refusals remain visible; there are no timeouts.
The dependency's focused review records commands, source and roster hashes.

## Production qualification and remaining limits

The normal production build and integrated-main rebuild pass. Authored initial
velocity, contact-impact pause and Griffin nominal-contact scenes pass all
5 + 10 + 5 checks, with actual process exits zero. The wrapper initially expected
the wrong Griffin verdict channel; existing logs and exit receipts were checked
against the actual `GRIFFIN_CONTACT_VALIDITY` verdict without repeating the run.

Each regular desktop observation completes full terrain readiness, a five-second
soak, ten measured seconds, clear runtime diagnostics and an inspected image.
The Griffin and Summer terrain images have no rectangular missing tiles. All
owned sessions close. Another worktree's Tesla was active during these windows;
the following values are contention-affected observations, not acceptance or
causal speed comparisons.

| Twin | Loading observation | Coupled tick median/p95 | Frame median/p95 |
|---|---:|---:|---:|
| Griffin | first ready 27.112 s | 4.442/6.208 ms | 33.586/35.929 ms |
| Summer | fully usable 4.059 s | 2.795/4.109 ms | 33.217/55.482 ms |
| Tesla | excluded: source changed during admission | 1.379/2.046 ms | 28.818/31.644 ms |

Griffin's 71.245-second landed milestone includes descent and is not loading
time. Its largest recorded constructor is 3.978 seconds; cache/source/cohort
differences prevent a causal constructor-speed claim. Tesla sources remain
stable from admission through its measured runtime window. Its pre-existing
HUD frame/datum message is outside this qualification.

The subsequent 1 kHz Tracy attempt connects the correct owned PID 2053508 on
API 5183/profiler 8424. Native framing and export pass: 13,992 blocks,
299,870,167 compressed payload bytes, 299,926,141 file bytes. However,
`GetReadiness` times out before landing qualification. The saved trace contains
a 37.655-second application update, the driver collects no qualified samples,
and the app later logs its API Exit. This trace is excluded from settled timing
comparisons. Its successful collector exit is not a successful performance gate.

Evidence remains ignored under `.cache/codex-evidence/performance-20261023/`:
`allocator-index-*`, the three `*-shader-stride-index-*-normal` cohorts,
`griffin-shader-stride-index-landed`, and `output-index-msl-canary`. Native traces
remain in ignored `scripts/perf/captures/`. Profiling data is not committed.
The complete sub-second loading, sub-millisecond coupled tick and sub-ten-
millisecond graphics requirements remain open.

After capture, `cargo build --locked -p lunco-luncosim -j 4` restores the normal
production artifact in 1m18s, SHA-256
`db779e2ff836180a7ac2306d52d1ba98a96e68c6dd78b366bf49b85c41c62156`.
The existing unused stage-import warning remains. The final rebuild adds only
the headless feature-edge correction; the desktop and physics observations above
belong to the recorded earlier artifacts.
