# Session-bound Modelica and co-simulation publication

The numerical exchange resolves names at interface/connection admission and
retains checked local bindings. Human-facing USD, Rhai, API and portable worker
names keep their text representation. No Modelica equations, communication
periods, substeps, precision, terrain or rendering-quality settings change.

Current main integration, startup verification and the changed allocator workload
are recorded in [the integration review](main-refresh-modelica-2026-11-04.md).
The performance results below identify their retained pre-refresh executable.

## Ownership and consumers

- Rumoca supplies canonical `VarName` identity and checked numeric observations.
  Its existing interner is reused; no dependency patch or second interner is needed.
- `lunco-modelica-runtime` owns immutable `ModelicaVariableDeclarations`, the
  accepted `ModelicaVariables` inventory, row bindings and public-port projection.
  The worker admits complete finite batches through that API. Compiler declarations,
  rather than live value-map membership, classify inputs and parameters.
- `lunco-usd-sim-cosim` owns public declaration reconciliation. It declares pending
  scalar slots without inventing a sample, then consumes the runtime projection.
  Internal observables remain inspectable; their full inventory is not exported
  into the causal surface. `ModelicaSignalLayout` remains provenance/grouping.
- `lunco-port-core` owns resolved endpoint capabilities and atomic writes. Registry
  table, World, entity, topology revision and complete precedence-prefix contract
  keys fence cached access. Local slots and leases never become portable identity.
  Duplicate-write admission uses entity/backend/storage slot, including aliases.
- `lunco-cosim-core::DeclaredOutputPorts` admits private immutable names and their
  structural key together. Every runtime backend reads that retained key;
  declaration replacement updates it before scheduled topology publication.
- `lunco-cosim` invalidates compiled wiring on registry replacement as well as its
  existing connection/topology/hold revisions. Twin closure drops the cache through
  the existing reset owner. Numeric conversion and reduction order are unchanged.
- `lunco-core::ReflectReadOnly` is reflection ownership metadata. The shared scripting
  adapter rejects raw mutation of marked types and root fields, including parent
  replacement through `set`/`add` and indexed field paths. `ModelicaVariables` and the live Modelica
  input/parameter maps use it; named reads and typed control commands remain available.

