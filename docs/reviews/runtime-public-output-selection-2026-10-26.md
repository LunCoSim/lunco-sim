# Indexed public output selection

## Attribution and owner

The qualified Griffin capture `griffin-usd-attribution-20261026.tracy`,
PID 201648, contains 242 complete `exposure_runtime_surface_facts` zones in
46–52 seconds. Median/p95 duration is 0.658/1.420 ms. Their enclosing runtime
surface publisher runs 121 times at 1.351/1.808 ms; these are nested costs,
not additive. The root refresh and telemetry children are much smaller.
The exact-zone native CPU selection has 164 samples, no missing stacks and a
1 ms sampling period. Its leaves include 24 SipHasher writes, seven
`RandomState` hash operations and five public-name membership checks, alongside
allocation for the complete output payload. This is attribution for recorded
application `d78ee3c70` and model `1c10a94e`, not current performance acceptance.

`lunco-luncosim-exposures` supplies each participant's complete scalar output
map and an authored public subset to the existing asynchronous UI policy
preparation. The Roadster's `roadster_ui.rhai` consumes that public subset for
instruments and course state. Complete outputs remain part of the policy input.
The current port owner already indexes names and orders selected declarations;
`PortMap::slots_for_names` now serves both declaration inspection and UI
projection. It resolves requested names, sorts and deduplicates their indices,
and returns borrowed names and opaque live handles. Absent names are omitted;
unsampled declarations retain handles, while the UI publishes only sampled
values. Appends, removals and layout replacement keep their existing semantics.
There is no persistent selection cache or second index.

The change is a generic indexed read, with no policy decision or new hook.
Authored USD still selects public names, Rhai still chooses presentation, and
the existing preparation owner retains cadence, source leases and publication.
No dependency source, numerical operation, precision or rendering setting changes.

## Validation

Independent baseline and candidate executables run the complete output-fact
projection in ABBA order, five processes per block. Each run projects 8,192
sampled outputs and 32 public outputs 512 times, retaining the complete output
map. All twenty runs pass exact public-value and map-order checks; missing and
unsampled names are omitted, and an absent authored public set yields an empty subset.
Binary hashes and distinct inode identities are checked before and after.
Median elapsed time is **214.002 → 143.710 ms (32.8% improvement)**.
This is a local fixture comparison under concurrent desktop/simulator workloads,
not a measured whole-scene, startup or coupled-tick improvement.

The unchanged production baseline passes the Roadster's nine authored
roof-overload checks, including failure HUD properties, at owned API 5464.
The process exits normally and its port is released.
The two focused port-owner checks pass: selected live slots/order/samples/layout
identity, and selected declaration order/name/direction. They cover duplicate and
missing requests, declaration before sampling, first publication, removal and
reinsertion, clone identity, clear, and declaration-side filtering.
The regular production build (`cargo build --locked -p lunco-luncosim -j 4`,
with sccache) passes without warnings. Binary SHA-256 is
`af2a980f7905b458771c5d6ce5afd7fa645edcd7de2f5ed664f254586386d041`.
The production UI preparation gate passes nine UI and sixteen authored scene
checks on owned API 5465, including held-clock preparation, failure and recovery.
The candidate repeats all nine Roadster roof-overload checks on API 5466.
Every published vessel-HUD property equals the baseline value, all 137 model
source hashes remain unchanged, and the inspected render matches the baseline
inverted, paused vehicle. The vessel HUD is not visible in this fixture; its
prepared properties are checked through the API and authored verdict.
Both sessions exit and release their ports. The skill catalogue validator passes
all 43 entries.

Griffin PID 780748/API 5467 reaches scene readiness at 29.967 s and sustained
landing/terrain qualification at 86.104 s. All 271 source hashes are preserved;
diagnostics have zero errors/warnings, and the lander finishes asleep and
unpaused. The inspected screenshot shows the landed vehicle, terrain, shadows
and the four-pad handoff UI. Coupled-tick p50/p95 is 3.260/3.916 ms; application
frame p50/p95 is 10.600/13.500 ms. These are whole application frames, not GPU
pass timings. The initializer records 16,482 native hits and zero compilations,
but prepared-cache reconstruction still takes 2.483 s and live construction
9.966 s. These are measured owner durations, not residual wall-time buckets.
Another simulator and a video player are present at startup and gone by the
qualified window. This changing workload prevents attributing scene timing
changes to the indexed output reader. The owned process and port close.
The follow-up PID 782805/API 5468 starts after those simulator/video workloads
have stopped. It reaches readiness at **7.486 s**, landing/terrain qualification
at 29.481 s and the sustained window at 34.794 s. Its initializer again has
16,482 native hits and zero compilations, with 0.674 s prepared-cache
reconstruction and 2.258 s live construction. All 271 hashes match, diagnostics
are clear, the lander finishes asleep/unpaused and rendering is inspected.
Coupled ticks are **3.549/4.146 ms p50/p95**; application frames are
**9.090/11.436 ms p50/p95** across 1,172 samples. The final camera view differs
from the preceding run. These are current production observations, not an
isolated scene speedup from this patch. The process exits and its port closes.
The sub-second loading, sub-millisecond coupled-tick and frame-tail targets
remain open. Native constructor and parent-path proposals still require their
pending dependency approval; this change does not modify either dependency.
Raw captures, profiles, executable snapshots and runtime receipts remain ignored.
