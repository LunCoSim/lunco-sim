# Native attribute handles during USD preparation

## Owner and change

`lunco-usd-bevy-stage::projection_plan` retains one native OpenUSD attribute
handle per property while preparing a prim. Value, type, authored-opinion,
connection and time-sample reads share those handles and their immutable paths.
Handles expire after the prim is prepared. No handle crosses the worker boundary,
no stage answer is memoized across revisions, and no dependency source changes.
OpenUSD remains the composition owner; existing UI-hint and asset-source readers
retain their authoritative interpretation.

This is preparation work, with no changeable selection or lifecycle policy to
move into Rhai. The real consumer is the initial immutable projection plan used
by the production visual and simulation adapters.

## Fixture results

Five alternating pairs of independently copied normal-profile test executables
run `projection_plan::tests::native_projection_snapshot_benchmark`. Each process
measures five iterations per case and checks exact geometry. Median process
medians, milliseconds:

| Transform opinions | Preparation | Baseline | Candidate | Reduction |
| --- | --- | ---: | ---: | ---: |
| Omitted | Snapshot | 261.752 | 253.564 | 3.13% |
| Omitted | Complete recipe | 318.474 | 298.930 | 6.14% |
| Authored double TRS | Snapshot | 337.462 | 333.555 | 1.16% |
| Authored double TRS | Complete recipe | 417.203 | 399.310 | 4.29% |

These are small fixture gains, not whole-mission loading results. The snapshot
checks pass: 16 tests, one ignored diagnostic. The existing live/prepared
composition test additionally compares values, types, authored opinions and time
samples with unset and blocked values and exact large double samples; it passes.
The 43-skill catalogue validator passes.

## Adjacent findings

Rumoca exposes `MeRetainedComponent::into_lease` and `MeComponentHost::finish`,
and its session options accept an initialization-only experiment. Both a live
session and that initialization-only route instantiate the same component before
FMI initialization. `BackendSimulationSession::new` then creates an RK45 plugin;
its constructor allocates method state, with derivative/state workspaces filled
on use. The native-code preparation already happened inside component
instantiation. Removing the integrator alone therefore does not remove the
identified native preparation. No alternate initializer was added or native
checks bypassed. The final portable-pin run still measures 2.487 s for the live
initializer and a separate 0.744 s for prepared-component lookup/reconstruction.

`TerrainStreamStatus.pending` includes a pending cover-selection task as well as
tile/render work (`stream_viz.rs`). Earlier API observations of one pending item
do not identify which category was active. They remain failed settled windows;
the driver is not relaxed.

Logs, executable hashes, benchmark rows and runtime receipts stay ignored
under `.cache/codex-evidence/performance-20261026/attribute-handles/`.

## Production qualification

The normal `cargo build --locked -p lunco-luncosim -j 4` succeeds without warnings.
Candidate SHA-256 is
`86ed2968d5e5b1cbb0062c1d199bbe9816378fbb5164f5f6a51ccbae82d935f8`.
The owned Griffin run, PID 1218792 / API 5501, passes the strict settled window:
readiness **8.653 s**, sleeping four-pad handoff, all 271 source hashes unchanged,
zero runtime diagnostic warnings/errors and an inspected rendered view. Its
coupled-tick p50/p95 is **3.607/4.163 ms**; application-frame p50/p95 is
**9.490/11.872 ms**. Initializer construction is **2.417 s**, with 16,482 native
hits and no compilations. Prepared-component lookup/reconstruction is separately
**0.641 s**. None of these owner times is an elapsed-time subtraction.

The preceding baseline, PID 1170089 / API 5500, passes the same source-preserved
handoff and diagnostics at 9.281 s readiness. Its launch overlaps the 5.22 s
focused test compilation, so it is not a clean isolated mission-speed comparison.
Both screenshots show the same distant lander view, terrain, shadows and handoff
HUD. Both processes close and ports release. The fixture remains the evidence
for the isolated reduction; the full loading and coupled-physics targets remain
open. Temporary benchmark executable copies are removed after hashing.

The production reference-edit scene passes all ten authored checks on owned
PID 1219523 / API 5502. API Exit and port release are verified. This exercises
the live authoring path after initial prepared admission.
