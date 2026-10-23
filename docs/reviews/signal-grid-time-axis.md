# One time viewport for related charts

The native signal grid compared a Modelica history retained from time zero
with a body sensor enabled after settling. Each line plot independently padded
its five-second window from its first sample. The numerical timestamps were
correct, but simultaneous events appeared at different horizontal positions.

## Ownership and scope

`lunco-viz` owns this display mechanism. `SignalRegistry` remains the only
history owner. `SignalGridPanel` combines the first/last timestamps of its
visible time-series bindings and passes a typed `SharedTimeAxis` through the
ordinary `VizPanel`/`Panel2DCtx` render context. It does not copy, resample,
shift or generate measurements. Endpoint reads are constant-time per binding;
the existing plot-style cache is reused during paint.

`LinePlot` uses that common interval for its existing follow-latest behavior
and uses egui_plot's public X-axis/cursor linking. Y scales and units remain
independent. Phase-space plots retain their own X source. Standalone plots
receive no group context and keep their existing behavior.

`ConfigureSignalGrid.sync_time_axes` defaults to true for a new grid; an
omitted value preserves an existing choice. Authored callers can set false.
`InspectSignalGrid` exposes that choice and the common native time span.
Individual appearance and following-window settings still use
`ConfigureLinePlot`; no second style or telemetry pipeline is introduced.

The grid's composition survives scene replacement through its existing
binding lifecycle. The shared interval is recomputed from current bindings;
there is no stale copied history to retire. This is view math and native plot
composition, not scenario policy; the authored configuration selects whether
the axes are linked. No new hook or dependency patch is required.

## Verification

The focused interval regression first failed for a late-starting sensor and
an idle channel, then passed with a union over all visible histories. It also
checks reversed input order, empty input and negative time origins.

An egui plot regression checks the actual five-second viewport against that
shared interval. The UI-feature Cargo runs passed five following/compact-label
tests and two interval tests. The production debug build completed successfully.
Directed native launch take v59 shows all four charts before the boost, matching
horizontal time coordinates through the event, and independent Y scales/units.
Opening, boost and final moving images were inspected, followed by the first
and last held images. No loading card or hover readout appears in those images.
Native receipts verify 246 moving images at one completed 60 Hz cycle per image
and 834 analysis images at one fixed tick. The observed launch time remains
0.6166667 s; view composition does not change the measured result.

The owned process used engine HEAD `64c571294` plus this patch and binary SHA256
`925a2ec86f3a48b24255d26fcbc6108ef6c840f3feb7e28f3f403bc8639cff77`.
The campaign retains source hashes, verification, sampled images and its
shared-axis capture plan. The native grid inspection reports four plots,
`sync_time_axes:true`, and common history span `[0,6.1]`. Full-film motion and
listening acceptance remain separate from this directed UI verification.
