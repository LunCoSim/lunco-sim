# Shared Rumoca runtime preparation and sweep storage

Status: native implementation is tested and published as
`fbfbcdc908bec19fa9b2f2b63486e1c8838e5340` on LunCoSim/rumoca branch
`fix/shared-refresh-runtime-preparation`. The application pin is under production
qualification.

## Owners and current mechanism

`rumoca-ir-solve::refresh::shared_schedule` issues one proven
`SharedAssignmentSchedule` for an exact source/owner schedule. It owns a checked
`ScalarProgramBlock` and per-program target rows. The existing immutable Arc
storage holds its operations once. The symbolic equivalence check runs before
the segments are moved into that checked storage.

`rumoca-solver::InterpretedSchedules` retains the proven schedule and prepares
the interpreter through a shallow clone of its checked block. The backend
trait and Cranelift entry points consume that same typed proven schedule.
Generated C consumes its programs, provenance and target rows through the same
current IR API. Program order and target commits retain their source semantics.
All discovered native callers and test backends use that contract.

The existing per-runtime `TornSweepCache` owns a reusable workspace for raw
residuals, dense native outputs, and the development-build independent
reference state/residuals. Every live buffer entry is initialized for its
current call; no numerical answer is cached. Cache cloning starts with empty
independent storage. Existing checked capacity helpers report reservation
failures, and reentrant use reports a typed runtime error. The per-row replay,
full bit comparison, finite checks and native decline decisions remain intact.

Hook review: these are immutable IR construction, numerical evaluation and
scratch-lifetime invariants. No changeable application/Twin policy is involved,
and no Rhai hook or simulation-clock change is introduced.

## Qualification so far

- 130 focused `rumoca-solver` runtime tests pass, including storage sharing,
  changed sweep inputs, overflow decline/recovery, warm capacity/pointer reuse,
  clone isolation, explicit reentry failure and signed-zero preservation after
  a backend decline.
- Five Cranelift assignment tests pass, covering ordered dependencies,
  correlated commits, compact guarded owners and prevalidation before writes.
- The five affected owning packages pass all-target/all-feature Clippy with
  `--no-deps -D warnings`. One redundant `usize` cast in an existing simulation
  test is removed to satisfy the application toolchain's lint.

The fixed twenty-model Tier 1 MSL canary completes: twelve models are strict
high against OpenModelica 1.27.1, with no trace deviations or simulation timeouts.
Six models stop before solve construction; two receive structural lowering
refusals (EL005). The native runner skips its full-cohort quality delta gate for
this focused scope. This is not full-cohort parity qualification, and no
baseline delta is asserted for the eight refused models.

Touched native Rust files are formatted once with the native repository's
nightly formatter, and the complete diff passes review and `git diff --check`.
The DCO-signed-off native commit is pushed and its remote SHA verified.
Application acceptance remains held for the replay investigation below. No
isolated timing gain is established. Raw receipts stay ignored under
`.cache/codex-evidence/performance-20261023/rumoca-*`.

## Normal production baseline

The pre-change normal artifact `786d8504` runs in owned PID 3579386/API 5344.
All 271 source hashes agree before admission and after measurement. Griffin
reaches sleeping four-pad handoff, with zero registered errors and finite
mass/inertia. The screenshot is inspected. The owned process and port close
before the candidate build.

First physics readiness is 45.696 s; complete landed/terrain eligibility is
79.318 s, followed by a five-second qualification soak. The final 240 coupled
ticks have median/p95 6.937/9.335 ms; application frames are 24.700/32.471 ms.
The initializer constructor is 18.198 s with 16,482 native attempts, 3,974 hits
and 12,508 compilations. Bounded-cache evictions and a concurrent foreign
LunCoSim workload make these contention/cache-condition observations, not clean
acceptance. Unchanged telemetry-capacity and sun-gate warnings remain outside
this native optimization.

## Exact replay qualification is held

The regular candidate passes eight full-matrix profiles, then intermittently
fails `jitter-0.5-seed-1234567890123456789`. The
[first-step replay investigation](physics-first-step-joint-replay-2026-10-25.md)
owns the reproduction, excluded diagnostic variants, matched native inputs and
remaining owner investigation. The reference and zero tolerance remain unchanged.
The cause is unresolved; no application integration or full replay pass is claimed.

All temporary application source probes are removed. The regular rebuild takes
10.62 s and restores the original candidate `4ede2601` exactly. A bounded normal
Griffin landing/performance run is independent of the unresolved replay gate.

## Normal candidate observation

The regular candidate `4ede2601` runs in owned PID 3737247/API 5346, then closes
both process and port. All 271 before/admitted/after source hashes match the
baseline. Griffin reaches sleeping four-pad handoff, remains unpaused and has
finite mass/inertia and zero registered errors. The screenshot is inspected.
First physics readiness is 23.392 s and complete landed/terrain eligibility is
46.916 s; the measurement follows a five-second soak.

The initializer constructor measures 7.346 s with exactly the same 16,482
attempts, 3,974 hits and 12,508 compilations as the baseline. The complete final
240 coupled ticks measure median/p95 4.062/4.993 ms. Frames measure
12.790/16.296 ms. The foreign LunCoSim workload changes from roughly 268% to
126% reported CPU; an external video probe also appears. The screenshots show
a changed camera, so the frames are not a matched-view graphics comparison.
These are contention-affected observations, not an isolated optimization ratio.
All original startup, coupled-tick and graphics targets remain open.

The regular artifact is unchanged after the run. Native tests establish shared
construction and warm workspace reuse; this bounded landing does not override
the unresolved exact replay gate or establish complete three-Twin acceptance.
