# Complete Modelica observation collection

Rumoca's common `MeSimulationSession` owns checked observations, warnings,
input overlays and output cardinality. `visit_visible_values` validates the
complete component observation before visiting borrowed `rumoca_core::VarName`
handles and native `f64` values. The common host interns declared output labels
once through the existing canonical variable-name owner. First input admission
retains its label and output position; subsequent writes change only the value.
Reset retires all applied-input facts together. Declared output order precedes
applied inputs absent from the output inventory, in admission order.

RK45 and LunCoSim's fixed session forward the same reader. The live stepper
clones shared name handles into worker rows, validates finite time and every
value before publication, and the worker removes `time` in place. There is no
per-step name interning or text copy. This is execution/transport work, so no
changeable Rhai policy or hook belongs at the boundary.

`ModelicaResult` and accepted `ModelicaStepSample` outputs carry those same
handles. Their serde representation remains textual names and `f64` values;
process-local name IDs never cross the worker wire. The worker's binary-codec
regression checks name bytes against ordinary textual rows, exact decoded f64
bits and rejection of numeric names. The reflected `ModelicaVariables` snapshot
retains its existing String keys. One normalized publication reader serves
compiled metadata and step outputs, preserving indexed alignment, input/parameter
classification, reordered and omitted rows, duplicate last writes and signed zero.
The SDK's named-map reader also collects the common visitor and renders names
as text; it has no independent numeric/input evaluator.

The ignored `observation_diagnostics::named_and_visited_observation_collection`
uses 2,304 outputs and one input at five changing communication points. It
checks every name and f64 bit, includes row destruction, and alternates 512
complete reads per representation. Construction and advancement are excluded.
Compare independent executable snapshots to isolate a producer change:

```sh
cargo test --release --locked -p lunco-modelica-solver --lib \
  observation_diagnostics::named_and_visited_observation_collection -j 4 -- \
  --ignored --nocapture
```

Six alternating executable runs yield fifteen timing samples per producer.
Median direct collection is 36.3211 ms with copied text and 14.5298 ms with
shared names, 60.00 percent lower. The complete ranges are 35.5682–39.5360 ms
and 13.9639–15.4763 ms. Names, input/time coordinates and reported value bits
agree. This is a collection result, not a complete worker or FPS result.

Nine fixed-step numerical tests pass through the current observation API,
including native/reference arithmetic and exact repeated input schedules.
Worker admission/fingerprint tests pass; native input/session tests preserve
hidden inputs, shared label payloads, signed zero, atomic rejection and reset.
The RK45-enabled SDK Clippy gate passes. The fixed twenty-model canary retains
twelve strict-high comparisons with zero missing/skipped traces and eight
unchanged typed refusals. All 562,411 data values and 16,841 timestamps match
the fresh baseline bit-for-bit. Production qualification status and artifact
identities belong in [the active Griffin review](griffin-2k-performance-2026-11-04.md).

LunCoSim pins the shared observation API and native optimizations to Rumoca
`a11b4b2e0adf52818a711af945c7bcbbfbf5c184` in its workspace manifest and lockfile.
The SDK changes are committed on its local `main`; the application consumer and
revision pin are integrated together. Both repositories remain unpublished, so
an independent machine needs that SDK commit before resolving the pin. The
recorded production evidence uses the same source through its recorded local build
override; no remote publication or fresh-clone build is claimed.
