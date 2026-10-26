# Direct symbolic proof terms

Status: native optimization published; production replay has two unresolved failures.

## Owner and change

Rumoca `aae2bbdd1c69f596b42ab60ab3b5d0f42d353ade`, published to
`LunCoSim/rumoca` on `perf/direct-symbolic-term-identity`, removes a redundant
lookup table in `rumoca-ir-solve::linear_op::shared_values`. A private symbolic
term directly identifies an original solver slot or an interned operation and
its output ordinal. Complete operation shapes and ordered operand terms still
own equality; hashing alone cannot establish equivalence. The independent
checker retains its complete original/transformed walks, output checks and
final-slot checks. Program order assigns operation IDs.

This is immutable proof storage at the native owner, consumed by checked runtime
construction. It changes no equation, execution order, precision, public API,
artifact format, solver setting or lifecycle policy; no Rhai hook is needed.
The application only pins the published revision, preserving the preceding
native-cache relocation and shared-assignment fixes. All eleven workspace
declarations and twenty-nine lockfile source entries use the same commit;
unrelated dependency selections are unchanged. No application implementation or
dependency path patch is retained.

## Native evidence

The existing scalar/wide derivation-and-check fixtures run in independent
baseline/candidate binaries, ABBA with ten processes per block. All eighty
runs pass; binary hashes are checked before and after. Scalar median falls from
5.280 to 5.065 ms (4.1%); wide median falls from 2.613 to 2.478 ms (5.2%). These
are proof-fixture measurements in the native regular test profile, not whole
scene speedups.

All 423 active IR tests pass, with one pre-existing ignored test. The eighteen
focused shared-value cases cover changed operands/outputs, final slots, effects,
caps and exact floating-point keys. Owner-only Clippy passes with `--no-deps
-D warnings`; dependency-inclusive Clippy stops on three existing `rumoca-core`
nesting findings. The fixed twenty-model MSL canary completes: twenty
parse/flatten, fourteen DAE/solve, twelve initialized/simulated. All twelve
traces compare in the high band against OpenModelica, with accurate initial
conditions and no timeouts or missing traces. Six DAE and two solve refusals
remain visible. Counts match the preceding published qualification; its deleted
raw results prevent an exact row-by-row comparison. This is not full-cohort
qualification. The native review records commands and contract details.

## Production evidence

The exact portable pin builds with `cargo build --locked -p lunco-luncosim -j 4`,
normal sccache and this checkout's target directory. Binary SHA-256 is
`84cb317d5cab9df5f75b68d8a28c84a3e193db4d967f0ed88aa4ca27672ce794`.
The initialization receipt driver passes 44 network checks and two retirement
checks on API 5442. Summer physics passes 102 checks at tick 722; Roadster's
`Roadster::VerifyRunner` passes eight at tick 302. Their source snapshots remain
unchanged. Both authored terminal Modelica-fault fixtures pass their expected
failure contracts. All those processes and ports close.

Eight of ten exact replay profiles pass. The serial four-rover profile and
jitter 0.5 with seed 6840157149251759617 fail articulated tick 11, for Rover02
and Rover01 respectively. These are the first-run results; no repeat replaces
either failure. Reference SHA-256 remains
`6247db0dfefbd0eaedad3d712c359578f30a75addb6daea4a79584b784a70451`, with zero
tolerance. The [existing native first-step investigation](physics-first-step-joint-replay-2026-10-25.md)
already reproduces this boundary before this pin, but that does not establish
the cause of these two failures or make the current matrix green. The private
proof change is qualified separately by its independent checker and native
tests; exact application replay remains an open acceptance issue.

Griffin runs on API 5457/5458 preserve all 271 source hashes before admission,
at admission and after closure. Both reach four-pad flight handoff, clear
registered diagnostics and complete the terrain-qualified measurement window.
Screenshots are inspected: terrain, lander and the appropriate landing/ramp UI
render. The first run receives a manual ramp-unfold command after touchdown;
its final lander is awake. The warm run has no ramp command and finishes asleep.
Both owned processes exit and their ports close.

| Measurement | First fill | Warm restart |
| --- | ---: | ---: |
| Requested Twin physics readiness | 24.337 s | 14.254 s |
| Live initializer construction | 8.039 s | 4.104 s |
| Initializer native attempts / hits / compilations | 16,482 / 7,964 / 8,518 | 16,482 / 16,482 / 0 |
| Fixed-tick service p50 / p95 | 5.572 / 12.349 ms | 7.377 / 9.245 ms |
| Application-frame p50 / p95 | 16.823 / 23.722 ms | 26.431 / 33.395 ms |

The SDK identity requires fresh prepared/compiled artifacts on first use. Warm
restart records thirteen compiled-artifact disk hits. Its initializer compiled
lookup takes 138 ms, prepared-solver lookup/preparation takes 1.276 s, and live
construction takes 4.104 s. These are distinct measured owner phases; their sum
does not attribute the rest of startup. Native executable hits still require
checked runtime/proof construction, allocation and initialization.

Concurrent video encoding and another simulator contend with both runs. The
first also has a different camera and a manual ramp command in the measured
window. These observations establish functional mission checks, not an isolated
startup/tick speedup or GPU time. All requested performance targets remain open.

Raw logs, binaries, canary results, source receipts and images stay ignored in
`.cache/codex-evidence/performance-20261026/term-identity-*` and
`griffin-term-identity-*`; profiling data is not tracked.
