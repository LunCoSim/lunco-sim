# Telemetry recording and surface projection

The Modelica recording loop consumes prepared source/recorder slot pairs, and
surface preparation consumes one Rhai visibility/properties decision. Numerical
models, cadence, precision and rendering quality are unchanged. The canonical
contracts are [telemetry](../architecture/telemetry-subsystem.md) and
[surface publication](../architecture/runtime-authored-ui.md).

## Owners and lifecycle

`lunco-signal` owns one slot store for signal identity, history, active state,
metadata and stable wire ownership. Its name index serves discovery and dynamic
admission. `ScalarRecordingBinding` carries the slot's exact lease; archival,
removal and scene retirement revoke it. Reuse and foreign registries cannot
accept stale capabilities. Explicit application slots survive scene teardown.
Bound and named producers append through the same checked numerical mechanism.

`lunco-modelica-telemetry` reuses inventory-issued `ModelicaVariableSlot` reads
and registry-issued recording capabilities. Only admitted rows enter its sample
loop. Source layout, live recorder membership/availability and channel-limit
settings govern admission; a full budget does not force repeated deferred-row
visits. Catalog changes refresh metadata and wire owners separately. Histories
retain rate, retention, backwards-time and session/archive semantics. Document
closure remains isolated; scene reset wakes surviving paused sources once.
No simulation result, timestamp-dependent expression or second history is cached.

`lunco-luncosim-exposures` invokes `runtime.ui.surface` once per admitted immutable
snapshot. Its map must contain boolean `visible` and a typed `properties` map;
the complete decision validates before publication. Rhai owns visibility,
including cameras, and formatting. The shipped runtime entry keeps rich facts
local and passes selected fields to helpers. Captured registration, execution
context, source epoch, scene identity and bounded async admission remain the
publication fences. Missing/invalid policy state fails visibly without partial
properties or authoritative simulation mutation.

`lunco-luncosim-runtime::SetRhaiPolicy` uses one existing compound USD operation
for the whole source policy. This also makes dependency preparation and undo
atomic. A referenced-scene production probe exposed rejection of parallel
single-property edits; its diagnostic is retained with the evidence.

File-backed policies consume `UsdReadObject::asset_identifier`, which anchors
the asset to its contributing USD layer. Prepared and live stages share the
same policy reader. A source-identifier error is retained and reported; it
cannot install a policy or guess an anchor from the containing scene.

These are engine storage/execution invariants, not new product-policy hooks.
The single surface seam is application-installed and Twin/scene replaceable;
it receives typed owner facts and returns the complete validated decision.
No JSON bridge, compatibility alias, numerical fallback, dependency fork or
new third-party package is introduced.

## Measurements

The current Tracy 0.14.1 capture uses 1 kHz sampling. Its ten-second qualified
region ends 100 ms before the final screenshot command. Both captures use the
canonical Griffin surface-operations scene at High/2560×1440, with thirteen
participants and the maintained Bevy pin.

| Diagnostic owner | Prior capture | Current capture |
|---|---:|---:|
| Modelica retention, mean ms/frame | 1.646 | 0.167 |
| Surface projection, mean ms/decision | 2.224 across two calls | 1.483 in one call |

Current retention has 348 events, median 0.00933 ms and 58.011 ms total in the
region. Surface projection has 398 events and 590.217 ms total. These diagnose
the changed owners. The prior capture predates the shared Rumoca visible-output
refactor; they are not an isolated whole-scene timing experiment. The current
capture precedes the compound-authoring, late-registry wake and anchored-source
corrections; those paths do not execute in this measured scene window.

Five paired recorder fixtures compare named and bound appends into the current
store: 4,096 admitted channels at 64 fresh times, retaining sixteen samples.
Medians are 18.996 and 4.318 ms respectively. Each pair runs named then bound;
this is a microbenchmark with fixed order. Every retained timestamp/value bit
is checked. It does not measure old-versus-new complete simulator cost.

