# Share immutable coordinate-frame identity

Status: focused and production checks pass; whole-scene performance targets remain open.

## Owner and change

The qualified [coupled-write profile](usd-parent-path-preparation-2026-10-26.md)
identifies live metadata preparation among Griffin's recurring costs.
`lunco-engineering-values::CoordinateFrameId` now shares its immutable text
through `Arc<str>`, following the existing shared `UnitReference` contract.
Metadata clones no longer allocate and copy that text. Full text still owns
equality and hashing; independently constructed identities remain equivalent.
String serialization and rejection of blank external input are unchanged.

This is a storage mechanism at the existing value owner, not lifecycle or
scenario policy; no Rhai hook is needed. Live port ownership, permissions,
units, bounds, coordinate-frame validation and atomic commit remain in their
existing readers. No topology cache, dependency change, numerical conversion,
solver setting or alternate write path is introduced. Freshly constructed
Avian frame names still allocate; this change does not remove that work.

## Matched measurement

The existing `resolved_surface_write_benchmark` performs 6,000 atomic
transactions across 128 surface sinks with live unit, frame and bound checks.
Independent baseline/candidate binaries run in ABBA order, five processes per
block, with SHA-256 verified before and after. All 20 runs complete 768,000
writes and retain final f64 bits `40b7700000000000`.

Pooled median time falls from **221.500 to 201.347 ms (9.1%)**. This measures
the complete resolved-write fixture, not an isolated clone or a scene tick.
Concurrent desktop workloads were present; per-run workload snapshots were
not recorded for this fixture. The alternating comparison provides local
evidence, not proof of whole-scene improvement.

## Validation

- `cargo test --locked -p lunco-engineering-values coordinate_frame_identity_shares_text_and_validates_serialization -j 4` passes the storage/serialization seam, Unicode text identity, hashing and blank-input rejection.
- The built `lunco-cosim` test `scalar_surface_aliases_share_units_bounds_and_frame_validation` passes, retaining rejection of incompatible independently constructed frames.
- The regular `cargo build --locked -p lunco-luncosim -j 4` succeeds using normal `sccache` and the checkout's `target/`.
- Production `scripts/api/test_connection_units.py --port 5436` passes six authored checks, including rejection of a live incompatible connection edit.
- Summer's production Lunokhod2 physics scene passes 102 checks at tick 722 on API 5438; Roadster's launch scene with `Roadster::VerifyRunner` passes eight at tick 302 on API 5439. All 72/137 respective source hashes remain unchanged.
- Griffin's production startup driver on API 5437 retains all 271 source hashes before/admitted/after, reaches sleeping four-pad landing handoff and clear registered diagnostics, and completes the qualified window. Its screenshot is inspected; rendered terrain and the landing UI are present. All owned processes and ports close.

The regular candidate binary is
`953f213b7608fb4b2063a3dc71a0a201c38ab0c445b9eda892bac3a09ba14932`.
Griffin readiness is 12.708 s, fixed-tick service p50/p95 is 3.402/4.089 ms,
and application-frame p50/p95 is 11.143/13.686 ms. Other simulation/desktop
workloads contend with this run. Application-frame timing is not GPU service
time. These observations establish neither an isolated scene speedup nor any
of the requested loading, coupled-tick or graphics targets.

The existing intermittent articulated exact-replay finding remains open;
this narrow identity-storage change does not resolve it. No reference or
tolerance is changed. Profiling binaries, matched results, logs, source receipts
and screenshots remain ignored under
`.cache/codex-evidence/performance-20261026/frame-sharing-*` and
`griffin-frame-sharing-normal/`.