The authoritative contract is [architecture 22](../architecture/22-domain-cosim.md#names-bindings-and-numerical-exchange),
with generic reflection handling in [architecture 12](../architecture/12-api.md#owner-maintained-reflected-state).

## Invariants

A stable observation compares canonical IDs and writes retained inventory indices.
A changed row sequence or declaration owner binds again before mutation; merely
matching cardinality is insufficient. Cold binding preserves existing ordered,
omitted and duplicate-row semantics and initial-symbol/output precedence. Layout
replacement and clear retire all inventory handles. Destination preparation covers
late arrival of the numerical component after USD declaration. Withdrawing samples
during initialization/recompilation preserves declared slots; interface retirement
removes them. Source and destination slot
publication validates the complete batch before its first write. Exact f64 bits,
including signed zero, survive publication.

Rust consumers have an immutable named inspection view and explicit bound readers;
they cannot obtain the inventory's mutable map. Generic reflected writes cannot
bypass its owner. Compile/reset, worker receipt/session fencing, actor authority,
fixed-tick input admission, holds, release and script execution context remain with
their existing owners. A cached port handle does not grant actor authority.

Backend contract keys are trusted owner facts: they must cover declared identity,
metadata and permission changes while excluding live samples. Their callback CPU
cost remains visible to profiling. Missing or stale capabilities reject; no name
retry routes to another owner and no missing sample becomes zero.

## Hook review

No new policy hook is needed. Declaration classification is compiler fact; numeric
binding, finite checks, capability lifetimes and exclusive commits are generic
engine invariants. Scenario policy, control ownership and presentation decisions
remain on the existing Rhai/typed-command path. No USD field/schema, model-specific
rule, JSON internal bridge, dependency fork or alternate numerical owner is added.

## Verification

The mechanism tests cover raw reflection ownership, canonical-ID binding, exact
bits, same-width layout replacement, omitted/duplicate rows, finite batch rejection,
changed public destination slots, foreign registry/entity/World rejection, alias
collisions, live contract changes and compiled-registry invalidation. Existing
worker lifecycle/source-lease and textual Bincode wire checks remain in the gate.

The authored `modelica_variable_projection` gate adds rejected nested/root snapshot
writes and rejected live input/parameter-map writes while preserving its positive
public-control and accepted-step checks. Both `add` overloads also reject snapshot
replacement. The production binary passes 297 unique authored checks (46 core,
102 second-pass, 10 projection, 47 initialization/clock/receipt, 35 metadata and
57 plume), plus two expected-fault scenes. No readiness assertion is weakened.

There are 53 passing focused mechanism/consumer tests: core reflection 1, ports
31, runtime observations 2, worker admission/lifecycle/wire 8, co-simulation 2,
immutable declaration admission 1 and telemetry 8. Consumer test targets compile.
Focused Clippy completes with pre-existing warnings; the new binding code adds
none. The skill catalogue validates all 43 skills.

Five alternating admission-fixture pairs reduce median time from 46.920254 ms
to 20.858637 ms (55.54%) for 512 transactions over 2,304 long labels. The fixture
asserts values, bits, clocks and the admitted inventory. It measures admission,
not whole-solver service or FPS.

The final normal release is
`510711b5dc3b6803893bcbe07cb7c3a89d6deec92bc062422a00e02e34d969bd`.
The High/2K bracket preserves all 528 model source hashes, thirteen participants,
3,035 MainPropulsion variables, completed terrain and zero runtime errors. Its
27.88 / 24.57 / 21.26 FPS results overlap external compilation and establish no
FPS gain. An earlier run with physical pointer input is excluded. The profiling
driver supports owned-window X11 pointer passthrough without changing the visible
shape or render target; it still rejects scene pointer events and retains all
source/readiness/quality gates. Native baseline/candidate images are inspected.
A subsequent normal run observes no compiler activity across the entire owned
session (one-second process polls). It qualifies at **50.2348 FPS**, 524 complete
frames, p50/p95 19.6604/23.8668 ms; the 150 FPS gate fails. MainPropulsion's
cumulative mean worker service is 1.7472 ms. The median native service window is
2.8487 ms of aggregate worker work per physics step and 1.4811 ms for its largest
participant. These are service-attribution scopes, not coupled-step latency or
evidence of a paired FPS gain. The 150 FPS and 1 ms solver targets remain unmet.

## Settled Tracy attribution

The opt-in Tracy executable has SHA-256
`232376712c1fbd3f525769c2d6b32b1d9ab67f16c62333276bf0275d296d2b9f`,
950,184 text symbols and a complete 346,120,573-byte capture. Matching Tracy 0.14.1
native readers validate framing and export the capture. Sampling is 1 kHz.
The final ten seconds contain 620 MainPropulsion jobs and 8,060 total jobs across
thirteen participants. The capture preserves the same 528 source hashes and
High/2K qualification. External compilation is recorded, so the timings are
instrumented diagnostics rather than a controlled product speedup claim.

| Main-world boundary | Before, ms/tick | Current, ms/tick |
| --- | ---: | ---: |
| Modelica response handling, self | 1.3091 | 0.2527 |
| Declared-output publication | 1.0866 | 0.0783 |
| Co-simulation input writes | 0.9876 | 0.7596 |

Scopes are normalized by complete MainPropulsion jobs and must not be added as
an end-to-end latency waterfall. Response handling has 163 sampled CPU leaves,
with none attributed to long-string hashing; the baseline has 800 leaves,
including 356 in `foldhash::hash_bytes_long`. Current leaves concentrate in
canonical-ID admission (67) and retiring accepted sample buffers (66). Public
projection has 50 leaves in its indexed publisher. Live port validation remains
visible: propagation has 787 leaves, including prepared-write admission, lease
checks and native writes. The complete-declaration-set hashing hotspot is absent;
small owner-key hashing remains, along with all permission and contract checks.

MainPropulsion remains 2.2452 ms/job in this instrumented cohort: integration
1.7701 ms, observation 0.4541 ms and input admission 0.0090 ms. Its integration
alone exceeds 1 ms. Opaque rendering averages 9.8470 ms and the directional shadow
pass 8.3928 ms on the GPU. The shadow child measures the same work and is not an
additional pass. Numerical evaluation and rendering require further owner-level
optimization; this change does not reduce either one's quality or accuracy.

The normal executable is restored after capture. All owned application processes
and API/Tracy ports are released.

Ignored evidence is `.cache/perf/compiled-port-bindings-20261104/`. The bounded
admission fixture, independent executable hashes and alternating runs are recorded
there; a fixture gain does not qualify normal scene FPS or a 1 ms solver target.
