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
The artifact tag is `rumoca-0.10.2+bincode2`. Native and browser decoders require
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
