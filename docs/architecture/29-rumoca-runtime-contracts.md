# 29 — Rumoca runtime contracts

LunCoSim uses Rumoca 0.10.2 with one dependency revision across compilation,
AST tools, lowering, simulation, and native execution. The LunCoSim fork supplies
compile source-closure provenance, prepared FMI session construction,
cooperative evaluation cancellation, and scoped retained-output admission.

## Compilation and source identity

`ModelicaCompiler` owns user overlays and admitted source roots. Source enters
unchanged through the shared parser and compilation session. The compile result
carries the exact reachable file closure and source-set keys used for content
identity. Invalid or unbalanced models stop at the compiler's phase diagnostic;
unchecked DAE construction is not a diagnostic or runtime entry point.

External input declaration defaults retain their qualified identity. Internal
and connected bindings retain equation ownership. The production
`modelica_scoped_input_defaults.rhai` scenario covers live values, overrides,
reset, and batch execution. `modelica_aggregate_output.rhai` covers outputs
consumed as state derivatives and invalid source rejection.

## Prepared execution and persistence

The worker prepares one checked `FmiComponent`, borrowing it for live session
construction. Its FMI inventory and executable model come from the same
correlated lowering. Prepared cache version 10 stores the canonical typed FMI
construction wire through a bounded JSON persistence adapter and zstd. Loading
replays Rumoca's checked construction; unsupported versions, malformed metadata,
identity mismatches, and oversized records are rejected before use.

Run inputs and tunable parameters enter `SimOptions::initial_inputs` and
`param_overrides`. Rumoca validates targets and recomputes initialization and
parameter dependents. The compiled DAE stays immutable across sweep points. Reset carries current
admitted inputs explicitly across the worker boundary, including values changed
while the participant is paused.
The source-library bundle stores `ParsedSourceDocument` with expanded source
text and syntax together. Admission uses `replace_in_memory_source_set`;
source spans and portable content identities address those retained bytes.
The native indexer writes the same paired-source record shape and returns terminal
admission errors to its CLI and UI consumers before publishing artifacts.
The artifact tag is `rumoca-0.10.2+bincode2-ast2`. Native and browser decoders require
complete records and full byte consumption; a partial bundle is never installed.
The packager admits every source before publishing either artifact; read or parse
errors abort publication with the source identity.

## Solvers and clocks

`solver_backends` registers BDF for offline implicit integration, RK45 for
adaptive authoritative live integration, and fixed RK4 for deterministic
prediction of qualified continuous, event-free models. A requested solver must
resolve to a registered capability; unsupported selections are terminal errors.

Live sessions have no stop time. Their finite options span supplies the
root-scan scale. Batch runs use the admitted `RunBounds` and canonical output
grid. Fixed RK4 applies exactly the configured step lattice and rejects models
with unsupported event, discrete, clock, or external-table constructs.

A scoped cancellation guard is observed by the evaluator and ME session loop.
A scoped output budget admits grid dimensions before allocation and every
recorded trace append, including event observations. Guards are thread-local,
restore nested callers, and cannot migrate between worker threads.

## Source-preserving editing and lint

Structured document edits splice the original source spans; untouched bytes,
comments, and formatting are preserved. Only new nodes use the local emitter.
Connect annotation routing has one parser-backed source reader in
`lunco-modelica-index::annotation_source`.

Parse-time `lint.modelica` policy checks declaration conflicts. Conditional
expressions and events are compiler/solver capabilities, not blanket syntax
errors. Runtime acceptance uses authored Rhai through the production binary.

## Ownership review

