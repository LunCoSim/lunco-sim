# Consume live declaration slots in port metadata

## Owner and change

The qualified Griffin [coupled-exchange profile](usd-parent-path-preparation-2026-10-26.md)
identifies repeated name and metadata lookup in `cosim_exchange_write`.
`lunco-port-core::PortRegistry` already selects a live backend, direction and
slot under an immutable World borrow. Its metadata callback now receives that
same slot. Avian reads its declared table entry directly; runtime output aliases
and component surfaces read their existing indexed maps; scene properties decode
their native slot. They no longer repeat name resolution for those metadata
reads. Other metadata owners retain their contract readers and consume the same
updated callback signature. All registered backends and test consumers are
updated together.

Named declaration selection still checks current owner precedence and native
membership. Every write still checks current units, frame, bounds, writability,
revision and slot identity before the existing atomic commit. A metadata change
without a published topology revision still rejects a stale resolved write.
No permission or metadata cache is introduced. Coalesced inspection retains the
first declaration's slot and merged direction; Avian selects the shared
input/output contract within that group, including input write authority.
Optional slots remain the declared contract for owners without indexed storage.

This is a generic registry mechanism with existing production consumers, not a
changeable lifecycle or scenario decision; no Rhai hook is needed. No dependency
source, numerical operation, precision, substeps or rendering settings change.
The canonical architecture and performance runbook describe the same contract.

## Matched fixture evidence

Independent baseline/candidate binaries execute the existing
`resolved_surface_write_benchmark` in ABBA order, five processes per block.
Each run stages and atomically commits 768,000 writes to 128 component surfaces,
checking live unit, frame and range metadata. All twenty runs pass and retain
final f64 bits `40b7700000000000`; binary hashes are verified before and after.
Median time falls from **229.279 to 221.440 ms (3.4%)**.

This measures the complete scalar-surface transaction fixture. It does not
quantify the Avian-specific improvement, whole coupled ticks or scene loading.
Concurrent desktop/video workloads remain present, so the alternating fixture
comparison is local evidence, not product performance acceptance.

## Validation

The port registry's 26 focused checks pass, including direct slot delivery,
selected versus coalesced directions, absent slots, live metadata changes,
stale layouts and transactional rejection. The eleven focused co-simulation
backend checks pass; the manual benchmark is run separately. Avian's private
name-selection helper serves only coalesced metadata inspection.

The regular production build (`cargo build --locked -p lunco-luncosim -j 4`,
with `sccache`) passes without warnings. Binary SHA-256 is
`a6107dd3ad482af2d2a1c0613c06686b87935227a7187a6efb9e40b3e1a26a73`.
The production connection gate passes all six checks, including rejection of
incompatible live units without changing native mass. Summer passes all 102
physics checks and Roadster all eight runner requirements. Their 72 and 137
source hashes respectively remain unchanged. Owned API ports 5460–5462 close.

Griffin's owned PID 664650/API 5463 reaches scene readiness at 13.363 s,
landing/terrain qualification at 38.364 s and the sustained measurement window
at 43.574 s. All 271 source hashes match before admission and after the run.
Final diagnostics contain zero warnings/errors; the lander remains admitted,
unpaused and moving below 0.001 m/s. The inspected screenshot shows the landed
vehicle and terrain, with ramp unfolding still active. This is a landing-handoff
qualification, not completion of the ramp or rover mission. Process and port
closure are confirmed in `metadata-slot-griffin/session.json`.

The last 240 coupled ticks report p50/p95 **5.239/12.162 ms**; the qualified
703-frame window reports **14.609/20.714 ms**. The run overlaps another
rendering simulator (about 251% CPU during qualification) and desktop/video
activity. These are contention-affected observations, not a matched scene
speedup or GPU-only timing. All requested performance targets remain unmet.
The two previously reported articulated exact-replay failures remain open;
this focused change does not alter references or repeat that matrix.

Raw binaries, benchmark runs, build logs and runtime receipts remain ignored
under `.cache/codex-evidence/performance-20261026/metadata-slot-*`.