Two alternating normal production pairs use independent binaries and their
matching shipped policy files. All 546 guarded Twin source hashes remain
identical across these windows; the film-policy migration occurs afterwards.

| Window | FPS | Frame p95, ms | Fixed service p50, ms |
|---|---:|---:|---:|
| Before 1 | 46.058 | 27.118 | 4.068 |
| After 1 | 45.634 | 29.157 | 4.566 |
| Before 2 | 41.688 | 32.042 | 4.017 |
| After 2 | 41.797 | 34.128 | 4.554 |

No stable FPS or coupled-step improvement is established. Worker attribution
is distinct from coupled latency; these changes do not optimize the numerical
solver. Existing opaque/directional-shadow GPU costs still constrain FPS.

## Authored consumers and verification

Seventeen active film scenes in the adjacent lunar-base-model Twin use a shared
`media/policies/runtime_surfaces.rhai` source asset with six distinct entries.
This includes three older v8 callers found after the initial fourteen-scene
inventory. Every scene was opened, inspected, edited through `ApplyUsdOps`,
saved with `SaveDocument`, and read back. Each file changes only the seam,
entry and source fields: six diff lines, with no other authored edits. Existing
untracked scenes and unrelated film work are preserved.

The Twin policy uses indexed participant/output access and selected scalar
formatter arguments. It preserves film properties and camera identity. A
disposable before/after Rhai comparison passes 200 presentation/rejection
checks across the six policies; it is evidence, not a retained alternate
implementation. The maintained authored contract passes 29 checks, and all
six file-backed entries execute through the installed hook and real camera
consumer (twelve checks). The maintained Twin driver is
`media/policies/tests/run_surface_contract.py`; run it from this checkout with
`LUNCOSIM_BIN` set and an explicit `--port`. It references only a film policy
prim into the canonical surface-operations scene, proving the relative source
asset resolves from its contributing film layer without running film scenarios.
The independent before binary fails this inherited-path regression; the final
production binary passes all 29 presentation and twelve live-consumer checks.

Focused native coverage passes nineteen signal tests and nine Modelica
projection tests; one recorder diagnostic is ignored by default. Bound lease
and storage mechanisms are Rust-owned. Public behavior uses production Rhai:
thirteen UI preparation checks in both host modes, twenty referenced-scene
checks in each run, ten Modelica replacement checks, forty-five hook-policy
checks, seven coordinate checks, and focused scene/policy/paused-reset/document
retirement verdicts. The final production binary also passes all ten checks in
the authored atomic policy creation/undo gate. The independent before binary
fails that gate at policy creation.

Targeted Clippy for the changed telemetry/exposure owners completes with six
pre-existing exposure warnings and inherited spatial warnings; introduced
warnings are corrected. Final runtime-owner Clippy completes with two inherited
warnings. Dependency-wide Clippy remains blocked by existing
disallowed `EntityCommands::insert` calls in mobility/camera owners. The broad
Twin-retirement driver fails six queued-gesture assertions; the independent
before binary reproduces exactly those six failures. Focused telemetry and
policy lifecycle checks pass separately.

## Evidence

Ignored `.cache/perf/recording-projection-20261105/` retains build/test logs,
independent binaries, paired normal windows, exact source guards, recorder
diagnostics, current native Tracy capture/exports, file-migration plans and
receipts, per-file diffs, and production Rhai/runtime logs. Earlier diagnostic
capture evidence is in `.cache/perf/latest-20261105/tracy/`. Film source changes
are deliberately separate from the normal measurement cohort. Owned sessions
use explicit ports and exit through their API; remote publication is outside
this change.

Final normal executable SHA-256:
`b96737d025b0ad5dd0fdb3a0ca0cb39a81496333059860027d25b503a4115e18`.
The film source edits and their shared policy/tests remain uncommitted in the
adjacent models checkout, preserving its unrelated tracked, untracked and
ignored work.