The reviewed release is upstream `v0.10.2` (`276e82bd`); upstream main was
`0b249f0f` on 2026-10-11. The pinned fork also includes upstream's
[external-function binary cache fix](https://github.com/CogniPilot/rumoca/commit/4afaf0af02dc1fbce0a189771434de49b27bab20).
Parsed AST fields must be serialized completely for positional readers.

| Mechanism | Authoritative owner | LunCoSim responsibility |
|---|---|---|
| Expression rendering | `rumoca-ir-ast::Expression` Display | Use its complete representation in component modifications, indexer defaults, and canvas labels. |
| String literal decoding and encoding | Rumoca parser tokens and `rumoca-core::escape_modelica_string` | Copy decoded values into typed metadata; use canonical escaping when inserting new source nodes. Descriptions concatenate decoded tokens without adding whitespace. |
| Checked DAE, initialization, parameter and input binding | Rumoca compiler and `SimOptions` preparation | Supply the complete authored source closure and admitted causal inputs. |
| FMI preparation and ordinary sessions | `rumoca_sim::prepare_fmi_component` and `SimulationSession::new` | Cache checked components and select registered live solver capabilities. |
| Visible values and algebraic refresh policy | `SolveRuntime::visible_values_for_names` and `rumoca-eval-solve::projection_policy` | Fixed prediction integration uses the same input slots, visible-value reader, tolerance, and iteration budget. |
| Fixed prediction step lattice | `lunco-modelica-solver::FixedStepSession` | Keep exact communication steps and reject unsupported event/discrete/clock/table models. |
| Source edits and diagram projection | Modelica document/AST/index owners | Splice exact authored spans, preserve untouched text, and project typed presentation facts. |
| Co-simulation and lifecycle | LunCoSim cosim/worker owners | Admit causal samples in stable wire order, retain typed execution identity, and retire Twin-owned work. |

Plain package/type/function documents have no automatic simulation target.
An explicit class selection or a runnable model/block/class is required.
Telemetry units come from decoded declaration metadata, rather than rendered
modifier strings. Optimistic unit edits wait for parsed metadata to reconcile.
`DescribeModel` numeric defaults include Rumoca's semantic start value when no
binding is authored. Input-binding admission and lint facts separately retain
the absence of an authored default; a start guess is not a causal input binding.
An authored non-literal binding stays unresolved in numeric authoring metadata;
it never falls through to a start value.

### Candidates for further upstream ownership

- A persistent fixed-step Model Exchange backend could move the generic RK4
  loop into Rumoca. The released `fmi_me::fixed_step::fixed_step_rk4` is a
  finite batch driver starting at zero and recording each step; it does not
  provide the open session, changing input, or step-lattice API used here.
  `MeIntegratorBackend` is the appropriate extension boundary. Solver selection,
  prediction admission, and ECS synchronization remain application-owned.
- Quote-aware qualified-name segmentation belongs in Rumoca's core name
  primitives. The inspected path splitters handle bracket depth but do not
  protect dots/brackets inside escaped quoted identifiers. LunCoSim's shared
  borrowed name-segment reader retains that required authoring contract.
- `Equation::Connect` should retain its authored annotation in the parsed AST.
  The inspected release and main store only endpoints. The current source-span
  annotation reader and source-preserving edit path remain necessary until the
  parser represents that field.
- The fork's prepared FMI borrowing/replay, exact compile source provenance,
  cancellation, and retained-output admission should be upstream contracts.
  The inspected main does not expose the equivalent public construction and
  guard surface consumed by this host. Their owning Rumoca crates remain the
  correct implementation location.
- Checked `FmiComponent::variables()` already supplies qualified scalar names,
  units, and descriptions. A live telemetry metadata cutover should carry those
  facts through the typed worker result and use them at telemetry admission.
  Source-index metadata remains necessary for authoring before compilation;
  generated USD member provenance remains application-owned. Replace the live
  source-name lookup only with that complete producer/consumer cutover.
- Upstream main exposes trajectory sensitivities, adjoint objective gradients,
  and A/B/C/D linearization. New numerical-analysis tools should consume those
  checked Rumoca APIs when that revision is admitted; application code should
  supply experiment orchestration and typed result presentation.

Verification covers decoded strings and complete expression metadata, source
index reconciliation, fixed-step/algebraic math, and deterministic input
schedules. The production metadata scenario observes decoded authoring facts through
`DescribeModel`; controller and scoped-input scenarios exercise live
initialization/reset through the worker. Browser compilation is a separate
check from browser runtime acceptance.
